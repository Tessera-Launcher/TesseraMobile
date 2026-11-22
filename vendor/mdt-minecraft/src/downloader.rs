//! Game-file download and SHA-1 verification.
//!
//! Parses `version.json`, builds concurrent download tasks, verifies every file against its
//! manifest SHA-1, applies mirror rewriting, and filters library rules.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::security::{is_safe_http_url, is_sha1_hex};
use serde::{Deserialize, Serialize};

// ===== 1. data structures =====

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Rule {
    pub action: String,
    pub os: Option<OsRule>,
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct OsRule {
    pub name: Option<String>,
    pub arch: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct LibraryDownloads {
    pub artifact: Option<Artifact>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Artifact {
    pub path: Option<String>,
    /// Empty when the profile has no digest for this artifact (loader profiles
    /// that only carry `{name, url}`, or a synthesized LWJGL entry). Such a task
    /// is still planned; it just cannot be hash-verified, so `verify` falls back
    /// to existence and size.
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub size: u64,
    pub url: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub downloads: LibraryDownloads,
    pub rules: Option<Vec<Rule>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct AssetIndexInfo {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

/// The parsed Minecraft `version.json` for a single version.
///
/// Returned by the plan builders so callers can inspect the resolved
/// version's assets, libraries, and downloads.
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct VersionJson {
    pub id: String,
    #[serde(rename = "assetIndex")]
    pub asset_index: Option<AssetIndexInfo>,
    pub downloads: Option<HashMap<String, Artifact>>,
    #[serde(default)]
    pub libraries: Vec<Library>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AssetIndex {
    pub objects: HashMap<String, AssetObject>,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

// ===== 2. download tasks =====

/// A single file to download and verify.
#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,
    pub dest_path: PathBuf,
    pub sha1: String,
    pub size: u64,
}

// ===== 3. rule evaluation =====

/// Evaluate whether the current platform should download this library.
fn evaluate_rules(rules: &Option<Vec<Rule>>) -> bool {
    // Mojang 你不干人事.png
    let Some(rules) = rules else {
        return true;
    };
    let os_name = std::env::consts::OS;
    let os_arch = std::env::consts::ARCH;

    let mut allowed = false;
    for rule in rules {
        if rule.features.is_some() {
            continue;
        }

        let os_match = rule.os.as_ref().is_none_or(|os| {
            let name_ok = os.name.as_ref().is_none_or(|n| match n.as_str() {
                "windows" => os_name == "windows",
                "osx" => os_name == "macos",
                "linux" => os_name == "linux",
                _ => false,
            });
            let arch_ok = os.arch.as_ref().is_none_or(|a| match a.as_str() {
                "x86" => os_arch == "x86",
                "x86_64" => os_arch == "x86_64",
                "arm64" => os_arch == "aarch64",
                _ => false,
            });
            name_ok && arch_ok
        });

        if os_match {
            allowed = rule.action == "allow";
        }
    }
    allowed
}

/// Build download tasks from a parsed version JSON (client JAR and non-LWJGL libraries).
pub fn build_tasks_from_version_json(
    version_json: &VersionJson,
    root_dir: &Path,
) -> Vec<DownloadTask> {
    build_tasks_from_version_json_into(version_json, root_dir, &version_json.id)
}

/// Build the download tasks for a version JSON, but put the version-scoped files
/// (`<id>.jar`) under `versions/<dir_name>` instead of
/// `versions/<version_json.id>`.
///
/// Used by the self-contained loader install: the loader's directory owns the
/// client jar, and no `versions/<mc_version>` directory is created at all.
pub fn build_tasks_from_version_json_into(
    version_json: &VersionJson,
    root_dir: &Path,
    dir_name: &str,
) -> Vec<DownloadTask> {
    let mut tasks = Vec::new();
    let libraries_dir = root_dir.join("libraries");

    // A version JSON is remote input. Every task is admitted only when its SHA-1 is a real digest,
    // its URL is http(s), and its Maven-relative path stays inside `libraries/` — otherwise the
    // downloader would happily verify nothing and write wherever the manifest says.
    let push = |tasks: &mut Vec<DownloadTask>,
                url: &str,
                dest_path: PathBuf,
                sha1: &str,
                size: u64| {
        if !(sha1.is_empty() || is_sha1_hex(sha1)) || !is_safe_http_url(url) {
            return;
        }
        tasks.push(DownloadTask {
            url: url.to_string(),
            dest_path,
            sha1: sha1.to_string(),
            size,
        });
    };

    // 1. client.jar
    if let Some(downloads) = &version_json.downloads {
        if let Some(client) = downloads.get("client") {
            push(
                &mut tasks,
                &client.url,
                root_dir
                    .join("versions")
                    .join(dir_name)
                    .join(format!("{dir_name}.jar")),
                &client.sha1,
                client.size,
            );
        }
    }

    // 2. libraries
    for lib in &version_json.libraries {
        if !evaluate_rules(&lib.rules) {
            continue;
        }

        // LWJGL is supplied by the launcher's Android runtime pack (FCL), not by
        // Mojang. Its Java jars are the desktop build; mixing them with the
        // Android natives is how a launch fails in a hard-to-see way. Any other
        // `:natives-` artifact is a desktop native too (jtracy on 26.x).
        if lib.name.starts_with("org.lwjgl") || lib.name.contains(":natives-") {
            continue;
        }

        if let Some(artifact) = &lib.downloads.artifact {
            if let Some(path) = &artifact.path {
                if let Ok(dest) = crate::safe_path::validate_extract_path(&libraries_dir, path) {
                    push(
                        &mut tasks,
                        &artifact.url,
                        dest,
                        &artifact.sha1,
                        artifact.size,
                    );
                }
            }
        }
    }

    // The asset index is NOT listed here: `asset::plan_objects` fetches it through
    // the global `index/` cache (so it is cached and verified separately from the file downloads).

    // Old manifests list the same library twice (beta 1.7.3 carries `jinput-platform:2.0.5`
    // twice) and loader profiles override a vanilla library by re-listing it. Two tasks pointed
    // at one destination race inside the downloader and can interleave their writes, so keep the
    // last occurrence — that is the one a launcher's merge order intends to win.
    dedupe_by_destination(tasks)
}

/// Drop tasks whose destination was already claimed, keeping the *last* declaration.
fn dedupe_by_destination(tasks: Vec<DownloadTask>) -> Vec<DownloadTask> {
    let mut index_of: HashMap<PathBuf, usize> = HashMap::new();
    let mut deduped: Vec<DownloadTask> = Vec::with_capacity(tasks.len());
    for task in tasks {
        match index_of.get(&task.dest_path) {
            Some(&existing) => deduped[existing] = task,
            None => {
                index_of.insert(task.dest_path.clone(), deduped.len());
                deduped.push(task);
            }
        }
    }
    deduped
}

/// Build download tasks for a set of asset objects into `objects_dir`.
pub fn build_tasks_from_assets(objects: &[AssetObject], objects_dir: &Path) -> Vec<DownloadTask> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(objects.len());
    objects
        .iter()
        // One object can be named many times by an index, and all later stages key off its hash.
        .filter(|object| seen.insert(object.hash.as_str()))
        .filter(|object| is_sha1_hex(&object.hash))
        .map(|object| {
            let prefix = &object.hash[..2];
            DownloadTask {
                url: format!(
                    "https://resources.download.minecraft.net/{prefix}/{}",
                    object.hash
                ),
                dest_path: objects_dir.join(prefix).join(&object.hash),
                sha1: object.hash.clone(),
                size: object.size,
            }
        })
        .collect()
}
