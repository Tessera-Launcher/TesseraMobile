//! The phone front-end's slice of the desktop's `配置.toml`.
//!
//! Only the GitHub proxy is owned here so far, but the file is edited in place:
//! keys the desktop build wrote (heap, window size, theme, …) are preserved
//! instead of being dropped the first time the phone saves. That keeps one
//! `配置.toml` usable from both front-ends.

use std::path::{Path, PathBuf};

/// Same name and location as the desktop build (`<data>/配置.toml`).
pub const FILE: &str = "配置.toml";

/// The schema this build understands. A newer file is refused, not guessed at.
pub const CURRENT_VERSION: u32 = 1;

const PROXY: &str = "github_proxy";
const DOWNLOAD_FILTER: &str = "download_filter";

/// What the phone front-end reads and writes today.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// `gh-proxy.com`-style base prefixed to every GitHub download URL; empty
    /// for none.
    pub github_proxy: String,
    /// `all` / `release` / `snapshot` / `old`, matching the desktop build's key.
    pub download_filter: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            github_proxy: String::new(),
            download_filter: "all".to_string(),
        }
    }
}

impl Settings {
    pub fn path(root: &Path) -> PathBuf {
        root.join(FILE)
    }

    /// Read the file, or the defaults when it is missing.
    ///
    /// A parse error or a newer schema is returned as the second element so the
    /// caller can say what happened instead of silently using other settings
    /// than the file says.
    pub fn load(root: &Path) -> (Self, Option<String>) {
        let path = Self::path(root);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), None)
            }
            Err(error) => {
                return (
                    Self::default(),
                    Some(format!("无法读取 {}: {error}，使用默认设置", path.display())),
                )
            }
        };
        let table: toml::Table = match text.parse() {
            Ok(table) => table,
            Err(error) => {
                return (
                    Self::default(),
                    Some(format!("{} 解析失败: {error}，使用默认设置", path.display())),
                )
            }
        };
        let version = table
            .get("version")
            .and_then(|value| value.as_integer())
            .unwrap_or(CURRENT_VERSION as i64);
        if version > CURRENT_VERSION as i64 {
            return (
                Self::default(),
                Some(format!(
                    "{} 来自更新的版本（schema {version}），已忽略并使用默认设置",
                    path.display()
                )),
            );
        }
        let github_proxy = table
            .get(PROXY)
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let download_filter = table
            .get(DOWNLOAD_FILTER)
            .and_then(|value| value.as_str())
            .filter(|value| matches!(*value, "all" | "release" | "snapshot" | "old"))
            .unwrap_or("all")
            .to_string();
        (
            Self {
                github_proxy,
                download_filter,
            },
            None,
        )
    }

    /// Write the proxy back, preserving every other key in the file.
    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(root)?;
        let path = Self::path(root);
        let mut table: toml::Table = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| text.parse().ok())
            .unwrap_or_default();
        let version = table
            .get("version")
            .and_then(|value| value.as_integer())
            .unwrap_or(0);
        if version < CURRENT_VERSION as i64 {
            table.insert(
                "version".to_string(),
                toml::Value::Integer(CURRENT_VERSION as i64),
            );
        }
        table.insert(
            PROXY.to_string(),
            toml::Value::String(self.github_proxy.clone()),
        );
        table.insert(
            DOWNLOAD_FILTER.to_string(),
            toml::Value::String(self.download_filter.clone()),
        );
        let body = toml::to_string_pretty(&table)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        write_atomic(&path, body.as_bytes())
    }
}

/// Replace `path` with `body` atomically, so a crash cannot truncate it.
fn write_atomic(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "路径没有文件名"))?;
    let temp = path.with_file_name(format!("{}.tmp", file_name.to_string_lossy()));
    let mut file = std::fs::File::create(&temp)?;
    file.write_all(body)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_the_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let (settings, note) = Settings::load(tmp.path());
        assert_eq!(settings, Settings::default());
        assert!(note.is_none());
    }

    #[test]
    fn a_save_load_round_trip_keeps_the_proxy() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = Settings {
            github_proxy: "gh-proxy.com".into(),
            download_filter: "release".into(),
        };
        settings.save(tmp.path()).unwrap();
        assert_eq!(Settings::load(tmp.path()).0, settings);
    }

    #[test]
    fn keys_the_desktop_wrote_survive_a_phone_save() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            Settings::path(tmp.path()),
            "version = 1\nmemory_mb = 4096\ngithub_proxy = \"\"\n",
        )
        .unwrap();

        Settings {
            github_proxy: "https://gh-proxy.com".into(),
            download_filter: "snapshot".into(),
        }
        .save(tmp.path())
        .unwrap();

        let table: toml::Table = std::fs::read_to_string(Settings::path(tmp.path()))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(table["memory_mb"].as_integer(), Some(4096));
        assert_eq!(table[PROXY].as_str(), Some("https://gh-proxy.com"));
        assert_eq!(table[DOWNLOAD_FILTER].as_str(), Some("snapshot"));
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(Settings::path(tmp.path()), "version = 99\ngithub_proxy = \"x\"\n").unwrap();
        let (settings, note) = Settings::load(tmp.path());
        assert_eq!(settings, Settings::default());
        assert!(note.unwrap().contains("更新的版本"));
    }
}
