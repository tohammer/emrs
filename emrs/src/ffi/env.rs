//! Checked wrapper around a live `emacs_env`.
//!
//! An `Env` is valid only during the module function call it was created for
//! and only on the calling thread. `Env` and `Value` hold raw pointers and are
//! therefore neither `Send` nor `Sync`.

use super::sys;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};

/// A non-local exit (signal or throw) is pending in the environment. The
/// caller must return to Emacs without making further API calls.
#[derive(Debug)]
pub(crate) struct Exit;

pub(crate) type Res<T> = Result<T, Exit>;

#[derive(Clone, Copy, Debug)]
#[repr(transparent)]
pub(crate) struct Value(pub(crate) sys::emacs_value);

pub(crate) struct Env {
    raw: *mut sys::emacs_env,
}

/// Number of limbs needed for a 128-bit magnitude.
const MAX_LIMBS: usize = 16 / std::mem::size_of::<sys::emacs_limb_t>();

macro_rules! raw {
    ($self:ident . $f:ident ( $($arg:expr),* )) => {
        unsafe { ((*$self.raw).$f)($self.raw $(, $arg)*) }
    };
}

impl Env {
    /// # Safety
    /// `raw` must be the environment pointer Emacs passed to the module
    /// function that is currently executing on this thread.
    pub(crate) unsafe fn from_raw(raw: *mut sys::emacs_env) -> Env {
        Env { raw }
    }

    fn check(&self) -> Res<()> {
        if raw!(self.non_local_exit_check()) == sys::FUNCALL_EXIT_RETURN { Ok(()) } else { Err(Exit) }
    }

    fn checked(&self, value: sys::emacs_value) -> Res<Value> {
        self.check().map(|()| Value(value))
    }

    pub(crate) fn intern(&self, name: &str) -> Res<Value> {
        let name = CString::new(name).expect("symbol name contains NUL");
        self.checked(raw!(self.intern(name.as_ptr())))
    }

    pub(crate) fn nil(&self) -> Res<Value> {
        self.intern("nil")
    }

    pub(crate) fn t(&self) -> Res<Value> {
        self.intern("t")
    }

    pub(crate) fn funcall(&self, function: Value, args: &[Value]) -> Res<Value> {
        let args_ptr = args.as_ptr() as *mut sys::emacs_value;
        self.checked(raw!(self.funcall(function.0, args.len() as isize, args_ptr)))
    }

    /// Call the Lisp function named `name`.
    pub(crate) fn call(&self, name: &str, args: &[Value]) -> Res<Value> {
        let function = self.intern(name)?;
        self.funcall(function, args)
    }

    pub(crate) fn is_not_nil(&self, value: Value) -> bool {
        raw!(self.is_not_nil(value.0))
    }

    /// Name of the symbol `type-of` returns for `value`, e.g. `"integer"`.
    pub(crate) fn type_name(&self, value: Value) -> Res<String> {
        let symbol = self.checked(raw!(self.type_of(value.0)))?;
        self.symbol_name(symbol)
    }

    pub(crate) fn symbol_name(&self, symbol: Value) -> Res<String> {
        let name = self.call("symbol-name", &[symbol])?;
        self.string(name)
    }

    pub(crate) fn make_i64(&self, n: i64) -> Res<Value> {
        self.checked(raw!(self.make_integer(n)))
    }

    pub(crate) fn extract_i64(&self, value: Value) -> Res<i64> {
        let n = raw!(self.extract_integer(value.0));
        self.check().map(|()| n)
    }

    /// Make an integer of any size up to 128 bits from sign and magnitude.
    pub(crate) fn make_big(&self, negative: bool, magnitude: u128) -> Res<Value> {
        if magnitude <= i64::MAX as u128 {
            let n = magnitude as i64;
            return self.make_i64(if negative { -n } else { n });
        }
        let limb_bits = 8 * std::mem::size_of::<sys::emacs_limb_t>();
        let mut limbs = [0 as sys::emacs_limb_t; MAX_LIMBS];
        for (i, limb) in limbs.iter_mut().enumerate() {
            *limb = (magnitude >> (i * limb_bits)) as sys::emacs_limb_t;
        }
        let sign: c_int = if negative { -1 } else { 1 };
        self.checked(raw!(self.make_big_integer(sign, MAX_LIMBS as isize, limbs.as_ptr())))
    }

    /// Sign and magnitude of an integer. `Ok(None)` if it needs more than
    /// 128 bits. Signals a Lisp error if `value` is not an integer.
    pub(crate) fn extract_big(&self, value: Value) -> Res<Option<(bool, u128)>> {
        let mut sign: c_int = 0;
        let mut count: isize = 0;
        raw!(self.extract_big_integer(value.0, &mut sign, &mut count, std::ptr::null_mut()));
        self.check()?;
        if sign == 0 {
            return Ok(Some((false, 0)));
        }
        if count as usize > MAX_LIMBS {
            return Ok(None);
        }
        let mut limbs = [0 as sys::emacs_limb_t; MAX_LIMBS];
        let mut count = MAX_LIMBS as isize;
        raw!(self.extract_big_integer(value.0, &mut sign, &mut count, limbs.as_mut_ptr()));
        self.check()?;
        let limb_bits = 8 * std::mem::size_of::<sys::emacs_limb_t>();
        let mut magnitude: u128 = 0;
        for (i, limb) in limbs.iter().enumerate().take(count as usize) {
            magnitude |= (*limb as u128) << (i * limb_bits);
        }
        Ok(Some((sign < 0, magnitude)))
    }

    pub(crate) fn make_float(&self, d: f64) -> Res<Value> {
        self.checked(raw!(self.make_float(d)))
    }

    pub(crate) fn extract_float(&self, value: Value) -> Res<f64> {
        let d = raw!(self.extract_float(value.0));
        self.check().map(|()| d)
    }

    /// Make a multibyte string from UTF-8 text.
    pub(crate) fn make_string(&self, s: &str) -> Res<Value> {
        self.checked(raw!(self.make_string(s.as_ptr() as *const c_char, s.len() as isize)))
    }

    /// Make a unibyte string holding exactly `bytes`.
    pub(crate) fn make_unibyte(&self, bytes: &[u8]) -> Res<Value> {
        self.checked(raw!(self.make_unibyte_string(bytes.as_ptr() as *const c_char, bytes.len() as isize)))
    }

    /// UTF-8 contents of a Lisp string, as Emacs encodes them.
    pub(crate) fn string_bytes(&self, value: Value) -> Res<Vec<u8>> {
        let mut len: isize = 0;
        raw!(self.copy_string_contents(value.0, std::ptr::null_mut(), &mut len));
        self.check()?;
        let mut buf = vec![0u8; len as usize];
        raw!(self.copy_string_contents(value.0, buf.as_mut_ptr() as *mut c_char, &mut len));
        self.check()?;
        buf.truncate((len as usize).saturating_sub(1)); // drop the trailing NUL
        Ok(buf)
    }

    pub(crate) fn string(&self, value: Value) -> Res<String> {
        let bytes = self.string_bytes(value)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub(crate) fn vec_size(&self, vector: Value) -> Res<isize> {
        let n = raw!(self.vec_size(vector.0));
        self.check().map(|()| n)
    }

    pub(crate) fn vec_get(&self, vector: Value, index: isize) -> Res<Value> {
        self.checked(raw!(self.vec_get(vector.0, index)))
    }

    pub(crate) fn cons(&self, car: Value, cdr: Value) -> Res<Value> {
        self.call("cons", &[car, cdr])
    }

    pub(crate) fn list(&self, items: &[Value]) -> Res<Value> {
        self.call("list", items)
    }

    pub(crate) fn car(&self, cons: Value) -> Res<Value> {
        self.call("car", &[cons])
    }

    pub(crate) fn cdr(&self, cons: Value) -> Res<Value> {
        self.call("cdr", &[cons])
    }

    pub(crate) fn make_function(
        &self,
        min_arity: isize,
        max_arity: isize,
        function: sys::emacs_function,
        data: *mut c_void,
    ) -> Res<Value> {
        self.checked(raw!(self.make_function(min_arity, max_arity, function, std::ptr::null(), data)))
    }

    /// Open a channel to a pipe process; the descriptor is writable from any
    /// thread.
    pub(crate) fn open_channel(&self, pipe_process: Value) -> Res<c_int> {
        let fd = raw!(self.open_channel(pipe_process.0));
        self.check().map(|()| fd)
    }

    /// Leave a pending `(signal SYMBOL DATA)` in the environment.
    pub(crate) fn signal(&self, symbol: Value, data: Value) {
        raw!(self.non_local_exit_signal(symbol.0, data.0));
    }

    /// Discard a pending non-local exit.
    pub(crate) fn clear_exit(&self) {
        raw!(self.non_local_exit_clear());
    }
}
