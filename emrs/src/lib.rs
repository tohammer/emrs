//! Message-passing services for Emacs dynamic modules.
#![allow(dead_code)] // Removed in Task 9, when every item has a user.

mod convert;
mod error;
mod ffi;
mod names;
mod payload;
mod runtime;

pub use error::{Error, ErrorKind};
pub use runtime::cancel::CancelToken;
