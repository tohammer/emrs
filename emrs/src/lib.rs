//! Message-passing services for Emacs dynamic modules.
#![allow(dead_code)] // Removed in Task 9, when every item has a user.

mod convert;
mod error;
mod ffi;
mod names;
mod payload;
mod runtime;
pub mod testing;

pub use error::{Error, ErrorKind};
pub use runtime::cancel::CancelToken;
pub use runtime::core::{Level, StreamEnd};
pub use runtime::instance::{Address, Context, Service, State};
pub use runtime::reply::{Reply, StreamClosed, StreamSender, TrySendError};
