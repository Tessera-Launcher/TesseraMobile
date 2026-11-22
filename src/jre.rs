//! The Android arm64 JRE installer.
//!
//! `BuildJRE4Android` publishes a machine-readable manifest at a stable
//! `releases/latest/download/` URL, so no GitHub API token or JSON API call is
//! needed: read `jre-manifest.json`, take the zip name and SHA-256 from it, then
//! fetch `releases/latest/download/<zip name>`.
//!
//! Every byte — manifest and archive — is fetched by the installed aria2c
//! binary, not by reqwest. The launcher only installs aria2c itself over HTTP;
//! once that exists, every other dependency goes through the same downloader as
//! the desktop build.
//!
//! The package is rootless (`lib/`, `conf/`, `legal/`, `release`) and is meant
//! to be unpacked into the app's private `files/jre`: the JVM is loaded with
//! `dlopen`, so it cannot live on the `noexec` `/sdcard` volume. Installation is
//! staged in `files/jre.incoming` and renamed into place, so an interrupted
//! download never leaves a half JRE that the launcher would treat as installed.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::aria2::{self, ListEntry};
use crate::{fsutil, http};

/// Stable URL that redirects to the newest release's manifest.
pub const MANIFEST_URL: &str =
    "https://github.com/Tessera-Launcher/BuildJRE4Android/releases/latest/download/jre-manifest.json";

/// Stable base for the asset named by the manifest.
const ASSET_BASE: &str =
    "https://github.com/Tessera-Launcher/BuildJRE4Android/releases/latest/download";

/// The manifest, trimmed to the fields the installer needs.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub abi: String,
    pub profile: String,
    pub min_sdk: u32,
    pub files: Files,
}

#[derive(Debug, Deserialize)]
pub struct Files {
    pub zip: ZipAsset,
}

#[derive(Debug, Deserialize)]
pub struct ZipAsset {
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

/// Whether this build has a pinned JRE build.
pub fn supported() -> bool {
    cfg!(all(target_os = "android", target_arch = "aarch64"))
}

/// `<private>/jre`, next to the aria2c binary but separate from the data root.
pub fn install_dir() -> PathBuf {
    crate::platform::private_dir().join("jre")
}

/// The installed JRE, if it looks complete.
pub fn installed() -> Option<PathBuf> {
    let dir = install_dir();
    (dir.join("release").is_file() && dir.join("lib/server/libjvm.so").is_file()).then_some(dir)
}

/// What the install thread reports.
#[derive(Debug)]
pub enum Event {
    Log(String),
    Progress {
        done: u64,
        total: u64,
        speed: f64,
    },
    /// The archive is verified; unpacking the 100+ MB tree can take a moment.
    Extracting,
    Installed {
        id: String,
        path: PathBuf,
        /// Which candidate aria2c downloaded from.
        source: String,
    },
    Failed(String),
}

/// Fetch the manifest, download and verify the zip, unpack it into `files/jre`.
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
        return Err("请先安装 aria2c，再下载 JRE".to_string());
    }
    let install = install_dir();
    let parent = install
        .parent()
        .ok_or_else(|| "JRE 安装路径没有父目录".to_string())?
        .to_path_buf();
    std::fs::create_dir_all(&parent)
        .map_err(|error| format!("无法创建 {}: {error}", parent.display()))?;
    let work = parent.join("jre.incoming");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|error| format!("无法创建临时目录: {error}"))?;

    let (manifest, _manifest_url) = fetch_manifest(&work, proxy, report)?;
    if manifest.abi != "arm64-v8a" {
        let _ = std::fs::remove_dir_all(&work);
        return Err(format!("JRE 清单是 {}，本机需要 arm64-v8a", manifest.abi));
    }
    if manifest.files.zip.name.is_empty() || manifest.files.zip.sha256.len() != 64 {
        let _ = std::fs::remove_dir_all(&work);
        return Err("JRE 清单里的 zip 名称或哈希不完整".to_string());
    }
    if manifest.profile != "safe" {
        report(Event::Log(format!(
            "JRE 清单是 {} 档，按清单安装",
            manifest.profile
        )));
    }

    // A name from the manifest must stay a file name: the URL is built from it.
    let zip_name = manifest
        .files
        .zip
        .name
        .rsplit('/')
        .next()
        .unwrap_or(&manifest.files.zip.name);
    let zip_url = format!("{ASSET_BASE}/{zip_name}");
    let source = download_zip(&work, &zip_url, &manifest, proxy, report)?;

    report(Event::Extracting);
    let archive = work.join("jre.zip");
    let unpack = work.join("unpack");
    std::fs::create_dir_all(&unpack).map_err(|error| format!("无法创建临时目录: {error}"))?;
    extract_zip(&archive, &unpack)?;

    // Only replace a working JRE once the new tree is complete.
    let _ = std::fs::remove_dir_all(&install);
    std::fs::rename(&unpack, &install)
        .map_err(|error| format!("无法移动到 {}: {error}", install.display()))?;
    if installed().is_none() {
        let _ = std::fs::remove_dir_all(&install);
        let _ = std::fs::remove_dir_all(&work);
        return Err("JRE 解压后缺少 release 或 lib/server/libjvm.so".to_string());
    }
    let _ = std::fs::remove_dir_all(&work);

    report(Event::Installed {
        id: manifest.id,
        path: install,
        source,
    });
    Ok(())
}

fn fetch_manifest(
    work: &Path,
    proxy: &str,
    report: &impl Fn(Event),
) -> Result<(Manifest, String), String> {
    let urls = http::github_candidates(MANIFEST_URL, proxy);
    let mut last_error = String::new();
    for url in urls {
        report(Event::Log(format!("JRE 清单：{}", http::shorten(&url))));
        let list = work.join("manifest.list");
        aria2::write_list(
            &list,
            &[ListEntry {
                urls: vec![url.clone()],
                dir: work.to_path_buf(),
                out: "jre-manifest.json".to_string(),
                sha256: None,
            }],
        )?;
        match aria2::run_input(
            &list,
            |progress| {
                report(Event::Progress {
                    done: progress.done,
                    total: progress.total,
                    speed: progress.speed,
                })
            },
            |_| {},
        ) {
            Ok(()) => {
                let text = std::fs::read_to_string(work.join("jre-manifest.json"))
                    .map_err(|error| format!("无法读取 JRE 清单: {error}"))?;
                match serde_json::from_str::<Manifest>(&text) {
                    Ok(manifest) => return Ok((manifest, url)),
                    Err(error) => {
                        last_error = format!("{}：清单解析失败：{error}", http::shorten(&url));
                    }
                }
            }
            Err(error) => last_error = error,
        }
    }
    let _ = std::fs::remove_dir_all(work);
    Err(format!("无法获取 JRE 清单：{last_error}"))
}

fn download_zip(
    work: &Path,
    zip_url: &str,
    manifest: &Manifest,
    proxy: &str,
    report: &impl Fn(Event),
) -> Result<String, String> {
    let urls = http::github_candidates(zip_url, proxy);
    let proxy_configured = http::normalize_proxy(proxy).is_some();
    let mut last_error = String::new();
    for url in urls {
        report(Event::Log(format!("JRE：{}", http::shorten(&url))));
        let list = work.join("zip.list");
        aria2::write_list(
            &list,
            &[ListEntry {
                urls: vec![url.clone()],
                dir: work.to_path_buf(),
                out: "jre.zip".to_string(),
                sha256: Some(manifest.files.zip.sha256.clone()),
            }],
        )?;
        let result = aria2::run_input(
            &list,
            |progress| {
                report(Event::Progress {
                    done: progress.done,
                    total: progress.total,
                    speed: progress.speed,
                })
            },
            |_| {},
        );
        if let Err(error) = result {
            last_error = error;
            let _ = std::fs::remove_file(work.join("jre.zip"));
            continue;
        }
        let actual = fsutil::sha256_file(&work.join("jre.zip"))?;
        if actual != manifest.files.zip.sha256 {
            last_error = format!(
                "JRE 校验失败：期望 {}，实际 {actual}",
                manifest.files.zip.sha256
            );
            let _ = std::fs::remove_file(work.join("jre.zip"));
            continue;
        }
        return Ok(http::source_label(&url, zip_url, &url, proxy_configured));
    }
    let _ = std::fs::remove_dir_all(work);
    Err(format!("JRE 下载失败：{last_error}"))
}

/// Unpack every regular file, keeping the archive's relative layout.
fn extract_zip(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|error| format!("无法打开下载文件: {error}"))?;
    let mut zip =
        zip::ZipArchive::new(BufReader::new(file)).map_err(|error| format!("解压失败：{error}"))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| format!("解压失败：{error}"))?;
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let out = dest.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)
                .map_err(|error| format!("无法创建 {}: {error}", out.display()))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("无法创建 {}: {error}", parent.display()))?;
        }
        let mut target =
            File::create(&out).map_err(|error| format!("无法写入 {}: {error}", out.display()))?;
        std::io::copy(&mut entry, &mut target).map_err(|error| format!("解压写入失败: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_fixture_shows_the_documented_shape() {
        let manifest: Manifest = serde_json::from_str(
            r#"{
                "id": "jre28-arm64-20261121",
                "abi": "arm64-v8a",
                "profile": "safe",
                "min_sdk": 28,
                "files": {
                    "zip": {
                        "name": "jre28-arm64-20261121-safe.zip",
                        "sha256": "4fc54d3985a3d51732e80a06f2d05550a17b8ba675110c9c575862b14a79ef68",
                        "size": 41488961
                    }
                }
            }"#,
        )
        .unwrap();
        assert_eq!(manifest.id, "jre28-arm64-20261121");
        assert_eq!(manifest.abi, "arm64-v8a");
        assert_eq!(manifest.files.zip.name, "jre28-arm64-20261121-safe.zip");
        assert_eq!(manifest.min_sdk, 28);
    }

    #[test]
    fn the_install_directory_is_under_private_files() {
        assert!(install_dir().ends_with("jre"));
    }
}
