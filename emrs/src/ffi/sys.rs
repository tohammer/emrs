//! Raw layout of the Emacs module API (`emacs-module.h`), Emacs 28 level.
//!
//! The environment structs of Emacs 29 and 30 add no members, so this layout
//! is valid for every Emacs from 28 on. Members the crate does not call are
//! declared as untyped function pointers to keep the layout exact.
#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_int, c_void};

pub(crate) type emacs_value = *mut c_void;
pub(crate) type emacs_limb_t = usize;

/// Placeholder for a member that is never called.
pub(crate) type Unused = Option<unsafe extern "C" fn()>;

pub(crate) type emacs_function =
    unsafe extern "C" fn(env: *mut emacs_env, nargs: isize, args: *mut emacs_value, data: *mut c_void) -> emacs_value;

pub(crate) const FUNCALL_EXIT_RETURN: c_int = 0;

#[repr(C)]
pub(crate) struct emacs_runtime {
    pub size: isize,
    pub private_members: *mut c_void,
    pub get_environment: unsafe extern "C" fn(rt: *mut emacs_runtime) -> *mut emacs_env,
}

#[repr(C)]
pub(crate) struct emacs_env {
    pub size: isize,
    pub private_members: *mut c_void,
    // Emacs 25
    pub make_global_ref: Unused,
    pub free_global_ref: Unused,
    pub non_local_exit_check: unsafe extern "C" fn(env: *mut emacs_env) -> c_int,
    pub non_local_exit_clear: unsafe extern "C" fn(env: *mut emacs_env),
    pub non_local_exit_get: Unused,
    pub non_local_exit_signal: unsafe extern "C" fn(env: *mut emacs_env, symbol: emacs_value, data: emacs_value),
    pub non_local_exit_throw: Unused,
    pub make_function: unsafe extern "C" fn(
        env: *mut emacs_env,
        min_arity: isize,
        max_arity: isize,
        func: emacs_function,
        docstring: *const c_char,
        data: *mut c_void,
    ) -> emacs_value,
    pub funcall: unsafe extern "C" fn(
        env: *mut emacs_env,
        func: emacs_value,
        nargs: isize,
        args: *mut emacs_value,
    ) -> emacs_value,
    pub intern: unsafe extern "C" fn(env: *mut emacs_env, name: *const c_char) -> emacs_value,
    pub type_of: unsafe extern "C" fn(env: *mut emacs_env, arg: emacs_value) -> emacs_value,
    pub is_not_nil: unsafe extern "C" fn(env: *mut emacs_env, arg: emacs_value) -> bool,
    pub eq: unsafe extern "C" fn(env: *mut emacs_env, a: emacs_value, b: emacs_value) -> bool,
    pub extract_integer: unsafe extern "C" fn(env: *mut emacs_env, arg: emacs_value) -> i64,
    pub make_integer: unsafe extern "C" fn(env: *mut emacs_env, n: i64) -> emacs_value,
    pub extract_float: unsafe extern "C" fn(env: *mut emacs_env, arg: emacs_value) -> f64,
    pub make_float: unsafe extern "C" fn(env: *mut emacs_env, d: f64) -> emacs_value,
    pub copy_string_contents:
        unsafe extern "C" fn(env: *mut emacs_env, value: emacs_value, buf: *mut c_char, len: *mut isize) -> bool,
    pub make_string: unsafe extern "C" fn(env: *mut emacs_env, s: *const c_char, len: isize) -> emacs_value,
    pub make_user_ptr: Unused,
    pub get_user_ptr: Unused,
    pub set_user_ptr: Unused,
    pub get_user_finalizer: Unused,
    pub set_user_finalizer: Unused,
    pub vec_get: unsafe extern "C" fn(env: *mut emacs_env, vector: emacs_value, index: isize) -> emacs_value,
    pub vec_set: Unused,
    pub vec_size: unsafe extern "C" fn(env: *mut emacs_env, vector: emacs_value) -> isize,
    // Emacs 26
    pub should_quit: Unused,
    // Emacs 27
    pub process_input: Unused,
    pub extract_time: Unused,
    pub make_time: Unused,
    pub extract_big_integer: unsafe extern "C" fn(
        env: *mut emacs_env,
        arg: emacs_value,
        sign: *mut c_int,
        count: *mut isize,
        magnitude: *mut emacs_limb_t,
    ) -> bool,
    pub make_big_integer: unsafe extern "C" fn(
        env: *mut emacs_env,
        sign: c_int,
        count: isize,
        magnitude: *const emacs_limb_t,
    ) -> emacs_value,
    // Emacs 28
    pub get_function_finalizer: Unused,
    pub set_function_finalizer: Unused,
    pub open_channel: unsafe extern "C" fn(env: *mut emacs_env, pipe_process: emacs_value) -> c_int,
    pub make_interactive: Unused,
    pub make_unibyte_string: unsafe extern "C" fn(env: *mut emacs_env, s: *const c_char, len: isize) -> emacs_value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_the_emacs_28_header() {
        let ptr = std::mem::size_of::<*mut c_void>();
        // size, private_members, then 27 (Emacs 25) + 1 (26) + 5 (27) + 5 (28) members.
        assert_eq!(std::mem::size_of::<emacs_env>(), ptr * (2 + 27 + 1 + 5 + 5));
        assert_eq!(std::mem::size_of::<emacs_runtime>(), ptr * 3);
    }
}
