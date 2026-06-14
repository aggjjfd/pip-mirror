mod local;
mod pipeline;

use crate::filters::{
    is_accepted_wheel, is_source_distribution, platform_to_target,
    sdist_fallback_allowed,
};
use crate::hex_digest;
use dashmap::DashMap;
use reqwest::Client;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
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

fn filter_stable_versions(files: &[FileInfo]) -> Vec<FileInfo> {
    let stable: Vec<_> = files
        .iter()
        .filter(|f| {
            f.version
                .parse::<pep440_rs::Version>()
                .map(|v| !v.any_prerelease())
                .unwrap_or(true)
        })
        .cloned()
        .collect();
    if !stable.is_empty() {
        return stable;
    }
    let pkg = files
        .first()
        .map(|f| f.package_name.as_str())
        .unwrap_or("?");
    let n = files
        .iter()
        .map(|f| &f.version)
        .collect::<HashSet<_>>()
        .len();
    tracing::warn!("  ! {pkg} 仅有预发行版 ({n} 个版本), 回退保留全部");
    files.to_vec()
}

fn group_by_version(
    files: Vec<FileInfo>,
) -> BTreeMap<pep440_rs::Version, Vec<FileInfo>> {
    let mut by_ver: BTreeMap<pep440_rs::Version, Vec<FileInfo>> =
        BTreeMap::new();
    for fi in files {
        if let Ok(v) = fi.version.parse::<pep440_rs::Version>() {
            by_ver.entry(v).or_default().push(fi);
        }
    }
    by_ver
}

/// Select the latest `max_versions` versions from a file list.
/// When `allow_prerelease` is false, prerelease versions are dropped.
/// If that leaves nothing, fall back to the original list with a warning.
pub fn select_latest_versions(
    files: &[FileInfo],
    max_versions: usize,
    allow_prerelease: bool,
) -> Vec<FileInfo> {
    if max_versions == 0 {
        return files.to_vec();
    }
    let candidates = if allow_prerelease {
        files.to_vec()
    } else {
        filter_stable_versions(files)
    };
    let by_ver = group_by_version(candidates);
    by_ver
        .into_iter()
        .rev()
        .take(max_versions)
        .flat_map(|(_, f)| f)
        .collect()
}

/// Collect all accepted wheels and sdists for a version.
/// If a version has any accepted wheel, only wheels are kept (sdist skipped).
/// If a version has no wheel, sdist is kept as fallback.
fn collect_wheels(files: &[FileInfo]) -> (Vec<FileInfo>, HashSet<String>) {
    let mut whl_versions = HashSet::new();
    let mut result = Vec::with_capacity(files.len());
    for fi in files {
        if fi.filename.ends_with(".whl") && is_accepted_wheel(&fi.filename) {
            whl_versions.insert(fi.version.clone());
            result.push(fi.clone());
        }
    }
    (result, whl_versions)
}

fn collect_sdists(
    files: &[FileInfo],
    whl_versions: &HashSet<String>,
) -> Vec<FileInfo> {
    let mut result = Vec::new();
    for fi in files {
        let is_sdist = is_source_distribution(&fi.filename);
        let no_wheel = !whl_versions.contains(&fi.version);
        if is_sdist && no_wheel {
            result.push(fi.clone());
        }
    }
    result
}

pub fn collect_version_files(files: &[FileInfo]) -> Vec<FileInfo> {
    let (mut result, whl_versions) = collect_wheels(files);
    if sdist_fallback_allowed(files, true) {
        result.extend(collect_sdists(files, &whl_versions));
    }
    result
}

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
    client: &Client,
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
) -> DownloadOutcome {
    if !bytes_match_sha256(fi, bytes) {
        return DownloadOutcome::Failed(
            fi.clone(),
            "预下载文件 hash 校验失败".to_string(),
        );
    }
    let (ok, msg) = write_atomic(dest, bytes).await;
    if ok {
        tracing::info!("复用预下载文件: {}", fi.filename);
        if let Some(s) = store {
            s.record_download(fi, dest).await;
        }
        return DownloadOutcome::Downloaded(fi.clone());
    }
    DownloadOutcome::Failed(fi.clone(), msg)
}

async fn try_network_download(
    client: &reqwest::Client,
    fi: &FileInfo,
    dest: &Path,
    store: &Option<Arc<crate::store::DownloadStore>>,
) -> DownloadOutcome {
    let (ok, msg) = download_file(client, fi, dest).await;
    if ok {
        tracing::info!("下载完成: {}", fi.filename);
        if let Some(s) = store {
            s.record_download(fi, dest).await;
        }
        DownloadOutcome::Downloaded(fi.clone())
    } else {
        DownloadOutcome::Failed(fi.clone(), msg)
    }
}

async fn try_download(
    client: &reqwest::Client,
    store: &Option<Arc<crate::store::DownloadStore>>,
    prefetched_files: &PrefetchedFiles,
    fi: &FileInfo,
    repo: &std::path::Path,
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
        return try_prefetched_write(fi, &dest, bytes, store).await;
    }
    try_network_download(client, fi, &dest, store).await
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
    client: &reqwest::Client,
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
    )
    .await
}
#[allow(clippy::too_many_arguments)]
pub async fn download_pkg_files_with_prefetched(
    client: &reqwest::Client,
    repo: &std::path::Path,
    files: &[FileInfo],
    prefetched_files: &PrefetchedFiles,
    include_source: bool,
    download_workers: usize,
) -> DownloadResult {
    pipeline::run_download_pipeline(
        client,
        repo,
        files,
        prefetched_files,
        include_source,
        download_workers,
    )
    .await
}
