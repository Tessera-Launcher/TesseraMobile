//! Desktop entry point, for shaping the UI without an emulator.
//!
//! On Android this binary is not used: the system loads the `cdylib` and calls
//! `android_main` instead.

use slint::ComponentHandle;

fn main() -> Result<(), slint::PlatformError> {
    let ui = tessera_mobile::create()?;
    // The real window is the phone's display; this only shapes the preview.
    ui.window().set_size(slint::PhysicalSize::new(390, 844));
    ui.run()
}
