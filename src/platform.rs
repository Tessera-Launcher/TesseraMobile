//! The launcher's data root, and the system capabilities the UI needs.
//!
//! The desktop build is portable: everything lives in `data/` next to the
//! binary. A phone has no such directory, so the launcher keeps its data on
//! shared storage (`/sdcard/Tessera`), like the other phone launchers, and
//! [`set_data_dir`] pins that path before the first frame.
//!
//! Slint's public Rust API has no "open this URL" and no clipboard, although its
//! platform layer supports both. So those two live here: JNI against the
//! activity on Android, the usual desktop helpers elsewhere.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::OnceLock;

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The app's private files directory (`Context.getFilesDir()`), kept apart from
/// the shared data root.
///
/// Only executables live here. `/sdcard` is mounted `noexec`, so aria2c cannot
/// run from the data directory; the app's own files directory can (this build
/// targets API 28, which predates the no-exec policy).
static PRIVATE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The Android `Activity`, kept as the global reference `android-activity` owns.
///
/// `ndk_context` only holds the `Application` context, which is enough for
/// `startActivity` and `getSystemService` but not for activity-only calls such
/// as `hasWindowFocus`. The pointer stays valid as long as the backend holds
/// the `AndroidApp`, i.e. for the life of the event loop.
static ACTIVITY: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// Remember the activity pointer. Called once from `android_main`.
pub fn set_activity(ptr: *mut c_void) {
    ACTIVITY.store(ptr, Ordering::SeqCst);
}

/// Device pixel ratio, pinned at start-up on Android.
///
/// The window backend reports it only once the native window exists, which is
/// after the first frame is built, and Slint's `.slint` language has no
/// accessor for it. The UI needs it early to size a hairline.
static SCALE_FACTOR: OnceLock<f32> = OnceLock::new();

/// Pin the data root. Called once at start-up; later calls are ignored.
pub fn set_data_dir(path: PathBuf) {
    let _ = DATA_DIR.set(path);
}

/// Pin the app-private files directory. Later calls are ignored.
pub fn set_private_dir(path: PathBuf) {
    let _ = PRIVATE_DIR.set(path);
}

/// The app-private files directory; the data root is the desktop fallback.
pub fn private_dir() -> PathBuf {
    PRIVATE_DIR.get().cloned().unwrap_or_else(data_dir)
}

/// Pin the device pixel ratio. Called once at start-up; later calls are ignored.
pub fn set_scale_factor(scale: f32) {
    let _ = SCALE_FACTOR.set(scale);
}

/// The device pixel ratio: the pinned value, or `fallback` while it is unknown.
pub fn scale_factor(fallback: f32) -> f32 {
    SCALE_FACTOR.get().copied().unwrap_or(fallback)
}

/// The shared-storage root (`/sdcard`), or `None` where there is none.
///
/// Asked from Android rather than hardcoded so per-user and multi-user devices
/// resolve to the right volume.
pub fn external_storage_root() -> Option<PathBuf> {
    imp::external_storage_root()
}

/// The launcher's data root.
///
/// `TESSERA_DATA` overrides it (tests, checkouts); otherwise `./data`, so
/// `cargo run` on a desktop behaves like the desktop build. On Android
/// `android_main` pins `/sdcard/Tessera` before this is ever called.
pub fn data_dir() -> PathBuf {
    if let Some(root) = DATA_DIR.get() {
        return root.clone();
    }
    if let Some(root) = std::env::var_os("TESSERA_DATA") {
        return PathBuf::from(root);
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("data")
}

/// Hand a URL to the system browser.
pub fn open_url(url: &str) -> Result<(), String> {
    imp::open_url(url)
}

/// Put `text` on the system clipboard.
pub fn copy_text(text: &str) -> Result<(), String> {
    imp::copy_text(text)
}

/// Whether the app can read and write anywhere under `/sdcard`.
///
/// Android 11 (API 30) introduced "all files access"; below that the legacy
/// storage model already gives the app the access it needs. Anything that
/// cannot be answered counts as missing, so the prompt still appears.
pub fn all_files_access() -> bool {
    imp::all_files_access()
}

/// Whether the activity currently has input focus.
///
/// The permission prompt must not fire while the system Settings page it just
/// opened is in front, or the page would be re-opened from under itself.
/// Regaining focus is the signal that the user is back.
pub fn window_focused() -> bool {
    imp::window_focused()
}

pub fn start_game_activity(version: &str) -> Result<(), String> {
    imp::start_game_activity(version)
}


/// Open the system page where "all files access" is granted.
pub fn request_all_files_access() -> Result<(), String> {
    imp::request_all_files_access()
}

#[cfg(target_os = "android")]
mod imp {
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    use jni::objects::{JObject, JString, JValue};
    use jni::JavaVM;

    /// `Intent.FLAG_ACTIVITY_NEW_TASK`: the activity is not the one that is
    /// currently resumed when the login sheet asks for a browser.
    const FLAG_ACTIVITY_NEW_TASK: i32 = 0x1000_0000;

    /// Run `f` on the current thread with a JNI env and the activity.
    ///
    /// The activity comes from [`set_activity`]; until that runs, `ndk_context`
    /// (which holds the `Application` context) is the fallback. Attaching is
    /// per-thread and the guard detaches again when the call returns.
    fn with_activity<T>(
        f: impl FnOnce(&mut jni::JNIEnv, &JObject) -> Result<T, jni::errors::Error>,
    ) -> Result<T, String> {
        let context = ndk_context::android_context();
        let vm = unsafe { JavaVM::from_raw(context.vm().cast()) }.map_err(|e| e.to_string())?;
        let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
        let stored = super::ACTIVITY.load(Ordering::SeqCst);
        let raw = if stored.is_null() {
            // Application context: a global reference owned by android-activity.
            context.context().cast()
        } else {
            stored
        };
        // Only a view of a global reference; nothing here owns or deletes it.
        let activity = unsafe { JObject::from_raw(raw.cast()) };
        f(&mut env, &activity).map_err(|e| e.to_string())
    }

    pub fn open_url(url: &str) -> Result<(), String> {
        with_activity(|env, activity| {
            let action = env.new_string("android.intent.action.VIEW")?;
            let url = env.new_string(url)?;
            let uri = env
                .call_static_method(
                    "android/net/Uri",
                    "parse",
                    "(Ljava/lang/String;)Landroid/net/Uri;",
                    &[JValue::Object(&JObject::from(url))],
                )?
                .l()?;
            let intent = env.new_object(
                "android/content/Intent",
                "(Ljava/lang/String;Landroid/net/Uri;)V",
                &[
                    JValue::Object(&JObject::from(action)),
                    JValue::Object(&uri),
                ],
            )?;
            env.call_method(
                &intent,
                "addFlags",
                "(I)Landroid/content/Intent;",
                &[JValue::Int(FLAG_ACTIVITY_NEW_TASK)],
            )?;
            env.call_method(
                activity,
                "startActivity",
                "(Landroid/content/Intent;)V",
                &[JValue::Object(&intent)],
            )?;
            Ok(())
        })
    }

    fn sdk_int(env: &mut jni::JNIEnv) -> Result<i32, jni::errors::Error> {
        env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?.i()
    }

    pub fn external_storage_root() -> Option<PathBuf> {
        with_activity(|env, _| {
            let file = env
                .call_static_method(
                    "android/os/Environment",
                    "getExternalStorageDirectory",
                    "()Ljava/io/File;",
                    &[],
                )?
                .l()?;
            let path = env
                .call_method(&file, "getAbsolutePath", "()Ljava/lang/String;", &[])?
                .l()?;
            let path: String = env.get_string(&JString::from(path))?.into();
            Ok(PathBuf::from(path))
        })
        .ok()
    }

    pub fn all_files_access() -> bool {
        with_activity(|env, _| {
            if sdk_int(env)? < 30 {
                return Ok(true);
            }
            env.call_static_method(
                "android/os/Environment",
                "isExternalStorageManager",
                "()Z",
                &[],
            )?
            .z()
        })
        .unwrap_or(false)
    }

    pub fn window_focused() -> bool {
        with_activity(|env, activity| {
            env.call_method(activity, "hasWindowFocus", "()Z", &[])?.z()
        })
        .unwrap_or(false)
    }

    pub fn start_game_activity(version: &str) -> Result<(), String> {
        with_activity(|env, activity| {
            // FindClass on a native thread uses the bootstrap loader, not the
            // app's dex loader. Go through the Activity's ClassLoader instead.
            let loader = env
                .call_method(activity, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?
                .l()?;
            let class_name = env.new_string("org.tessera.mobile.GameActivity")?;
            let class = env
                .call_method(
                    &loader,
                    "loadClass",
                    "(Ljava/lang/String;)Ljava/lang/Class;",
                    &[JValue::Object(&JObject::from(class_name))],
                )?
                .l()?;
            let intent = env.new_object(
                "android/content/Intent",
                "(Landroid/content/Context;Ljava/lang/Class;)V",
                &[
                    JValue::Object(activity),
                    JValue::Object(&class),
                ],
            )?;
            let key = env.new_string("version")?;
            let value = env.new_string(version)?;
            env.call_method(
                &intent,
                "putExtra",
                "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
                &[
                    JValue::Object(&JObject::from(key)),
                    JValue::Object(&JObject::from(value)),
                ],
            )?;
            env.call_method(
                activity,
                "startActivity",
                "(Landroid/content/Intent;)V",
                &[JValue::Object(&intent)],
            )?;
            Ok(())
        })
    }

    /// Start one of the two Settings intents. `uri` picks the per-app page.
    fn start_settings(
        env: &mut jni::JNIEnv,
        activity: &JObject,
        action: &str,
        uri: Option<&JObject>,
    ) -> Result<(), jni::errors::Error> {
        let action = env.new_string(action)?;
        let intent = match uri {
            Some(uri) => env.new_object(
                "android/content/Intent",
                "(Ljava/lang/String;Landroid/net/Uri;)V",
                &[
                    JValue::Object(&JObject::from(action)),
                    JValue::Object(uri),
                ],
            )?,
            None => env.new_object(
                "android/content/Intent",
                "(Ljava/lang/String;)V",
                &[JValue::Object(&JObject::from(action))],
            )?,
        };
        env.call_method(
            &intent,
            "addFlags",
            "(I)Landroid/content/Intent;",
            &[JValue::Int(FLAG_ACTIVITY_NEW_TASK)],
        )?;
        env.call_method(
            activity,
            "startActivity",
            "(Landroid/content/Intent;)V",
            &[JValue::Object(&intent)],
        )?;
        Ok(())
    }

    pub fn request_all_files_access() -> Result<(), String> {
        with_activity(|env, activity| {
            if sdk_int(env)? < 30 {
                return Ok(());
            }
            // The per-app page is `package:<package name>`. Falls back to the
            // general list for devices whose Settings lacks the per-app one.
            let package = env
                .call_method(activity, "getPackageName", "()Ljava/lang/String;", &[])?
                .l()?;
            let scheme = env.new_string("package")?;
            let scheme = JObject::from(scheme);
            let null = JObject::null();
            let uri = env
                .call_static_method(
                    "android/net/Uri",
                    "fromParts",
                    "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Landroid/net/Uri;",
                    &[
                        JValue::Object(&scheme),
                        JValue::Object(&package),
                        JValue::Object(&null),
                    ],
                )?
                .l()?;

            let per_app = start_settings(
                env,
                activity,
                "android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION",
                Some(&uri),
            );
            if per_app.is_err() {
                // A thrown ActivityNotFound leaves a pending exception; clear
                // it before trying the general page.
                let _ = env.exception_clear();
                start_settings(
                    env,
                    activity,
                    "android.settings.MANAGE_ALL_FILES_ACCESS_PERMISSION",
                    None,
                )?;
            }
            Ok(())
        })
    }

    pub fn copy_text(text: &str) -> Result<(), String> {
        with_activity(|env, activity| {
            let service = env.new_string("clipboard")?;
            let manager = env
                .call_method(
                    activity,
                    "getSystemService",
                    "(Ljava/lang/String;)Ljava/lang/Object;",
                    &[JValue::Object(&JObject::from(service))],
                )?
                .l()?;
            let label = env.new_string("tessera")?;
            let body = env.new_string(text)?;
            let clip = env
                .call_static_method(
                    "android/content/ClipData",
                    "newPlainText",
                    "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
                    &[
                        JValue::Object(&JObject::from(label)),
                        JValue::Object(&JObject::from(body)),
                    ],
                )?
                .l()?;
            env.call_method(
                &manager,
                "setPrimaryClip",
                "(Landroid/content/ClipData;)V",
                &[JValue::Object(&clip)],
            )?;
            Ok(())
        })
    }
}

#[cfg(not(target_os = "android"))]
mod imp {
    use std::io::Write;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    /// The desktop build's version: hand the URL to whatever the session has.
    pub fn open_url(url: &str) -> Result<(), String> {
        #[cfg(target_os = "windows")]
        let mut command = {
            let mut command = Command::new("cmd");
            command.args(["/C", "start", ""]);
            command
        };
        #[cfg(target_os = "macos")]
        let mut command = Command::new("open");
        #[cfg(all(unix, not(target_os = "macos")))]
        let mut command = Command::new("xdg-open");

        command
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("无法打开浏览器: {error}"))
    }

    /// A desktop has no shared-storage volume to report.
    pub fn external_storage_root() -> Option<PathBuf> {
        None
    }

    /// The desktop has no such permission: everything is already reachable.
    pub fn all_files_access() -> bool {
        true
    }

    pub fn window_focused() -> bool {
        true
    }

    pub fn request_all_files_access() -> Result<(), String> {
        Ok(())
    }

    /// `wl-copy` first (Wayland), then `xclip`; either is a preview convenience.
    pub fn copy_text(text: &str) -> Result<(), String> {
        for (program, args) in [
            ("wl-copy", vec![]),
            ("xclip", vec!["-selection", "clipboard"]),
        ] {
            let Ok(mut child) = Command::new(program)
                .args(&args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            else {
                continue;
            };
            let wrote = child
                .stdin
                .as_mut()
                .is_some_and(|stdin| stdin.write_all(text.as_bytes()).is_ok());
            drop(child.stdin.take());
            if wrote && child.wait().is_ok_and(|status| status.success()) {
                return Ok(());
            }
        }
        Err("剪贴板工具不可用（wl-copy / xclip）".to_string())
    }
}
