//! The launcher app ids a user may sign in with, read from
//! `<data>/凭据/启动器/azure_app_ids.toml`.
//!
//! Each entry is an Azure application's *public* client id: not a secret, and
//! only useful for starting a device-code flow, which is why they travel in a
//! plain text file.
//!
//! ```toml
//! [[api_keys]]
//! name = "HMCL"
//! key = "6a3728d6-27a3-4180-99bb-479895b8f88e"
//! ```
//!
//! Ported from `tessera-core::app_ids`. One difference: a *missing* file is an
//! empty list rather than an error. On a phone the file usually is not there and
//! the official launcher identity is all anyone needs, so complaining about it
//! would be noise; a file that exists but does not parse still says so.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// One launcher identity: a label for the UI and the Azure client id to use.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AppId {
    pub name: String,
    pub key: String,
}

impl AppId {
    /// The id as it goes to Microsoft, trimmed of the whitespace a copy-paste
    /// tends to leave behind.
    pub fn client_id(&self) -> &str {
        self.key.trim()
    }

    /// A name worth showing even when the file left it out.
    pub fn label(&self) -> &str {
        let name = self.name.trim();
        if name.is_empty() {
            self.client_id()
        } else {
            name
        }
    }
}

/// The whole document. Later sections are ignored so the file can grow.
#[derive(Debug, Deserialize)]
struct Document {
    #[serde(default)]
    api_keys: Vec<AppId>,
}

/// `data/凭据/启动器/azure_app_ids.toml`, next to the accounts.
pub fn path_under(credentials: &Path) -> PathBuf {
    credentials.join("启动器").join("azure_app_ids.toml")
}

/// Parse the file body. Entries without a usable id are dropped rather than
/// offered, since choosing one could only end in a failed sign-in.
pub fn parse(text: &str) -> Result<Vec<AppId>, String> {
    let document: Document = toml::from_str(text).map_err(|error| error.to_string())?;

    let mut ids: Vec<AppId> = Vec::with_capacity(document.api_keys.len());
    for entry in document.api_keys {
        if entry.client_id().is_empty() {
            continue;
        }
        // A duplicated id would only add an identical row.
        if ids.iter().any(|seen| seen.client_id() == entry.client_id()) {
            continue;
        }
        ids.push(entry);
    }
    Ok(ids)
}

/// Read and parse the file. Missing is empty; unreadable or malformed is a
/// message the screen can show.
pub fn load(path: &Path) -> Result<Vec<AppId>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("无法读取 {}: {error}", path.display())),
    };
    parse(&text).map_err(|error| format!("{} 解析失败: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_documented_shape() {
        let ids = parse(
            r#"
[[api_keys]]
name = "HMCL"
key = "6a3728d6-27a3-4180-99bb-479895b8f88e"
"#,
        )
        .unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].label(), "HMCL");
        assert_eq!(ids[0].client_id(), "6a3728d6-27a3-4180-99bb-479895b8f88e");
    }

    #[test]
    fn keeps_file_order_and_skips_unusable_entries() {
        let ids = parse(
            r#"
[[api_keys]]
name = "first"
key = "aaaaaaaa-0000-0000-0000-000000000000"

[[api_keys]]
name = "missing key"
key = "   "

[[api_keys]]
name = "first again"
key = "aaaaaaaa-0000-0000-0000-000000000000"

[[api_keys]]
name = "  "
key = " bbbbbbbb-0000-0000-0000-000000000000 "
"#,
        )
        .unwrap();
        assert_eq!(ids.len(), 2, "duplicates and blank keys are dropped");
        assert_eq!(ids[0].label(), "first");
        // A nameless entry shows its id.
        assert_eq!(ids[1].label(), "bbbbbbbb-0000-0000-0000-000000000000");
    }

    #[test]
    fn a_missing_file_is_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(&dir.path().join("nope.toml")).unwrap().is_empty());
    }

    #[test]
    fn a_malformed_file_names_itself() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("azure_app_ids.toml");
        std::fs::write(&path, "api_keys = 1\n").unwrap();
        let error = load(&path).unwrap_err();
        assert!(error.contains("azure_app_ids.toml"), "{error}");
        assert!(error.contains("解析失败"), "{error}");
    }

    #[test]
    fn the_list_sits_beside_the_accounts() {
        assert_eq!(
            path_under(Path::new("/data/凭据")),
            Path::new("/data/凭据/启动器/azure_app_ids.toml")
        );
    }
}
