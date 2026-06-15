mod batch;
mod file;
mod local;
mod select;

use crate::filters::{is_accepted_wheel, platform_to_target};
use crate::hex_digest;
use dashmap::DashMap;
use reqwest_middleware::ClientWithMiddleware;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub use file::{Downloadable, DownloadableItem, ExplicitWheel, RemoteFile};

/// Backward-compatible alias for existing code that refers to `FileInfo`.
pub type FileInfo = RemoteFile;

/// Download result summary.
#[derive(Debug, Default)]
pub struct DownloadResult {
    pub downloaded: Vec<DownloadableItem>,
    pub skipped: Vec<DownloadableItem>,
    pub failed: Vec<(DownloadableItem, String)>,
    pub warnings: Vec<String>,
}

pub type PrefetchedFiles = std::collections::HashMap<(String, String), Vec<u8>>;

pub use batch::{BatchDownloader, DownloadPolicy};

pub use select::{collect_version_files, select_latest_versions};

/// Check if a version has a wheel covering the given target platform.
pub fn version_has_target(files: &[FileInfo], target: &str) -> bool {
    files.iter().any(|fi| {
        fi.filename.ends_with(".whl")
            && is_accepted_wheel(&fi.filename)
            && crate::filters::parse_wheel_platform(&fi.filename).is_some_and(
                |tags| {
                    tags.iter()
                        .any(|sub| platform_to_target(sub).contains(target))
                },
            )
    })
}

/// For each missing target platform, scan older versions to find a wheel that covers it.
pub fn backfill_one_target(
    target: &str,
    older_versions: &[String],
    all_versions_grouped: &DashMap<String, Vec<FileInfo>>,
) -> Option<(Vec<FileInfo>, bool)> {
    for ver in older_versions {
        let Some(files) = all_versions_grouped.get(ver) else {
            continue;
        };
        if !version_has_target(files.value(), target) {
            continue;
        }
        let result = collect_version_files(files.value());
        let is_pre = ver
            .parse::<pep440_rs::Version>()
            .map(|v| v.any_prerelease())
            .unwrap_or(false);
        return Some((result, is_pre));
    }
    None
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

async fn write_atomic(dest_path: &Path, bytes: &[u8]) -> (bool, String) {
    if let Some(parent) = dest_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let file_name = dest_path
        .file_name()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let tmp =
        dest_path.with_file_name(format!("{file_name}.{pid}.{counter}.tmp"));
    if let Err(e) = tokio::fs::write(&tmp, bytes).await {
        return (false, format!("写入: {e}"));
    }
    if let Err(e) = tokio::fs::rename(&tmp, dest_path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return (false, format!("重命名: {e}"));
    }
    (true, String::new())
}

/// Download a single file (HTTP/HTTPS) or copy a local file (file://).
pub async fn download_file(
    client: &ClientWithMiddleware,
    fi: &dyn Downloadable,
    dest: &Path,
) -> (bool, String) {
    let url = fi.source_url().split('#').next().unwrap_or(fi.source_url());
    if url
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file://"))
    {
        return local::copy_local_wheel(url, fi, dest).await;
    }
    let Ok(resp) = client.get(url).send().await else {
        return (false, "网络错误".into());
    };
    if !resp.status().is_success() {
        return (false, format!("HTTP {}", resp.status()));
    }
    let Ok(bytes) = resp.bytes().await else {
        return (false, "读取失败".into());
    };
    if fi.sha256().is_some_and(|expected| {
        let mut h = Sha256::new();
        h.update(&bytes);
        let actual = hex_digest(h.finalize().as_slice());
        !actual.eq_ignore_ascii_case(expected)
    }) {
        return (false, "hash 校验失败".into());
    }
    write_atomic(dest, &bytes).await
}
