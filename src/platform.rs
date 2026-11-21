//! The launcher's data root.
//!
//! The desktop build is portable: everything lives in `data/` next to the
//! binary. A phone has no writable directory next to the executable, so Android
//! hands us one (`Context.getFilesDir()`) and [`set_data_dir`] stashes it before
//! the first frame.

use std::path::PathBuf;
use std::sync::OnceLock;

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Pin the data root. Called once at start-up; later calls are ignored.
pub fn set_data_dir(path: PathBuf) {
    let _ = DATA_DIR.set(path);
}

/// The launcher's data root.
///
/// `TESSERA_DATA` overrides it (tests, checkouts); otherwise `./data`, so
/// `cargo run` on a desktop behaves like the desktop build.
pub fn data_dir() -> PathBuf {
    if let Some(root) = DATA_DIR.get() {
        return root.clone();
    }
    if let Some(root) = std::env::var_os("TESSERA_DATA") {
        return PathBuf::from(root);
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("data")
}
