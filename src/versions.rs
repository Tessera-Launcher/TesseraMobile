//! The Minecraft version index for the download page.
//!
//! MDT already knows how to fetch and parse the manifest, including the
//! BMCLAPI → Mojang fallback. The Android-specific part is the HTTP client: MDT
//! is handed our webpki-roots client so it never touches
//! `rustls-platform-verifier`.
//!
//! The picker keeps the 26.2+ stable release list, the search state and the
//! rows the UI renders. Snapshots and the old channels are filtered out before
//! they reach the UI.

use mdt::{HttpDownloader, McVersion, MdtClient, Mirror, NoopReporter, VersionType};

use crate::version_tree;

/// One version from the manifest.
#[derive(Debug, Clone)]
pub struct Version {
    pub id: String,
    pub kind: VersionType,
    /// `YYYY-MM-DD` from the manifest's `releaseTime`.
    pub date: String,
}

/// Search over the stable release list.
#[derive(Debug)]
pub struct Picker {
    pub all: Vec<Version>,
    visible: Vec<usize>,
    expanded: Vec<String>,
    query: String,
    rows: Vec<version_tree::Row>,
}

impl Default for Picker {
    fn default() -> Self {
        Self {
            all: Vec::new(),
            visible: Vec::new(),
            expanded: Vec::new(),
            query: String::new(),
            rows: Vec::new(),
        }
    }
}

impl Picker {
    pub fn set_versions(&mut self, versions: Vec<Version>) {
        self.all = versions;
        self.expanded.clear();
        self.rebuild();
    }

    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.expanded.clear();
        self.rebuild();
    }

    pub fn toggle(&mut self, key: &str) {
        if let Some(position) = self.expanded.iter().position(|open| open == key) {
            self.expanded.remove(position);
        } else {
            self.expanded.push(key.to_string());
        }
        self.rebuild();
    }

    pub fn visible(&self) -> &[usize] {
        &self.visible
    }

    pub fn rows(&self) -> &[version_tree::Row] {
        &self.rows
    }

    /// How many versions match the current search.
    pub fn count(&self) -> usize {
        self.visible.len()
    }

    pub fn version(&self, visible_index: usize) -> Option<&Version> {
        self.visible
            .get(visible_index)
            .and_then(|&index| self.all.get(index))
    }

    pub fn expanded(&self) -> &[String] {
        &self.expanded
    }

    fn rebuild(&mut self) {
        let query = self.query.trim().to_lowercase();
        self.visible.clear();
        for (index, version) in self.all.iter().enumerate() {
            if !query.is_empty() && !version.id.to_lowercase().contains(&query) {
                continue;
            }
            self.visible.push(index);
        }
        // There are no snapshot families in this list, so every row is flat.
        self.rows = version_tree::build(&self.all, &self.visible, &self.expanded, false);
    }
}

/// Fetch the index on a worker thread and report once.
pub fn spawn(report: impl Fn(Result<Vec<Version>, String>) + Send + 'static) {
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("无法创建异步运行时: {error}"))
            .and_then(|runtime| runtime.block_on(fetch()));
        report(result);
    });
}

async fn fetch() -> Result<Vec<Version>, String> {
    let root = crate::platform::data_dir();
    let client = crate::http::client()?;
    let mdt = MdtClient::builder()
        .root_dir(&root)
        .list_dir(root.join(".cache").join("列表"))
        .mirror(Mirror::BMCLAPI)
        .downloader(HttpDownloader::new(client))
        .progress_reporter(NoopReporter)
        .build()
        .map_err(|error| format!("无法初始化版本索引：{error}"))?;
    let versions = mdt
        .versions_since(McVersion::new(26, 2, 0))
        .await
        .map_err(|error| format!("无法获取版本列表：{error}"))?;
    Ok(versions
        .into_iter()
        .map(|version| Version {
            id: version.id,
            kind: version.version_type,
            date: version
                .release_time
                .get(..10)
                .unwrap_or(&version.release_time)
                .to_string(),
        })
        .collect())
}

pub fn kind_label(kind: VersionType) -> &'static str {
    match kind {
        VersionType::Release => "正式版",
        VersionType::Snapshot => "快照",
        VersionType::OldBeta => "旧版 Beta",
        VersionType::OldAlpha => "旧版 Alpha",
        VersionType::Other => "其他",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(id: &str, kind: VersionType) -> Version {
        Version {
            id: id.to_string(),
            kind,
            date: String::new(),
        }
    }

    #[test]
    fn kind_labels_are_stable() {
        assert_eq!(kind_label(VersionType::Release), "正式版");
        assert_eq!(kind_label(VersionType::Snapshot), "快照");
        assert_eq!(kind_label(VersionType::OldBeta), "旧版 Beta");
    }

    #[test]
    fn search_keeps_only_matching_release_rows() {
        let mut picker = Picker::default();
        picker.set_versions(vec![
            version("26.3", VersionType::Release),
            version("26.2", VersionType::Release),
        ]);
        picker.set_query("26.3".to_string());
        assert_eq!(picker.rows().len(), 1);
        assert_eq!(picker.count(), 1);
        assert!(picker
            .rows()
            .iter()
            .all(|row| matches!(row.kind, version_tree::RowKind::Version(_))));
    }
}
