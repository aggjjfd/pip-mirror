use std::collections::HashMap;
use std::path::{Path, PathBuf};

use reqwest::Client;
use sha2::{Digest, Sha256};
use tracing::info;

const UV_METADATA_URL: &str =
    "https://raw.githubusercontent.com/astral-sh/uv/main/crates/uv-python/download-metadata.json";

const TARGET_MINORS: &[u32] = &[8, 9, 10, 11, 12, 13, 14];

#[derive(Debug, Clone)]
pub struct PythonBuildEntry {
    pub key: String,
    pub url: String,
    pub sha256: Option<String>,
    pub filename: String,
}

/// Fetch uv's Python build metadata, filter to target platforms, keep latest build per group.
pub async fn fetch_python_builds(
    client: &Client,
) -> Result<Vec<PythonBuildEntry>, Box<dyn std::error::Error>> {
    info!("获取 uv metadata: {UV_METADATA_URL}");
    let resp: serde_json::Value = client.get(UV_METADATA_URL).send().await?.json().await?;

    let target_entries: Vec<_> = resp
        .as_object()
        .map(|obj| {
            obj.iter()
                .filter(|(k, v)| is_target_entry(k, v))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();
    info!("过滤后目标条目: {}", target_entries.len());

    let latest = group_by_platform(&target_entries);
    info!("去重后最新 build: {}", latest.len());

    let entries: Vec<PythonBuildEntry> = latest
        .into_iter()
        .map(|(key, entry)| {
            let url = entry["url"].as_str().unwrap_or("").to_string();
            let filename = if let Some(pos) = url.rfind('/') {
                url[pos + 1..].to_string()
            } else {
                String::new()
            };
            PythonBuildEntry {
                key,
                url,
                sha256: entry["sha256"].as_str().map(String::from),
                filename,
            }
        })
        .collect();

    Ok(entries)
}

fn is_target_entry(key: &str, entry: &serde_json::Value) -> bool {
    if entry
        .get("prerelease")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return false;
    }
    if entry.get("major").and_then(|v| v.as_u64()) != Some(3) {
        return false;
    }
    let minor = entry.get("minor").and_then(|v| v.as_u64());
    if !minor.is_some_and(|m| TARGET_MINORS.contains(&(m as u32))) {
        return false;
    }
    if key.contains("+debug") {
        return false;
    }
    let url = entry.get("url").and_then(|u| u.as_str()).unwrap_or("");
    if !url.contains("install_only_stripped") {
        return false;
    }

    let os = entry.get("os").and_then(|o| o.as_str()).unwrap_or("");
    let arch_family = entry
        .get("arch")
        .and_then(|a| a.get("family"))
        .and_then(|f| f.as_str())
        .unwrap_or("");
    let libc = entry.get("libc").and_then(|l| l.as_str()).unwrap_or("");

    matches!(
        (os, arch_family, libc),
        ("windows", "x86_64" | "i686", "none") | ("linux", "x86_64", "gnu")
    )
}

fn group_by_platform(
    entries: &[(String, serde_json::Value)],
) -> HashMap<String, serde_json::Value> {
    type PlatformKey = (u64, String, String, String, String);
    let mut groups: HashMap<PlatformKey, Vec<&(String, serde_json::Value)>> = HashMap::new();

    for item in entries {
        let entry = &item.1;
        let arch = entry.get("arch").unwrap();
        let variant = arch
            .get("variant")
            .and_then(|v| v.as_str())
            .unwrap_or("base");
        let group_key = (
            entry.get("minor").and_then(|v| v.as_u64()).unwrap_or(0),
            entry
                .get("os")
                .and_then(|o| o.as_str())
                .unwrap_or("")
                .to_string(),
            arch.get("family")
                .and_then(|f| f.as_str())
                .unwrap_or("")
                .to_string(),
            variant.to_string(),
            entry
                .get("libc")
                .and_then(|l| l.as_str())
                .unwrap_or("")
                .to_string(),
        );
        groups.entry(group_key).or_default().push(item);
    }

    let mut result = HashMap::new();
    for (_group_key, mut items) in groups {
        items.sort_by(|a, b| {
            let a_build = a.1.get("build").and_then(|v| v.as_str()).unwrap_or("");
            let b_build = b.1.get("build").and_then(|v| v.as_str()).unwrap_or("");
            b_build.cmp(a_build)
        });
        let best = items[0];
        result.insert(best.0.clone(), best.1.clone());
    }
    result
}

/// Download a single Python build, return status.
pub async fn download_python_build(
    client: &Client,
    entry: &PythonBuildEntry,
    dest_dir: &Path,
) -> Result<(PathBuf, bool), Box<dyn std::error::Error>> {
    let dest = dest_dir.join(&entry.filename);

    // Skip if already exists and sha256 matches
    if dest.exists() {
        let skip = match &entry.sha256 {
            Some(expected) => crate::downloader::sha256_file(&dest)
                .is_ok_and(|a| a.to_lowercase() == expected.to_lowercase()),
            None => true,
        };
        if skip {
            return Ok((dest, false));
        }
    }

    let resp = client.get(&entry.url).send().await?;
    let bytes = resp.bytes().await?;

    // Verify sha256
    if let Some(expected) = &entry.sha256 {
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let actual = format!("{:x}", hasher.finalize());
        if actual.to_lowercase() != expected.to_lowercase() {
            return Err(format!("sha256 校验失败: {}", entry.filename).into());
        }
    }

    let tmp = dest_dir.join(format!("{}.tmp", entry.filename));
    tokio::fs::write(&tmp, &bytes).await?;
    tokio::fs::rename(&tmp, &dest).await?;

    Ok((dest, true)) // downloaded
}
