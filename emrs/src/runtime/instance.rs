//! Service instances: the actor loop, lifecycle, and the handles given to
//! service code.

use super::cancel::CancelToken;
use super::core::{Core, Delivery, Level, StreamEnd};
use super::reply::Reply;
use crate::convert::de::from_lisp;
use crate::error::{Error, ErrorKind, Fail};
use crate::ffi::env::{Env, Value};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// A message-passing service hosted in an Emacs module.
///
/// For one instance, `start`, `handle` and `stop` never run concurrently and
/// never on the Emacs thread.
pub trait Service: Sized + Send + 'static {
    /// The name Elisp passes to `emrs-start`.
    const NAME: &'static str;
    /// Start arguments, decoded from the `ARGS` of `emrs-start`.
    type Args: DeserializeOwned + Send + 'static;
    /// Requests from Elisp, and self-messages sent through [`Address`].
    type Request: DeserializeOwned + Send + 'static;
    /// Unsolicited events for the instance's `on-event` handler.
    type Event: Serialize + Send + 'static;

    /// Build the instance state. May block.
    fn start(args: Self::Args, ctx: &Context<Self>) -> Result<Self, Error>;

    /// Handle one request. May block; move long work to [`Context::spawn`].
    fn handle(&mut self, request: Self::Request, reply: Reply, ctx: &Context<Self>);

    /// Called once after the last `handle` when the instance stops normally.
    fn stop(&mut self) {}
}

/// Lifecycle state of an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

impl State {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            State::Starting => "starting",
            State::Running => "running",
            State::Stopping => "stopping",
            State::Stopped => "stopped",
            State::Failed => "failed",
        }
    }

    fn is_terminal(self) -> bool {
        matches!(self, State::Stopped | State::Failed)
    }
}

/// How the sender of a request wants to be answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// No answer expected.
    Send,
    /// Exactly one reply or error.
    Call,
    /// Items followed by one end.
    Stream,
}

struct CallEntry {
    mode: Mode,
    token: CancelToken,
    /// Shared with the call's queued items; see `Delivery::Item`.
    discard: Arc<AtomicBool>,
}

struct Calls {
    state: State,
    open: HashMap<u64, CallEntry>,
}

/// The part of an instance that does not depend on the service type.
pub(crate) struct Shared {
    pub(crate) id: u64,
    pub(crate) core: Arc<Core>,
    pub(crate) token: CancelToken,
    /// Guards the state and the open calls together, and is held while
    /// pushing their deliveries so that completions and the final state
    /// reach Emacs in a consistent order.
    calls: Mutex<Calls>,
}

impl Shared {
    pub(crate) fn state(&self) -> State {
        self.calls.lock().unwrap().state
    }

    /// Register an open call and return its reply handle.
    fn begin(self: &Arc<Self>, call: Option<u64>, mode: Mode) -> Result<Reply, Error> {
        let mut calls = self.calls.lock().unwrap();
        if !matches!(calls.state, State::Starting | State::Running) {
            return Err(Error::with_kind(ErrorKind::Stopped, "the instance is not running"));
        }
        let token = self.token.child();
        let discard = Arc::new(AtomicBool::new(false));
        if let Some(id) = call {
            calls.open.insert(id, CallEntry { mode, token: token.clone(), discard: discard.clone() });
        }
        Ok(Reply::new(self.clone(), call, mode, token, discard))
    }

    /// Push the deliveries that complete `call`, unless it is already
    /// completed or cancelled.
    pub(crate) fn complete_with(&self, call: u64, make: impl FnOnce() -> Vec<Delivery>) {
        let mut calls = self.calls.lock().unwrap();
        if calls.open.remove(&call).is_some() {
            for delivery in make() {
                self.core.push(delivery);
            }
        }
    }

    pub(crate) fn complete(&self, call: u64, make: impl FnOnce() -> Delivery) {
        self.complete_with(call, || vec![make()]);
    }

    /// Forget `call`, discard its queued items and cancel its token. Nothing
    /// more is delivered for it.
    pub(crate) fn cancel_call(&self, call: u64) {
        let entry = self.calls.lock().unwrap().open.remove(&call);
        if let Some(entry) = entry {
            entry.discard.store(true, Ordering::SeqCst);
            entry.token.cancel();
        }
    }

    /// Move from `Starting` to `Running`. Returns false if a stop was
    /// requested meanwhile.
    fn set_running(&self) -> bool {
        let mut calls = self.calls.lock().unwrap();
        if calls.state != State::Starting {
            return false;
        }
        calls.state = State::Running;
        self.core.push(Delivery::State { instance: self.id, state: State::Running, error: None });
        true
    }

    /// Move to `Stopping`. Returns false if the instance was already
    /// stopping or terminal.
    fn set_stopping(&self) -> bool {
        let mut calls = self.calls.lock().unwrap();
        if !matches!(calls.state, State::Starting | State::Running) {
            return false;
        }
        calls.state = State::Stopping;
        true
    }

    /// Enter a terminal state: fail every open call, announce the state and
    /// drop the instance from the table.
    fn terminate(&self, state: State, error: Option<Error>) {
        let kind = if state == State::Failed { ErrorKind::Failed } else { ErrorKind::Stopped };
        let message = match &error {
            Some(error) => error.message.clone(),
            None => "the instance stopped".to_string(),
        };
        let mut calls = self.calls.lock().unwrap();
        if calls.state.is_terminal() {
            return;
        }
        calls.state = state;
        self.token.cancel();
        let mut open: Vec<(u64, CallEntry)> = calls.open.drain().collect();
        open.sort_by_key(|(id, _)| *id);
        for (call, entry) in open {
            let error = Error::with_kind(kind, message.clone());
            self.core.push(match entry.mode {
                Mode::Stream => {
                    Delivery::End { instance: self.id, call, status: StreamEnd::Failed, error: Some(error) }
                }
                _ => Delivery::Error { instance: self.id, call, error },
            });
        }
        self.core.push(Delivery::State { instance: self.id, state, error });
        drop(calls);
        self.core.unregister(self.id);
    }
}

enum Envelope<S: Service> {
    Start(S::Args),
    Request(S::Request, Reply),
    Stop,
}

struct Mailbox<S: Service> {
    queue: VecDeque<Envelope<S>>,
    /// A pool job is running or queued for this instance.
    scheduled: bool,
    /// No further requests are accepted.
    closed: bool,
}

pub(crate) struct Actor<S: Service> {
    shared: Arc<Shared>,
    mailbox: Mutex<Mailbox<S>>,
    /// The service state; touched only by the worker running this instance.
    service: Mutex<Option<S>>,
    me: Weak<Actor<S>>,
}

/// Operations on an instance that do not need its service type.
pub(crate) trait AnyActor: Send + Sync {
    fn shared(&self) -> &Arc<Shared>;
    fn request_stop(&self);
    /// Decode `request` on the Emacs thread and enqueue it.
    fn send_lisp(&self, env: &Env, request: Value, call: Option<u64>, mode: Mode) -> Result<(), Fail>;
}

fn panic_message(panic: Box<dyn Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

impl<S: Service> Actor<S> {
    /// Create an instance and queue its start.
    pub(crate) fn spawn(core: &Arc<Core>, args: S::Args) -> Arc<Actor<S>> {
        let shared = Arc::new(Shared {
            id: core.next_id(),
            core: core.clone(),
            token: CancelToken::new(),
            calls: Mutex::new(Calls { state: State::Starting, open: HashMap::new() }),
        });
        let actor = Arc::new_cyclic(|me| Actor {
            shared,
            mailbox: Mutex::new(Mailbox { queue: VecDeque::new(), scheduled: false, closed: false }),
            service: Mutex::new(None),
            me: me.clone(),
        });
        core.register(actor.shared.id, actor.clone());
        actor.enqueue(Envelope::Start(args));
        actor
    }

    fn context(&self) -> Context<S> {
        Context { shared: self.shared.clone(), actor: self.me.clone() }
    }

    /// Enqueue a typed request.
    pub(crate) fn send(&self, request: S::Request, call: Option<u64>, mode: Mode) -> Result<(), Error> {
        let reply = self.shared.begin(call, mode)?;
        self.enqueue(Envelope::Request(request, reply));
        Ok(())
    }

    fn enqueue(&self, envelope: Envelope<S>) {
        let mut mailbox = self.mailbox.lock().unwrap();
        if mailbox.closed && !matches!(envelope, Envelope::Stop) {
            drop(mailbox);
            if let Envelope::Request(_, reply) = envelope {
                reply.fail(ErrorKind::Stopped, "the instance is stopping");
            }
            return;
        }
        mailbox.queue.push_back(envelope);
        if !mailbox.scheduled {
            mailbox.scheduled = true;
            drop(mailbox);
            let actor = self.me.upgrade().expect("actor is alive while it is used");
            self.shared.core.pool.submit(move || actor.run());
        }
    }

    /// Pool job: process messages until the mailbox is empty.
    fn run(self: Arc<Self>) {
        loop {
            let envelope = {
                let mut mailbox = self.mailbox.lock().unwrap();
                match mailbox.queue.pop_front() {
                    Some(envelope) => envelope,
                    None => {
                        mailbox.scheduled = false;
                        return;
                    }
                }
            };
            if !self.process(envelope) {
                return;
            }
        }
    }

    /// Returns false once the instance has terminated.
    fn process(&self, envelope: Envelope<S>) -> bool {
        let ctx = self.context();
        let mut service = self.service.lock().unwrap();
        match envelope {
            Envelope::Start(args) => match catch_unwind(AssertUnwindSafe(|| S::start(args, &ctx))) {
                Ok(Ok(state)) => {
                    *service = Some(state);
                    self.shared.set_running();
                    true
                }
                Ok(Err(error)) => self.fail(Error::with_kind(ErrorKind::Failed, error.message)),
                Err(panic) => self.fail_panic("start", panic),
            },
            Envelope::Request(request, reply) => {
                let Some(state) = service.as_mut() else {
                    return true;
                };
                match catch_unwind(AssertUnwindSafe(|| state.handle(request, reply, &ctx))) {
                    Ok(()) => true,
                    Err(panic) => {
                        let dropped = catch_unwind(AssertUnwindSafe(|| drop(service.take())));
                        if dropped.is_err() {
                            self.shared.core.log(Level::Error, "service state panicked while being dropped");
                        }
                        self.fail_panic("handle", panic)
                    }
                }
            }
            Envelope::Stop => {
                // The state stays outside the closure: if `stop` panics it must
                // not be dropped during that unwind, where a second panic
                // from its `Drop` would abort Emacs.
                let mut state = service.take();
                let stopped = catch_unwind(AssertUnwindSafe(|| {
                    if let Some(state) = state.as_mut() {
                        state.stop();
                    }
                }));
                if catch_unwind(AssertUnwindSafe(move || drop(state))).is_err() {
                    self.shared.core.log(Level::Error, "service state panicked while being dropped");
                }
                match stopped {
                    Ok(()) => {
                        self.shared.terminate(State::Stopped, None);
                        false
                    }
                    Err(panic) => self.fail_panic("stop", panic),
                }
            }
        }
    }

    fn fail_panic(&self, phase: &str, panic: Box<dyn Any + Send>) -> bool {
        let message = format!("panic in {phase}: {}", panic_message(panic));
        self.fail(Error::with_kind(ErrorKind::Failed, message))
    }

    /// Terminate as failed and discard queued requests. Always returns false.
    fn fail(&self, error: Error) -> bool {
        self.shared.terminate(State::Failed, Some(error));
        let queued: Vec<Envelope<S>> = {
            let mut mailbox = self.mailbox.lock().unwrap();
            mailbox.closed = true;
            mailbox.queue.drain(..).collect()
        };
        drop(queued);
        false
    }
}

impl<S: Service> AnyActor for Actor<S> {
    fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    fn request_stop(&self) {
        if !self.shared.set_stopping() {
            return;
        }
        self.shared.token.cancel();
        let queued: Vec<Envelope<S>> = {
            let mut mailbox = self.mailbox.lock().unwrap();
            mailbox.closed = true;
            mailbox.queue.drain(..).collect()
        };
        for envelope in queued {
            if let Envelope::Request(_, reply) = envelope {
                reply.fail(ErrorKind::Stopped, "the instance stopped before handling the request");
            }
        }
        self.enqueue(Envelope::Stop);
    }

    fn send_lisp(&self, env: &Env, request: Value, call: Option<u64>, mode: Mode) -> Result<(), Fail> {
        let request: S::Request = from_lisp(env, request)?;
        Ok(self.send(request, call, mode)?)
    }
}

/// The handle through which service code talks to the runtime.
pub struct Context<S: Service> {
    shared: Arc<Shared>,
    actor: Weak<Actor<S>>,
}

impl<S: Service> Clone for Context<S> {
    fn clone(&self) -> Self {
        Context { shared: self.shared.clone(), actor: self.actor.clone() }
    }
}

impl<S: Service> Context<S> {
    /// Queue an event for the instance's `on-event` handler. Never blocks.
    /// Does nothing once the instance has stopped or failed.
    pub fn emit(&self, event: S::Event) {
        if !self.shared.state().is_terminal() {
            self.shared.core.push(Delivery::Event { instance: self.shared.id, payload: Box::new(event) });
        }
    }

    /// Run `task` on the runtime's pool. Its token is cancelled when the
    /// instance stops.
    pub fn spawn(&self, task: impl FnOnce(CancelToken) + Send + 'static) {
        let token = self.shared.token.child();
        let shared = self.shared.clone();
        self.shared.core.pool.submit(move || {
            if let Err(panic) = catch_unwind(AssertUnwindSafe(|| task(token))) {
                let message = format!("instance {}: spawned task panicked: {}", shared.id, panic_message(panic));
                shared.core.log(Level::Error, message);
            }
        });
    }

    /// A handle for sending requests to this instance from other threads.
    pub fn address(&self) -> Address<S> {
        Address { actor: self.actor.clone() }
    }

    /// Cancelled when the instance stops or fails.
    pub fn cancel_token(&self) -> CancelToken {
        self.shared.token.clone()
    }

    /// Stop this instance, as `emrs-stop` does.
    pub fn stop(&self) {
        if let Some(actor) = self.actor.upgrade() {
            actor.request_stop();
        }
    }

    /// Write a line to the `*emrs-log*` buffer.
    pub fn log(&self, level: Level, message: impl Into<String>) {
        self.shared.core.log(level, format!("instance {}: {}", self.shared.id, message.into()));
    }
}

/// Sends requests into an instance's own mailbox.
pub struct Address<S: Service> {
    actor: Weak<Actor<S>>,
}

impl<S: Service> Clone for Address<S> {
    fn clone(&self) -> Self {
        Address { actor: self.actor.clone() }
    }
}

impl<S: Service> Address<S> {
    /// Enqueue a request. Its `Reply` reports `is_wanted() == false`. Does
    /// nothing once the instance has stopped or failed.
    pub fn send(&self, request: S::Request) {
        if let Some(actor) = self.actor.upgrade() {
            let _ = actor.send(request, None, Mode::Send);
        }
    }
}
