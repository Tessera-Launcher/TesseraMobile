//! The launcher's data root, and the two system capabilities the UI needs.
//!
//! The desktop build is portable: everything lives in `data/` next to the
//! binary. A phone has no writable directory next to the executable, so Android
//! hands us one (`Context.getFilesDir()`) and [`set_data_dir`] stashes it before
//! the first frame.
//!
//! Slint's public Rust API has no "open this URL" and no clipboard, although its
//! platform layer supports both. So those two live here: JNI against the
//! activity on Android, the usual desktop helpers elsewhere.

use std::path::PathBuf;
use std::sync::OnceLock;

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Pin the data root. Called once at start-up; later calls are ignored.
pub fn set_data_dir(path: PathBuf) {
    let _ = DATA_DIR.set(path);
}

/// The launcher's data root.
///
/// `TESSERA_DATA` overrides it (tests, checkouts); otherwise `./data`, so
/// `cargo run` on a desktop behaves like the desktop build.
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

#[cfg(target_os = "android")]
mod imp {
    use jni::objects::{JObject, JValue};
    use jni::JavaVM;

    /// `Intent.FLAG_ACTIVITY_NEW_TASK`: the activity is not the one that is
    /// currently resumed when the login sheet asks for a browser.
    const FLAG_ACTIVITY_NEW_TASK: i32 = 0x1000_0000;

    /// Run `f` on the current thread with a JNI env and the activity.
    ///
    /// `ndk_context` is filled in by `android-activity` during start-up, so the
    /// pointers are the process's own and stay valid. Attaching is per-thread
    /// and the guard detaches again when the call returns.
    fn with_activity<T>(
        f: impl FnOnce(&mut jni::JNIEnv, &JObject) -> Result<T, jni::errors::Error>,
    ) -> Result<T, String> {
        let context = ndk_context::android_context();
        let vm = unsafe { JavaVM::from_raw(context.vm().cast()) }.map_err(|e| e.to_string())?;
        let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
        // A global reference owned by android-activity; this is only a view.
        let activity = unsafe { JObject::from_raw(context.context().cast()) };
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
