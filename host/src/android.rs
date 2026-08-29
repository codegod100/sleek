//! C entrypoint loaded by GTK's Android runtime.

use std::os::raw::{c_char, c_int};

#[no_mangle]
pub extern "C" fn main(
    _argc: c_int,
    _argv: *mut *mut c_char,
    _envp: *mut *mut c_char,
) -> c_int {
    crate::run();
    0
}
