//! The phone front-end's state: what is on disk and which of it the user picked.
//!
//! Deliberately small. Everything that is not "which screen am I on" is derived
//! from the files, so a list refresh is a re-read rather than a cache update —
//! the account directory is a handful of small JSON files.
//!
//! The chosen account is persisted as a *file name* under the data root, not as
//! a path: the root itself moves (desktop checkout, Android app directory), and
//! a stored absolute path would go stale.

use std::path::{Path, PathBuf};

use crate::accounts;

/// Name of the file remembering the active account, under the data root.
const ACTIVE_FILE: &str = "active_account";

#[derive(Debug)]
pub struct State {
    root: PathBuf,
    pub accounts: Vec<accounts::Account>,
    /// The account the launcher would launch with, as a credential file.
    pub active: Option<PathBuf>,
}

impl State {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let mut state = Self {
            root: root.into(),
            accounts: Vec::new(),
            active: None,
        };
        state.reload();
        state
    }

    /// `<data>/凭据/账户` — one JSON file per account, same layout as the desktop
    /// build so a credential directory can be copied between them.
    pub fn accounts_dir(&self) -> PathBuf {
        self.root.join("凭据").join("账户")
    }

    /// Re-read the account directory and the remembered choice.
    ///
    /// Cheap and idempotent: called every time the account screen opens, because
    /// files can appear or vanish while it is closed. An active account whose
    /// file is gone is forgotten rather than remembered as a dangling path.
    pub fn reload(&mut self) {
        self.accounts = accounts::list(&self.accounts_dir());
        let stored = self.stored_active();
        self.active = stored.as_deref().and_then(|name| self.file_named(name));
        if self.active.is_none() && stored.is_some() {
            // The remembered account is gone: stop pointing at it.
            let _ = std::fs::remove_file(self.active_path());
        }
    }

    /// Whether this row is the one the launcher would use right now.
    pub fn is_active(&self, account: &accounts::Account) -> bool {
        self.active.as_deref() == Some(account.file.as_path())
    }

    /// The active account, if the file is still there.
    pub fn active_account(&self) -> Option<&accounts::Account> {
        let active = self.active.as_deref()?;
        self.accounts.iter().find(|a| a.file == active)
    }

    /// Make the account at `index` the active one and remember it.
    ///
    /// Returns false when the index is out of range, which is what a stale row
    /// from a model the user has already changed looks like.
    pub fn activate(&mut self, index: usize) -> bool {
        let Some(account) = self.accounts.get(index) else {
            return false;
        };
        self.active = Some(account.file.clone());
        self.store_active(account.file.as_path());
        true
    }

    /// Write a new offline account and make it the active one.
    ///
    /// Mirrors the desktop login screen: the list is re-read rather than
    /// appended to, because the numbering of a repeated name comes from disk.
    pub fn add_offline(&mut self, name: &str) -> std::io::Result<usize> {
        let (file, _copy) = accounts::save_offline(&self.accounts_dir(), name)?;
        self.reload();
        let index = self
            .accounts
            .iter()
            .position(|account| account.file == file)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::Other, "新账户没有出现在列表里")
            })?;
        self.activate(index);
        Ok(index)
    }

    /// Delete the account at `index` and forget it.
    ///
    /// A missing index is a stale row, not an error; a file that cannot be
    /// deleted is reported so the caller can keep the row on screen.
    pub fn remove(&mut self, index: usize) -> std::io::Result<()> {
        let account = self.accounts.get(index).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "这个账户已经不在了")
        })?;
        accounts::remove(account)?;
        self.reload();
        Ok(())
    }

    fn file_named(&self, name: &str) -> Option<PathBuf> {
        self.accounts
            .iter()
            .find(|account| account.file.file_name().is_some_and(|f| f == name))
            .map(|account| account.file.clone())
    }

    fn active_path(&self) -> PathBuf {
        self.root.join(ACTIVE_FILE)
    }

    fn stored_active(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.active_path()).ok()?;
        let name = text.trim();
        (!name.is_empty()).then(|| name.to_string())
    }

    fn store_active(&self, file: &Path) {
        let Some(name) = file.file_name() else {
            return;
        };
        let _ = std::fs::create_dir_all(&self.root);
        let _ = std::fs::write(self.active_path(), name.to_string_lossy().as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, State) {
        let tmp = tempfile::tempdir().unwrap();
        let state = State::new(tmp.path());
        (tmp, state)
    }

    #[test]
    fn a_fresh_root_has_no_accounts_and_no_active_one() {
        let (_tmp, state) = setup();
        assert!(state.accounts.is_empty());
        assert!(state.active.is_none());
    }

    #[test]
    fn activating_persists_the_file_name() {
        let (tmp, mut state) = setup();
        accounts::save_offline(&state.accounts_dir(), "Steve").unwrap();
        accounts::save_offline(&state.accounts_dir(), "Alex").unwrap();
        state.reload();

        assert!(state.activate(1));
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(ACTIVE_FILE)).unwrap(),
            "Steve.json"
        );

        // A second State over the same root remembers it.
        let again = State::new(tmp.path());
        assert_eq!(
            again.active_account().map(|a| a.name()),
            Some("Steve".to_string())
        );
    }

    #[test]
    fn an_out_of_range_index_changes_nothing() {
        let (_tmp, mut state) = setup();
        accounts::save_offline(&state.accounts_dir(), "Steve").unwrap();
        state.reload();
        assert!(!state.activate(7));
        assert!(state.active.is_none());
    }

    #[test]
    fn remove_deletes_the_file_and_forgets_the_choice() {
        let (tmp, mut state) = setup();
        accounts::save_offline(&state.accounts_dir(), "Steve").unwrap();
        accounts::save_offline(&state.accounts_dir(), "Alex").unwrap();
        state.reload();
        assert!(state.activate(1));

        state.remove(1).unwrap();
        assert_eq!(state.accounts.len(), 1);
        assert_eq!(state.accounts[0].name(), "Alex");
        assert!(state.active.is_none());
        assert!(!tmp.path().join(ACTIVE_FILE).exists());
    }

    #[test]
    fn adding_an_offline_account_makes_it_active() {
        let (_tmp, mut state) = setup();
        let index = state.add_offline("Steve").unwrap();
        assert_eq!(index, 0);
        assert_eq!(state.accounts.len(), 1);
        assert_eq!(
            state.active_account().map(|a| a.name()),
            Some("Steve".to_string())
        );
        assert!(state.accounts[0].offline);
    }

    #[test]
    fn removing_a_missing_index_is_an_error() {
        let (_tmp, mut state) = setup();
        assert!(state.remove(0).is_err());
    }

    #[test]
    fn a_remembered_account_that_was_deleted_is_forgotten() {
        let (_tmp, mut state) = setup();
        let (path, _) = accounts::save_offline(&state.accounts_dir(), "Steve").unwrap();
        accounts::save_offline(&state.accounts_dir(), "Alex").unwrap();
        state.reload();
        assert!(state.activate(1));

        std::fs::remove_file(path).unwrap();
        state.reload();
        assert!(state.active.is_none());
        assert!(state.active_account().is_none());
    }
}
