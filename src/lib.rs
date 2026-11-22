//! Tessera's phone front-end.
//!
//! The window is Slint: `ui/app.slint` owns the layout, this file owns the
//! process entry points and the little state the UI reads. Android has no
//! `main`: the system calls [`android_main`] on a dedicated thread after the
//! activity is created, and the window runs its own event loop until the
//! activity is destroyed.

use std::cell::RefCell;
#[cfg(target_os = "android")]
use std::cell::Cell;
#[cfg(target_os = "android")]
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};

pub mod accounts;
pub mod app_ids;
pub mod aria2;
pub mod download;
pub mod fsutil;
pub mod http;
pub mod jre;
pub mod login;
pub mod settings;
pub mod platform;
pub mod runtime;
pub mod state;
pub mod version_tree;
pub mod versions;

slint::include_modules!();

use login::{Event, Identity};
use settings::Settings;
use state::State;

/// Show the window and run until it closes.
pub fn run() -> Result<(), slint::PlatformError> {
    create()?.run()
}

/// Build the window, load the account list and wire the callbacks.
pub fn create() -> Result<AppWindow, slint::PlatformError> {
    let ui = AppWindow::new()?;
    // Android pins this in `android_main` from the activity's density. On the
    // desktop the window's own scale factor is the fallback.
    let scale = platform::scale_factor(ui.window().scale_factor());
    ui.set_scale_factor(scale);

    let state = Rc::new(RefCell::new(State::new(platform::data_dir())));
    // Fails silently while the storage permission is missing; the permission
    // timer retries once it is granted.
    let _ = state.borrow().ensure_root();
    let rows = Rc::new(VecModel::<AccountRow>::from(Vec::new()));
    ui.set_accounts(ModelRc::from(rows.clone()));
    reload_for(&ui, &state, &rows);

    // `配置.toml` is shared with the desktop build; the phone only owns the
    // GitHub proxy for now. A note means the file was unreadable or newer.
    let (loaded, settings_note) = Settings::load(state.borrow().root());
    let config = Rc::new(RefCell::new(loaded));
    ui.set_github_proxy(config.borrow().github_proxy.clone().into());
    if let Some(note) = settings_note {
        ui.set_settings_status(note.into());
        ui.set_settings_status_error(true);
    }
    ui.set_aria2_installed(aria2::installed().is_some());
    ui.set_jre_installed(jre::installed().is_some());
    ui.set_runtime_installed(runtime::installed().is_some());
    // One install at a time per dependency, independent of what the UI property
    // says.
    let aria2_busy = Arc::new(AtomicBool::new(false));
    let jre_busy = Arc::new(AtomicBool::new(false));
    let runtime_busy = Arc::new(AtomicBool::new(false));
    let versions_loading = Arc::new(AtomicBool::new(false));
    let versions_loaded = Arc::new(AtomicBool::new(false));
    let picker = Arc::new(Mutex::new(versions::Picker::default()));
    {
        let initial = versions::Filter::from_key(&config.borrow().download_filter);
        picker.lock().unwrap().set_filter(initial);
        ui.set_version_filter(slint_filter(initial));
    }

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

    // The download tab opened: files can appear while it is closed, so ask
    // again instead of trusting the value from startup.
    {
        let weak = ui.as_weak();
        let versions_loading = versions_loading.clone();
        let versions_loaded = versions_loaded.clone();
        let picker = picker.clone();
        ui.on_download_opened(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            // A dependency can disappear while the tab is closed (a reset, an
            // uninstall, a failed re-download). Drop the old "installed" line
            // and progress in that case, but never interrupt a live install.
            let aria2_now = aria2::installed().is_some();
            if ui.get_aria2_installed() && !aria2_now && !ui.get_aria2_busy() {
                ui.set_aria2_progress(0.0);
                ui.set_aria2_message("".into());
            }
            ui.set_aria2_installed(aria2_now);

            let jre_now = jre::installed().is_some();
            if ui.get_jre_installed() && !jre_now && !ui.get_jre_busy() {
                ui.set_jre_progress(0.0);
                ui.set_jre_message("".into());
            }
            ui.set_jre_installed(jre_now);

            let runtime_now = runtime::installed().is_some();
            if ui.get_runtime_installed() && !runtime_now && !ui.get_runtime_busy() {
                ui.set_runtime_progress(0.0);
                ui.set_runtime_message("".into());
            }
            ui.set_runtime_installed(runtime_now);
            load_versions(&ui, &versions_loading, &versions_loaded, &picker, false);
        });
    }

    // Install the pinned aria2c. The worker reports through the event loop, so
    // no state is touched from the download thread.
    {
        let weak = ui.as_weak();
        let config = config.clone();
        let busy = aria2_busy.clone();
        let versions_loading = versions_loading.clone();
        let versions_loaded = versions_loaded.clone();
        let picker = picker.clone();
        ui.on_aria2_install_requested(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if busy.swap(true, Ordering::SeqCst) {
                return;
            }
            if !aria2::supported() {
                busy.store(false, Ordering::SeqCst);
                ui.set_aria2_message("当前平台没有固定构建（需要 Android arm64）".into());
                return;
            }
            ui.set_aria2_busy(true);
            ui.set_aria2_progress(0.0);
            ui.set_aria2_message("正在获取 aria2c…".into());

            let proxy = config.borrow().github_proxy.clone();
            let weak = ui.as_weak();
            let busy = busy.clone();
            let versions_loading = versions_loading.clone();
            let versions_loaded = versions_loaded.clone();
            let picker = picker.clone();
            aria2::spawn_install(proxy, move |event| {
                let weak = weak.clone();
                let busy = busy.clone();
                let versions_loading = versions_loading.clone();
                let versions_loaded = versions_loaded.clone();
                let picker = picker.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = weak.upgrade() else {
                        return;
                    };
                    match event {
                        aria2::Event::Log(line) => {
                            ui.set_aria2_message(line.into());
                        }
                        aria2::Event::Progress { done, total, speed } => {
                            let fraction = if total == 0 {
                                0.0
                            } else {
                                done as f32 / total as f32
                            };
                            ui.set_aria2_progress(fraction);
                            let text = if speed > 1.0 {
                                format!(
                                    "下载 aria2c… {} / {} · {}/s",
                                    http::human_size(done),
                                    http::human_size(total),
                                    http::human_size(speed as u64)
                                )
                            } else {
                                format!(
                                    "下载 aria2c… {} / {}",
                                    http::human_size(done),
                                    http::human_size(total)
                                )
                            };
                            ui.set_aria2_message(text.into());
                        }
                        aria2::Event::Installed {
                            version,
                            source,
                            ..
                        } => {
                            busy.store(false, Ordering::SeqCst);
                            ui.set_aria2_busy(false);
                            ui.set_aria2_installed(true);
                            ui.set_aria2_progress(1.0);
                            ui.set_aria2_message(
                                format!("aria2c {version} 已安装 · {source}").into(),
                            );
                            load_versions(&ui, &versions_loading, &versions_loaded, &picker, false);
                        }
                        aria2::Event::Failed(problem) => {
                            busy.store(false, Ordering::SeqCst);
                            ui.set_aria2_busy(false);
                            ui.set_aria2_progress(0.0);
                            ui.set_aria2_message(problem.into());
                        }
                    }
                });
            });
        });
    }

    // Install the JRE. Same staging as aria2c: the manifest names the zip and
    // its hash, and the worker reports through the event loop.
    {
        let weak = ui.as_weak();
        let config = config.clone();
        let busy = jre_busy.clone();
        let versions_loading = versions_loading.clone();
        let versions_loaded = versions_loaded.clone();
        let picker = picker.clone();
        ui.on_jre_install_requested(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if busy.swap(true, Ordering::SeqCst) {
                return;
            }
            if !jre::supported() {
                busy.store(false, Ordering::SeqCst);
                ui.set_jre_message("当前平台没有固定构建（需要 Android arm64）".into());
                return;
            }
            if aria2::installed().is_none() {
                busy.store(false, Ordering::SeqCst);
                ui.set_jre_message("请先安装 aria2c，再下载 JRE".into());
                return;
            }
            ui.set_jre_busy(true);
            ui.set_jre_progress(0.0);
            ui.set_jre_message("正在获取 JRE 清单…".into());

            let proxy = config.borrow().github_proxy.clone();
            let weak = ui.as_weak();
            let busy = busy.clone();
            let versions_loading = versions_loading.clone();
            let versions_loaded = versions_loaded.clone();
            let picker = picker.clone();
            jre::spawn_install(proxy, move |event| {
                let weak = weak.clone();
                let busy = busy.clone();
                let versions_loading = versions_loading.clone();
                let versions_loaded = versions_loaded.clone();
                let picker = picker.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = weak.upgrade() else {
                        return;
                    };
                    match event {
                        jre::Event::Log(line) => {
                            ui.set_jre_message(line.into());
                        }
                        jre::Event::Progress { done, total, speed } => {
                            let fraction = if total == 0 {
                                0.0
                            } else {
                                done as f32 / total as f32
                            };
                            ui.set_jre_progress(fraction);
                            let text = if speed > 1.0 {
                                format!(
                                    "下载 JRE… {} / {} · {}/s",
                                    http::human_size(done),
                                    http::human_size(total),
                                    http::human_size(speed as u64)
                                )
                            } else {
                                format!(
                                    "下载 JRE… {} / {}",
                                    http::human_size(done),
                                    http::human_size(total)
                                )
                            };
                            ui.set_jre_message(text.into());
                        }
                        jre::Event::Extracting => {
                            ui.set_jre_progress(1.0);
                            ui.set_jre_message("校验完成，正在解压 JRE…".into());
                        }
                        jre::Event::Installed { id, source, .. } => {
                            busy.store(false, Ordering::SeqCst);
                            ui.set_jre_busy(false);
                            ui.set_jre_installed(true);
                            ui.set_jre_progress(1.0);
                            ui.set_jre_message(
                                format!("JRE {id} 已安装 · {source}").into(),
                            );
                            load_versions(&ui, &versions_loading, &versions_loaded, &picker, false);
                        }
                        jre::Event::Failed(problem) => {
                            busy.store(false, Ordering::SeqCst);
                            ui.set_jre_busy(false);
                            ui.set_jre_progress(0.0);
                            ui.set_jre_message(problem.into());
                        }
                    }
                });
            });
        });
    }

    // Install the Android runtime pack (FCL LWJGL/OpenAL/SDL/JNA). Downloads
    // go through aria2c, so the same gate as the JRE applies.
    {
        let weak = ui.as_weak();
        let config = config.clone();
        let busy = runtime_busy.clone();
        let versions_loading = versions_loading.clone();
        let versions_loaded = versions_loaded.clone();
        let picker = picker.clone();
        ui.on_runtime_install_requested(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if busy.swap(true, Ordering::SeqCst) {
                return;
            }
            if !runtime::supported() {
                busy.store(false, Ordering::SeqCst);
                ui.set_runtime_message("当前平台没有固定构建（需要 Android arm64）".into());
                return;
            }
            if aria2::installed().is_none() {
                busy.store(false, Ordering::SeqCst);
                ui.set_runtime_message("请先安装 aria2c，再下载运行库".into());
                return;
            }
            ui.set_runtime_busy(true);
            ui.set_runtime_progress(0.0);
            ui.set_runtime_message("正在准备运行库…".into());

            let proxy = config.borrow().github_proxy.clone();
            let weak = ui.as_weak();
            let busy = busy.clone();
            let versions_loading = versions_loading.clone();
            let versions_loaded = versions_loaded.clone();
            let picker = picker.clone();
            runtime::spawn_install(proxy, move |event| {
                let weak = weak.clone();
                let busy = busy.clone();
                let versions_loading = versions_loading.clone();
                let versions_loaded = versions_loaded.clone();
                let picker = picker.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = weak.upgrade() else {
                        return;
                    };
                    match event {
                        runtime::Event::Log(line) => {
                            ui.set_runtime_message(line.into());
                        }
                        runtime::Event::Progress { done, total, speed } => {
                            let fraction = if total == 0 {
                                0.0
                            } else {
                                done as f32 / total as f32
                            };
                            ui.set_runtime_progress(fraction);
                            let text = if speed > 1.0 {
                                format!(
                                    "下载运行库… {} / {} · {}/s",
                                    http::human_size(done),
                                    http::human_size(total),
                                    http::human_size(speed as u64)
                                )
                            } else {
                                format!(
                                    "下载运行库… {} / {}",
                                    http::human_size(done),
                                    http::human_size(total)
                                )
                            };
                            ui.set_runtime_message(text.into());
                        }
                        runtime::Event::Extracting => {
                            ui.set_runtime_progress(1.0);
                            ui.set_runtime_message("校验完成，正在解压运行库…".into());
                        }
                        runtime::Event::Installed { .. } => {
                            busy.store(false, Ordering::SeqCst);
                            ui.set_runtime_busy(false);
                            ui.set_runtime_installed(true);
                            ui.set_runtime_progress(1.0);
                            ui.set_runtime_message("MobileGlues 渲染器已安装".into());
                            load_versions(&ui, &versions_loading, &versions_loaded, &picker, false);
                        }
                        runtime::Event::Failed(problem) => {
                            busy.store(false, Ordering::SeqCst);
                            ui.set_runtime_busy(false);
                            ui.set_runtime_progress(0.0);
                            ui.set_runtime_message(problem.into());
                        }
                    }
                });
            });
        });
    }

    // The version list's refresh button: force a new fetch even if the last
    // one succeeded.
    {
        let weak = ui.as_weak();
        let versions_loading = versions_loading.clone();
        let versions_loaded = versions_loaded.clone();
        let picker = picker.clone();
        ui.on_versions_refresh(move || {
            if let Some(ui) = weak.upgrade() {
                load_versions(&ui, &versions_loading, &versions_loaded, &picker, true);
            }
        });
    }

    // Search and kind filter. The picker owns the filtered indices and the
    // folded rows; this just republishes the model.
    {
        let weak = ui.as_weak();
        let picker = picker.clone();
        let config = config.clone();
        let state = state.clone();
        ui.on_versions_refilter(move |filter, query| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let filter = match filter {
                VersionFilter::All => versions::Filter::All,
                VersionFilter::Release => versions::Filter::Release,
                VersionFilter::Snapshot => versions::Filter::Snapshot,
                VersionFilter::Old => versions::Filter::Old,
            };
            let mut picker = picker.lock().unwrap();
            picker.set_filter(filter);
            picker.set_query(query.to_string());
            publish_versions(&ui, &picker);
            drop(picker);

            let key = filter.key().to_string();
            let mut next = config.borrow().clone();
            if next.download_filter != key {
                next.download_filter = key;
                let root = state.borrow().root().to_path_buf();
                if next.save(&root).is_ok() {
                    *config.borrow_mut() = next;
                }
            }
        });
    }

    {
        let weak = ui.as_weak();
        let picker = picker.clone();
        ui.on_version_group_toggled(move |key| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut picker = picker.lock().unwrap();
            picker.toggle(&key);
            publish_versions(&ui, &picker);
        });
    }

    // Save the GitHub proxy into the shared `配置.toml`.
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let config = config.clone();
        ui.on_github_proxy_saved(move |value| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let value = value.trim().to_string();
            let mut next = config.borrow().clone();
            next.github_proxy = value.clone();
            *config.borrow_mut() = next.clone();
            ui.set_github_proxy(value.clone().into());

            let root = state.borrow().root().to_path_buf();
            match next.save(&root) {
                Ok(()) => {
                    ui.set_settings_status_error(false);
                    ui.set_settings_status(
                        if value.is_empty() {
                            "已设为直连".to_string()
                        } else {
                            format!("已保存：{value}")
                        }
                        .into(),
                    );
                }
                Err(error) => {
                    ui.set_settings_status_error(true);
                    ui.set_settings_status(format!("保存失败：{error}").into());
                }
            }
        });
    }

    // Instances live under `/sdcard`, which Android 11+ only allows with the
    // "all files access" special permission. Ask as soon as the window has
    // focus, and ask again every time the user comes back without granting it:
    // the system page is the only place that can grant it.
    #[cfg(target_os = "android")]
    {
        let timer = Timer::default();
        // Fast while the permission is missing; a slow re-check once it is
        // granted, so revoking it in Settings is noticed too.
        let granted = Rc::new(Cell::new(false));
        let ticks = Rc::new(Cell::new(0u32));
        let last_prompt = Rc::new(RefCell::new(None::<std::time::Instant>));
        let weak_ui = ui.as_weak();
        let timer_state = state.clone();
        let timer_rows = rows.clone();
        timer.start(TimerMode::Repeated, Duration::from_millis(500), move || {
            let tick = ticks.get().wrapping_add(1);
            ticks.set(tick);
            if granted.get() && tick % 4 != 0 {
                return;
            }
            if platform::all_files_access() {
                if !granted.replace(true) {
                    // The data root lives on `/sdcard`, so it could not be
                    // created before this permission existed. Now it can, and
                    // the account list can be read from it for the first time.
                    let _ = timer_state.borrow().ensure_root();
                    if let Some(ui) = weak_ui.upgrade() {
                        reload_for(&ui, &timer_state, &timer_rows);
                    }
                }
                return;
            }
            granted.set(false);
            // While Settings is in front the user is deciding; re-opening it
            // from under itself would make the page unreachable.
            if !platform::window_focused() {
                return;
            }
            // The activity can keep focus for a frame after `startActivity`,
            // so a cooldown stops a second page from being stacked on it.
            let now = std::time::Instant::now();
            {
                let mut last = last_prompt.borrow_mut();
                if last.is_some_and(|at| now.duration_since(at) < Duration::from_millis(1200)) {
                    return;
                }
                *last = Some(now);
            }
            if let Err(problem) = platform::request_all_files_access() {
                eprintln!("无法打开「所有文件访问权限」设置：{problem}");
            }
        });
        // The timer has to outlive `create`; the window is the only owner it
        // could have, so let the process keep it.
        std::mem::forget(timer);
    }

    Ok(ui)
}

/// Re-read the account directory and push it into the window's model.
fn reload_for(ui: &AppWindow, state: &Rc<RefCell<State>>, rows: &Rc<VecModel<AccountRow>>) {
    state.borrow_mut().reload();
    refresh(ui, state, rows);
}

fn slint_filter(filter: versions::Filter) -> VersionFilter {
    match filter {
        versions::Filter::All => VersionFilter::All,
        versions::Filter::Release => VersionFilter::Release,
        versions::Filter::Snapshot => VersionFilter::Snapshot,
        versions::Filter::Old => VersionFilter::Old,
    }
}

/// Fetch the Minecraft version index once all four pieces are installed.
///
/// Runs from the UI thread (either a tab open, an install completion, or the
/// refresh button), so it reads the current installed properties instead of
/// keeping a second copy of the dependency state.
fn load_versions(
    ui: &AppWindow,
    loading: &Arc<AtomicBool>,
    loaded: &Arc<AtomicBool>,
    picker: &Arc<Mutex<versions::Picker>>,
    force: bool,
) {
    if !(ui.get_aria2_installed() && ui.get_jre_installed() && ui.get_runtime_installed()) {
        return;
    }
    if loading.load(Ordering::SeqCst) {
        return;
    }
    if !force && loaded.load(Ordering::SeqCst) {
        return;
    }
    loading.store(true, Ordering::SeqCst);
    loaded.store(false, Ordering::SeqCst);
    ui.set_versions_loading(true);
    ui.set_versions_error("".into());

    let weak = ui.as_weak();
    let loading = loading.clone();
    let loaded = loaded.clone();
    let picker = picker.clone();
    versions::spawn(move |result| {
        let weak = weak.clone();
        let loading = loading.clone();
        let loaded = loaded.clone();
        let picker = picker.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            loading.store(false, Ordering::SeqCst);
            ui.set_versions_loading(false);
            match result {
                Ok(rows) => {
                    loaded.store(true, Ordering::SeqCst);
                    let mut picker = picker.lock().unwrap();
                    picker.set_versions(rows);
                    publish_versions(&ui, &picker);
                }
                Err(problem) => {
                    ui.set_versions_error(problem.into());
                }
            }
        });
    });
}

/// Turn the picker's folded rows into the Slint model.
fn publish_versions(ui: &AppWindow, picker: &versions::Picker) {
    let mut built: Vec<VersionRow> = Vec::with_capacity(picker.rows().len());
    for row in picker.rows() {
        match &row.kind {
            version_tree::RowKind::Version(index) => {
                if let Some(version) = picker.version(*index) {
                    built.push(VersionRow {
                        label: version.id.clone().into(),
                        detail: format!(
                            "{} · {}",
                            versions::kind_label(version.kind),
                            version.date
                        )
                        .into(),
                        group: false,
                        expanded: false,
                        depth: row.depth as i32,
                        key: "".into(),
                    });
                }
            }
            version_tree::RowKind::Group {
                key,
                first,
                last,
                expanded,
            } => {
                let label = version_tree::title(&picker.all, picker.visible(), *first, *last);
                built.push(VersionRow {
                    label: label.into(),
                    detail: format!("{} 个版本", last - first + 1).into(),
                    group: true,
                    expanded: *expanded,
                    depth: row.depth as i32,
                    key: key.clone().into(),
                });
            }
        }
    }
    ui.set_version_count(picker.count() as i32);
    ui.set_versions(ModelRc::from(Rc::new(VecModel::from_slice(&built))));
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
    // `ndk_context` only carries the Application, so keep the activity from
    // `android-activity` for the calls that need it (window focus).
    platform::set_activity(app.activity_as_ptr());
    // Executables (aria2c) cannot run from `/sdcard`; keep them in the app's
    // private files directory, separate from the shared data root.
    if let Some(dir) = app.internal_data_path() {
        platform::set_private_dir(dir);
    }
    // Everything the launcher owns lives on shared storage, next to the
    // instances, so it survives reinstalls and is reachable over USB. Android
    // reports the per-user volume; `/sdcard` is the conventional fallback.
    let root = platform::external_storage_root().unwrap_or_else(|| PathBuf::from("/sdcard"));
    platform::set_data_dir(root.join("Tessera"));
    // Same mapping Slint's Android backend uses (dpi / 160). Read before
    // `init` consumes the app; the window's own value is not ready until the
    // native window is.
    if let Some(dpi) = app.config().density() {
        platform::set_scale_factor(dpi as f32 / 160.0);
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
