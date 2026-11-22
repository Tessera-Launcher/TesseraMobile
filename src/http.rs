//! One HTTP policy for every request the launcher makes, plus the GitHub proxy.
//!
//! Ported from `tessera-core::http`: requests verify against a bundled Mozilla
//! root store, and GitHub release assets can be fetched through the user's
//! gh-proxy address (the same one the desktop build uses).
//!
//! Why a bundled store rather than the platform verifier: reqwest's `rustls`
//! feature verifies through `rustls-platform-verifier`, which panics on Android
//! until it is handed a JNI context ("Expect rustls-platform-verifier to be
//! initialized"). Supplying the config here keeps every request free of JNI and
//! gives the same trust decisions on a phone and on a desktop, at the cost of
//! ignoring CAs the user installed.

use std::sync::Arc;
use std::time::Duration;

/// Names the project to servers (Modrinth rejects requests without one).
const USER_AGENT: &str = concat!(
    "tessera-launcher/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/Tessera-Launcher/Tessera)"
);

/// A client that trusts a bundled root store.
pub fn client() -> Result<reqwest::Client, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("TLS 配置失败: {error}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();

    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        // A release asset is a few megabytes; one stalled connection should not
        // hold the install forever.
        .connect_timeout(Duration::from_secs(15))
        .use_preconfigured_tls(config)
        .build()
        .map_err(|error| format!("无法创建 HTTP 客户端: {error}"))
}

/// Keep log lines readable: the host and the file name, not the whole URL.
pub fn shorten(url: &str) -> String {
    let without_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let (host, path) = without_scheme
        .split_once('/')
        .unwrap_or((without_scheme, ""));
    let file = path.rsplit('/').next().unwrap_or("");
    if file.is_empty() {
        host.to_string()
    } else {
        format!("{host}/…/{file}")
    }
}

/// Normalise what the user typed into a proxy base.
///
/// `gh-proxy.com`, `https://gh-proxy.com/` and `http://127.0.0.1:8080` all work;
/// anything without a scheme gets `https://`.
pub fn normalize_proxy(input: &str) -> Option<String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    Some(if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    })
}

/// Every URL to try for a `github.com` release asset, in order.
///
/// The direct link comes last: a proxy that is down is a slow download rather
/// than a dead end. Both spellings proxies expect are offered — `proxy/host/…`
/// and `proxy/https://host/…` — because the two are not interchangeable.
pub fn github_candidates(url: &str, proxy: &str) -> Vec<String> {
    let mut urls = Vec::new();
    if let Some(proxy) = normalize_proxy(proxy) {
        let bare = url.split_once("://").map_or(url, |(_, rest)| rest);
        urls.push(format!("{proxy}/{bare}"));
        urls.push(format!("{proxy}/{url}"));
    }
    urls.push(url.to_string());
    urls.dedup();
    urls
}

/// `1_234_567` → `1.2 MB`, for progress lines.
pub fn human_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.0} KB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_goes_in_front_of_a_github_url_both_ways() {
        let url = "https://github.com/a/b/releases/download/v1/x.zip";
        let proxied = github_candidates(url, "gh-proxy.com");
        assert_eq!(
            proxied[0],
            "https://gh-proxy.com/github.com/a/b/releases/download/v1/x.zip"
        );
        assert_eq!(proxied[1], format!("https://gh-proxy.com/{url}"));
        assert_eq!(proxied[2], url);
    }

    #[test]
    fn an_empty_proxy_is_a_direct_link() {
        assert_eq!(
            github_candidates("https://github.com/a/x", "  "),
            vec!["https://github.com/a/x"]
        );
    }

    #[test]
    fn a_scheme_less_proxy_defaults_to_https() {
        assert_eq!(
            normalize_proxy(" gh-proxy.com/ "),
            Some("https://gh-proxy.com".into())
        );
        assert_eq!(
            normalize_proxy("http://127.0.0.1:8080"),
            Some("http://127.0.0.1:8080".into())
        );
    }
}
