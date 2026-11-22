//! What is already on disk under `游戏/versions`.
//!
//! The home page only needs a snapshot of the directory: the version id, the
//! channel from its JSON and the release date. No launcher state is inferred
//! here; a directory with a readable version JSON is what "installed" means
//! until launch writes its own marker.

use std::path::Path;

/// One row for the home page.
#[derive(Debug, Clone)]
pub struct Installed {
    pub id: String,
    /// Directory name under `versions/`; what uninstall removes.
    pub dir: String,
    pub kind: String,
    pub date: String,
}

impl Installed {
    /// `正式版 · 2026-09-15`.
    pub fn detail(&self) -> String {
        let mut parts = vec![self.kind.clone()];
        if !self.date.is_empty() {
            parts.push(self.date.clone());
        }
        parts.join(" · ")
    }
}

/// Scan `<game>/versions` and return newest first.
pub fn scan(game: &Path) -> Vec<Installed> {
    let versions = game.join("versions");
    let Ok(entries) = std::fs::read_dir(&versions) else {
        return Vec::new();
    };
    let mut found: Vec<(String, Installed)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(dir_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let json = version_json(&path, dir_name);
        let value = json
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
        let Some(value) = value else {
            continue;
        };
        let id = value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(dir_name)
            .to_string();
        let kind = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .map(kind_label)
            .unwrap_or_else(|| "未知".to_string());
        let date = value
            .get("releaseTime")
            .or_else(|| value.get("time"))
            .and_then(serde_json::Value::as_str)
            .map(short_date)
            .unwrap_or_default();
        let sort = value
            .get("releaseTime")
            .or_else(|| value.get("time"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        found.push((
            sort,
            Installed {
                id,
                dir: dir_name.to_string(),
                kind,
                date,
            },
        ));
    }
    found.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.id.cmp(&left.1.id))
    });
    found.into_iter().map(|(_, row)| row).collect()
}

fn version_json(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    let preferred = dir.join(format!("{name}.json"));
    if preferred.is_file() {
        return Some(preferred);
    }
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "json"))
}

fn kind_label(kind: &str) -> String {
    match kind {
        "release" => "正式版",
        "snapshot" => "快照",
        "old_beta" => "旧版 Beta",
        "old_alpha" => "旧版 Alpha",
        other => other,
    }
    .to_string()
}

fn short_date(value: &str) -> String {
    value.get(..10).unwrap_or(value).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_labels_follow_the_manifest() {
        assert_eq!(kind_label("release"), "正式版");
        assert_eq!(kind_label("old_beta"), "旧版 Beta");
    }

    #[test]
    fn a_date_is_only_the_day() {
        assert_eq!(short_date("2026-09-15T10:11:12+00:00"), "2026-09-15");
    }

    #[test]
    fn scan_reads_version_json_newest_first_and_skips_empty_directories() {
        let root = tempfile::tempdir().unwrap();
        for (id, kind, date) in [
            ("1.20.1", "release", "2023-06-12T12:00:00+00:00"),
            ("26.3", "release", "2026-09-15T12:00:00+00:00"),
        ] {
            let dir = root.path().join("versions").join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(format!("{id}.json")),
                format!(r#"{{"id":"{id}","type":"{kind}","releaseTime":"{date}"}}"#),
            )
            .unwrap();
        }
        std::fs::create_dir_all(root.path().join("versions/empty")).unwrap();

        let rows = scan(root.path());
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["26.3", "1.20.1"]
        );
        assert_eq!(rows[0].detail(), "正式版 · 2026-09-15");
    }
}
