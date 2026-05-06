use crate::filters::{
    is_accepted_wheel, is_source_distribution, platform_to_target,
};
use dashmap::DashMap;
use reqwest::Client;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

pub struct HttpCtx<'a> {
    pub client: &'a Client,
    pub pypi_url: &'a str,
}

/// File metadata as returned by PyPI JSON API.
#[derive(Debug, Clone)]
pub struct FileInfo {
    pub filename: String,
    pub url: String,
    pub sha256: Option<String>,
    pub size: Option<u64>,
    pub package_name: String,
    pub version: String,
}

/// Download result summary.
#[derive(Debug, Default)]
pub struct DownloadResult {
    pub downloaded: Vec<FileInfo>,
    pub skipped: Vec<FileInfo>,
    pub failed: Vec<(FileInfo, String)>,
    pub warnings: Vec<String>,
}

fn collect_release_files(
    releases: &serde_json::Value,
    pkg: &str,
) -> Vec<FileInfo> {
    let mut files = Vec::new();
    for (version, file_list) in
        releases.as_object().unwrap_or(&serde_json::Map::new())
    {
        for f in file_list.as_array().unwrap_or(&vec![]) {
            files.push(FileInfo {
                filename: f["filename"].as_str().unwrap_or("").to_string(),
                url: f["url"].as_str().unwrap_or("").to_string(),
                sha256: f
                    .get("digests")
                    .and_then(|d| d.get("sha256"))
                    .and_then(|s| s.as_str())
                    .map(String::from),
                size: f["size"].as_u64(),
                package_name: pkg.to_string(),
                version: version.clone(),
            });
        }
    }
    files
}

pub async fn fetch_json_api(
    http: &HttpCtx<'_>,
    pkg: &str,
) -> Result<Vec<FileInfo>, reqwest::Error> {
    // 一次性 PEP 503 normalize + 剥 extras,
    // 之后 simple/<pkg>/ 目录、.store.db、tar 路径全部走这个名字
    let normalized = super::filters::normalize_package_name(pkg);
    let url = format!(
        "{}/pypi/{}/json",
        http.pypi_url.trim_end_matches('/'),
        normalized
    );
    let resp: serde_json::Value =
        http.client.get(&url).send().await?.json().await?;
    Ok(resp
        .get("releases")
        .map(|r| collect_release_files(r, &normalized))
        .unwrap_or_default())
}

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
pub fn collect_version_files(files: &[FileInfo]) -> Vec<FileInfo> {
    files
        .iter()
        .filter(|fi| {
            let is_whl = fi.filename.ends_with(".whl");
            is_whl && is_accepted_wheel(&fi.filename)
                || !is_whl && is_source_distribution(&fi.filename)
        })
        .cloned()
        .collect()
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

async fn write_atomic(dest_path: &Path, bytes: &[u8]) -> (bool, String) {
    if let Some(parent) = dest_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let tmp = dest_path.with_extension("tmp");
    if let Err(e) = tokio::fs::write(&tmp, bytes).await {
        return (false, format!("写入: {e}"));
    }
    if let Err(e) = tokio::fs::rename(&tmp, dest_path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return (false, format!("重命名: {e}"));
    }
    (true, String::new())
}

/// Download a single file.
pub async fn download_file(
    client: &Client,
    fi: &FileInfo,
    dest: &Path,
) -> (bool, String) {
    let url = fi.url.split('#').next().unwrap_or(&fi.url);
    let Ok(resp) = client.get(url).send().await else {
        return (false, "网络错误".into());
    };
    if !resp.status().is_success() {
        return (false, format!("HTTP {}", resp.status()));
    }
    let Ok(bytes) = resp.bytes().await else {
        return (false, "读取失败".into());
    };
    if fi.sha256.as_ref().is_some_and(|e| {
        let mut h = Sha256::new();
        h.update(&bytes);
        format!("{:x}", h.finalize()).to_lowercase() != e.to_lowercase()
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

fn warn_stale_record(
    store: &crate::store::DownloadStore,
    pkg: &str,
    filename: &str,
    dest: &std::path::Path,
) {
    match store.has_file(pkg, filename) {
        Ok(true) => tracing::warn!(
            "DB 有记录但磁盘文件缺失，重新下载: {}",
            dest.display()
        ),
        Ok(false) => {}
        Err(e) => tracing::warn!("查询 .store.db 失败: {e}"),
    }
}

async fn try_download(
    client: &reqwest::Client,
    store: &Option<crate::store::DownloadStore>,
    fi: &FileInfo,
    repo: &std::path::Path,
) -> DownloadOutcome {
    let dest = repo
        .join("simple")
        .join(&fi.package_name)
        .join(&fi.filename);
    if dest.exists() {
        return DownloadOutcome::Skipped(fi.clone());
    }
    if let Some(s) = store {
        warn_stale_record(s, &fi.package_name, &fi.filename, &dest);
    }
    let (ok, msg) = download_file(client, fi, &dest).await;
    if ok {
        if let Some(s) = store {
            s.record_download(fi, &dest).await;
        }
        DownloadOutcome::Downloaded(fi.clone())
    } else {
        DownloadOutcome::Failed(fi.clone(), msg)
    }
}

pub async fn download_pkg_files(
    client: &reqwest::Client,
    repo: &std::path::Path,
    files: &[FileInfo],
    include_source: bool,
) -> DownloadResult {
    let mut result = DownloadResult::default();
    let store = crate::store::DownloadStore::open(&repo.join(".store.db")).ok();
    for fi in files {
        if !include_source && is_source_distribution(&fi.filename) {
            result.skipped.push(fi.clone());
            continue;
        }
        match try_download(client, &store, fi, repo).await {
            DownloadOutcome::Skipped(f) => result.skipped.push(f),
            DownloadOutcome::Downloaded(f) => result.downloaded.push(f),
            DownloadOutcome::Failed(f, msg) => result.failed.push((f, msg)),
        }
    }
    result
}
