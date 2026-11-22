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
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

/// One entry in an `aria2c --input-file` list.
#[derive(Debug, Clone)]
pub struct ListEntry {
    /// Every source for this file. aria2 tries them in order and treats a
    /// tab-separated line as one entity with mirrors.
    pub urls: Vec<String>,
    /// Absolute destination directory.
    pub dir: PathBuf,
    /// Bare file name inside `dir`.
    pub out: String,
    /// Optional `checksum=sha-256=…` line.
    pub sha256: Option<String>,
}

/// Write an aria2 input list. Every destination directory is created here:
/// aria2 fails the whole entry when `dir=` does not exist.
pub fn write_list(path: &Path, entries: &[ListEntry]) -> Result<(), String> {
    let mut body = String::new();
    for entry in entries {
        if entry.out.contains('/') || entry.out.contains('\\') {
            return Err(format!("非法的文件名：{}", entry.out));
        }
        std::fs::create_dir_all(&entry.dir)
            .map_err(|error| format!("无法创建 {}: {error}", entry.dir.display()))?;
        body.push_str(&entry.urls.join("\t"));
        body.push('\n');
        body.push_str(&format!("  dir={}\n", entry.dir.display()));
        body.push_str(&format!("  out={}\n", entry.out));
        if let Some(sha256) = &entry.sha256 {
            body.push_str(&format!("  checksum=sha-256={sha256}\n"));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建 {}: {error}", parent.display()))?;
    }
    std::fs::write(path, body)
        .map_err(|error| format!("无法写入 {}: {error}", path.display()))
}

/// A PEM bundle of the system CA certificates.
///
/// aria2 uses OpenSSL and has no Android trust-store integration; without
/// `--ca-certificate` every HTTPS download fails with "self signed certificate
/// in certificate chain". The bundle lives in the app's private directory so
/// aria2 (same UID) can read it.
pub fn ca_bundle() -> Result<PathBuf, String> {
    let path = crate::platform::private_dir().join("ca.pem");
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > 0) {
        return Ok(path);
    }
    let mut data = Vec::new();
    for dir in [
        "/system/etc/security/cacerts",
        "/apex/com.android.conscrypt/cacerts",
    ] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            if bytes.starts_with(b"-----BEGIN CERTIFICATE-----") {
                data.extend_from_slice(&bytes);
                if !data.ends_with(b"\n") {
                    data.push(b'\n');
                }
            }
        }
    }
    if data.is_empty() {
        return Err("找不到系统 CA 证书".to_string());
    }
    std::fs::write(&path, data)
        .map_err(|error| format!("无法写入 {}: {error}", path.display()))?;
    Ok(path)
}

/// Run one aria2 input list and report the aggregate progress.
///
/// stdout and stderr are both drained (stderr on its own thread) so a chatty
/// failure cannot fill the pipe and stall the child. `--show-console-readout`
/// with a one-second summary gives a parseable line such as
/// `[#gid 1.5MiB/2.3MiB(64%) CN:1 DL:526KiB ETA:1s]`.
pub fn run_input(
    list: &Path,
    on_progress: impl Fn(crate::download::Progress),
    on_log: impl Fn(&str),
) -> Result<(), String> {
    let binary = installed().ok_or_else(|| "aria2c 还未安装".to_string())?;
    let ca = if cfg!(target_os = "android") {
        Some(ca_bundle()?)
    } else {
        None
    };
    let mut args = vec![
        "-i".to_string(),
        list.display().to_string(),
        "--no-conf=true".to_string(),
        "--enable-color=false".to_string(),
        "--show-console-readout=true".to_string(),
        "--summary-interval=1".to_string(),
        "--console-log-level=warn".to_string(),
        "--file-allocation=none".to_string(),
        "--auto-file-renaming=false".to_string(),
        "--allow-overwrite=true".to_string(),
        "--max-tries=3".to_string(),
        "--retry-wait=1".to_string(),
        "--connect-timeout=10".to_string(),
        "--timeout=30".to_string(),
        "-j8".to_string(),
        "-x1".to_string(),
        "-s1".to_string(),
        format!("--stop-with-process={}", std::process::id()),
    ];
    if let Some(ca) = &ca {
        args.push(format!("--ca-certificate={}", ca.display()));
    }

    let mut child = Command::new(binary)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法启动 aria2c: {error}"))?;

    let stderr = child.stderr.take();
    let (sender, receiver) = std::sync::mpsc::channel::<String>();
    let drain = stderr.map(|stderr| {
        let sender = sender.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = sender.send(line);
            }
        })
    });

    let mut errors: Vec<String> = Vec::new();
    let mut last: Option<(u64, u64)> = None;
    let mut handle = |line: &str| {
        on_log(line);
        if let Some(progress) = parse_progress(line) {
            if progress.total > 0 {
                let key = (progress.done, progress.total);
                if last != Some(key) {
                    last = Some(key);
                    on_progress(progress);
                }
            }
        }
        if line.contains("errorCode=") || line.contains("Exception:") {
            errors.push(line.to_string());
            if errors.len() > 8 {
                errors.remove(0);
            }
        }
    };

    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            handle(&line);
            while let Ok(line) = receiver.try_recv() {
                handle(&line);
            }
        }
    }
    let status = child
        .wait()
        .map_err(|error| format!("等待 aria2c 失败: {error}"))?;
    drop(sender);
    for line in receiver.iter() {
        handle(&line);
    }
    if let Some(drain) = drain {
        let _ = drain.join();
    }
    if status.success() {
        Ok(())
    } else {
        let detail = errors
            .last()
            .cloned()
            .unwrap_or_else(|| format!("退出码 {}", status.code().unwrap_or(-1)));
        Err(format!("aria2c 下载失败：{detail}"))
    }
}

/// Parse `[#gid 1.5MiB/2.3MiB(64%) CN:1 DL:526KiB ETA:1s]`.
fn parse_progress(line: &str) -> Option<crate::download::Progress> {
    let start = line.find("[#")?;
    let rest = &line[start + 2..];
    let (_, rest) = rest.split_once(' ')?;
    let (done, rest) = rest.split_once('/')?;
    let (total, rest) = rest.split_once('(')?;
    let done = parse_size(done)?;
    let total = parse_size(total)?;
    let speed = rest
        .split_once("DL:")
        .and_then(|(_, tail)| tail.split_whitespace().next())
        .and_then(parse_size)
        .unwrap_or(0);
    Some(crate::download::Progress {
        done,
        total,
        speed: speed as f64,
    })
}

/// `1.5MiB` / `526KiB` / `0B` → bytes.
fn parse_size(value: &str) -> Option<u64> {
    let value = value.trim();
    for (suffix, factor) in [
        ("TiB", 1024_u64.pow(4)),
        ("GiB", 1024_u64.pow(3)),
        ("MiB", 1024_u64.pow(2)),
        ("KiB", 1024),
        ("B", 1),
    ] {
        if let Some(number) = value.strip_suffix(suffix) {
            let number: f64 = number.trim().parse().ok()?;
            return Some((number * factor as f64) as u64);
        }
    }
    None
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
