//! The phone front-end's state: what is on disk and which of it the user picked.
//!
//! Deliberately small. Everything that is not "which screen am I on" is derived
//! from the files, so a list refresh is a re-read rather than a cache update —
//! the account directory is a handful of small JSON files.

use std::path::PathBuf;

use crate::{accounts, platform};

#[derive(Debug, Default)]
pub struct State {
    pub accounts: Vec<accounts::Account>,
    /// The account the launcher would launch with, as a credential file.
    pub active: Option<PathBuf>,
}

impl State {
    /// Re-read the account directory.
    ///
    /// Cheap and idempotent: called every time the account screen opens, because
    /// files can appear or vanish while it is closed.
    pub fn reload(&mut self) {
        self.accounts = accounts::list(&platform::accounts_dir());
    }

    /// Whether this row is the one the launcher would use right now.
    pub fn is_active(&self, account: &accounts::Account) -> bool {
        self.active.as_deref() == Some(account.file.as_path())
    }
}
