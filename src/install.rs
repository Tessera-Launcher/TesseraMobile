//! Version install: MDT writes the plan, aria2 moves the bytes.
//!
//! The game tree lives at `<data>/游戏`, the same directory name and layout the
//! desktop build uses (`versions/`, `libraries/`, `assets/`). MDT only resolves
//! metadata and writes the aria2 list into `<data>/缓存`; the installed aria2c
//! reads that list, then the plan is verified. LWJGL is not in that list: the
//! Android runtime pack downloaded earlier owns those jars and natives.
//! No JSON-RPC, no progress parsing: aria2's per-file notice lines are the
//! activity log, exactly like the desktop install screen.

use std::path::PathBuf;
use std::time::Instant;

use mdt::{manifest, HttpDownloader, MdtClient, Mirror, ModLoader, NoopReporter};

use crate::aria2;
use crate::http;

/// `<data>/游戏`.
pub fn game_root() -> PathBuf {
    crate::platform::data_dir().join("游戏")
}

/// What the user attached to the selected version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Loader {
    /// The game alone.
    #[default]
    None,
    /// Fabric Loader plus the Fabric API jar.
    Fabric,
}

impl Loader {
    pub fn from_fabric(fabric: bool) -> Self {
        if fabric {
            Loader::Fabric
        } else {
            Loader::None
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Loader::None => "无",
            Loader::Fabric => "Fabric Loader & API",
        }
    }
}

/// What the install worker reports.
#[derive(Debug)]
pub enum Event {
    /// One line for the install screen's activity log.
    Log(String),
    /// The list is downloaded and verified.
    Finished {
        version: String,
        bytes: u64,
        seconds: f64,
    },
    Failed(String),
}

pub fn spawn(version: String, loader: Loader, report: impl Fn(Event) + Send + 'static) {
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("无法创建异步运行时: {error}"))
            .and_then(|runtime| runtime.block_on(install(&version, loader, &report)));
        if let Err(problem) = result {
            report(Event::Failed(problem));
        }
    });
}

async fn install(version: &str, loader: Loader, report: &impl Fn(Event)) -> Result<(), String> {
    if aria2::installed().is_none() {
        return Err("请先安装 aria2c，再安装游戏".to_string());
    }
    let started = Instant::now();
    let root = game_root();
    let cache = crate::platform::data_dir().join("缓存");
    std::fs::create_dir_all(&root)
        .map_err(|error| format!("无法创建 {}: {error}", root.display()))?;
    std::fs::create_dir_all(&cache)
        .map_err(|error| format!("无法创建 {}: {error}", cache.display()))?;
    // MDT keeps its content-addressed registry here; without this it writes to
    // `$HOME/.mdt`, outside the data we own.
    if std::env::var_os("MDT_HOME").is_none() {
        std::env::set_var("MDT_HOME", &cache);
    }

    let client = http::client()?;
    let mdt = MdtClient::builder()
        .root_dir(&root)
        .list_dir(&cache)
        // Official URLs for the install: BMCLAPI's asset CDN is unreliable on
        // some mobile networks (TLS I/O errors), and Mojang's CDN works here.
        .mirror(Mirror::Mojang)
        .downloader(HttpDownloader::new(client))
        .progress_reporter(NoopReporter)
        .build()
        .map_err(|error| format!("无法初始化下载器：{error}"))?;

    report(Event::Log(format!("安装 {version}（{}）", loader.label())));
    report(Event::Log(format!("解析 {version} 的版本清单…")));
    // MDT writes the complete aria2 list into `缓存/<id>.txt`; it downloads
    // nothing. This is the same artifact the desktop passes to aria2.
    let mut plan = match loader {
        Loader::None => mdt
            .plan_vanilla(version)
            .await
            .map_err(|error| format!("无法解析 {version}：{error}"))?,
        Loader::Fabric => {
            let loader_version = resolve_fabric_version(&mdt, version).await?;
            report(Event::Log(format!("Fabric Loader {loader_version}")));
            mdt.plan_loader(version, ModLoader::Fabric, &loader_version)
                .await
                .map_err(|error| format!("无法安装 Fabric：{error}"))?
        }
    };
    if loader == Loader::Fabric {
        let added = add_fabric_api(&mut plan, version, report).await;
        // `plan_loader` wrote the list before the API was appended; rewrite it
        // so aria2 sees the mod jar too.
        plan.write(&root, &cache)
            .await
            .map_err(|error| format!("无法写入下载清单：{error}"))?;
        if added {
            report(Event::Log("Fabric API 已加入下载清单".to_string()));
        }
    }
    let list = plan.path(&cache);
    report(Event::Log(format!(
        "清单：{} 个文件 · {}",
        plan.entries.len(),
        http::human_size(plan.total_bytes())
    )));
    report(Event::Log(format!("清单文件：{}", list.display())));

    let binary = aria2::installed().ok_or_else(|| "请先安装 aria2c，再安装游戏".to_string())?;
    report(Event::Log(format!("开始下载（{}）…", binary.display())));
    let mut done = 0usize;
    let exit = aria2::run_list(&list, &root, |file| {
        done += 1;
        report(Event::Log(file));
    })?;
    report(Event::Log(format!(
        "aria2 结束（退出码 {exit}），已写入 {done} 个文件"
    )));

    // The list carries no checksum; this is the one place a file is judged.
    let failed = plan.verify(&root).await;
    if !failed.is_empty() {
        report(Event::Log(format!(
            "校验：{} 个文件缺失或哈希不符",
            failed.len()
        )));
        for entry in failed.iter().take(3) {
            report(Event::Log(format!("  {}", entry.path.display())));
        }
        return Err(format!(
            "校验未通过：{} 个文件（可重试安装，已完成的会跳过）",
            failed.len()
        ));
    }
    report(Event::Log(format!(
        "校验通过：{} 个文件",
        plan.entries.len()
    )));

    // The raw Mojang/Fabric jars are signed and some are sealed. A transformer
    // (Fabric/Mixin) cannot rewrite classes in a signed jar, so normalize every
    // jar the plan put on disk — client, libraries and mods alike.
    let mut normalized = 0usize;
    for entry in &plan.entries {
        if !entry
            .path
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".jar")
        {
            continue;
        }
        if crate::jar::normalize(&root.join(&entry.path))? {
            normalized += 1;
        }
    }
    if normalized > 0 {
        report(Event::Log(format!("去签名 / 去密封：{normalized} 个 jar")));
    }

    let bytes = plan.total_bytes();
    report(Event::Finished {
        version: plan.id,
        bytes,
        seconds: started.elapsed().as_secs_f64(),
    });
    Ok(())
}

/// Latest stable Fabric loader build from Fabric's meta manifest.
async fn resolve_fabric_version(mdt: &MdtClient, version: &str) -> Result<String, String> {
    let versions = manifest::fetch_fabric_manifest(mdt.downloader(), mdt.mirror())
        .await
        .map_err(|error| format!("获取 Fabric 版本列表失败：{error}"))?;
    versions
        .loader
        .first()
        .map(|entry| entry.version.clone())
        .ok_or_else(|| format!("Fabric 没有适用于 {version} 的版本"))
}

/// Append Fabric API to `mods/` under the loader's version directory.
///
/// A missing API must not throw away the several hundred megabytes about to be
/// downloaded, so metadata failures are logged and the loader install continues.
async fn add_fabric_api(
    plan: &mut mdt::plan::DownloadPlan,
    mc_version: &str,
    report: &impl Fn(Event),
) -> bool {
    report(Event::Log("Fabric API：解析版本…".to_string()));
    let api = match crate::fabric_api::resolve(mc_version).await {
        Ok(Some(api)) => api,
        Ok(None) => {
            report(Event::Log(format!(
                "Fabric API：没有适用于 {mc_version} 的版本 —— 未安装"
            )));
            return false;
        }
        Err(error) => {
            report(Event::Log(format!(
                "Fabric API：获取失败（{error}）—— 未安装"
            )));
            return false;
        }
    };
    report(Event::Log(format!(
        "Fabric API {}（{}，{}）",
        api.version,
        api.origin.label(),
        http::human_size(api.size)
    )));
    let path = PathBuf::from("versions")
        .join(&plan.id)
        .join("mods")
        .join(api.file_name());
    if plan.entries.iter().any(|entry| entry.path == path) {
        return false;
    }
    plan.entries.push(mdt::plan::PlanEntry {
        url: api.url,
        path,
        size: api.size,
        sha1: api.sha1,
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_game_root_matches_the_desktop_folder() {
        assert!(game_root().ends_with("游戏"));
    }

    #[test]
    fn lwjgl_and_its_natives_never_reach_the_download_list() {
        let json = serde_json::json!({
            "id": "test",
            "libraries": [
                {
                    "name": "org.lwjgl:lwjgl:3.4.3",
                    "downloads": {
                        "artifact": {
                            "url": "https://libraries.minecraft.net/org/lwjgl/lwjgl/3.4.3/lwjgl-3.4.3.jar",
                            "path": "org/lwjgl/lwjgl/3.4.3/lwjgl-3.4.3.jar",
                            "sha1": "0123456789abcdef0123456789abcdef01234567",
                            "size": 1
                        },
                        "classifiers": {
                            "natives-linux": {
                                "url": "https://libraries.minecraft.net/org/lwjgl/lwjgl/3.4.3/lwjgl-3.4.3-natives-linux.jar",
                                "path": "org/lwjgl/lwjgl/3.4.3/lwjgl-3.4.3-natives-linux.jar",
                                "sha1": "0123456789abcdef0123456789abcdef01234567",
                                "size": 1
                            }
                        }
                    }
                },
                {
                    "name": "com.mojang:jtracy:1.14.38:natives-linux",
                    "downloads": {
                        "artifact": {
                            "url": "https://libraries.minecraft.net/com/mojang/jtracy/1.14.38/jtracy-1.14.38-natives-linux.jar",
                            "path": "com/mojang/jtracy/1.14.38/jtracy-1.14.38-natives-linux.jar",
                            "sha1": "0123456789abcdef0123456789abcdef01234567",
                            "size": 1
                        }
                    }
                },
                {
                    "name": "com.example:keep:1",
                    "downloads": {
                        "artifact": {
                            "url": "https://libraries.minecraft.net/com/example/keep/1/keep-1.jar",
                            "path": "com/example/keep/1/keep-1.jar",
                            "sha1": "0123456789abcdef0123456789abcdef01234567",
                            "size": 1
                        }
                    }
                }
            ]
        });
        let version: mdt::downloader::VersionJson = serde_json::from_value(json).unwrap();
        let tasks =
            mdt::downloader::build_tasks_from_version_json(&version, std::path::Path::new("/game"));
        assert_eq!(tasks.len(), 1, "{tasks:?}");
        assert!(tasks[0]
            .dest_path
            .ends_with("com/example/keep/1/keep-1.jar"));
    }

    #[test]
    fn fabric_libraries_stay_but_lwjgl_does_not() {
        let profile = serde_json::json!({
            "libraries": [
                {"name": "org.lwjgl:lwjgl:3.4.3", "url": "https://maven.example/"},
                {"name": "com.mojang:jtracy:1.14.38:natives-linux", "url": "https://maven.example/"},
                {"name": "net.fabricmc:fabric-loader:0.15.11", "url": "https://maven.fabricmc.net/"}
            ]
        });
        let entries = mdt::plan::entries_from_libraries(&profile);
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(
            entries[0].path,
            std::path::Path::new(
                "libraries/net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11.jar"
            )
        );
    }
}
