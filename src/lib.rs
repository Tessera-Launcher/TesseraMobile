//! Tessera's phone front-end.
//!
//! The window is Slint: `ui/app.slint` owns the layout, `src/lib.rs` owns the
//! process entry points. Android has no `main`: the system calls
//! [`android_main`] on a dedicated thread after the activity is created, and
//! the window runs its own event loop until the activity is destroyed.

use slint::ComponentHandle;

slint::include_modules!();

/// Show the window and run until it closes.
///
/// Desktop `main` and [`android_main`] both end here, so there is exactly one
/// place that decides what the app looks like.
pub fn create() -> Result<AppWindow, slint::PlatformError> {
    AppWindow::new()
}

pub fn run() -> Result<(), slint::PlatformError> {
    create()?.run()
}

/// Android's entry point, called from `NativeActivity.onCreate`.
///
/// `slint::android::init` installs the platform backend and must run before any
/// component is created; it also wires Slint's embedded Java helper, which is
/// what gives `TextInput` a real input connection (the soft keyboard).
#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: slint::android::AndroidApp) {
    slint::android::init(app).expect("slint android backend must start");
    run().expect("window must run");
}
