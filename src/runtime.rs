//! The Android native runtime pack borrowed from FCL.
//!
//! Mojang's version JSON points LWJGL at desktop glibc natives, which cannot be
//! loaded on Android. FCL solves this by replacing the whole LWJGL stack: its
//! own Android-built LWJGL jars and `.so` files, plus OpenAL, SDL and JNA. Those
//! binaries live in FCL's GitHub repository under `FCL/libs` and the LWJGL
//! assets tree, so they can be fetched at runtime instead of bloating the APK.
//!
//! Everything here is downloaded by aria2c (the launcher installs that first),
//! verified against a pinned SHA-256, unpacked into the app's private
//! `files/runtime`, and never written to `/sdcard` (which is `noexec`).
//!
//! Pinned to FCL commit `722e2361…` ("更新 lwjgl 3.4.1 资源版本", includes the
//! shaderc natives fix). URLs use `github.com/.../raw/...` rather than
//! `raw.githubusercontent.com` so gh-proxy prefixes keep working.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use crate::aria2::{self, ListEntry};
use crate::{fsutil, http};

/// FCL commit the files below were taken from.
pub const FCL_COMMIT: &str = "722e2361f8f3013b6ebc026e52749c1bf3b5be71";

const RAW_BASE: &str =
    "https://github.com/FCL-Team/FoldCraftLauncher/raw/722e2361f8f3013b6ebc026e52749c1bf3b5be71";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// AAR whose `assets/app_runtime/lwjgl/3.4.1/natives/arm64-v8a/*` is unpacked.
    LwjglNatives,
    /// AAR whose `jni/arm64-v8a/*` is unpacked to `runtime/native/arm64-v8a`.
    OpenAl,
    Sdl,
    /// FCL's MobileGlues renderer: `jni/arm64-v8a/libmobileglues.so` plus its license.
    MobileGlues,
    /// A jar copied to `runtime/lwjgl/3.4.1/`.
    LwjglJar,
    /// `runtime/jna/<version>/libjnidispatch.so`.
    Jna(&'static str),
}

struct RuntimeFile {
    /// Repository-relative path under `RAW_BASE`.
    path: &'static str,
    size: u64,
    sha256: &'static str,
    kind: Kind,
}

/// Files checked in at FCL `722e2361`. Hashes are of the repository files
/// themselves, not of the extracted members.
const FILES: &[RuntimeFile] = &[
    RuntimeFile {
        path: "FCL/libs/lwjgl-3.4.1-natives-release.aar",
        size: 17119989,
        sha256: "5a8c40b5b94525385a90b64eaac1df83bc12efcb89ddcfa6bd4b967b85a89ffb",
        kind: Kind::LwjglNatives,
    },
    RuntimeFile {
        path: "FCL/libs/openal-soft-release.aar",
        size: 3620056,
        sha256: "424947ee10e26b76a3aff6769e6a74fa4fb11b556183997b270f8278e7d3532c",
        kind: Kind::OpenAl,
    },
    RuntimeFile {
        path: "FCL/libs/SDL-release.aar",
        size: 4064412,
        sha256: "5527d2e59e76b7a58eef0d54bb8ae0ca153593a9dfc0ed73ac84fb829520298b",
        kind: Kind::Sdl,
    },
    RuntimeFile {
        path: "FCL/libs/MobileGlues-release.aar",
        size: 8280469,
        sha256: "d92070c32c155822ca9d6be200130cbe74b8b8e725eb7bbc9da1a2d3348434b3",
        kind: Kind::MobileGlues,
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/lwjgl/3.4.1/lwjgl-3.4.1-merged-modules.jar",
        size: 2094689,
        sha256: "d2e1a65beb53ecbc4538ec3624a5e129adc46174198b3ccaeb49b3e5ec4fbe18",
        kind: Kind::LwjglJar,
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/lwjgl/3.4.1/lwjgl-freetype.jar",
        size: 465796,
        sha256: "e7001800c634f13f15f767493c1debfb92d5422942c5160c524df0fa3de54a46",
        kind: Kind::LwjglJar,
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/lwjgl/3.4.1/lwjgl-lwjglx.jar",
        size: 237121,
        sha256: "03838ef76b021ea9e3b50c392659d34e61e851a1d327344e088fb939781cde2c",
        kind: Kind::LwjglJar,
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/lwjgl/3.4.1/lwjgl-nanovg.jar",
        size: 76348,
        sha256: "e825f799ac378d13e61bcae419953ecc23b8ee272c26408ffdb09f151af602c4",
        kind: Kind::LwjglJar,
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/lwjgl/3.4.1/lwjgl-openal.jar",
        size: 150986,
        sha256: "67c72306d4a233b473f66faf72de5be8f589a3c34823faf56cbd7e0f52796fc6",
        kind: Kind::LwjglJar,
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/lwjgl/3.4.1/lwjgl-shaderc.jar",
        size: 147618,
        sha256: "fa02faae80950e5bb28bdcc5a9cc7cbb7e552d723b72777d2ab1adece9d2287e",
        kind: Kind::LwjglJar,
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/lwjgl/3.4.1/lwjgl-spng.jar",
        size: 114650,
        sha256: "10daa293281297d1a968a615a957233d4b8268cf7446fae118212e2330b0fb12",
        kind: Kind::LwjglJar,
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/lwjgl/3.4.1/lwjgl-spvc.jar",
        size: 141553,
        sha256: "ddd4ba883d72b6ac2759370c81877b1ac291c552e2eb1d97911199755cb6e571",
        kind: Kind::LwjglJar,
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/lwjgl/3.4.1/lwjgl-stb.jar",
        size: 136953,
        sha256: "4f36cacea19d64f37a16dac91095d3542626fac726bb6934ecd480f5677ce7b6",
        kind: Kind::LwjglJar,
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/lwjgl/3.4.1/lwjgl-tinyfd.jar",
        size: 7677,
        sha256: "879e1403a5547cc335eb44a27052f5e33510ccc04ddd317f5825f76cc91cdc09",
        kind: Kind::LwjglJar,
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/lwjgl/3.4.1/lwjgl-vma.jar",
        size: 103366,
        sha256: "1895f7f3ad8d16e90db490a8abdc094048976e462e42d747c40941cf559fef2b",
        kind: Kind::LwjglJar,
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/lwjgl/3.4.1/lwjgl-vulkan.jar",
        size: 8540747,
        sha256: "75957d2845320adf2799c9f154c4e2b35202cf007d0d1e36a238a2fb212391fa",
        kind: Kind::LwjglJar,
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/lwjgl/3.4.1/lwjgl.jar",
        size: 1169477,
        sha256: "a675ff2a93ecb9f94bdd72bb3a9ab648f4413225749c46fc83d5aae8a8260275",
        kind: Kind::LwjglJar,
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/jna/5.13.0/natives/arm64-v8a/libjnidispatch.so",
        size: 168176,
        sha256: "95fd6129757ceae73472b014c0c7e23a27a193adc4c355693706998aa4757be8",
        kind: Kind::Jna("5.13.0"),
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/jna/5.14.0/natives/arm64-v8a/libjnidispatch.so",
        size: 168176,
        sha256: "3730ce014f7a6807ae74af78c3b9f5a3cba1f5886316b3d3858493df923efe96",
        kind: Kind::Jna("5.14.0"),
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/jna/5.15.0/natives/arm64-v8a/libjnidispatch.so",
        size: 168280,
        sha256: "1daf1544f2718b2dd694fc94b7418fd201181450e211c4d15fa532626a488c78",
        kind: Kind::Jna("5.15.0"),
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/jna/5.16.0/natives/arm64-v8a/libjnidispatch.so",
        size: 168328,
        sha256: "7e9c0158491077e5b9f249d81e30ed873bd7dd711cbb5c06b98c291402498b76",
        kind: Kind::Jna("5.16.0"),
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/jna/5.17.0/natives/arm64-v8a/libjnidispatch.so",
        size: 176520,
        sha256: "abc26e994517bcaa3309acdb0a27373864086c7569c89d3087b8626fada9ef06",
        kind: Kind::Jna("5.17.0"),
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/jna/5.18.0/natives/arm64-v8a/libjnidispatch.so",
        size: 176520,
        sha256: "abc26e994517bcaa3309acdb0a27373864086c7569c89d3087b8626fada9ef06",
        kind: Kind::Jna("5.18.0"),
    },
    RuntimeFile {
        path: "FCL/src/main/assets/app_runtime/jna/5.19.1/natives/arm64-v8a/libjnidispatch.so",
        size: 176520,
        sha256: "abc26e994517bcaa3309acdb0a27373864086c7569c89d3087b8626fada9ef06",
        kind: Kind::Jna("5.19.1"),
    },
];

const LWJGL: &str = "lwjgl/3.4.1";
const LWJGL_NATIVES: &str = "lwjgl/3.4.1/natives/arm64-v8a";
const NATIVE: &str = "native/arm64-v8a";

/// Whether this build has an Android arm64 runtime pack.
pub fn supported() -> bool {
    cfg!(all(target_os = "android", target_arch = "aarch64"))
}

/// The installed pack, if its markers are all present.
pub fn install_dir() -> PathBuf {
    crate::platform::private_dir().join("runtime")
}

pub fn installed() -> Option<PathBuf> {
    let dir = install_dir();
    [
        format!("{LWJGL}/lwjgl.jar"),
        format!("{LWJGL_NATIVES}/liblwjgl.so"),
        format!("{NATIVE}/libopenal.so"),
        format!("{NATIVE}/libSDL3.so"),
        format!("{NATIVE}/libmobileglues.so"),
    ]
    .iter()
    .all(|relative| dir.join(relative).is_file())
    .then_some(dir)
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
    let aar_dir = work.join("aar");
    let tree = work.join("runtime");

    let total: u64 = FILES.iter().map(|file| file.size).sum();
    report(Event::Log(format!(
        "运行库：{} 个文件，约 {}",
        FILES.len(),
        http::human_size(total)
    )));

    // First try the configured proxy (all candidates as tab-separated mirrors).
    // If aria2 fails, retry the direct URLs once.
    let direct = urls_for(None);
    let proxied = urls_for(Some(proxy));
    let mut last_error = String::new();
    let mut fetched = false;
    for (attempt, choices) in [proxied, direct].into_iter().enumerate() {
        let list = work.join(format!("runtime-{attempt}.list"));
        let entries: Vec<ListEntry> = FILES
            .iter()
            .zip(choices.iter())
            .map(|(file, urls)| ListEntry {
                urls: urls.clone(),
                dir: destination_dir(&aar_dir, &tree, file.kind),
                out: output_name(file),
                sha256: Some(file.sha256.to_string()),
            })
            .collect();
        aria2::write_list(&list, &entries)?;
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
                fetched = true;
                break;
            }
            Err(error) => last_error = error,
        }
    }
    if !fetched {
        let _ = std::fs::remove_dir_all(&work);
        return Err(format!("运行库下载失败：{last_error}"));
    }

    report(Event::Extracting);
    let mut extracted = Vec::new();
    for file in FILES {
        let name = output_name(file);
        match file.kind {
            Kind::LwjglNatives => {
                let archive = aar_dir.join(&name);
                verify(&archive, file)?;
                extracted.push(extract_aar(
                    &archive,
                    "assets/app_runtime/lwjgl/3.4.1/natives/arm64-v8a/",
                    &tree.join(LWJGL_NATIVES),
                )?);
            }
            Kind::OpenAl | Kind::Sdl => {
                let archive = aar_dir.join(&name);
                verify(&archive, file)?;
                extracted.push(extract_aar(&archive, "jni/arm64-v8a/", &tree.join(NATIVE))?);
            }
            Kind::MobileGlues => {
                let archive = aar_dir.join(&name);
                verify(&archive, file)?;
                extracted.push(extract_aar(&archive, "jni/arm64-v8a/", &tree.join(NATIVE))?);
                extract_member(
                    &archive,
                    "assets/licenses/mobileglues-lgplv2.1-license.txt",
                    &tree.join("licenses/mobileglues-lgplv2.1-license.txt"),
                )?;
            }
            Kind::LwjglJar => {
                let path = tree.join(LWJGL).join(&name);
                verify(&path, file)?;
            }
            Kind::Jna(version) => {
                let path = tree.join("jna").join(version).join(&name);
                verify(&path, file)?;
            }
        }
    }
    if installed_at(&tree).is_none() {
        let _ = std::fs::remove_dir_all(&work);
        return Err(format!(
            "运行库解压后缺少文件（{} 个成员）",
            extracted.iter().sum::<usize>()
        ));
    }

    let _ = std::fs::remove_dir_all(&install);
    std::fs::rename(&tree, &install)
        .map_err(|error| format!("无法移动到 {}: {error}", install.display()))?;
    let _ = std::fs::remove_dir_all(&work);
    report(Event::Installed { path: install });
    Ok(())
}

fn urls_for(proxy: Option<&str>) -> Vec<Vec<String>> {
    FILES
        .iter()
        .map(|file| {
            let url = format!("{RAW_BASE}/{}", file.path);
            match proxy {
                Some(proxy) => http::github_candidates(&url, proxy),
                None => vec![url],
            }
        })
        .collect()
}

fn destination_dir(aar_dir: &Path, tree: &Path, kind: Kind) -> PathBuf {
    match kind {
        Kind::LwjglNatives | Kind::OpenAl | Kind::Sdl | Kind::MobileGlues => aar_dir.to_path_buf(),
        Kind::LwjglJar => tree.join(LWJGL),
        Kind::Jna(version) => tree.join("jna").join(version),
    }
}

fn output_name(file: &RuntimeFile) -> String {
    file.path.rsplit('/').next().unwrap_or("file").to_string()
}

fn installed_at(tree: &Path) -> Option<()> {
    [
        format!("{LWJGL}/lwjgl.jar"),
        format!("{LWJGL_NATIVES}/liblwjgl.so"),
        format!("{NATIVE}/libopenal.so"),
        format!("{NATIVE}/libSDL3.so"),
        format!("{NATIVE}/libmobileglues.so"),
    ]
    .iter()
    .all(|relative| tree.join(relative).is_file())
    .then_some(())
}

fn verify(path: &Path, file: &RuntimeFile) -> Result<(), String> {
    let actual = fsutil::sha256_file(path)?;
    if actual != file.sha256 {
        return Err(format!(
            "{} 校验失败：期望 {}，实际 {actual}",
            path.display(),
            file.sha256
        ));
    }
    Ok(())
}

/// Copy the regular files under `prefix` into `dest`, flattened.
fn extract_aar(archive: &Path, prefix: &str, dest: &Path) -> Result<usize, String> {
    let file =
        File::open(archive).map_err(|error| format!("无法打开 {}: {error}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|error| format!("解压 {} 失败：{error}", archive.display()))?;
    std::fs::create_dir_all(dest)
        .map_err(|error| format!("无法创建 {}: {error}", dest.display()))?;
    let mut count = 0;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| format!("解压失败：{error}"))?;
        if entry.is_dir() {
            continue;
        }
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        let Some(name) = relative.strip_prefix(prefix) else {
            continue;
        };
        if name.is_empty() || name.contains('/') {
            continue;
        }
        let out = dest.join(name);
        let mut target =
            File::create(&out).map_err(|error| format!("无法写入 {}: {error}", out.display()))?;
        std::io::copy(&mut entry, &mut target).map_err(|error| format!("解压写入失败: {error}"))?;
        count += 1;
    }
    Ok(count)
}

/// Copy one named member out of an AAR.
fn extract_member(archive: &Path, member: &str, dest: &Path) -> Result<(), String> {
    let file =
        File::open(archive).map_err(|error| format!("无法打开 {}: {error}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|error| format!("解压 {} 失败：{error}", archive.display()))?;
    let mut entry = zip
        .by_name(member)
        .map_err(|error| format!("包里没有 {member}：{error}"))?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建 {}: {error}", parent.display()))?;
    }
    let mut target =
        File::create(dest).map_err(|error| format!("无法写入 {}: {error}", dest.display()))?;
    std::io::copy(&mut entry, &mut target).map_err(|error| format!("解压写入失败: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pack_is_pinned_to_a_commit_and_github_raw() {
        assert_eq!(FCL_COMMIT.len(), 40);
        assert!(RAW_BASE.starts_with("https://github.com/"));
        assert!(RAW_BASE.contains(FCL_COMMIT));
        for file in FILES {
            assert_eq!(file.sha256.len(), 64, "{}", file.path);
            assert!(file.path.starts_with("FCL/"));
            assert!(!output_name(file).contains('/'));
        }
    }

    #[test]
    fn the_install_directory_is_under_private_files() {
        assert!(install_dir().ends_with("runtime"));
    }
}
