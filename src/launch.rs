//! Build the JVM command and start Minecraft in this process.
//!
//! The installed JRE is the rootless BuildJRE4Android image (`lib/server/libjvm.so`,
//! no `bin/java`). JvmTest proved the route: preload the JRE libraries with
//! absolute paths, dlopen `libjvm.so`, create the VM on a fresh pthread through
//! `JNI_CreateJavaVM`, then call the game's `main`.
//!
//! The launch command is the modern (26.x) subset of the desktop builder:
//! client jar, non-LWJGL libraries, Fabric loader libraries and the Tessera
//! LWJGL jars; no legacy agent, no AOT, no natives extraction. Tessera's
//! Android LWJGL and SDL runtime replaces Mojang's desktop natives; rendering
//! is Vulkan.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::accounts;
use crate::install;
use crate::jar;
use crate::platform;

/// What one launch reports.
#[derive(Debug)]
pub enum Event {
    Log(String),
    Started { pid: u32 },
    Exited { code: Option<i32> },
    Failed(String),
}

/// The player identity passed to the game.
#[derive(Debug, Clone)]
pub struct Credentials {
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    pub user_type: String,
    pub xuid: String,
}

impl Credentials {
    fn offline(name: &str) -> Self {
        Self {
            username: name.to_string(),
            uuid: accounts::offline_uuid(name),
            access_token: "0".to_string(),
            user_type: "legacy".to_string(),
            xuid: String::new(),
        }
    }

    /// Read the active account, or fall back to an offline `Player` for a
    /// machine that has no account yet (single-player still works).
    pub fn active(path: Option<&Path>) -> Result<Self, String> {
        let Some(path) = path else {
            return Ok(Self::offline("Player"));
        };
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("读取账户 {} 失败：{error}", path.display()))?;
        let doc: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| format!("账户格式错误：{error}"))?;
        let player = doc
            .get("player")
            .ok_or_else(|| "账户缺少 player 字段".to_string())?;
        let username = player
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "账户缺少玩家名".to_string())?
            .to_string();
        let uuid = player
            .get("uuid")
            .and_then(serde_json::Value::as_str)
            .filter(|uuid| !uuid.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| accounts::offline_uuid(&username));
        let offline = doc
            .get("offline")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if offline {
            return Ok(Self {
                username,
                uuid,
                access_token: "0".to_string(),
                user_type: "legacy".to_string(),
                xuid: String::new(),
            });
        }
        let token = doc
            .get("session")
            .and_then(|session| session.get("minecraft_token"))
            .ok_or_else(|| "该账户没有 Minecraft 令牌，请重新登录".to_string())?;
        Ok(Self {
            username,
            uuid,
            access_token: token
                .get("access_token")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "令牌缺少 access_token".to_string())?
                .to_string(),
            user_type: "msa".to_string(),
            xuid: token
                .get("xuid")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
        })
    }
}

/// Launches are one per process: HotSpot cannot create a second VM.
static LAUNCHED: AtomicBool = AtomicBool::new(false);

/// Start one installed version on a worker thread and report until `main`
/// returns.
pub fn spawn(
    dir_name: String,
    active: Option<PathBuf>,
    report: impl Fn(Event) + Send + Sync + 'static,
) {
    let debug = crate::platform::private_dir().join("native-start.log");
    let note = |text: &str| {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&debug)
        {
            let _ = writeln!(file, "spawn: {text}");
        }
    };
    note("enter");
    if LAUNCHED.swap(true, Ordering::SeqCst) {
        note("already launched");
        report(Event::Failed(
            "本进程已经创建过 JVM；请重启应用后再启动游戏".to_string(),
        ));
        return;
    }
    use std::sync::Arc;
    let report = Arc::new(report);
    let thread_report = report.clone();
    let thread_debug = debug.clone();
    let spawned = std::thread::Builder::new()
        .name("minecraft".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let note = |text: &str| {
                use std::io::Write;
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&thread_debug)
                {
                    let _ = writeln!(file, "minecraft: {text}");
                }
            };
            note("thread enter");
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                prepare(&dir_name, active.as_deref(), thread_report.as_ref())
                    .and_then(|plan| run(plan, thread_report.as_ref()))
            }));
            match outcome {
                Ok(Ok(())) => note("thread done"),
                Ok(Err(problem)) => {
                    note(&format!("thread failed: {problem}"));
                    thread_report(Event::Failed(problem));
                }
                Err(payload) => {
                    let message = payload
                        .downcast_ref::<&str>()
                        .map(|text| text.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "non-string panic".to_string());
                    note(&format!("thread panicked: {message}"));
                }
            }
        });
    if let Err(error) = spawned {
        note(&format!("thread failed: {error}"));
        report(Event::Failed(format!("创建启动线程失败：{error}")));
    } else {
        note("thread started");
    }
}

/// A fully prepared launch.
#[derive(Debug, Clone)]
struct Plan {
    main_class: String,
    classpath: Vec<PathBuf>,
    jvm_args: Vec<String>,
    game_args: Vec<String>,
    mods: Vec<PathBuf>,
    jre: PathBuf,
    work_dir: PathBuf,
}

fn prepare(dir_name: &str, active: Option<&Path>, report: &impl Fn(Event)) -> Result<Plan, String> {
    let game = install::game_root();
    let version_dir = game.join("versions").join(dir_name);
    let json_path = version_dir.join(format!("{dir_name}.json"));
    let text = std::fs::read_to_string(&json_path)
        .map_err(|error| format!("读取 {} 失败：{error}", json_path.display()))?;
    let version: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("版本 JSON 错误：{error}"))?;
    let version_id = version
        .get("id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(dir_name)
        .to_string();
    let main_class = version
        .get("mainClass")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "版本 JSON 没有 mainClass".to_string())?
        .to_string();
    let asset_index = version
        .get("assetIndex")
        .and_then(|index| index.get("id"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("legacy")
        .to_string();

    let client = version_dir.join(format!("{dir_name}.jar"));
    if !client.is_file() {
        return Err(format!("缺少客户端 jar：{}", client.display()));
    }

    // ---- classpath -------------------------------------------------------
    let mut classpath = Vec::new();
    let mut missing = Vec::new();
    if let Some(libraries) = version
        .get("libraries")
        .and_then(serde_json::Value::as_array)
    {
        for library in libraries {
            if !rules_allow(library.get("rules"), &HashMap::new()) {
                continue;
            }
            let name = library
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if name.starts_with("org.lwjgl") || name.contains(":natives-") {
                continue;
            }
            let path = library
                .get("downloads")
                .and_then(|downloads| downloads.get("artifact"))
                .and_then(|artifact| artifact.get("path"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .or_else(|| mdt::installer::library_path_from_name(name).ok());
            let Some(path) = path else {
                continue;
            };
            if path.contains("/org/lwjgl/") || path.contains("/natives-") {
                continue;
            }
            let full = game.join("libraries").join(&path);
            if full.is_file() {
                classpath.push(full);
            } else {
                missing.push(path);
            }
        }
    }
    // MDT did not download Linux-only native variants on Android (netty epoll,
    // for example). They are optional: Netty falls back to NIO, so log and keep
    // going instead of refusing to launch.
    for path in &missing {
        report(Event::Log(format!("跳过缺失依赖：{path}")));
    }

    let runtime = platform::private_dir().join("runtime");
    let lwjgl_dir = runtime.join("lwjgl").join(crate::runtime::RUNTIME_VERSION);
    let mut lwjgl_jars: Vec<PathBuf> = std::fs::read_dir(&lwjgl_dir)
        .map_err(|error| format!("读取 {} 失败：{error}", lwjgl_dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
        })
        .collect();
    lwjgl_jars.sort();
    if lwjgl_jars.is_empty() {
        return Err("Tessera LWJGL 运行库缺失，请先安装运行库".to_string());
    }
    // FCL's libpojavexec expects the app-side CallbackBridge (its native
    // declarations are bound by the library); it must win over the copy in the
    // merged LWJGL jar.
    let callback_bridge = game.join("fcl-callbackbridge.jar");
    if callback_bridge.is_file() {
        classpath.push(callback_bridge);
    }
    classpath.extend(lwjgl_jars);
    // The safe BuildJRE profile drops `java.compiler`; Fabric's Mixin still
    // references `javax.annotation.processing.Messager`. A small API jar next
    // to the game (or in the runtime pack later) fills the gap.
    let compiler_api = game.join("java.compiler.jar");
    if compiler_api.is_file() {
        classpath.push(compiler_api);
    }
    classpath.push(client.clone());

    let mut mods: Vec<PathBuf> = std::fs::read_dir(version_dir.join("mods"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
        })
        .collect();
    mods.sort();

    // Normalize before the VM opens the jars; this also repairs installs from
    // before the install-time rewrite existed.
    let mut normalized = 0usize;
    for path in classpath.iter().chain(mods.iter()) {
        if jar::normalize(path)? {
            normalized += 1;
        }
    }
    if normalized > 0 {
        report(Event::Log(format!("去签名 / 去密封：{normalized} 个 jar")));
    }

    let creds = Credentials::active(active)?;
    report(Event::Log(format!(
        "账户：{}（{}）{}",
        creds.username,
        creds.user_type,
        if active.is_none() {
            " · 未选择账户，使用离线 Player"
        } else {
            ""
        }
    )));

    let jre = crate::jre::installed().ok_or_else(|| "请先安装 JRE".to_string())?;
    let natives = runtime.join("native").join("arm64-v8a");
    let lwjgl_natives = lwjgl_dir.join("natives").join("arm64-v8a");
    let jna = newest_jna(&runtime).ok_or_else(|| "缺少 JNA 运行库".to_string())?;
    let apk_native =
        platform::native_library_dir().ok_or_else(|| "找不到 APK nativeLibraryDir".to_string())?;
    let tmp = platform::private_dir().join("tmp");
    std::fs::create_dir_all(&tmp).map_err(|error| format!("创建临时目录失败：{error}"))?;
    std::fs::create_dir_all(&lwjgl_natives)
        .map_err(|error| format!("检查 LWJGL natives 失败：{error}"))?;

    let mut variables: HashMap<&str, String> = HashMap::new();
    variables.insert("auth_player_name", creds.username.clone());
    variables.insert("version_name", version_id.clone());
    variables.insert("game_directory", version_dir.display().to_string());
    variables.insert("assets_root", game.join("assets").display().to_string());
    variables.insert("assets_index_name", asset_index);
    variables.insert("auth_uuid", creds.uuid.clone());
    variables.insert("auth_access_token", creds.access_token.clone());
    variables.insert("clientid", String::new());
    variables.insert("auth_xuid", creds.xuid.clone());
    variables.insert("user_type", creds.user_type.clone());
    variables.insert(
        "version_type",
        version
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("release")
            .to_string(),
    );
    variables.insert("natives_directory", lwjgl_natives.display().to_string());
    variables.insert("launcher_name", "Tessera".to_string());
    variables.insert("launcher_version", env!("CARGO_PKG_VERSION").to_string());
    variables.insert("resolution_width", "854".to_string());
    variables.insert("resolution_height", "480".to_string());

    let features = HashMap::new();
    let profile_jvm = argument_values(
        version.get("arguments").and_then(|a| a.get("jvm")),
        &features,
        &variables,
    );
    let mut jvm_args = profile_jvm
        .into_iter()
        .filter(|arg| keep_profile_jvm(arg))
        .collect::<Vec<_>>();

    let classpath_string = classpath
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(":");
    let native_path = format!(
        "{}:{}:{}:{}:/system/lib64:/vendor/lib64",
        lwjgl_natives.display(),
        natives.display(),
        jna.display(),
        apk_native.display()
    );
    jvm_args.extend([
        format!("-Djava.home={}", jre.display()),
        format!("-Djava.class.path={classpath_string}"),
        format!("-Djava.library.path={native_path}"),
        format!(
            "-Dorg.lwjgl.librarypath={}:{}",
            lwjgl_natives.display(),
            natives.display()
        ),
        format!(
            "-Dorg.lwjgl.openal.libname={}/libopenal.so",
            natives.display()
        ),
        // Android ships the Vulkan loader as `libvulkan.so`; LWJGL's
        // desktop default is `libvulkan.so.1`, which does not exist here.
        "-Dorg.lwjgl.vulkan.libname=libvulkan.so".to_string(),
        format!(
            "-Dorg.lwjgl.system.SharedLibraryExtractPath={}/lwjgl",
            tmp.display()
        ),
        format!("-Djna.boot.library.path={}", jna.display()),
        format!("-Djava.io.tmpdir={}", tmp.display()),
        format!("-Duser.home={}", platform::private_dir().display()),
        format!("-Duser.dir={}", version_dir.display()),
        "-Dos.name=Linux".to_string(),
        "-Dos.version=Android-13".to_string(),
        "-Dos.arch=aarch64".to_string(),
        "-Djava.awt.headless=false".to_string(),
        "-Duser.language=zh".to_string(),
        "-Duser.region=CN".to_string(),
        "-Duser.timezone=Asia/Shanghai".to_string(),
        "-Dminecraft.launcher.brand=Tessera".to_string(),
        format!("-Dminecraft.launcher.version={}", env!("CARGO_PKG_VERSION")),
        "-Xms512m".to_string(),
        "-Xmx2048m".to_string(),
        "-XX:+UseG1GC".to_string(),
        format!("-XX:AltTempDir={}", tmp.display()),
        format!(
            "-XX:ErrorFile={}/hs_err_pid%p.log",
            platform::private_dir().display()
        ),
    ]);

    let mut game_args = argument_values(
        version.get("arguments").and_then(|a| a.get("game")),
        &features,
        &variables,
    );
    // Tessera Mobile is Vulkan-only. The install step already removed the
    // OpenGL fallback and applied the KHR/OIT client edits; this flag sets the
    // preferred backend to match.
    if !game_args.iter().any(|arg| arg == "--graphicsBackend") {
        game_args.push("--graphicsBackend".to_string());
        game_args.push("vulkan".to_string());
    }

    std::env::set_var("HOME", platform::private_dir());
    std::env::set_var("TMPDIR", &tmp);
    let ld = format!(
        "{}:{}:{}:{}:{}:{}",
        jre.join("lib").display(),
        jre.join("lib/server").display(),
        lwjgl_natives.display(),
        natives.display(),
        jna.display(),
        apk_native.display()
    );
    std::env::set_var("LD_LIBRARY_PATH", ld);

    report(Event::Log(format!(
        "classpath：{} 项 · 游戏参数：{} 项 · JVM 参数：{} 项",
        classpath.len(),
        game_args.len(),
        jvm_args.len()
    )));
    Ok(Plan {
        main_class,
        classpath,
        jvm_args,
        game_args,
        mods,
        jre,
        work_dir: version_dir,
    })
}

fn newest_jna(runtime: &Path) -> Option<PathBuf> {
    let mut versions: Vec<PathBuf> = std::fs::read_dir(runtime.join("jna"))
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("libjnidispatch.so").is_file())
        .collect();
    versions.sort();
    versions.pop()
}

fn keep_profile_jvm(arg: &str) -> bool {
    if !(arg.starts_with("-D")
        || arg.starts_with("-X")
        || arg.starts_with("-javaagent")
        || arg.starts_with("-agentlib"))
    {
        return false;
    }
    if arg == "-cp"
        || arg == "-classpath"
        || arg.starts_with("-XstartOnFirstThread")
        || arg.starts_with("--")
    {
        return false;
    }
    if arg.starts_with("-Xmx")
        || arg.starts_with("-Xms")
        || arg.starts_with("-Xmn")
        || arg.starts_with("-Djava.class.path")
        || arg.starts_with("-Djava.library.path")
        || arg.starts_with("-Dorg.lwjgl")
        || arg.starts_with("-Djna.")
        || arg.starts_with("-Dos.")
        || arg.starts_with("-Duser.")
        || arg.starts_with("-Dminecraft.launcher")
    {
        return false;
    }
    !arg.starts_with("-XX:HeapDumpPath")
}

fn argument_values(
    value: Option<&serde_json::Value>,
    features: &HashMap<String, bool>,
    variables: &HashMap<&str, String>,
) -> Vec<String> {
    let Some(array) = value.and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in array {
        if let Some(text) = item.as_str() {
            out.push(substitute(text, variables));
            continue;
        }
        if !rules_allow(item.get("rules"), features) {
            continue;
        }
        match item.get("value") {
            Some(serde_json::Value::String(text)) => out.push(substitute(text, variables)),
            Some(serde_json::Value::Array(values)) => {
                for text in values.iter().filter_map(serde_json::Value::as_str) {
                    out.push(substitute(text, variables));
                }
            }
            _ => {}
        }
    }
    out
}

fn rules_allow(rules: Option<&serde_json::Value>, features: &HashMap<String, bool>) -> bool {
    let Some(rules) = rules.and_then(serde_json::Value::as_array) else {
        return true;
    };
    let mut allowed = false;
    for rule in rules {
        if !features_match(rule.get("features"), features) {
            continue;
        }
        if !os_match(rule.get("os")) {
            continue;
        }
        if rule.get("action").and_then(serde_json::Value::as_str) == Some("allow") {
            allowed = true;
        } else if rule.get("action").and_then(serde_json::Value::as_str) == Some("disallow") {
            allowed = false;
        }
    }
    allowed
}

fn features_match(features: Option<&serde_json::Value>, values: &HashMap<String, bool>) -> bool {
    let Some(features) = features.and_then(serde_json::Value::as_object) else {
        return true;
    };
    features.iter().all(|(key, wanted)| {
        let wanted = wanted.as_bool().unwrap_or(false);
        values.get(key).copied().unwrap_or(false) == wanted
    })
}

fn os_match(os: Option<&serde_json::Value>) -> bool {
    let Some(os) = os.and_then(serde_json::Value::as_object) else {
        return true;
    };
    if let Some(name) = os.get("name").and_then(serde_json::Value::as_str) {
        let ok = match name {
            "linux" => true,
            "osx" => false,
            "windows" => false,
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    if let Some(arch) = os.get("arch").and_then(serde_json::Value::as_str) {
        let ok = match arch {
            "x86" | "x86_64" | "arm" => false,
            "arm64" => true,
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

fn substitute(text: &str, variables: &HashMap<&str, String>) -> String {
    let mut out = text.to_string();
    for (key, value) in variables {
        out = out.replace(&format!("${{{key}}}"), value);
    }
    out
}

#[cfg(target_os = "android")]
fn run(plan: Plan, report: &impl Fn(Event)) -> Result<(), String> {
    use jni::objects::{JObject, JObjectArray, JValue};
    use jni::sys::{jint, JavaVM as RawVm, JavaVMInitArgs, JavaVMOption, JNI_VERSION_1_6};
    use jni::JavaVM;
    use std::ffi::CString;
    use std::os::raw::{c_char, c_void};

    // Java's relative file paths are resolved by the OS, not by the
    // `user.dir` property. Minecraft's log4j config opens `logs/latest.log`
    // relative to the working directory, so make the version directory the
    // real cwd before creating the VM.
    std::env::set_current_dir(&plan.work_dir)
        .map_err(|error| format!("切换工作目录 {} 失败：{error}", plan.work_dir.display()))?;

    let native = crate::platform::private_dir().join("runtime/native/arm64-v8a");
    let libjvm = preload(&plan.jre, &native, report)?;
    // SDL3 is built with SDL_MAIN_NEEDED on this Android build. The normal
    // SDLActivity path calls SDL_SetMainReady from Java; our host loads SDL3
    // early, so do the equivalent on the thread that will run `main`.
    if let Err(problem) = sdl_main_ready(&native.join("libSDL3.so")) {
        report(Event::Log(format!("SDL_SetMainReady 失败：{problem}")));
    }
    let symbol = CString::new("JNI_CreateJavaVM").unwrap();
    let create_ptr = unsafe { dlsym(libjvm, symbol.as_ptr()) };
    if create_ptr.is_null() {
        return Err("libjvm.so 没有 JNI_CreateJavaVM".to_string());
    }
    type CreateFn =
        unsafe extern "system" fn(*mut *mut RawVm, *mut *mut c_void, *mut c_void) -> jint;
    let create: CreateFn = unsafe { std::mem::transmute(create_ptr) };

    let c_options: Vec<CString> = plan
        .jvm_args
        .iter()
        .map(|option| CString::new(option.as_bytes()).unwrap_or_default())
        .collect();
    let mut options: Vec<JavaVMOption> = c_options
        .iter()
        .map(|option| JavaVMOption {
            optionString: option.as_ptr() as *mut c_char,
            extraInfo: std::ptr::null_mut(),
        })
        .collect();
    let mut args = JavaVMInitArgs {
        version: JNI_VERSION_1_6,
        nOptions: options.len() as i32,
        options: options.as_mut_ptr(),
        ignoreUnrecognized: 0,
    };

    report(Event::Log(format!("JVM 参数：{}", plan.jvm_args.join(" "))));
    let mut vm_ptr: *mut RawVm = std::ptr::null_mut();
    let mut env_ptr: *mut c_void = std::ptr::null_mut();
    // HotSpot writes its rejection reason to stderr, which Android otherwise
    // sends to logcat. Keep a copy in files/jvm-start.log for debugging.
    let stderr_path = crate::platform::private_dir().join("jvm-start.log");
    let stderr_file = std::fs::File::create(&stderr_path).ok();
    let saved_stderr = stderr_file
        .as_ref()
        .map(|file| unsafe { libc::dup(2) })
        .filter(|fd| *fd >= 0);
    if let (Some(file), Some(saved)) = (stderr_file.as_ref(), saved_stderr) {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::dup2(file.as_raw_fd(), 2);
        }
        let _ = saved;
    }
    let rc = unsafe {
        create(
            &mut vm_ptr,
            &mut env_ptr,
            &mut args as *mut JavaVMInitArgs as *mut c_void,
        )
    };
    if let Some(saved) = saved_stderr {
        unsafe {
            libc::dup2(saved, 2);
            libc::close(saved);
        }
    }
    drop(stderr_file);
    if rc != 0 || vm_ptr.is_null() {
        if let Ok(text) = std::fs::read_to_string(&stderr_path) {
            for line in text
                .lines()
                .rev()
                .take(8)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                report(Event::Log(format!("JVM: {line}")));
            }
        }
        return Err(format!("JNI_CreateJavaVM 返回 {rc}"));
    }
    let vm =
        unsafe { JavaVM::from_raw(vm_ptr) }.map_err(|error| format!("JavaVM 指针无效：{error}"))?;
    let mut env = vm
        .get_env()
        .or_else(|_| vm.attach_current_thread_permanently())
        .map_err(|error| format!("取得 JNIEnv 失败：{error}"))?;

    report(Event::Started {
        pid: std::process::id(),
    });
    let class_name = plan.main_class.replace('.', "/");
    let class = env
        .find_class(&class_name)
        .map_err(|error| format!("找不到主类 {}：{error}", plan.main_class))?;
    let array: JObjectArray = env
        .new_object_array(
            plan.game_args.len() as i32,
            "java/lang/String",
            JObject::null(),
        )
        .map_err(|error| format!("创建游戏参数失败：{error}"))?;
    for (index, arg) in plan.game_args.iter().enumerate() {
        let text = env
            .new_string(arg)
            .map_err(|error| format!("创建游戏参数失败：{error}"))?;
        env.set_object_array_element(&array, index as i32, text)
            .map_err(|error| format!("设置游戏参数失败：{error}"))?;
    }
    report(Event::Log(format!(
        "调用 {}（{} 项游戏参数，{} 个 mod）",
        plan.main_class,
        plan.game_args.len(),
        plan.mods.len()
    )));
    // Let the game's stderr land in a file we can read back; Android has no
    // useful console for the JVM.
    let game_log = crate::platform::private_dir().join("game.log");
    let game_file = std::fs::File::create(&game_log).ok();
    let saved = game_file
        .as_ref()
        .map(|file| unsafe { libc::dup(2) })
        .filter(|fd| *fd >= 0);
    if let (Some(file), Some(_saved)) = (game_file.as_ref(), saved) {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::dup2(file.as_raw_fd(), 2);
        }
    }
    let result = env.call_static_method(
        class,
        "main",
        "([Ljava/lang/String;)V",
        &[JValue::Object(&array)],
    );
    let failed = env.exception_check().unwrap_or(false);
    if failed {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    if let Some(saved) = saved {
        unsafe {
            libc::dup2(saved, 2);
            libc::close(saved);
        }
    }
    drop(game_file);
    if failed {
        if let Ok(text) = std::fs::read_to_string(&game_log) {
            for line in text
                .lines()
                .rev()
                .take(20)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                report(Event::Log(format!("游戏: {line}")));
            }
        }
        return Err("游戏 main 抛出异常（见日志）".to_string());
    }
    result.map_err(|error| format!("调用游戏 main 失败：{error}"))?;
    report(Event::Exited { code: None });
    Ok(())
}

#[cfg(target_os = "android")]
unsafe extern "C" {
    fn dlsym(
        handle: *mut std::os::raw::c_void,
        symbol: *const std::os::raw::c_char,
    ) -> *mut std::os::raw::c_void;
}

#[cfg(target_os = "android")]
fn sdl_main_ready(path: &Path) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int, c_void};

    const RTLD_NOW: c_int = 2;
    const RTLD_GLOBAL: c_int = 0x100;
    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlerror() -> *mut c_char;
    }
    let name = CString::new(path.to_string_lossy().as_bytes())
        .map_err(|_| "SDL3 路径包含 NUL".to_string())?;
    let handle = unsafe { dlopen(name.as_ptr(), RTLD_NOW | RTLD_GLOBAL) };
    if handle.is_null() {
        return Err("无法加载 libSDL3.so".to_string());
    }
    let symbol = CString::new("SDL_SetMainReady").unwrap();
    let ptr = unsafe { dlsym(handle, symbol.as_ptr()) };
    if ptr.is_null() {
        let error = unsafe { dlerror() };
        let message = if error.is_null() {
            "unknown".to_string()
        } else {
            unsafe { std::ffi::CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned()
        };
        return Err(message);
    }
    let call: unsafe extern "C" fn() = unsafe { std::mem::transmute(ptr) };
    unsafe { call() };
    Ok(())
}

#[cfg(target_os = "android")]
fn preload(
    jre: &Path,
    native: &Path,
    report: &impl Fn(Event),
) -> Result<*mut std::os::raw::c_void, String> {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int, c_void};

    const RTLD_NOW: c_int = 2;
    const RTLD_GLOBAL: c_int = 0x100;

    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        fn dlerror() -> *mut c_char;
    }
    let open = |relative: &str| -> Result<*mut c_void, String> {
        let path = jre.join("lib").join(relative);
        let text = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| format!("路径包含 NUL：{}", path.display()))?;
        let handle = unsafe { dlopen(text.as_ptr(), RTLD_NOW | RTLD_GLOBAL) };
        if handle.is_null() {
            let error = unsafe { dlerror() };
            let message = if error.is_null() {
                "unknown".to_string()
            } else {
                unsafe { std::ffi::CStr::from_ptr(error) }
                    .to_string_lossy()
                    .into_owned()
            };
            return Err(format!("dlopen {} 失败：{message}", path.display()));
        }
        Ok(handle)
    };

    let mut handles = Vec::new();
    handles.push(open("libc++_shared.so")?);
    let jvm_handle = open("server/libjvm.so")?;
    handles.push(jvm_handle);
    report(Event::Log("JVM 库已加载".to_string()));

    let preload = [
        "libjli.so",
        "libjava.so",
        "libverify.so",
        "libzip.so",
        "libjimage.so",
        "libnet.so",
        "libnio.so",
        "libextnet.so",
        "libinstrument.so",
        "libmanagement.so",
        "libmanagement_ext.so",
        "libj2pkcs11.so",
        "libj2gss.so",
        "libsleef.so",
        "libsyslookup.so",
        "libjsig.so",
    ];
    let mut remaining: Vec<&str> = preload.to_vec();
    for _ in 0..4 {
        let mut progressed = false;
        let mut next = Vec::new();
        for name in remaining {
            match open(name) {
                Ok(handle) => {
                    handles.push(handle);
                    progressed = true;
                }
                Err(_) => next.push(name),
            }
        }
        remaining = next;
        if !progressed || remaining.is_empty() {
            break;
        }
    }
    if !remaining.is_empty() {
        report(Event::Log(format!(
            "JRE 库未全部加载：{}",
            remaining.join("、")
        )));
    }
    // FCL's LWJGL replacement loads `libpojavexec.so`; its DT_NEEDED chain has
    // to be in the global namespace before the JVM asks for it from
    // java.library.path. Preload the dependencies, not pojavexec itself, so
    // its JNI_OnLoad runs when JVM System.loadLibrary opens it.
    let apk_native = platform::native_library_dir();
    for name in [
        "libbytehook.so",
        "libandroidnsbypass.so",
        "libfcl.so",
        "libpojavexec_awt.so",
    ] {
        // FCL's JNI host libraries live in the APK's nativeLibraryDir, not in
        // the downloaded runtime pack. Keep the runtime path as a fallback for
        // a future self-contained FCL replacement.
        let path = apk_native
            .as_ref()
            .map(|dir| dir.join(name))
            .filter(|path| path.is_file())
            .unwrap_or_else(|| native.join(name));
        let text = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| format!("路径包含 NUL：{}", path.display()))?;
        let handle = unsafe { dlopen(text.as_ptr(), RTLD_NOW | RTLD_GLOBAL) };
        if handle.is_null() {
            let error = unsafe { dlerror() };
            let message = if error.is_null() {
                "unknown".to_string()
            } else {
                unsafe { std::ffi::CStr::from_ptr(error) }
                    .to_string_lossy()
                    .into_owned()
            };
            report(Event::Log(format!("预加载 {name} 失败：{message}")));
        } else {
            handles.push(handle);
        }
    }
    std::mem::forget(handles);
    Ok(jvm_handle)
}

#[cfg(not(target_os = "android"))]
fn run(_plan: Plan, _report: &impl Fn(Event)) -> Result<(), String> {
    Err("启动只在 Android 上可用".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_replaced() {
        let mut vars = HashMap::new();
        vars.insert("auth_player_name", "Steve".to_string());
        assert_eq!(
            substitute("--username ${auth_player_name}", &vars),
            "--username Steve"
        );
        assert_eq!(
            substitute("--uuid ${auth_uuid}", &vars),
            "--uuid ${auth_uuid}"
        );
    }

    #[test]
    fn demo_rules_are_rejected_without_the_feature() {
        let json = serde_json::json!([
            {"rules": [{"action": "allow", "features": {"is_demo_user": true}}], "value": "--demo"},
            "--username"
        ]);
        let values = argument_values(Some(&json), &HashMap::new(), &HashMap::new());
        assert_eq!(values, vec!["--username"]);
    }

    #[test]
    fn macos_and_windows_arguments_are_skipped() {
        let json = serde_json::json!([
            {"rules": [{"action": "allow", "os": {"name": "osx"}}], "value": "-XstartOnFirstThread"},
            {"rules": [{"action": "allow", "os": {"name": "linux"}}], "value": "-Dlinux=true"}
        ]);
        let values = argument_values(Some(&json), &HashMap::new(), &HashMap::new());
        assert_eq!(values, vec!["-Dlinux=true"]);
    }
}
