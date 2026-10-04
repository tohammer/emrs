//! The module side of the protocol between `emrs.el` and a module:
//! initialisation, the `M--emrs-*` functions, and the wake-up pipe.

use crate::convert::de::from_lisp;
use crate::convert::ser::SerError;
use crate::error::{Error, ErrorKind, Fail};
use crate::ffi::env::{Env, Exit, Value};
use crate::ffi::sys;
use crate::payload::Payload;
use crate::runtime::core::{Core, Delivery, StreamEnd};
use crate::runtime::instance::{Actor, AnyActor, Mode, Service};
use std::os::raw::{c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Version of the interface between `emrs.el` and modules.
pub const PROTOCOL: i64 = 1;

/// A service registered by [`crate::module!`].
pub struct ServiceEntry {
    name: &'static str,
    start: fn(&Arc<Core>, &Env, Value) -> Result<u64, Fail>,
}

/// Registry entry for service `S`.
pub fn entry<S: Service>() -> ServiceEntry {
    ServiceEntry { name: S::NAME, start: start_service::<S> }
}

fn start_service<S: Service>(core: &Arc<Core>, env: &Env, args: Value) -> Result<u64, Fail> {
    let args: S::Args = from_lisp(env, args)?;
    Ok(Actor::<S>::spawn(core, args).shared().id)
}

struct Module {
    name: &'static str,
    version: &'static str,
    core: Arc<Core>,
    /// Write end of the wake-up pipe, or -1 while there is none. The lock is
    /// held across every write and across swap-and-close, so a wake-up can
    /// never write to a descriptor number that was closed and reused.
    fd: Arc<Mutex<c_int>>,
    services: Vec<ServiceEntry>,
}

/// Write the wake-up byte if a pipe is attached.
fn wake(fd: &Mutex<c_int>) {
    let fd = fd.lock().unwrap();
    if *fd >= 0 {
        // A failed write means the pipe is gone; emrs.el re-attaches and drains.
        unsafe {
            libc::write(*fd, b"!".as_ptr() as *const c_void, 1);
        }
    }
}

/// Install `new` as the wake-up descriptor and close the previous one.
fn replace_fd(fd: &Mutex<c_int>, new: c_int) {
    let mut fd = fd.lock().unwrap();
    let old = std::mem::replace(&mut *fd, new);
    if old >= 0 {
        unsafe {
            libc::close(old);
        }
    }
}

/// Convert a payload, turning a panic in a user `Serialize` impl into an
/// encode error so that it cannot take the rest of a drain with it.
fn encode(env: &Env, payload: &dyn Payload) -> Result<Value, SerError> {
    match catch_unwind(AssertUnwindSafe(|| payload.to_lisp(env))) {
        Ok(result) => result,
        Err(_) => Err(SerError::Msg("panic while encoding the value".to_string())),
    }
}

type Handler = fn(&Env, &Module, &[Value]) -> Result<Value, Fail>;

/// Body shared by all protocol functions: build the safe wrappers, contain
/// panics, and turn failures into Lisp signals.
unsafe fn run(
    env: *mut sys::emacs_env,
    nargs: isize,
    args: *mut sys::emacs_value,
    data: *mut c_void,
    handler: Handler,
) -> sys::emacs_value {
    let env = unsafe { Env::from_raw(env) };
    let module = unsafe { &*(data as *const Module) };
    let args: &[Value] =
        if nargs == 0 { &[] } else { unsafe { std::slice::from_raw_parts(args as *const Value, nargs as usize) } };
    let fail = match catch_unwind(AssertUnwindSafe(|| handler(&env, module, args))) {
        Ok(Ok(value)) => return value.0,
        Ok(Err(fail)) => fail,
        Err(_) => Fail::Error(Error::with_kind(ErrorKind::Protocol, "panic inside an emrs protocol function")),
    };
    if let Fail::Error(error) = fail {
        signal_error(&env, &error);
    }
    std::ptr::null_mut()
}

/// Leave `(signal 'emrs-error (KIND MESSAGE DATA))` pending.
fn signal_error(env: &Env, error: &Error) {
    let build = || -> Result<(Value, Value), Exit> {
        let data = match &error.data {
            Some(data) => match data.to_lisp(env) {
                Ok(value) => value,
                Err(_) => {
                    env.clear_exit();
                    env.nil()?
                }
            },
            None => env.nil()?,
        };
        let payload = env.list(&[env.intern(error.kind.as_str())?, env.make_string(&error.message)?, data])?;
        Ok((env.intern("emrs-error")?, payload))
    };
    if let Ok((symbol, payload)) = build() {
        env.signal(symbol, payload);
    }
}

fn error_plist(env: &Env, error: &Error) -> Result<Value, Exit> {
    let data = match &error.data {
        Some(data) => match data.to_lisp(env) {
            Ok(value) => value,
            Err(SerError::Exit) => return Err(Exit),
            Err(SerError::Msg(_)) => env.nil()?,
        },
        None => env.nil()?,
    };
    env.list(&[
        env.intern(":kind")?,
        env.intern(error.kind.as_str())?,
        env.intern(":message")?,
        env.make_string(&error.message)?,
        env.intern(":data")?,
        data,
    ])
}

fn optional_error(env: &Env, error: &Option<Error>) -> Result<Value, Exit> {
    match error {
        Some(error) => error_plist(env, error),
        None => env.nil(),
    }
}

fn record(env: &Env, kind: &str, instance: Option<u64>, call: Option<u64>, payload: Value) -> Result<Value, Exit> {
    let id = |n: Option<u64>| match n {
        Some(n) => env.make_big(false, n as u128),
        None => env.nil(),
    };
    env.call("vector", &[env.intern(kind)?, id(instance)?, id(call)?, payload])
}

fn instance_arg(env: &Env, module: &Module, value: Value) -> Result<Arc<dyn AnyActor>, Fail> {
    let id = env.extract_i64(value)?;
    let actor = u64::try_from(id).ok().and_then(|id| module.core.get(id));
    actor.ok_or_else(|| Fail::Error(Error::with_kind(ErrorKind::Stopped, "the instance is not running")))
}

fn call_arg(env: &Env, value: Value) -> Result<u64, Fail> {
    u64::try_from(env.extract_i64(value)?)
        .map_err(|_| Fail::Error(Error::with_kind(ErrorKind::Protocol, "call id must not be negative")))
}

fn info(env: &Env, module: &Module, _: &[Value]) -> Result<Value, Fail> {
    let mut services = Vec::new();
    for service in &module.services {
        services.push(env.make_string(service.name)?);
    }
    Ok(env.list(&[
        env.intern(":protocol")?,
        env.make_i64(PROTOCOL)?,
        env.intern(":emrs")?,
        env.make_string(env!("CARGO_PKG_VERSION"))?,
        env.intern(":module")?,
        env.make_string(module.name)?,
        env.intern(":version")?,
        env.make_string(module.version)?,
        env.intern(":services")?,
        env.list(&services)?,
        env.intern(":target")?,
        env.make_string(env!("EMRS_TARGET"))?,
    ])?)
}

fn attach(env: &Env, module: &Module, args: &[Value]) -> Result<Value, Fail> {
    let fd = env.open_channel(args[0])?;
    replace_fd(&module.fd, fd);
    Ok(env.nil()?)
}

fn start(env: &Env, module: &Module, args: &[Value]) -> Result<Value, Fail> {
    let name = env.string(args[0])?;
    let Some(service) = module.services.iter().find(|s| s.name == name) else {
        let message = format!("module {} has no service named \"{name}\"", module.name);
        return Err(Fail::Error(Error::with_kind(ErrorKind::Protocol, message)));
    };
    let id = (service.start)(&module.core, env, args[1])?;
    Ok(env.make_big(false, id as u128)?)
}

fn send(env: &Env, module: &Module, args: &[Value]) -> Result<Value, Fail> {
    let actor = instance_arg(env, module, args[0])?;
    let (call, mode) = if env.is_not_nil(args[3]) {
        let mode = match env.symbol_name(args[3])?.as_str() {
            "call" => Mode::Call,
            "stream" => Mode::Stream,
            other => {
                let message = format!("unknown request mode `{other}`");
                return Err(Fail::Error(Error::with_kind(ErrorKind::Protocol, message)));
            }
        };
        (Some(call_arg(env, args[2])?), mode)
    } else {
        (None, Mode::Send)
    };
    actor.send_lisp(env, args[1], call, mode)?;
    Ok(env.nil()?)
}

fn cancel(env: &Env, module: &Module, args: &[Value]) -> Result<Value, Fail> {
    let call = call_arg(env, args[1])?;
    if let Ok(actor) = instance_arg(env, module, args[0]) {
        actor.shared().cancel_call(call);
    }
    Ok(env.nil()?)
}

fn stop(env: &Env, module: &Module, args: &[Value]) -> Result<Value, Fail> {
    if let Ok(actor) = instance_arg(env, module, args[0]) {
        actor.request_stop();
    }
    Ok(env.nil()?)
}

fn drain(env: &Env, module: &Module, args: &[Value]) -> Result<Value, Fail> {
    let max = usize::try_from(env.extract_i64(args[0])?).unwrap_or(1).max(1);
    let mut records = Vec::new();
    let mut deliveries = module.core.take(max).into_iter().peekable();
    while let Some(delivery) = deliveries.next() {
        match delivery {
            Delivery::Event { instance, payload } => match encode(env, payload.as_ref()) {
                Ok(value) => records.push(record(env, "event", Some(instance), None, value)?),
                Err(SerError::Exit) => return Err(Fail::Exit),
                Err(SerError::Msg(message)) => {
                    let text = format!("instance {instance}: an event could not be encoded: {message}");
                    let payload = env.cons(env.intern("error")?, env.make_string(&text)?)?;
                    records.push(record(env, "log", None, None, payload)?);
                }
            },
            Delivery::Reply { instance, call, payload } => match encode(env, payload.as_ref()) {
                Ok(value) => records.push(record(env, "reply", Some(instance), Some(call), value)?),
                Err(SerError::Exit) => return Err(Fail::Exit),
                Err(SerError::Msg(message)) => {
                    let error = Error::with_kind(ErrorKind::Encode, message);
                    records.push(record(env, "error", Some(instance), Some(call), error_plist(env, &error)?)?);
                }
            },
            Delivery::Error { instance, call, error } => {
                records.push(record(env, "error", Some(instance), Some(call), error_plist(env, &error)?)?);
            }
            Delivery::Item { instance, call, payload, token, discard, .. } => {
                // Merge the run of items of this stream into one record.
                let mut run = vec![payload];
                while let Some(Delivery::Item { instance: i, call: c, .. }) = deliveries.peek() {
                    if (*i, *c) != (instance, call) {
                        break;
                    }
                    if let Some(Delivery::Item { payload, .. }) = deliveries.next() {
                        run.push(payload);
                    }
                }
                if discard.load(Ordering::SeqCst) {
                    continue;
                }
                let mut values = Vec::with_capacity(run.len());
                let mut failure = None;
                for payload in run {
                    match encode(env, payload.as_ref()) {
                        Ok(value) => values.push(value),
                        Err(SerError::Exit) => return Err(Fail::Exit),
                        Err(SerError::Msg(message)) => {
                            failure = Some(message);
                            break;
                        }
                    }
                }
                if !values.is_empty() {
                    records.push(record(env, "items", Some(instance), Some(call), env.list(&values)?)?);
                }
                if let Some(message) = failure {
                    if let Some(actor) = module.core.get(instance) {
                        actor.shared().cancel_call(call);
                    }
                    discard.store(true, Ordering::SeqCst);
                    token.cancel();
                    let error = Error::with_kind(ErrorKind::Encode, message);
                    let payload = env.cons(env.intern("failed")?, error_plist(env, &error)?)?;
                    records.push(record(env, "end", Some(instance), Some(call), payload)?);
                }
            }
            Delivery::End { instance, call, status, error } => {
                let status = env.intern(if status == StreamEnd::Done { "done" } else { "failed" })?;
                let payload = env.cons(status, optional_error(env, &error)?)?;
                records.push(record(env, "end", Some(instance), Some(call), payload)?);
            }
            Delivery::State { instance, state, error } => {
                let payload = env.cons(env.intern(state.as_str())?, optional_error(env, &error)?)?;
                records.push(record(env, "state", Some(instance), None, payload)?);
            }
            Delivery::Log { level, message } => {
                let payload = env.cons(env.intern(level.as_str())?, env.make_string(&message)?)?;
                records.push(record(env, "log", None, None, payload)?);
            }
        }
    }
    Ok(env.list(&records)?)
}

fn shutdown(env: &Env, module: &Module, args: &[Value]) -> Result<Value, Fail> {
    let seconds = env.extract_float(env.call("float", &[args[0]])?)?;
    // The wake-up descriptor stays open: if the user cancels the exit, the
    // module must still be able to wake Emacs. The process exit closes it.
    let all_stopped = module.core.shutdown(Duration::from_secs_f64(seconds.clamp(0.0, 3600.0)));
    Ok(if all_stopped { env.t()? } else { env.nil()? })
}

macro_rules! protocol_fn {
    ($name:ident => $handler:path) => {
        unsafe extern "C" fn $name(
            env: *mut sys::emacs_env,
            nargs: isize,
            args: *mut sys::emacs_value,
            data: *mut c_void,
        ) -> sys::emacs_value {
            unsafe { run(env, nargs, args, data, $handler) }
        }
    };
}

protocol_fn!(raw_info => info);
protocol_fn!(raw_attach => attach);
protocol_fn!(raw_start => start);
protocol_fn!(raw_send => send);
protocol_fn!(raw_cancel => cancel);
protocol_fn!(raw_stop => stop);
protocol_fn!(raw_drain => drain);
protocol_fn!(raw_shutdown => shutdown);

fn define(env: &Env, module: &'static Module) -> Result<(), Exit> {
    let functions: [(&str, isize, sys::emacs_function); 8] = [
        ("info", 0, raw_info),
        ("attach", 1, raw_attach),
        ("start", 2, raw_start),
        ("send", 4, raw_send),
        ("cancel", 2, raw_cancel),
        ("stop", 1, raw_stop),
        ("drain", 1, raw_drain),
        ("shutdown", 1, raw_shutdown),
    ];
    let data = module as *const Module as *mut c_void;
    for (op, arity, function) in functions {
        let symbol = env.intern(&format!("{}--emrs-{op}", module.name))?;
        let function = env.make_function(arity, arity, function, data)?;
        env.call("fset", &[symbol, function])?;
    }
    env.call("provide", &[env.intern(&format!("{}-module", module.name))?])?;
    Ok(())
}

/// Entry point called by the code [`crate::module!`] generates.
///
/// # Safety
/// `runtime` must be the pointer Emacs passed to `emacs_module_init`.
pub unsafe fn init(
    runtime: *mut c_void,
    name: &'static str,
    version: &'static str,
    services: Vec<ServiceEntry>,
) -> c_int {
    let runtime = runtime as *mut sys::emacs_runtime;
    if unsafe { (*runtime).size } < std::mem::size_of::<sys::emacs_runtime>() as isize {
        return 1;
    }
    let raw_env = unsafe { ((*runtime).get_environment)(runtime) };
    if unsafe { (*raw_env).size } < std::mem::size_of::<sys::emacs_env>() as isize {
        return 2;
    }
    let mut names: Vec<&str> = services.iter().map(|s| s.name).collect();
    names.sort_unstable();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return 3;
    }
    let fd = Arc::new(Mutex::new(-1));
    let wake_fd = fd.clone();
    let core = Core::new(name, move || wake(&wake_fd));
    let module: &'static Module = Box::leak(Box::new(Module { name, version, core, fd, services }));
    let env = unsafe { Env::from_raw(raw_env) };
    match catch_unwind(AssertUnwindSafe(|| define(&env, module))) {
        Ok(Ok(())) => 0,
        _ => 4,
    }
}

/// Define the entry point of an Emacs module that hosts the given services.
///
/// ```ignore
/// emrs::module!("git-rs", [Git, Blame]);
/// ```
///
/// The module must be built as a `cdylib` with `panic = "unwind"` and
/// installed as `git-rs-module` plus the platform's module suffix.
#[macro_export]
macro_rules! module {
    ($name:literal, [$($service:ty),+ $(,)?]) => {
        #[cfg(panic = "abort")]
        compile_error!("emrs modules must be built with panic = \"unwind\"; a panic would otherwise abort Emacs");

        #[unsafe(no_mangle)]
        #[allow(non_upper_case_globals)]
        pub static plugin_is_GPL_compatible: ::std::os::raw::c_int = 0;

        /// # Safety
        /// Called by Emacs when it loads the module.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn emacs_module_init(runtime: *mut ::std::os::raw::c_void) -> ::std::os::raw::c_int {
            unsafe {
                $crate::__private::init(
                    runtime,
                    $name,
                    env!("CARGO_PKG_VERSION"),
                    vec![$($crate::__private::entry::<$service>()),+],
                )
            }
        }
    };
}
