//! Type-erased values waiting to be converted to Lisp on the Emacs thread.

use crate::convert::ser::{SerError, to_lisp};
use crate::ffi::env::{Env, Value};
use serde::Serialize;
use std::any::Any;

pub(crate) trait Payload: Send + 'static {
    fn to_lisp(&self, env: &Env) -> Result<Value, SerError>;
    /// Recover the concrete value; used by the testing host.
    fn into_any(self: Box<Self>) -> Box<dyn Any + Send>;
}

impl<T: Serialize + Send + 'static> Payload for T {
    fn to_lisp(&self, env: &Env) -> Result<Value, SerError> {
        to_lisp(env, self)
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any + Send> {
        self
    }
}
