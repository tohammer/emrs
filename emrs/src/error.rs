//! The error type crossing the Rust/Lisp boundary.

use crate::convert::de::DeError;
use crate::ffi::env::Exit;
use crate::payload::Payload;
use serde::Serialize;
use std::fmt;

/// Why a call, stream or instance ended unsuccessfully. Shown to Elisp as the
/// `:kind` symbol of an error plist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The handler answered with an error.
    Service,
    /// The reply or stream sender was dropped unanswered.
    NoReply,
    /// The call was cancelled.
    Cancelled,
    /// A synchronous wait timed out.
    Timeout,
    /// The instance stopped before answering, or was not running.
    Stopped,
    /// The instance failed before answering.
    Failed,
    /// A Lisp value could not be converted to the Rust type.
    Decode,
    /// A Rust value could not be converted to Lisp.
    Encode,
    /// Version or mode mismatch, unknown service, or internal fault.
    Protocol,
}

impl ErrorKind {
    /// The Lisp symbol name of this kind.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Service => "service",
            ErrorKind::NoReply => "no-reply",
            ErrorKind::Cancelled => "cancelled",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Stopped => "stopped",
            ErrorKind::Failed => "failed",
            ErrorKind::Decode => "decode",
            ErrorKind::Encode => "encode",
            ErrorKind::Protocol => "protocol",
        }
    }
}

/// An error reported to Elisp.
///
/// `Error` deliberately does not implement `std::error::Error`, which allows
/// the blanket `From` conversion below so that `?` works on any error type.
pub struct Error {
    pub(crate) kind: ErrorKind,
    pub(crate) message: String,
    pub(crate) data: Option<Box<dyn Payload>>,
}

impl Error {
    /// A service error with a message.
    pub fn new(message: impl Into<String>) -> Error {
        Error::with_kind(ErrorKind::Service, message)
    }

    /// A service error with a message and a payload for the `:data` slot.
    pub fn with_data(message: impl Into<String>, data: impl Serialize + Send + 'static) -> Error {
        Error { kind: ErrorKind::Service, message: message.into(), data: Some(Box::new(data)) }
    }

    pub(crate) fn with_kind(kind: ErrorKind, message: impl Into<String>) -> Error {
        Error { kind, message: message.into(), data: None }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// Take the data payload if it has type `T`. Intended for tests.
    pub fn take_data<T: 'static>(&mut self) -> Option<T> {
        let data = self.data.take()?;
        data.into_any().downcast::<T>().ok().map(|b| *b)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error")
            .field("kind", &self.kind)
            .field("message", &self.message)
            .field("has_data", &self.data.is_some())
            .finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.as_str(), self.message)
    }
}

impl<E: std::error::Error> From<E> for Error {
    fn from(error: E) -> Error {
        Error::new(error.to_string())
    }
}

/// Why a protocol function could not complete.
pub(crate) enum Fail {
    /// A Lisp non-local exit is pending; return to Emacs untouched.
    Exit,
    /// Signal this error to Lisp as `emrs-error`.
    Error(Error),
}

impl From<Exit> for Fail {
    fn from(_: Exit) -> Fail {
        Fail::Exit
    }
}

impl From<Error> for Fail {
    fn from(error: Error) -> Fail {
        Fail::Error(error)
    }
}

impl From<DeError> for Fail {
    fn from(error: DeError) -> Fail {
        match error {
            DeError::Exit => Fail::Exit,
            other => Fail::Error(Error::with_kind(ErrorKind::Decode, other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, ErrorKind};

    #[test]
    fn new_is_a_service_error() {
        let e = Error::new("boom");
        assert_eq!(e.kind(), ErrorKind::Service);
        assert_eq!(e.message(), "boom");
        assert_eq!(e.to_string(), "service: boom");
    }

    #[test]
    fn question_mark_converts_std_errors() {
        fn parse() -> Result<i32, Error> {
            Ok("x".parse::<i32>()?)
        }
        let e = parse().unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Service);
        assert!(e.message().contains("invalid digit"));
    }

    #[test]
    fn data_can_be_taken_back_by_type() {
        let mut e = Error::with_data("bad", 42u32);
        assert_eq!(e.take_data::<String>(), None);
        let mut e2 = Error::with_data("bad", 42u32);
        assert_eq!(e2.take_data::<u32>(), Some(42));
        assert_eq!(e.take_data::<u32>(), None, "a failed downcast consumes the data");
    }

    #[test]
    fn kind_names_are_kebab_symbols() {
        assert_eq!(ErrorKind::NoReply.as_str(), "no-reply");
        assert_eq!(ErrorKind::Protocol.as_str(), "protocol");
    }
}
