//! Tessera's phone front-end.
//!
//! The window is Slint: `ui/app.slint` owns the layout, this file owns the
//! process entry points and the little state the UI reads. Android has no
//! `main`: the system calls [`android_main`] on a dedicated thread after the
//! activity is created, and the window runs its own event loop until the
//! activity is destroyed.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};

pub mod accounts;
pub mod app_ids;
pub mod login;
pub mod platform;
pub mod state;

slint::include_modules!();

use login::{Event, Identity};
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

    // ---- login ----

    // Identities for the login screen, in the order the rows show them: the
    // official launcher first, then the ids from azure_app_ids.toml.
    let identities: Rc<RefCell<Vec<Identity>>> = Rc::new(RefCell::new(Vec::new()));
    // The worker's channel, plus the timer that drains it. The timer is a
    // handle: dropping every strong reference stops it.
    let pending: Rc<RefCell<Option<Receiver<Event>>>> = Rc::new(RefCell::new(None));
    let login_timer = Rc::new(Timer::default());

    // The chooser picked the Microsoft flow.
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let identities = identities.clone();
        ui.on_login_requested(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let state = state.borrow();
            let path = app_ids::path_under(&state.credentials_dir());
            match app_ids::load(&path) {
                Ok(ids) => {
                    let mut rows = vec![IdentityRow {
                        index: 0,
                        label: "官方启动器".into(),
                        detail: "微软账户".into(),
                    }];
                    let mut built = vec![Identity::Official];
                    for (position, id) in ids.iter().enumerate() {
                        rows.push(IdentityRow {
                            index: position as i32 + 1,
                            label: id.label().into(),
                            detail: id.client_id().into(),
                        });
                        built.push(Identity::Saved {
                            name: id.label().to_string(),
                            app_id: id.client_id().to_string(),
                        });
                    }
                    drop(state);
                    *identities.borrow_mut() = built;
                    ui.set_identities(ModelRc::from(Rc::new(VecModel::from(rows))));
                    ui.set_login_error("".into());
                    ui.set_login_phase(LoginPhase::Choosing);
                    ui.set_route(Route::Login);
                }
                Err(problem) => {
                    drop(state);
                    ui.set_login_error(problem.into());
                    ui.set_login_phase(LoginPhase::Failed);
                    ui.set_route(Route::Login);
                }
            }
        });
    }

    // An identity was picked: start the worker and drain it from a timer.
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let rows = rows.clone();
        let identities = identities.clone();
        let pending = pending.clone();
        let login_timer = login_timer.clone();
        ui.on_login_start(move |index| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let Some(identity) = identities.borrow().get(index.max(0) as usize).cloned() else {
                return;
            };
            begin_login(&ui, &state, &rows, &pending, &login_timer, identity);
        });
    }

    // A client id was typed instead of picked from the file.
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let rows = rows.clone();
        let pending = pending.clone();
        let login_timer = login_timer.clone();
        ui.on_login_start_custom(move |app_id| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let app_id = app_id.trim().to_string();
            if app_id.is_empty() {
                // The desktop falls back to the row under the cursor here;
                // on a phone there is no cursor, so say what is missing.
                ui.set_login_custom_error("请输入 app id，或返回选一个现有身份".into());
                return;
            }
            ui.set_login_custom_error("".into());
            ui.set_login_custom_open(false);
            begin_login(
                &ui,
                &state,
                &rows,
                &pending,
                &login_timer,
                Identity::Custom { app_id },
            );
        });
    }

    // Back from the login screen: stop waiting, go back to the list.
    {
        let weak = ui.as_weak();
        let pending = pending.clone();
        let login_timer = login_timer.clone();
        ui.on_login_cancelled(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            login_timer.stop();
            *pending.borrow_mut() = None;
            ui.set_login_phase(LoginPhase::Choosing);
            ui.set_login_code("".into());
            ui.set_login_url("".into());
            ui.set_route(Route::Accounts);
        });
    }

    // The two things the device-code screen hands the system.
    {
        let weak = ui.as_weak();
        ui.on_login_open_url(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let url = ui.get_login_url();
            if let Err(problem) = platform::open_url(&url) {
                ui.set_login_error(problem.into());
            }
        });
    }

    {
        let weak = ui.as_weak();
        ui.on_login_copy_code(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let code = ui.get_login_code();
            if let Err(problem) = platform::copy_text(&code) {
                ui.set_login_error(problem.into());
            }
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

/// Start one sign-in and keep draining it until it ends.
///
/// Both entry points (a listed identity, a typed client id) land here, so the
/// timer setup and the teardown on a terminal event live in one place.
fn begin_login(
    ui: &AppWindow,
    state: &Rc<RefCell<State>>,
    rows: &Rc<VecModel<AccountRow>>,
    pending: &Rc<RefCell<Option<Receiver<Event>>>>,
    login_timer: &Rc<Timer>,
    identity: Identity,
) {
    let accounts_dir = state.borrow().accounts_dir();
    let receiver = login::spawn(identity, accounts_dir);
    *pending.borrow_mut() = Some(receiver);

    ui.set_login_code("".into());
    ui.set_login_url("".into());
    ui.set_login_error("".into());
    ui.set_login_phase(LoginPhase::Waiting);

    let weak = ui.as_weak();
    let state = state.clone();
    let rows = rows.clone();
    let pending = pending.clone();
    // Weak, so the timer's own callback does not keep the timer alive.
    let timer = Rc::downgrade(login_timer);
    login_timer.start(TimerMode::Repeated, Duration::from_millis(150), move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let stop = |timer: &std::rc::Weak<Timer>| {
            if let Some(timer) = timer.upgrade() {
                timer.stop();
            }
        };

        // Three outcomes, not two: an empty channel means "keep waiting", a
        // closed one means the thread died. Collapsing both into `Option`
        // reported a failure on the first tick.
        enum Polled {
            Event(Event),
            Closed,
            Idle,
        }
        let polled = {
            let receiver = pending.borrow();
            match receiver.as_ref() {
                Some(receiver) => match receiver.try_recv() {
                    Ok(event) => Polled::Event(event),
                    Err(std::sync::mpsc::TryRecvError::Empty) => Polled::Idle,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Polled::Closed,
                },
                None => Polled::Idle,
            }
        };

        let event = match polled {
            Polled::Idle => return,
            Polled::Closed => {
                *pending.borrow_mut() = None;
                stop(&timer);
                ui.set_login_error("登录线程意外结束".into());
                ui.set_login_phase(LoginPhase::Failed);
                return;
            }
            Polled::Event(event) => event,
        };

        match event {
            // Device code is the only progress the worker reports until it is
            // done.
            Event::DeviceCode { user_code, url } => {
                ui.set_login_code(user_code.into());
                ui.set_login_url(url.into());
            }
            Event::Succeeded { file, .. } => {
                *pending.borrow_mut() = None;
                stop(&timer);
                // Re-read instead of appending: the numbering of a repeated
                // name comes from disk. Then make the account the worker just
                // wrote the active one.
                state.borrow_mut().reload();
                let added = state
                    .borrow()
                    .accounts
                    .iter()
                    .position(|account| account.file == file);
                if let Some(index) = added {
                    state.borrow_mut().activate(index);
                }
                refresh(&ui, &state, &rows);
                ui.set_login_phase(LoginPhase::Choosing);
                ui.set_login_code("".into());
                ui.set_login_url("".into());
                ui.set_route(Route::Accounts);
            }
            Event::Failed(problem) => {
                *pending.borrow_mut() = None;
                stop(&timer);
                ui.set_login_error(problem.into());
                ui.set_login_phase(LoginPhase::Failed);
            }
        }
    });
}
