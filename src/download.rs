//! Streaming HTTP download shared by the dependency installers.
//!
//! Both aria2c and the JRE are single files behind GitHub Releases, so one
//! progress-reporting routine covers them. It returns the final URL after
//! redirects, which is what lets the UI say whether a proxy or the direct link
//! served the bytes.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::http;

/// One progress tick, at most a few per second.
#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    pub speed: f64,
}

/// Stream `url` into `dest`, reporting progress as it goes.
pub async fn file(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected: u64,
    on_progress: impl Fn(Progress),
) -> Result<String, String> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("无法访问 {}：{error}", http::shorten(url)))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("{} → HTTP {status}", http::shorten(url)));
    }
    // `response.url()` is the final URL after redirects: a proxy that hands
    // the transfer off to a CDN is visible here instead of being guessed at.
    let response_url = response.url().to_string();
    let total = response.content_length().unwrap_or(expected);
    let mut file =
        File::create(dest).map_err(|error| format!("无法写入 {}: {error}", dest.display()))?;

    let started = Instant::now();
    let mut last = Instant::now();
    let mut done = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("下载中断：{error}"))?
    {
        file.write_all(&chunk)
            .map_err(|error| format!("无法写入下载结果: {error}"))?;
        done += chunk.len() as u64;
        if last.elapsed() >= Duration::from_millis(200) {
            let elapsed = started.elapsed().as_secs_f64().max(0.001);
            on_progress(Progress {
                done,
                total: total.max(done),
                speed: done as f64 / elapsed,
            });
            last = Instant::now();
        }
    }
    file.sync_all()
        .map_err(|error| format!("无法写入下载结果: {error}"))?;
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    on_progress(Progress {
        done,
        total: total.max(done),
        speed: done as f64 / elapsed,
    });
    Ok(response_url)
}
