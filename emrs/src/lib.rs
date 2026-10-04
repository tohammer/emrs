//! Message-passing services for Emacs dynamic modules.
//!
//! Implement [`Service`], register it with [`module!`], and talk to it from
//! Elisp through `emrs.el`. Service code never runs on the Emacs thread and
//! never sees the Emacs module API.

mod convert;
mod crt;
mod error;
mod ffi;
mod module;
mod names;
mod payload;
mod runtime;
pub mod testing;

pub use error::{Error, ErrorKind};
pub use runtime::cancel::CancelToken;
pub use runtime::core::{Level, StreamEnd};
pub use runtime::instance::{Address, Context, Service, State};
pub use runtime::reply::{Reply, StreamClosed, StreamSender, TrySendError};

#[doc(hidden)]
pub mod __private {
    pub use crate::module::{ServiceEntry, entry, init};
}
