//! The one place that knows where this launcher keeps its files.
//!
//! The desktop build is portable: everything lives in `data/` next to the
//! binary. A phone has no writable directory next to the executable, so Android
//! hands us one (`Context.getFilesDir()`) and [`set_data_dir`] stashes it before
//! the first frame. Everything downstream only ever asks for `data_dir()`.

use std::path::PathBuf;
use std::sync::OnceLock;

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Pin the data root. Called once at start-up; later calls are ignored.
pub fn set_data_dir(path: PathBuf) {
    let _ = DATA_DIR.set(path);
}

/// The launcher's data root.
///
/// Falls back to `./data` so `cargo run` on a desktop behaves like the desktop
/// build instead of writing into the source tree's parent.
pub fn data_dir() -> PathBuf {
    DATA_DIR.get().cloned().unwrap_or_else(|| {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("data")
    })
}

/// `<data>/凭据/账户` — one JSON file per account, same layout as the desktop
/// build so a credential directory can be copied between them.
pub fn accounts_dir() -> PathBuf {
    data_dir().join("凭据").join("账户")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_live_under_the_data_root() {
        // `set_data_dir` is process-wide and already taken by another test, so
        // only the derived path is checked here.
        assert!(accounts_dir().ends_with("凭据/账户"));
    }
}
