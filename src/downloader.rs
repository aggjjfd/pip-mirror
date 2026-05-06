use std::collections::HashSet;
use std::path::Path;

use dashmap::DashMap;
use flate2::Compression;
use flate2::write::GzEncoder;
use reqwest::Client;
use sha2::{Digest, Sha256};

use crate::filters::{
    is_accepted_wheel, is_source_distribution, platform_to_target,
};

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

// ── helpers for flattening deep nesting ──

fn extract_file_fields(
    value: &serde_json::Value,
) -> (String, String, Option<String>, Option<u64>) {
    let filename = value["filename"].as_str().unwrap_or("").to_string();
    let file_url = value["url"].as_str().unwrap_or("").to_string();
    let sha256 = value
        .get("digests")
        .and_then(|d| d.get("sha256"))
        .and_then(|s| s.as_str())
        .map(String::from);
    let size = value["size"].as_u64();
    (filename, file_url, sha256, size)
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
            let (filename, file_url, sha256, size) = extract_file_fields(f);
            files.push(FileInfo {
                filename,
                url: file_url,
                sha256,
                size,
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

    let mut candidates = files.to_vec();
    if !allow_prerelease {
        let stable: Vec<_> = candidates
            .iter()
            .filter(|f| {
                f.version
                    .parse::<pep440_rs::Version>()
                    .map(|v| !v.any_prerelease())
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        if stable.is_empty() {
            let pkg = files
                .first()
                .map(|f| f.package_name.as_str())
                .unwrap_or("?");
            let n = candidates
                .iter()
                .map(|f| &f.version)
                .collect::<HashSet<_>>()
                .len();
            tracing::warn!("  ! {pkg} 仅有预发行版 ({n} 个版本), 回退保留全部");
        } else {
            candidates = stable;
        }
    }

    let mut versions: Vec<String> = candidates
        .iter()
        .map(|f| f.version.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    versions.sort_by(|a, b| {
        b.parse::<pep440_rs::Version>()
            .unwrap_or_else(|_| unreachable!())
            .cmp(
                &a.parse::<pep440_rs::Version>()
                    .unwrap_or_else(|_| unreachable!()),
            )
    });

    let selected: HashSet<_> =
        versions.into_iter().take(max_versions).collect();
    candidates
        .iter()
        .filter(|f| selected.contains(&f.version))
        .cloned()
        .collect()
}

/// Collect all accepted wheels and sdists for a version.
pub fn collect_version_files(files: &[FileInfo]) -> Vec<FileInfo> {
    let mut result = Vec::new();
    for fi in files {
        let is_whl = fi.filename.ends_with(".whl");
        let accepted = is_whl && is_accepted_wheel(&fi.filename);
        let is_sdist = !is_whl && is_source_distribution(&fi.filename);
        if accepted || is_sdist {
            result.push(fi.clone());
        }
    }
    result
}

/// Check if a version has a wheel covering the given target platform.
pub fn version_has_target(files: &[FileInfo], target: &str) -> bool {
    for fi in files {
        if !fi.filename.ends_with(".whl") || !is_accepted_wheel(&fi.filename) {
            continue;
        }
        let plat = fi.filename[..fi.filename.len() - 4]
            .rsplit('-')
            .next()
            .unwrap_or("");
        if platform_to_target(plat).contains(target) {
            return true;
        }
    }
    false
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

fn hash_ok(bytes: &[u8], expected: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize()).to_lowercase() == expected.to_lowercase()
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
    if fi.sha256.as_ref().is_some_and(|e| !hash_ok(&bytes, e)) {
        return (false, "hash 校验失败".into());
    }
    write_atomic(dest, &bytes).await
}

/// Package the repository directory into a tar.gz archive.
pub fn pack_full_mirror(
    repo: &Path,
    output: &Path,
    compression: Compression,
) -> std::io::Result<()> {
    let archive = std::fs::File::create(output)?;
    let encoder = GzEncoder::new(archive, compression);
    let mut tar = tar::Builder::new(encoder);
    tar.follow_symlinks(false);
    tar.append_dir_all(repo.file_name().unwrap_or(repo.as_os_str()), repo)?;
    let encoder = tar.into_inner()?;
    encoder.finish()?;
    Ok(())
}

/// Stream-sha256 a file.
pub fn sha256_file(path: &Path) -> Result<String, std::io::Error> {
    crate::store::DownloadStore::hash_file(path)
}

// ── PyPI data fetching (used by resolver) ──

use pep440_rs::Version;
use std::str::FromStr;

pub async fn get_all_versions(
    http: &HttpCtx<'_>,
    package: &str,
) -> Result<Vec<Version>, reqwest::Error> {
    let bare = package.split_once('[').map_or(package, |(n, _)| n);
    let normalized = crate::filters::normalize_package_name(bare);
    let url = format!(
        "{}/pypi/{}/json",
        http.pypi_url.trim_end_matches('/'),
        normalized
    );
    let resp: serde_json::Value =
        http.client.get(&url).send().await?.json().await?;
    let mut versions: Vec<Version> = resp
        .get("releases")
        .and_then(|r| r.as_object())
        .map(|obj| {
            obj.keys()
                .filter_map(|v| Version::from_str(v).ok())
                .collect()
        })
        .unwrap_or_default();
    versions.sort_by(|a, b| b.cmp(a));
    Ok(versions)
}

/// Get requires_dist for a specific package version. Returns list of (dep_name, specifier, extras_marker_str).
pub async fn get_requires_dist(
    http: &HttpCtx<'_>,
    package: &str,
    version: &str,
) -> Result<Option<Vec<String>>, reqwest::Error> {
    let bare = package.split_once('[').map_or(package, |(n, _)| n);
    let normalized = crate::filters::normalize_package_name(bare);
    let url = format!(
        "{}/pypi/{}/{}/json",
        http.pypi_url.trim_end_matches('/'),
        normalized,
        version
    );
    let resp: serde_json::Value =
        http.client.get(&url).send().await?.json().await?;
    let rd = resp
        .get("info")
        .and_then(|i| i.get("requires_dist"))
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });
    Ok(rd)
}

pub async fn download_pkg_files(
    client: &reqwest::Client,
    repo: &std::path::Path,
    files: &[FileInfo],
) {
    let store = crate::store::DownloadStore::open(&repo.join(".store.db")).ok();
    for fi in files {
        let dest = repo
            .join("simple")
            .join(&fi.package_name)
            .join(&fi.filename);
        let (ok, _) = download_file(client, fi, &dest).await;
        if !ok {
            continue;
        }
        let Some(ref s) = store else { continue };
        let sha256 = fi.sha256.clone().unwrap_or_else(|| {
            crate::store::DownloadStore::hash_file(&dest).unwrap_or_default()
        });
        let rec = crate::store::FileRecord {
            filename: &fi.filename,
            package_name: &fi.package_name,
            version: &fi.version,
            sha256: &sha256,
            size: std::fs::metadata(&dest).ok().map(|m| m.len()),
        };
        let _ = s.add_file(&rec);
    }
}
