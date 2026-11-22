//! The Minecraft version index for the download page.
//!
//! MDT already knows how to fetch and parse the manifest, including the
//! BMCLAPI → Mojang fallback. The Android-specific part is the HTTP client: MDT
//! is handed our webpki-roots client so it never touches
//! `rustls-platform-verifier`.

use mdt::{HttpDownloader, MdtClient, Mirror, NoopReporter, VersionType};

/// One row shown on the download page.
#[derive(Debug, Clone)]
pub struct Version {
    pub id: String,
    pub kind: String,
    /// `YYYY-MM-DD` from the manifest's `releaseTime`.
    pub date: String,
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
            kind: kind_label(version.version_type),
            date: version
                .release_time
                .get(..10)
                .unwrap_or(&version.release_time)
                .to_string(),
        })
        .collect())
}

fn kind_label(kind: VersionType) -> String {
    match kind {
        VersionType::Release => "正式版",
        VersionType::Snapshot => "快照",
        VersionType::OldBeta => "旧版 Beta",
        VersionType::OldAlpha => "旧版 Alpha",
        VersionType::Other => "其他",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_kinds_have_labels() {
        assert_eq!(kind_label(VersionType::Release), "正式版");
        assert_eq!(kind_label(VersionType::Snapshot), "快照");
        assert_eq!(kind_label(VersionType::OldBeta), "旧版 Beta");
    }
}
