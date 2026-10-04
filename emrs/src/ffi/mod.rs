//! Private bindings to the Emacs module API.

pub(crate) mod env;
#[cfg(test)]
pub(crate) mod fake;
pub(crate) mod sys;
