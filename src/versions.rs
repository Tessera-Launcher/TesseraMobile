//! The Minecraft version index for the download page.
//!
//! MDT already knows how to fetch and parse the manifest, including the
//! BMCLAPI → Mojang fallback. The Android-specific part is the HTTP client: MDT
//! is handed our webpki-roots client so it never touches
//! `rustls-platform-verifier`.
//!
//! The picker keeps the full list, the search/filter state and the folded tree
//! rows. The UI only ever renders the rows this module produces.

use mdt::{HttpDownloader, MdtClient, Mirror, NoopReporter, VersionType};

use crate::version_tree;

/// One version from the manifest.
#[derive(Debug, Clone)]
pub struct Version {
    pub id: String,
    pub kind: VersionType,
    /// `YYYY-MM-DD` from the manifest's `releaseTime`.
    pub date: String,
}

/// The kind-of-release filter above the list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Release,
    Snapshot,
    Old,
}

impl Filter {
    pub fn key(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Release => "release",
            Filter::Snapshot => "snapshot",
            Filter::Old => "old",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "release" => Filter::Release,
            "snapshot" => Filter::Snapshot,
            "old" => Filter::Old,
            _ => Filter::All,
        }
    }

    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Filter::Release,
            2 => Filter::Snapshot,
            3 => Filter::Old,
            _ => Filter::All,
        }
    }

    fn matches(self, kind: VersionType) -> bool {
        match self {
            Filter::All => true,
            Filter::Release => kind == VersionType::Release,
            Filter::Snapshot => kind == VersionType::Snapshot,
            Filter::Old => matches!(kind, VersionType::OldBeta | VersionType::OldAlpha),
        }
    }
}

/// Search + filter + folded rows.
#[derive(Debug)]
pub struct Picker {
    pub all: Vec<Version>,
    visible: Vec<usize>,
    expanded: Vec<String>,
    query: String,
    filter: Filter,
    rows: Vec<version_tree::Row>,
}

impl Default for Picker {
    fn default() -> Self {
        Self {
            all: Vec::new(),
            visible: Vec::new(),
            expanded: Vec::new(),
            query: String::new(),
            filter: Filter::All,
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

    pub fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
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

    /// How many versions match the current search/filter.
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
            if !self.filter.matches(version.kind) {
                continue;
            }
            if !query.is_empty() && !version.id.to_lowercase().contains(&query) {
                continue;
            }
            self.visible.push(index);
        }
        let tree = self.filter == Filter::All && query.is_empty();
        self.rows = version_tree::build(&self.all, &self.visible, &self.expanded, tree);
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
        .versions()
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
    fn a_search_flattens_the_tree() {
        let mut picker = Picker::default();
        picker.set_versions(vec![
            version("1.21.9-pre2", VersionType::Snapshot),
            version("1.21.9-pre1", VersionType::Snapshot),
            version("1.21.8", VersionType::Release),
        ]);
        picker.set_query("pre".to_string());
        assert_eq!(picker.rows().len(), 2);
        assert!(picker
            .rows()
            .iter()
            .all(|row| matches!(row.kind, version_tree::RowKind::Version(_))));
    }

    #[test]
    fn the_release_filter_hides_snapshots() {
        let mut picker = Picker::default();
        picker.set_versions(vec![
            version("1.21.9-pre1", VersionType::Snapshot),
            version("1.21.8", VersionType::Release),
        ]);
        picker.set_filter(Filter::Release);
        assert_eq!(picker.count(), 1);
        assert_eq!(picker.rows().len(), 1);
    }

    #[test]
    fn toggling_a_group_expands_and_collapses_it() {
        let mut picker = Picker::default();
        picker.set_versions(vec![
            version("1.21.9-pre2", VersionType::Snapshot),
            version("1.21.9-pre1", VersionType::Snapshot),
        ]);
        assert_eq!(picker.rows().len(), 1);
        picker.toggle("1.21.9-pre");
        assert_eq!(picker.rows().len(), 3);
        picker.toggle("1.21.9-pre");
        assert_eq!(picker.rows().len(), 1);
    }
}
