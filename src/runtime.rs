//! The Tessera Android runtime pack.
//!
//! Mojang's version JSON points LWJGL at desktop glibc natives, which cannot be
//! loaded on Android. Tessera's runtime replaces the whole LWJGL stack with
//! Android-built jars and `.so` files, plus SDL3, OpenAL, shaderc, SPIRV-Cross,
//! FreeType, JNA and libc++.
//!
//! The pack is published as one rootless zip with a machine-readable manifest:
//! `tessera-runtime-manifest.json`. aria2c downloads both, the zip is verified
//! against the manifest SHA-256, and it is extracted into the app's private
//! `files/runtime` (never `/sdcard`, which is `noexec`).

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::aria2::{self, ListEntry};
use crate::http;

/// LWJGL/runtime version. The directory layout is versioned.
pub const RUNTIME_VERSION: &str = "3.4.3";

const MANIFEST_URL: &str = "https://github.com/Tessera-Launcher/BuildNatives4Android/releases/latest/download/tessera-runtime-manifest.json";
const RELEASE_BASE: &str =
    "https://github.com/Tessera-Launcher/BuildNatives4Android/releases/latest/download";

const LWJGL: &str = "lwjgl/3.4.3";
const LWJGL_NATIVES: &str = "lwjgl/3.4.3/natives/arm64-v8a";
const NATIVE: &str = "native/arm64-v8a";
const JNA: &str = "jna/5.17.0";

#[derive(Debug, Deserialize)]
struct Manifest {
    files: ManifestFiles,
}

#[derive(Debug, Deserialize)]
struct ManifestFiles {
    zip: ManifestFile,
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    name: String,
    sha256: String,
    size: u64,
}

/// Whether this build has an Android arm64 runtime pack.
pub fn supported() -> bool {
    cfg!(all(target_os = "android", target_arch = "aarch64"))
}

/// The installed pack, if its markers are all present.
pub fn install_dir() -> PathBuf {
    crate::platform::private_dir().join("runtime")
}

pub fn installed() -> Option<PathBuf> {
    installed_at(&install_dir()).map(|_| install_dir())
}

/// What the install thread reports.
#[derive(Debug)]
pub enum Event {
    Log(String),
    Progress { done: u64, total: u64, speed: f64 },
    Extracting,
    Installed { path: PathBuf },
    Failed(String),
}

pub fn spawn_install(proxy: String, report: impl Fn(Event) + Send + 'static) {
    if !supported() {
        report(Event::Failed(
            "当前平台没有固定构建（需要 Android arm64）".to_string(),
        ));
        return;
    }
    std::thread::spawn(move || {
        if let Err(problem) = install(&proxy, &report) {
            report(Event::Failed(problem));
        }
    });
}

fn install(proxy: &str, report: &impl Fn(Event)) -> Result<(), String> {
    if aria2::installed().is_none() {
        return Err("请先安装 aria2c，再下载运行库".to_string());
    }

    let install = install_dir();
    let parent = install
        .parent()
        .ok_or_else(|| "运行库安装路径没有父目录".to_string())?
        .to_path_buf();
    std::fs::create_dir_all(&parent)
        .map_err(|error| format!("无法创建 {}: {error}", parent.display()))?;
    let work = parent.join("runtime.incoming");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|error| format!("无法创建临时目录: {error}"))?;

    report(Event::Log("运行库：获取 Tessera 运行时索引…".to_string()));
    let manifest_path = work.join("tessera-runtime-manifest.json");
    download(
        &manifest_path,
        http::github_candidates(MANIFEST_URL, proxy),
        None,
        report,
        false,
    )?;
    let manifest: Manifest = serde_json::from_slice(
        &std::fs::read(&manifest_path).map_err(|error| format!("读取运行库索引失败：{error}"))?,
    )
    .map_err(|error| format!("运行库索引格式错误：{error}"))?;
    let zip = manifest.files.zip;
    if zip.name.contains('/') || zip.name.contains('\\') || !zip.name.ends_with(".zip") {
        return Err("运行库索引中的 zip 文件名不合法".to_string());
    }
    if zip.sha256.len() != 64 || !zip.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("运行库索引中的 SHA-256 不合法".to_string());
    }

    report(Event::Log(format!(
        "运行库：{} · {}",
        zip.name,
        http::human_size(zip.size)
    )));
    let archive = work.join(&zip.name);
    let zip_url = format!("{RELEASE_BASE}/{}", zip.name);
    download(
        &archive,
        http::github_candidates(&zip_url, proxy),
        Some(&zip.sha256),
        report,
        true,
    )?;
    let actual_size = std::fs::metadata(&archive)
        .map_err(|error| format!("读取运行库大小失败：{error}"))?
        .len();
    if actual_size != zip.size {
        let _ = std::fs::remove_dir_all(&work);
        return Err(format!(
            "运行库大小不符：索引 {}，实际 {actual_size}",
            zip.size
        ));
    }

    report(Event::Extracting);
    let tree = work.join("runtime");
    std::fs::create_dir_all(&tree).map_err(|error| format!("无法创建运行库目录: {error}"))?;
    let members = extract_pack(&archive, &tree)?;
    if installed_at(&tree).is_none() {
        let _ = std::fs::remove_dir_all(&work);
        return Err(format!("运行库解压后缺少文件（解出 {members} 个成员）"));
    }

    let _ = std::fs::remove_dir_all(&install);
    std::fs::rename(&tree, &install)
        .map_err(|error| format!("无法移动到 {}: {error}", install.display()))?;
    let _ = std::fs::remove_dir_all(&work);
    report(Event::Installed { path: install });
    Ok(())
}

/// Download one file through aria2. `sha256` is supplied for the zip; the
/// manifest itself is fetched first and only its contents are trusted.
fn download(
    path: &Path,
    urls: Vec<String>,
    sha256: Option<&str>,
    report: &impl Fn(Event),
    show_progress: bool,
) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| "下载路径没有父目录".to_string())?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "下载文件名无效".to_string())?;
    let list = path.with_extension("aria2.list");
    let entries = vec![ListEntry {
        urls,
        dir: dir.to_path_buf(),
        out: name.to_string(),
        sha256: sha256.map(str::to_string),
    }];
    aria2::write_list(&list, &entries)?;
    let on_progress = |progress: crate::download::Progress| {
        if show_progress {
            report(Event::Progress {
                done: progress.done,
                total: progress.total,
                speed: progress.speed,
            });
        }
    };
    let result = aria2::run_input(&list, on_progress, |_| {});
    let _ = std::fs::remove_file(&list);
    result
}

fn installed_at(tree: &Path) -> Option<()> {
    [
        format!("{LWJGL}/lwjgl.jar"),
        format!("{LWJGL}/lwjgl-sdl.jar"),
        format!("{LWJGL_NATIVES}/liblwjgl.so"),
        format!("{NATIVE}/libopenal.so"),
        format!("{NATIVE}/libSDL3.so"),
        format!("{JNA}/libjnidispatch.so"),
    ]
    .iter()
    .all(|relative| tree.join(relative).is_file())
    .then_some(())
}

/// Extract the rootless runtime zip. Nested paths are recreated; entries that
/// escape the destination are rejected.
fn extract_pack(archive: &Path, dest: &Path) -> Result<usize, String> {
    let file =
        File::open(archive).map_err(|error| format!("无法打开 {}: {error}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|error| format!("解压 {} 失败：{error}", archive.display()))?;
    let mut count = 0;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| format!("解压失败：{error}"))?;
        if entry.is_dir() {
            continue;
        }
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| "运行库 zip 包含非法路径".to_string())?;
        let out = dest.join(&relative);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("无法创建 {}: {error}", parent.display()))?;
        }
        let mut target =
            File::create(&out).map_err(|error| format!("无法写入 {}: {error}", out.display()))?;
        std::io::copy(&mut entry, &mut target)
            .map_err(|error| format!("解压写入 {} 失败: {error}", out.display()))?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_install_directory_is_under_private_files() {
        assert!(install_dir().ends_with("runtime"));
    }

    #[test]
    fn the_runtime_version_is_pinned() {
        assert_eq!(RUNTIME_VERSION, "3.4.3");
        assert!(MANIFEST_URL.contains("Tessera-Launcher/BuildNatives4Android"));
        assert_eq!(JNA, "jna/5.17.0");
    }
}
