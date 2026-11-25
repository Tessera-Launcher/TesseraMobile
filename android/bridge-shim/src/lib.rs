//! Tiny native state shared by the launcher VM and the game HotSpot VM.
//!
//! Both VMs load this library in the same Android process, so a static
//! `AtomicBool` is the only cross-VM state currently needed. This replaces the
//! Pojav `nativeSetGrabbing` path without pulling in the whole exec bridge.

#![no_std]

use core::ffi::c_void;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

static GRABBING: AtomicBool = AtomicBool::new(false);

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    extern "C" {
        fn _exit(code: i32) -> !;
    }
    unsafe { _exit(101) }
}

// Rust's cdylib runtime references this C++ ABI symbol. Bionic does not export
// it to libraries loaded outside the C++ runtime namespace, so provide a no-op.
#[no_mangle]
pub extern "C" fn __cxa_finalize(_dso: *mut c_void) {}

#[no_mangle]
pub extern "system" fn Java_org_lwjgl_glfw_CallbackBridge_nativeSetGrabbing(
    _env: *mut c_void,
    _class: *mut c_void,
    grabbing: u8,
) {
    GRABBING.store(grabbing != 0, Ordering::Release);
}

#[no_mangle]
pub extern "system" fn Java_org_lwjgl_glfw_CallbackBridge_nativeIsGrabbing(
    _env: *mut c_void,
    _class: *mut c_void,
) -> u8 {
    u8::from(GRABBING.load(Ordering::Acquire))
}
