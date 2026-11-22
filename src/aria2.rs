//! The pinned Android aria2c build and its installer.
//!
//! Same route as the desktop build: `mdt-minecraft` writes an
//! `aria2c --input-file` list and aria2 moves the bytes. The phone-specific
//! part is placement — `/sdcard` is mounted `noexec`, so the binary lives in
//! the app's private files directory (this build targets API 28, which predates
//! Android's no-exec policy) while the data it downloads stays on
//! `/sdcard/Tessera`.
//!
//! The upstream 1.37.0 release ships an aarch64 Android build. It is one
//! self-contained ELF (OpenSSL, c-ares, expat, zlib and libssh2 are linked
//! statically) but it is built for API 33, so an older phone will pass the
//! checksum and then fail `--version`; that is reported as a failure instead of
//! leaving a dead binary behind.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use crate::http;

/// The release every build below was cut from.
pub const VERSION: &str = "1.37.0";

/// One pinned download (the official Android arm64 asset).
#[derive(Debug, Clone, Copy)]
pub struct Build {
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    /// Path inside the zip.
    pub member: &'static str,
}

pub const BUILD: Build = Build {
    url: "https://github.com/aria2/aria2/releases/download/release-1.37.0/aria2-1.37.0-aarch64-linux-android-build1.zip",
    size: 2_508_263,
    sha256: "a22d18b621f06025926f581875b094e89899fda123a7d2286fe5841e1b4e13a1",
    member: "aria2-1.37.0-aarch64-linux-android-build1/aria2c",
};

/// Whether this build has a pinned Android binary.
pub fn supported() -> bool {
    cfg!(all(target_os = "android", target_arch = "aarch64"))
}

/// Where the installed binary lives (`<private>/bin/aria2c`).
pub fn binary_path() -> PathBuf {
    crate::platform::private_dir().join("bin").join("aria2c")
}

/// The installed binary, if it is there.
pub fn installed() -> Option<PathBuf> {
    supported().then(binary_path).filter(|path| path.is_file())
}

/// What the install thread reports.
#[derive(Debug)]
pub enum Event {
    /// A line for the status card.
    Log(String),
    Progress { done: u64, total: u64, speed: f64 },
    Installed {
        version: String,
        path: PathBuf,
        /// Which URL actually served the archive, e.g. `直连 github.com` or
        /// `代理 ghfast.top → objects.githubusercontent.com`.
        source: String,
    },
    Failed(String),
}

/// Download, verify and install aria2c on a worker thread.
pub fn spawn_install(proxy: String, report: impl Fn(Event) + Send + 'static) {
    if !supported() {
        report(Event::Failed(
            "当前平台没有固定构建（需要 Android arm64）".to_string(),
        ));
        return;
    }
    std::thread::spawn(move || {
        let outcome = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("无法创建异步运行时: {error}"))
            .and_then(|runtime| runtime.block_on(install(&proxy, &report)));
        if let Err(problem) = outcome {
            report(Event::Failed(problem));
        }
    });
}

async fn install(proxy: &str, report: &impl Fn(Event)) -> Result<(), String> {
    let binary = binary_path();
    let dir = binary
        .parent()
        .ok_or_else(|| "安装路径没有父目录".to_string())?
        .to_path_buf();
    std::fs::create_dir_all(&dir).map_err(|error| format!("无法创建 {}: {error}", dir.display()))?;
    let work = dir.join(".incoming");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|error| format!("无法创建临时目录: {error}"))?;
    let archive = work.join("aria2.zip");

    let client = http::client()?;
    let urls = http::github_candidates(BUILD.url, proxy);
    let mut last_error = String::new();
    let mut fetched = false;
    let mut used_url = String::new();
    let mut final_url = String::new();
    for url in &urls {
        report(Event::Log(format!("aria2c：{}", http::shorten(url))));
        match crate::download::file(&client, url, &archive, BUILD.size, |progress| {
            report(Event::Progress {
                done: progress.done,
                total: progress.total,
                speed: progress.speed,
            });
        })
        .await
        {
            Ok(response_url) => {
                fetched = true;
                used_url = url.clone();
                final_url = response_url;
                break;
            }
            Err(error) => {
                last_error = format!("{}：{error}", http::shorten(url));
                let _ = std::fs::remove_file(&archive);
            }
        }
    }
    if !fetched {
        let _ = std::fs::remove_dir_all(&work);
        return Err(format!("aria2c 下载失败：{last_error}"));
    }

    let actual = crate::fsutil::sha256_file(&archive)?;
    if actual != BUILD.sha256 {
        let _ = std::fs::remove_dir_all(&work);
        return Err(format!(
            "aria2c 校验失败：期望 {}，实际 {actual}",
            BUILD.sha256
        ));
    }

    let unpacked = extract(&archive, &work, BUILD.member)?;
    let _ = std::fs::remove_file(&binary);
    place(&unpacked, &binary)?;
    mark_executable(&binary);
    let _ = std::fs::remove_dir_all(&work);

    // A binary that cannot answer `--version` is a broken download, not a
    // dependency: report it as a failure instead of leaving it on disk.
    let Some(version) = probe(&binary) else {
        let _ = std::fs::remove_file(&binary);
        return Err(format!(
            "{} 跑不起来（官方构建需要 Android 13+，或 CPU 不是 arm64）",
            binary.display()
        ));
    };
    if !version.starts_with(VERSION) {
        report(Event::Log(format!(
            "aria2c 报告 {version}，清单写的是 {VERSION}（已按清单校验）"
        )));
    }
    report(Event::Installed {
        version,
        path: binary,
        source: http::source_label(
            &used_url,
            BUILD.url,
            &final_url,
            http::normalize_proxy(proxy).is_some(),
        ),
    });
    Ok(())
}

/// Pull `member` out of a zip archive.
fn extract(archive: &Path, dest: &Path, member: &str) -> Result<PathBuf, String> {
    let file = File::open(archive).map_err(|error| format!("无法打开下载文件: {error}"))?;
    let mut zip =
        zip::ZipArchive::new(BufReader::new(file)).map_err(|error| format!("解压失败：{error}"))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| format!("解压失败：{error}"))?;
        let matches = entry
            .enclosed_name()
            .is_some_and(|path| path == Path::new(member));
        if !matches || entry.is_dir() {
            continue;
        }
        let out = dest.join("aria2c");
        let mut target =
            File::create(&out).map_err(|error| format!("无法写入 {}: {error}", out.display()))?;
        std::io::copy(&mut entry, &mut target)
            .map_err(|error| format!("解压写入失败: {error}"))?;
        target
            .sync_all()
            .map_err(|error| format!("无法写入 {}: {error}", out.display()))?;
        return Ok(out);
    }
    Err(format!("压缩包里没有 {member}"))
}

/// Write `source` to `dest`, falling back to a copy across filesystems.
fn place(source: &Path, dest: &Path) -> Result<(), String> {
    match std::fs::rename(source, dest) {
        Ok(()) => Ok(()),
        Err(_) => std::fs::copy(source, dest)
            .map(|_| ())
            .map_err(|error| format!("无法写入 {}: {error}", dest.display())),
    }
}

fn mark_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Run `aria2c --version` and return the version it reports.
pub fn probe(binary: &Path) -> Option<String> {
    let output = std::process::Command::new(binary)
        .arg("--version")
        .output()
        .ok()?;
    parse_version(&String::from_utf8_lossy(&output.stdout))
}

/// The version out of the first line of `aria2c --version`.
pub fn parse_version(output: &str) -> Option<String> {
    let line = output.lines().next()?;
    let rest = line.strip_prefix("aria2 version ")?;
    let version = rest.split_whitespace().next()?;
    Some(version.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_is_pinned_and_on_github() {
        assert!(BUILD.url.starts_with("https://github.com/"));
        assert_eq!(BUILD.sha256.len(), 64);
        assert!(BUILD.sha256.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(BUILD.size > 0);
        assert!(BUILD.member.ends_with("/aria2c"));
    }

    #[test]
    fn the_version_line_is_parsed() {
        assert_eq!(
            parse_version("aria2 version 1.37.0\nCopyright (C) 2006, 2019"),
            Some("1.37.0".to_string())
        );
        assert_eq!(parse_version("not aria2"), None);
    }

    #[test]
    fn the_install_path_is_in_the_private_directory() {
        // `cargo test` has no pinned private dir, so it falls back to the data
        // root; the important part is the `/bin/aria2c` tail.
        assert!(binary_path().ends_with("bin/aria2c"));
    }
}
