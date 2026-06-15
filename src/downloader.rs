pub mod client;
mod local;
mod pipeline;
mod select;

use crate::filters::{
    is_accepted_wheel, is_source_distribution, platform_to_target,
};
use crate::hex_digest;
use crate::progress::{FileStatus, ProgressHandle, SyncEvent};
use dashmap::DashMap;
use reqwest_middleware::ClientWithMiddleware;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use type_state_builder::TypeStateBuilder;

/// File metadata as returned by PyPI JSON API.
#[derive(Debug, Clone, TypeStateBuilder)]
#[builder(impl_into)]
pub struct FileInfo {
    #[builder(required)]
    pub filename: String,
    #[builder(required)]
    pub url: String,
    pub sha256: Option<String>,
    pub size: Option<u64>,
    pub yanked: Option<String>,
    #[builder(required)]
    pub package_name: String,
    #[builder(required)]
    pub version: String,
    /// True when this file came from an explicit user-provided URL
    /// (rather than discovered via PyPI metadata), so platform filtering
    /// should be skipped.
    #[builder(default = false)]
    pub explicit_url: bool,
}

/// Download result summary.
#[derive(Debug, Default)]
pub struct DownloadResult {
    pub downloaded: Vec<FileInfo>,
    pub skipped: Vec<FileInfo>,
    pub failed: Vec<(FileInfo, String)>,
    pub warnings: Vec<String>,
}

pub type PrefetchedFiles = HashMap<(String, String), Vec<u8>>;

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
    fi: &FileInfo,
    dest: &Path,
) -> (bool, String) {
    let url = fi.url.split('#').next().unwrap_or(&fi.url);
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
    if fi.sha256.as_ref().is_some_and(|expected| {
        let mut h = Sha256::new();
        h.update(&bytes);
        let actual = hex_digest(h.finalize().as_slice());
        !actual.eq_ignore_ascii_case(expected)
    }) {
        return (false, "hash 校验失败".into());
    }
    write_atomic(dest, &bytes).await
}

enum DownloadOutcome {
    Skipped(FileInfo),
    Downloaded(FileInfo),
    Failed(FileInfo, String),
}

async fn try_prefetched_write(
    fi: &FileInfo,
    dest: &Path,
    bytes: &[u8],
    store: &Option<Arc<crate::store::DownloadStore>>,
    progress: &Option<ProgressHandle>,
) -> DownloadOutcome {
    if !bytes_match_sha256(fi, bytes) {
        return DownloadOutcome::Failed(
            fi.clone(),
            "预下载文件 hash 校验失败".to_string(),
        );
    }
    let (ok, msg) = write_atomic(dest, bytes).await;
    if ok {
        tracing::debug!("复用预下载文件: {}", fi.filename);
        if let Some(p) = progress {
            p.emit(SyncEvent::FileDone {
                package: fi.package_name.clone(),
                filename: fi.filename.clone(),
                status: FileStatus::Reused,
            });
        }
        if let Some(s) = store {
            s.record_download(fi, dest).await;
        }
        return DownloadOutcome::Downloaded(fi.clone());
    }
    DownloadOutcome::Failed(fi.clone(), msg)
}

async fn try_network_download(
    client: &ClientWithMiddleware,
    fi: &FileInfo,
    dest: &Path,
    store: &Option<Arc<crate::store::DownloadStore>>,
    progress: &Option<ProgressHandle>,
) -> DownloadOutcome {
    let (ok, msg) = download_file(client, fi, dest).await;
    if ok {
        tracing::debug!("下载完成: {}", fi.filename);
        if let Some(p) = progress {
            p.emit(SyncEvent::FileDone {
                package: fi.package_name.clone(),
                filename: fi.filename.clone(),
                status: FileStatus::Downloaded,
            });
        }
        if let Some(s) = store {
            s.record_download(fi, dest).await;
        }
        DownloadOutcome::Downloaded(fi.clone())
    } else {
        DownloadOutcome::Failed(fi.clone(), msg)
    }
}

#[allow(clippy::too_many_arguments)]
async fn try_download(
    client: &ClientWithMiddleware,
    store: &Option<Arc<crate::store::DownloadStore>>,
    prefetched_files: &PrefetchedFiles,
    fi: &FileInfo,
    repo: &std::path::Path,
    progress: &Option<ProgressHandle>,
) -> DownloadOutcome {
    let dest = repo
        .join("simple")
        .join(&fi.package_name)
        .join(&fi.filename);
    if store.as_ref().is_some_and(|s| {
        s.has_file(&fi.package_name, &fi.filename).unwrap_or(false)
    }) {
        return DownloadOutcome::Skipped(fi.clone());
    }
    if dest.exists() {
        // 文件已存在但数据库里没有记录：补录，避免重复下载后仍然丢失记录。
        if let Some(s) = store {
            s.record_download(fi, &dest).await;
        }
        return DownloadOutcome::Skipped(fi.clone());
    }
    let key = (fi.package_name.clone(), fi.filename.clone());
    if let Some(bytes) = prefetched_files.get(&key) {
        return try_prefetched_write(fi, &dest, bytes, store, progress).await;
    }
    try_network_download(client, fi, &dest, store, progress).await
}

fn bytes_match_sha256(fi: &FileInfo, bytes: &[u8]) -> bool {
    fi.sha256.as_ref().is_none_or(|expected| {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let actual = hex_digest(hasher.finalize().as_slice());
        actual.eq_ignore_ascii_case(expected)
    })
}
fn should_skip(fi: &FileInfo, include_source: bool) -> bool {
    if !include_source && is_source_distribution(&fi.filename) {
        return true;
    }
    if fi.filename.ends_with(".whl") && fi.explicit_url {
        return false;
    }
    fi.filename.ends_with(".whl") && !is_accepted_wheel(&fi.filename)
}
pub async fn download_pkg_files(
    client: &ClientWithMiddleware,
    repo: &std::path::Path,
    files: &[FileInfo],
    include_source: bool,
    download_workers: usize,
) -> DownloadResult {
    let prefetched_files = PrefetchedFiles::new();
    download_pkg_files_with_prefetched(
        client,
        repo,
        files,
        &prefetched_files,
        include_source,
        download_workers,
        None,
    )
    .await
}
#[allow(clippy::too_many_arguments)]
pub async fn download_pkg_files_with_prefetched(
    client: &ClientWithMiddleware,
    repo: &std::path::Path,
    files: &[FileInfo],
    prefetched_files: &PrefetchedFiles,
    include_source: bool,
    download_workers: usize,
    progress: Option<ProgressHandle>,
) -> DownloadResult {
    pipeline::run_download_pipeline(
        client,
        repo,
        files,
        prefetched_files,
        include_source,
        download_workers,
        progress,
    )
    .await
}
