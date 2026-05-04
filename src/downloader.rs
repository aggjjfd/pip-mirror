use std::collections::HashSet;
use std::path::Path;

use dashmap::DashMap;
use flate2::write::GzEncoder;
use flate2::Compression;
use reqwest::Client;
use sha2::{Digest, Sha256};

use crate::filters::{is_accepted_wheel, is_source_distribution, platform_to_target};

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

/// Target platforms we care about.
const TARGET_PLATFORMS: &[&str] = &["win32", "win_amd64", "linux_x86_64"];

/// Maximum number of older versions to scan when backfilling missing platforms.
const BACKFILL_SCAN_LIMIT: usize = 50;

/// Download result summary.
#[derive(Debug, Default)]
pub struct DownloadResult {
    pub downloaded: Vec<FileInfo>,
    pub skipped: Vec<FileInfo>,
    pub failed: Vec<(FileInfo, String)>,
    pub warnings: Vec<String>,
}

/// Fetch file list from PyPI JSON API.
pub async fn fetch_json_api(
    client: &Client,
    package_name: &str,
    pypi_url: &str,
) -> Result<Vec<FileInfo>, reqwest::Error> {
    let normalized = super::filters::normalize_package_name(package_name);
    let url = format!("{}/pypi/{}/json", pypi_url.trim_end_matches('/'), normalized);
    let resp: serde_json::Value = client.get(&url).send().await?.json().await?;

    let mut files = Vec::new();
    if let Some(releases) = resp.get("releases") {
        for (version, file_list) in releases.as_object().unwrap_or(&serde_json::Map::new()) {
            for f in file_list.as_array().unwrap_or(&vec![]) {
                let filename = f["filename"].as_str().unwrap_or("").to_string();
                let file_url = f["url"].as_str().unwrap_or("").to_string();
                let sha256 = f
                    .get("digests")
                    .and_then(|d| d.get("sha256"))
                    .and_then(|s| s.as_str())
                    .map(String::from);
                let size = f["size"].as_u64();
                files.push(FileInfo {
                    filename,
                    url: file_url,
                    sha256,
                    size,
                    package_name: package_name.to_string(),
                    version: version.clone(),
                });
            }
        }
    }
    Ok(files)
}

/// Select the latest `max_versions` versions from a file list.
pub fn select_latest_versions(files: &[FileInfo], max_versions: usize) -> Vec<FileInfo> {
    if max_versions == 0 {
        return files.to_vec();
    }
    let mut versions: Vec<String> = files
        .iter()
        .map(|f| f.version.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    versions.sort_by(|a, b| {
        b.parse::<pep440_rs::Version>()
            .unwrap_or_else(|_| unreachable!())
            .cmp(&a.parse::<pep440_rs::Version>().unwrap_or_else(|_| unreachable!()))
    });

    let selected: HashSet<_> = versions.into_iter().take(max_versions).collect();
    files
        .iter()
        .filter(|f| selected.contains(&f.version))
        .cloned()
        .collect()
}

/// For each missing target platform, scan older versions to find a wheel that covers it.
pub fn backfill_one_target(
    target: &str,
    older_versions: &[String],
    all_versions_grouped: &DashMap<String, Vec<FileInfo>>,
) -> Option<(Vec<FileInfo>, bool)> {
    for ver in older_versions {
        let files = match all_versions_grouped.get(ver) {
            Some(f) => f,
            None => continue,
        };
        for fi in files.value() {
            if !fi.filename.ends_with(".whl") {
                continue;
            }
            if !is_accepted_wheel(&fi.filename) {
                continue;
            }
            let plat = fi.filename[..fi.filename.len() - 4]
                .rsplit('-')
                .next()
                .unwrap_or("");
            if platform_to_target(plat).contains(target) {
                // Found a version covering this target — collect all its files
                let mut result: Vec<FileInfo> = Vec::new();
                for fi2 in files.value() {
                    if fi2.filename.ends_with(".whl") {
                        if is_accepted_wheel(&fi2.filename) {
                            result.push(fi2.clone());
                        }
                    } else if is_source_distribution(&fi2.filename) {
                        result.push(fi2.clone());
                    }
                }
                let is_pre = ver.parse::<pep440_rs::Version>()
                    .map(|v| v.any_prerelease())
                    .unwrap_or(false);
                return Some((result, is_pre));
            }
        }
    }
    None
}

/// Download a single file.
async fn download_file(
    client: &Client,
    file_info: &FileInfo,
    dest_path: &Path,
) -> (bool, String) {
    if let Some(parent) = dest_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    let url = file_info.url.split('#').next().unwrap_or(&file_info.url);
    let resp = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => return (false, format!("网络错误: {e}")),
    };

    if !resp.status().is_success() {
        return (false, format!("HTTP {}", resp.status()));
    }

    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => return (false, format!("读取失败: {e}")),
    };

    // sha256 verify
    if let Some(expected) = &file_info.sha256 {
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let actual = format!("{:x}", hasher.finalize());
        if actual.to_lowercase() != expected.to_lowercase() {
            return (false, "hash 校验失败".into());
        }
    }

    let tmp = dest_path.with_extension("tmp");
    if let Err(e) = tokio::fs::write(&tmp, &bytes).await {
        return (false, format!("文件写入错误: {e}"));
    }
    if let Err(e) = tokio::fs::rename(&tmp, dest_path).await {
        return (false, format!("文件重命名错误: {e}"));
    }

    (true, String::new())
}

/// Package the repository directory into a tar.gz archive.
pub fn pack_full_mirror(repo: &Path, output: &Path, compression: Compression) -> std::io::Result<()> {
    let archive = std::fs::File::create(output)?;
    let encoder = GzEncoder::new(archive, compression);
    let mut tar = tar::Builder::new(encoder);
    tar.follow_symlinks(false);
    tar.append_dir_all(
        repo.file_name().unwrap_or_else(|| repo.as_os_str()),
        repo,
    )?;
    let encoder = tar.into_inner()?;
    encoder.finish()?;
    Ok(())
}

/// Stream-sha256 a file.
pub fn sha256_file(path: &Path) -> Result<String, std::io::Error> {
    crate::store::DownloadStore::hash_file(path)
}
