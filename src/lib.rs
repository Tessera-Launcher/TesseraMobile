//! Tessera's phone front-end.
//!
//! The window is Slint: `ui/app.slint` owns the layout, this file owns the
//! process entry points and the little state the UI reads. Android has no
//! `main`: the system calls [`android_main`] on a dedicated thread after the
//! activity is created, and the window runs its own event loop until the
//! activity is destroyed.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

pub mod accounts;
pub mod platform;
pub mod state;

slint::include_modules!();

use state::State;

/// Show the window and run until it closes.
pub fn run() -> Result<(), slint::PlatformError> {
    create()?.run()
}

/// Build the window, load the account list and wire the callbacks.
pub fn create() -> Result<AppWindow, slint::PlatformError> {
    let ui = AppWindow::new()?;

    let state = Rc::new(RefCell::new(State::default()));
    let rows = Rc::new(VecModel::<AccountRow>::from(Vec::new()));
    ui.set_accounts(ModelRc::from(rows.clone()));
    reload(&state, &rows);

    // The 账户 button on the home page: re-read the directory, then push the
    // screen. Files can change while the screen is closed.
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let rows = rows.clone();
        ui.on_accounts_requested(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            reload(&state, &rows);
            ui.set_route(Route::Accounts);
        });
    }

    // The back arrow on the account screen.
    {
        let weak = ui.as_weak();
        ui.on_accounts_closed(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_route(Route::Tabs);
            }
        });
    }

    Ok(ui)
}

/// Re-read the account directory and push it into the window's model.
fn reload(state: &Rc<RefCell<State>>, rows: &Rc<VecModel<AccountRow>>) {
    let mut state = state.borrow_mut();
    state.reload();
    let built: Vec<AccountRow> = state
        .accounts
        .iter()
        .enumerate()
        .map(|(index, account)| AccountRow {
            index: index as i32,
            name: account.name().into(),
            application: account.application.clone().into(),
            offline: account.offline,
            active: state.is_active(account),
        })
        .collect();
    rows.set_vec(built);
}

/// Android's entry point, called from `NativeActivity.onCreate`.
///
/// `slint::android::init` installs the platform backend and must run before any
/// component is created; it also wires Slint's embedded Java helper, which is
/// what gives `TextInput` a real input connection (the soft keyboard).
#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: slint::android::AndroidApp) {
    // The app's private storage. Everything the launcher owns lives below it.
    if let Some(dir) = app.internal_data_path() {
        platform::set_data_dir(dir);
    }
    slint::android::init(app).expect("slint android backend must start");
    run().expect("window must run");
}
