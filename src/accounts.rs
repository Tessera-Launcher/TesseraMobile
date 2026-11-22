//! One JSON file per account, under `<data>/凭据/账户/`.
//!
//! Ported from `tessera-core::accounts`. The desktop module also writes real
//! sessions (that is the login flow's job); this front-end only reads the files
//! and writes offline identities, so the format stays the same and the same
//! directory can be shared with a desktop checkout.
//!
//! ```json
//! {
//!   "format": 1,
//!   "offline": true,
//!   "application": "Offline",
//!   "app_id": "",
//!   "copy": 0,
//!   "player": { "name": "Steve", "uuid": "…" },
//!   "saved_at": 1762450000
//! }
//! ```

use std::path::{Path, PathBuf};

/// Current on-disk format. Shared with the desktop build.
pub const FORMAT: u32 = 1;

/// A saved account, as listed in the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub file: PathBuf,
    pub player_name: String,
    /// Which numbered session of that player this is: `0` for the first one,
    /// `1` for the second. The same player may be signed in any number of
    /// times, and each session keeps its own tokens, so the number is the only
    /// thing telling them apart.
    pub copy: u32,
    /// Which launcher identity issued the session, or `Offline`.
    pub application: String,
    /// An offline account: a stored name and its offline UUID, no tokens.
    pub offline: bool,
}

/// The `(1)`-style suffix a repeated session carries, empty for the first one.
pub fn suffix(copy: u32) -> String {
    if copy == 0 {
        String::new()
    } else {
        format!(" ({copy})")
    }
}

impl Account {
    /// The player name with its number, i.e. what the user calls this session.
    pub fn name(&self) -> String {
        format!("{}{}", self.player_name, suffix(self.copy))
    }

    /// One line for a list row: `Steve (1) · Offline`.
    pub fn label(&self) -> String {
        format!("{} · {}", self.name(), self.application)
    }
}

/// List every readable account file, sorted by player name.
///
/// A file that does not parse is skipped, not fatal: one bad credential must not
/// hide the others.
pub fn list(dir: &Path) -> Vec<Account> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Account> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let path = entry.path();
            let text = std::fs::read_to_string(&path).ok()?;
            let doc: serde_json::Value = serde_json::from_str(&text).ok()?;
            let player = doc.get("player")?;
            Some(Account {
                file: path,
                player_name: player.get("name")?.as_str()?.to_string(),
                copy: doc
                    .get("copy")
                    .and_then(|copy| copy.as_u64())
                    .unwrap_or(0)
                    .try_into()
                    .unwrap_or(u32::MAX),
                application: doc
                    .get("application")
                    .and_then(|v| v.as_str())
                    .unwrap_or("未知身份")
                    .to_string(),
                // Written by `save_offline`; absent from every session file.
                offline: doc
                    .get("offline")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect();
    // Sessions of the same player stay together, in their numbered order.
    found.sort_by(|a, b| {
        a.player_name
            .cmp(&b.player_name)
            .then_with(|| a.copy.cmp(&b.copy))
    });
    found
}

/// Write one signed-in account file, numbered if that player already has one.
///
/// Signing in the same player again is never a replacement: the second session
/// gets `Steve (1).json`, the third `Steve (2).json`, so two identities (or two
/// accounts that happen to share a name) keep their own refresh tokens. Existing
/// files are never touched.
///
/// `session` is `JavaAuthManager::to_json()` verbatim, so restoring it later is
/// a matter of handing the same document back to the crate.
pub fn save(
    dir: &Path,
    application: &str,
    app_id: &str,
    player_name: &str,
    player_uuid: &str,
    session: &serde_json::Value,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    restrict_dir(dir);
    let (path, copy) = free_slot(dir, &safe_file_stem(player_name));

    let document = serde_json::json!({
        "format": FORMAT,
        "application": application,
        "app_id": app_id,
        "copy": copy,
        "player": { "name": player_name, "uuid": player_uuid },
        "saved_at": unix_now(),
        "session": session,
    });
    let body = serde_json::to_vec_pretty(&document)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;

    write_private(&path, &body)?;
    Ok(path)
}

/// Write an offline account: a name and its offline UUID, no session.
///
/// Numbered like a real session — signing the same name in twice gives
/// `Steve.json` and `Steve (1).json` — so the two kinds share one list.
pub fn save_offline(dir: &Path, player_name: &str) -> std::io::Result<(PathBuf, u32)> {
    std::fs::create_dir_all(dir)?;
    restrict_dir(dir);
    let (path, copy) = free_slot(dir, &safe_file_stem(player_name));

    let document = serde_json::json!({
        "format": FORMAT,
        "offline": true,
        "application": "Offline",
        "app_id": "",
        "copy": copy,
        "player": {
            "name": player_name,
            "uuid": offline_uuid(player_name),
        },
        "saved_at": unix_now(),
    });
    let body = serde_json::to_vec_pretty(&document)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;

    write_private(&path, &body)?;
    Ok((path, copy))
}

/// Why this name cannot be an offline identity, or `None` when it can.
///
/// Same rules as the desktop build: the vanilla client accepts 3–16 characters
/// of `[A-Za-z0-9_]`, and a server compares the name as typed. `allow_any` is
/// the escape hatch for Chinese names and the like, which only single player
/// and offline servers will accept.
pub fn offline_name_problem(name: &str, allow_any: bool) -> Option<String> {
    if name.is_empty() {
        return Some("请输入用户名".to_string());
    }
    if allow_any {
        return None;
    }
    let length = name.chars().count();
    if !(3..=16).contains(&length) {
        return Some("原版用户名是 3–16 个字符（勾选后可输入其他名字）".to_string());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Some("原版用户名只能用字母、数字和下划线（勾选后可输入其他名字）".to_string());
    }
    None
}

/// Delete one account file.
pub fn remove(account: &Account) -> std::io::Result<()> {
    std::fs::remove_file(&account.file)
}

/// The UUID a server hands an unauthenticated player with this name.
///
/// This is Java's `UUID.nameUUIDFromBytes(("OfflinePlayer:" + name).getBytes())`,
/// i.e. RFC 4122 version 3 — **MD5**, not SHA-1. Getting it wrong does not break
/// single player, but the player is then a different person to every server.
pub fn offline_uuid(name: &str) -> String {
    use md5::Digest;
    let mut hasher = md5::Md5::new();
    hasher.update(format!("OfflinePlayer:{name}").as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Replace `path` with `body` atomically, readable only by its owner.
fn write_private(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "路径没有文件名"))?;
    let temp = path.with_file_name(format!("{}.tmp", file_name.to_string_lossy()));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(&temp)?;
    file.write_all(body)?;
    // A rename is only as durable as the data it publishes; without this a
    // power loss can leave the new name pointing at unwritten blocks.
    file.sync_all()?;
    drop(file);

    std::fs::rename(&temp, path)
}

/// Keep a credential directory closed to everyone but its owner (Unix only).
fn restrict_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// The first free `stem.json` / `stem (N).json` in `dir`, with its number.
fn free_slot(dir: &Path, stem: &str) -> (PathBuf, u32) {
    for copy in 0..=u32::MAX {
        let name = if copy == 0 {
            format!("{stem}.json")
        } else {
            format!("{stem} ({copy}).json")
        };
        let path = dir.join(name);
        if !path.exists() {
            return (path, copy);
        }
    }
    // Unreachable in practice: 4 billion sessions of one player.
    (dir.join(format!("{stem}.json")), u32::MAX)
}

/// Keep letters, digits, `-` and `_`; replace everything else so a player name
/// can never escape the account directory.
fn safe_file_stem(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "account".to_string()
    } else {
        cleaned
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_offline_account_is_a_file_without_a_session() {
        let tmp = tempfile::tempdir().unwrap();
        let (path, copy) = save_offline(tmp.path(), "Steve").unwrap();
        assert_eq!(copy, 0);
        assert_eq!(path.file_name().unwrap(), "Steve.json");

        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["format"], FORMAT);
        assert_eq!(doc["offline"], true);
        assert_eq!(doc["application"], "Offline");
        assert_eq!(doc["player"]["name"], "Steve");
        assert_eq!(doc["player"]["uuid"], offline_uuid("Steve"));
        assert!(doc.get("session").is_none());
    }

    #[test]
    fn the_same_name_twice_is_two_files() {
        let tmp = tempfile::tempdir().unwrap();
        let (first, _) = save_offline(tmp.path(), "Steve").unwrap();
        let (second, copy) = save_offline(tmp.path(), "Steve").unwrap();
        assert_eq!(copy, 1);
        assert_ne!(first, second);
        assert_eq!(second.file_name().unwrap(), "Steve (1).json");
        assert_eq!(list(tmp.path()).len(), 2);
    }

    #[test]
    fn list_sorts_sessions_of_one_player_by_number() {
        let tmp = tempfile::tempdir().unwrap();
        save_offline(tmp.path(), "Alex").unwrap();
        save_offline(tmp.path(), "Steve").unwrap();
        save_offline(tmp.path(), "Steve").unwrap();
        let names: Vec<String> = list(tmp.path()).iter().map(|a| a.name()).collect();
        assert_eq!(names, ["Alex", "Steve", "Steve (1)"]);
    }

    #[test]
    fn a_broken_file_is_skipped_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        save_offline(tmp.path(), "Steve").unwrap();
        std::fs::write(tmp.path().join("broken.json"), "{").unwrap();
        std::fs::write(tmp.path().join("not-an-account.txt"), "hello").unwrap();
        assert_eq!(list(tmp.path()).len(), 1);
    }

    #[test]
    fn a_missing_directory_lists_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(list(&tmp.path().join("nope")).is_empty());
    }

    #[test]
    fn remove_deletes_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (path, _) = save_offline(tmp.path(), "Steve").unwrap();
        let account = list(tmp.path()).remove(0);
        remove(&account).unwrap();
        assert!(!path.exists());
        assert!(list(tmp.path()).is_empty());
    }

    #[test]
    fn a_player_name_cannot_escape_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let (path, _) = save_offline(tmp.path(), "../evil/name").unwrap();
        assert_eq!(path.parent().unwrap(), tmp.path());
    }

    #[test]
    fn vanilla_names_are_three_to_sixteen_of_the_allowed_characters() {
        assert!(offline_name_problem("Steve", false).is_none());
        assert!(offline_name_problem("a_1", false).is_none());
        assert!(offline_name_problem("", false).is_some());
        assert!(offline_name_problem("ab", false).is_some());
        assert!(offline_name_problem(&"a".repeat(17), false).is_some());
        assert!(offline_name_problem("史蒂夫", false).is_some());
        assert!(offline_name_problem("has space", false).is_some());
    }

    #[test]
    fn any_name_is_accepted_when_the_box_is_ticked() {
        assert!(offline_name_problem("史蒂夫", true).is_none());
        assert!(offline_name_problem("a", true).is_none());
        assert!(offline_name_problem("", true).is_some());
    }

    #[test]
    fn a_session_is_one_file_per_sign_in() {
        let tmp = tempfile::tempdir().unwrap();
        let session = serde_json::json!({ "_save_version": 1 });
        let first = save(
            tmp.path(),
            "官方启动器",
            "00000000402b5328",
            "Steve",
            "00000000-0000-0000-0000-000000000000",
            &session,
        )
        .unwrap();
        assert_eq!(first.file_name().unwrap(), "Steve.json");

        let second = save(
            tmp.path(),
            "官方启动器",
            "00000000402b5328",
            "Steve",
            "00000000-0000-0000-0000-000000000000",
            &session,
        )
        .unwrap();
        assert_eq!(second.file_name().unwrap(), "Steve (1).json");

        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&first).unwrap()).unwrap();
        assert_eq!(doc["format"], FORMAT);
        assert_eq!(doc["application"], "官方启动器");
        assert_eq!(doc["app_id"], "00000000402b5328");
        assert_eq!(doc["session"]["_save_version"], 1);
        assert!(doc.get("offline").is_none());

        // Both are listed, neither is an offline identity.
        let listed = list(tmp.path());
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().all(|account| !account.offline));
        assert_eq!(listed[0].label(), "Steve · 官方启动器");
    }

    #[test]
    fn the_offline_uuid_is_the_one_the_game_computes() {
        // The value the game gives an unauthenticated "Steve".
        assert_eq!(
            offline_uuid("Steve"),
            "5627dd98-e6be-3c21-b8a8-e92344183641"
        );
        // Version 3 (name based, MD5) and the RFC 4122 variant bits.
        assert!(offline_uuid("Steve").starts_with("5627dd98-e6be-3c21-b8a8"));
    }

    #[cfg(unix)]
    #[test]
    fn a_credential_file_is_private_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let (path, _) = save_offline(tmp.path(), "Steve").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the file must not be group/world readable");
        let dir_mode = std::fs::metadata(tmp.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "the account directory is private too");
    }
}
