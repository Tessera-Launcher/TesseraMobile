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
pub mod app_ids;
pub mod login;
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

    let state = Rc::new(RefCell::new(State::new(platform::data_dir())));
    let rows = Rc::new(VecModel::<AccountRow>::from(Vec::new()));
    ui.set_accounts(ModelRc::from(rows.clone()));
    reload_for(&ui, &state, &rows);

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
            reload_for(&ui, &state, &rows);
            ui.set_route(Route::Accounts);
        });
    }

    // Tapping a row makes it the account the launcher would use.
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let rows = rows.clone();
        ui.on_account_activated(move |index| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            // Bind first: a borrow held across `refresh` would re-enter the
            // RefCell and panic.
            let activated = state.borrow_mut().activate(index.max(0) as usize);
            if activated {
                refresh(&ui, &state, &rows);
            }
        });
    }

    // The confirmation sheet on the account screen was accepted.
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let rows = rows.clone();
        ui.on_account_remove(move |index| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let removed = state.borrow_mut().remove(index.max(0) as usize);
            if let Err(error) = removed {
                // The row stays on screen; this is the only report there is
                // until the launcher has a message surface.
                eprintln!("移除账户失败：{error}");
            }
            refresh(&ui, &state, &rows);
        });
    }

    // The add sheet was accepted: validate, write, make it the active account.
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let rows = rows.clone();
        ui.on_account_add_offline(move |name, allow_any| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let name = name.trim().to_string();
            if let Some(problem) = accounts::offline_name_problem(&name, allow_any) {
                ui.set_add_error(problem.into());
                return;
            }
            let added = state.borrow_mut().add_offline(&name);
            match added {
                Ok(_) => {
                    ui.set_adding(false);
                    ui.set_add_name("".into());
                    ui.set_add_any(false);
                    ui.set_add_error("".into());
                    refresh(&ui, &state, &rows);
                }
                Err(error) => {
                    ui.set_add_error(format!("无法写入账户文件：{error}").into());
                }
            }
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
fn reload_for(ui: &AppWindow, state: &Rc<RefCell<State>>, rows: &Rc<VecModel<AccountRow>>) {
    state.borrow_mut().reload();
    refresh(ui, state, rows);
}

/// Rebuild the row model from the state that is already in memory.
fn refresh(ui: &AppWindow, state: &Rc<RefCell<State>>, rows: &Rc<VecModel<AccountRow>>) {
    let state = state.borrow();
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
    ui.set_active_label(
        state
            .active_account()
            .map(|account| account.label())
            .unwrap_or_default()
            .into(),
    );
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
