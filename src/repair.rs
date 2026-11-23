//! Verify an installed version and repair only the files that are wrong.
//!
//! MDT still owns the metadata: the repair rebuilds the same plan as an install
//! and then checks the files on disk. Normalized/patched jars cannot be checked
//! against Mojang's original SHA-1, so successful installs record the final
//! hashes of every jar in `.tessera-integrity.json`. Non-jar resources (assets,
//! indexes, logging config) keep using the plan's original hashes.
//!
//! An instance installed before that sidecar existed gets a conservative
//! baseline: existing jars are recorded as-is, so the repair does not mass
//! redownload the whole library set just to change their signatures. Missing
//! jars are still repaired, and later repairs can check every jar.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use mdt::{HttpDownloader, MdtClient, Mirror, NoopReporter};
use serde::{Deserialize, Serialize};

use crate::aria2;
use crate::http;
use crate::install;

const STATE_FILE: &str = ".tessera-integrity.json";
const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct IntegrityState {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    files: BTreeMap<String, FileState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileState {
    size: u64,
    sha1: Option<String>,
}

#[derive(Debug)]
pub enum Event {
    Log(String),
    Finished {
        version: String,
        repaired: usize,
        checked: usize,
        seconds: f64,
    },
    Failed(String),
}

pub fn spawn(version_dir: String, report: impl Fn(Event) + Send + 'static) {
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("无法创建异步运行时: {error}"))
            .and_then(|runtime| runtime.block_on(repair(&version_dir, &report)));
        if let Err(problem) = result {
            report(Event::Failed(problem));
        }
    });
}

async fn repair(version_dir: &str, report: &impl Fn(Event)) -> Result<(), String> {
    let started = Instant::now();
    let game = install::game_root();
    let dir = game.join("versions").join(version_dir);
    if !dir.is_dir() {
        return Err(format!("版本目录不存在：{}", dir.display()));
    }
    let (plan, base_version) = plan_for_installed(&dir, version_dir, report).await?;
    report(Event::Log(format!(
        "完整性检查：{} 个文件 · Minecraft {}",
        plan.entries.len(),
        base_version
    )));

    let state_path = dir.join(STATE_FILE);
    let previous = load_state(&state_path);
    let mut state = previous.clone().unwrap_or_default();
    let mut bad = Vec::new();
    let legacy = previous.is_none();

    for entry in &plan.entries {
        let path = game.join(&entry.path);
        if is_jar(&entry.path) {
            if let Some(record) = state.files.get(&key(&entry.path)) {
                if !matches_record(&path, record) {
                    bad.push(entry.clone());
                }
            } else if legacy {
                // No post-install baseline. Trust a jar that is present and
                // record its current hash; a missing jar still needs repair.
                if let Some(record) = current_record(&path) {
                    state.files.insert(key(&entry.path), record);
                } else {
                    bad.push(entry.clone());
                }
            } else {
                // A jar from a new plan revision (e.g. updated Fabric API) has
                // no record yet; download it and then establish the state.
                bad.push(entry.clone());
            }
        } else if !matches_plan(&path, entry) {
            bad.push(entry.clone());
        }
    }

    if legacy {
        report(Event::Log(
            "没有 jar 完整性记录，先跳过并记录现有 jar 的最终哈希".to_string(),
        ));
    }

    for entry in bad.iter().take(12) {
        report(Event::Log(format!("需要修复：{}", entry.path.display())));
    }
    if bad.len() > 12 {
        report(Event::Log(format!("……另有 {} 个错误文件", bad.len() - 12)));
    }
    if bad.is_empty() {
        ensure_client_patch(&dir, version_dir, report)?;
        record_jars(&game, &plan, &mut state)?;
        save_state(&state_path, &state)?;
        report(Event::Finished {
            version: version_dir.to_string(),
            repaired: 0,
            checked: plan.entries.len(),
            seconds: started.elapsed().as_secs_f64(),
        });
        return Ok(());
    }

    let cache = crate::platform::data_dir().join("缓存");
    let repair_plan = mdt::plan::DownloadPlan::new(format!("{}-repair", plan.id), bad.clone());
    let list = repair_plan
        .write(&game, &cache)
        .await
        .map_err(|error| format!("无法写入修复清单：{error}"))?;
    report(Event::Log(format!(
        "修复清单：{} 个文件 · {}",
        bad.len(),
        http::human_size(bad.iter().map(|entry| entry.size).sum())
    )));

    let exit = aria2::run_list(&list, &game, |file| {
        report(Event::Log(file));
    });
    match exit {
        Ok(code) => report(Event::Log(format!("aria2 修复结束（退出码 {code}）"))),
        Err(problem) => report(Event::Log(format!("aria2 未正常结束：{problem}"))),
    }

    // A downloaded jar arrives as Mojang's original signed jar. Apply the same
    // post-download transforms as install before recording its final hash.
    let mut jar_failed = BTreeSet::new();
    for entry in &bad {
        if !is_jar(&entry.path) {
            continue;
        }
        let path = game.join(&entry.path);
        if !path.is_file() {
            jar_failed.insert(key(&entry.path));
            continue;
        }
        let normalized = crate::jar::normalize(&path).and_then(|_| {
            if is_client_jar(version_dir, &entry.path) {
                crate::client_patch::apply(&path)
                    .map(|_| ())
                    .map_err(|error| format!("客户端补丁失败：{error}"))
            } else {
                Ok(())
            }
        });
        if let Err(problem) = normalized {
            report(Event::Log(format!(
                "jar 修复失败：{}：{problem}",
                entry.path.display()
            )));
            jar_failed.insert(key(&entry.path));
        }
    }

    // Check the subset again: non-jars against the plan, jars after the
    // normalize/client-patch pass. A jar that is still absent or failed to
    // normalize is not blessed.
    let mut still_bad = Vec::new();
    for entry in &bad {
        let path = game.join(&entry.path);
        if is_jar(&entry.path) {
            if jar_failed.contains(&key(&entry.path)) || current_record(&path).is_none() {
                still_bad.push(entry.clone());
            }
        } else if !matches_plan(&path, entry) {
            still_bad.push(entry.clone());
        }
    }
    if !still_bad.is_empty() {
        for entry in still_bad.iter().take(12) {
            report(Event::Log(format!("仍然错误：{}", entry.path.display())));
        }
        return Err(format!(
            "仍有 {} 个文件未修复；请再次点「修复游戏…」",
            still_bad.len()
        ));
    }

    ensure_client_patch(&dir, version_dir, report)?;
    record_jars(&game, &plan, &mut state)?;
    save_state(&state_path, &state)?;
    report(Event::Finished {
        version: version_dir.to_string(),
        repaired: bad.len(),
        checked: plan.entries.len(),
        seconds: started.elapsed().as_secs_f64(),
    });
    Ok(())
}

/// The client jar has been normalized and patched, so an integrity pass must
/// re-apply the Vulkan-only edits even when the jar itself was not listed as
/// damaged. This is idempotent for an already patched jar.
fn ensure_client_patch(
    dir: &Path,
    version_dir: &str,
    report: &impl Fn(Event),
) -> Result<(), String> {
    let client = dir.join(format!("{version_dir}.jar"));
    if !client.is_file() {
        return Ok(());
    }
    match crate::client_patch::apply(&client)? {
        crate::client_patch::Outcome::Patched => {
            report(Event::Log("客户端补丁：已重新应用".to_string()));
        }
        _ => {}
    }
    Ok(())
}

/// Rebuild the MDT plan for an already installed version.
async fn plan_for_installed(
    version_dir: &Path,
    dir_name: &str,
    report: &impl Fn(Event),
) -> Result<(mdt::plan::DownloadPlan, String), String> {
    let json_path = version_dir.join(format!("{dir_name}.json"));
    let text = std::fs::read_to_string(&json_path)
        .map_err(|error| format!("读取 {} 失败：{error}", json_path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("版本 JSON 错误：{error}"))?;
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(dir_name);
    let base = id
        .split_once("-fabric-")
        .map(|(base, _)| base)
        .unwrap_or(id)
        .to_string();

    let loader = value.get("mdtLoader");
    let fabric_version = loader
        .and_then(|loader| loader.get("id"))
        .and_then(serde_json::Value::as_str)
        .filter(|id| *id == "fabric")
        .map(|_| {
            loader
                .and_then(|loader| loader.get("version"))
                .and_then(serde_json::Value::as_str)
        })
        .flatten();
    if loader.is_some() && fabric_version.is_none() {
        return Err("这个版本带有未知的 Mod Loader，无法重建修复清单".to_string());
    }

    let root = install::game_root();
    let client = http::client()?;
    let mdt = MdtClient::builder()
        .root_dir(&root)
        .list_dir(crate::platform::data_dir().join("缓存"))
        .mirror(Mirror::Mojang)
        .downloader(HttpDownloader::new(client))
        .progress_reporter(NoopReporter)
        .build()
        .map_err(|error| format!("无法初始化下载器：{error}"))?;

    let plan = if let Some(loader_version) = fabric_version {
        report(Event::Log(format!(
            "修复：解析 Fabric Loader {loader_version}"
        )));
        mdt.plan_loader(&base, mdt::ModLoader::Fabric, loader_version)
            .await
            .map_err(|error| format!("无法解析版本 {base}：{error}"))?
    } else {
        mdt.plan_vanilla(&base)
            .await
            .map_err(|error| format!("无法解析版本 {base}：{error}"))?
    };

    // Repair only the game tree. Modrinth mods are deliberately left out:
    // resolving "latest" during a repair could add a second TouchController
    // version beside the installed one. Mod jars are not part of a resource
    // integrity pass.
    Ok((plan, base))
}

fn is_jar(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"))
}

fn is_client_jar(dir_name: &str, path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(&format!("{dir_name}.jar")))
}

fn key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn current_record(path: &Path) -> Option<FileState> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() == 0 {
        return None;
    }
    Some(FileState {
        size: metadata.len(),
        sha1: mdt::asset::sha1_of_file(path),
    })
}

fn matches_record(path: &Path, record: &FileState) -> bool {
    let Some(actual) = current_record(path) else {
        return false;
    };
    actual.size == record.size
        && record
            .sha1
            .as_deref()
            .is_none_or(|expected| actual.sha1.as_deref() == Some(expected))
}

fn matches_plan(path: &Path, entry: &mdt::plan::PlanEntry) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !entry.size_match(metadata.len()) {
        return false;
    }
    entry
        .sha1
        .as_deref()
        .is_none_or(|expected| mdt::asset::sha1_of_file(path).as_deref() == Some(expected))
}

fn record_jars(
    game: &Path,
    plan: &mdt::plan::DownloadPlan,
    state: &mut IntegrityState,
) -> Result<(), String> {
    state.version = STATE_VERSION;
    for entry in &plan.entries {
        if !is_jar(&entry.path) {
            continue;
        }
        if let Some(record) = current_record(&game.join(&entry.path)) {
            state.files.insert(key(&entry.path), record);
        }
    }
    Ok(())
}

fn load_state(path: &Path) -> Option<IntegrityState> {
    let text = std::fs::read_to_string(path).ok()?;
    let state: IntegrityState = serde_json::from_str(&text).ok()?;
    (state.version == STATE_VERSION).then_some(state)
}

fn save_state(path: &Path, state: &IntegrityState) -> Result<(), String> {
    let body = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("无法序列化完整性状态：{error}"))?;
    std::fs::write(path, body).map_err(|error| format!("无法写入 {}: {error}", path.display()))
}

/// Public because install calls this after normalization and client patching.
pub fn record_install_jars(
    game: &Path,
    version_dir: &Path,
    plan: &mdt::plan::DownloadPlan,
) -> Result<(), String> {
    let mut state = load_state(&version_dir.join(STATE_FILE)).unwrap_or_default();
    record_jars(game, plan, &mut state)?;
    save_state(&version_dir.join(STATE_FILE), &state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_and_record_predicates_are_safe() {
        assert!(is_jar(Path::new("a/b.jar")));
        assert!(!is_jar(Path::new("a/b.json")));
        assert!(is_client_jar(
            "26.3-fabric-0.20.1",
            Path::new("versions/x/26.3-fabric-0.20.1.jar")
        ));
        assert_eq!(key(Path::new("versions/x/a.jar")), "versions/x/a.jar");
    }
}
