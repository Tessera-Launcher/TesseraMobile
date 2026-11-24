//! Minimal replacement for FCL's `libfcl.so`.
//!
//! `libpojavexec.so` still has a `DT_NEEDED` entry for `libfcl.so` and calls
//! the single `nominal_exit(int)` symbol from it. Tessera never uses FCL's
//! Java-side log redirection or exit-trap methods, so this shim only provides
//! the process-exit primitive. The soname stays `libfcl.so` because the
//! prebuilt `libpojavexec.so` cannot be relinked here.

#![no_std]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    extern "C" {
        fn _exit(code: i32) -> !;
    }
    unsafe { _exit(101) }
}

#[no_mangle]
pub extern "C" fn nominal_exit(code: i32) -> ! {
    extern "C" {
        fn _exit(code: i32) -> !;
    }
    unsafe { _exit(code) }
}
