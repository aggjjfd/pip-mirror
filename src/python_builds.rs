use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::hex_digest;
use reqwest::Client;
use sha2::{Digest, Sha256};
use tracing::info;
use type_state_builder::TypeStateBuilder;

const UV_METADATA_URL: &str = "https://raw.githubusercontent.com/astral-sh/uv/main/crates/uv-python/download-metadata.json";

const TARGET_MINORS: &[u32] = &[8, 9, 10, 11, 12];

/// Allowed (os, arch_family, libc) triplets for python-build-standalone.
const PLATFORM_TRIPLETS: &[(&str, &str, &str)] =
    &[("windows", "x86_64", "none"), ("linux", "x86_64", "gnu")];

/// Accepted x86_64 micro-architecture variants.
/// v1 = SSE2 baseline (all x86_64 CPUs).
/// v3 = AVX2 (Haswell / Broadwell+).
const ACCEPTED_ARCH_VARIANTS: &[&str] = &["v1", "v3"];

#[derive(Debug, Clone, TypeStateBuilder)]
#[builder(impl_into)]
pub struct PythonBuildEntry {
    #[builder(required)]
    pub key: String,
    #[builder(required)]
    pub url: String,
    pub sha256: Option<String>,
    #[builder(required)]
    pub filename: String,
    #[builder(default = serde_json::Value::Null)]
    pub raw: serde_json::Value,
}

/// Fetch uv's Python build metadata, filter to target platforms, keep latest build per group.
pub async fn fetch_python_builds(
    client: &Client,
) -> Result<Vec<PythonBuildEntry>, Box<dyn std::error::Error>> {
    info!("获取 uv metadata: {UV_METADATA_URL}");
    let resp: serde_json::Value =
        client.get(UV_METADATA_URL).send().await?.json().await?;

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
            let filename = url
                .rfind('/')
                .map(|p| url[p + 1..].replace("%2B", "+").to_string())
                .unwrap_or_default();
            let sha256 = entry["sha256"].as_str().map(String::from);
            PythonBuildEntry::builder()
                .key(key)
                .url(url)
                .filename(filename)
                .sha256(sha256)
                .raw(entry)
                .build()
        })
        .collect();
    Ok(entries)
}

fn bail_entry(entry: &serde_json::Value) -> bool {
    let prerelease = entry
        .get("prerelease")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !prerelease.is_empty() {
        return true;
    }
    entry.get("major").and_then(|v| v.as_u64()) != Some(3)
}

fn target_minor_ok(entry: &serde_json::Value) -> bool {
    entry
        .get("minor")
        .and_then(|v| v.as_u64())
        .is_some_and(|m| TARGET_MINORS.contains(&(m as u32)))
}

fn platform_match(entry: &serde_json::Value) -> bool {
    let os = entry.get("os").and_then(|o| o.as_str()).unwrap_or("");
    let arch = entry.get("arch");
    let arch_family = arch
        .and_then(|a| a.get("family"))
        .and_then(|f| f.as_str())
        .unwrap_or("");
    let arch_variant = arch
        .and_then(|a| a.get("variant"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let libc = entry.get("libc").and_then(|l| l.as_str()).unwrap_or("");
    let os_ok = PLATFORM_TRIPLETS
        .iter()
        .any(|(o, f, l)| *o == os && *f == arch_family && *l == libc);
    let variant_ok = arch_variant.is_empty()
        || ACCEPTED_ARCH_VARIANTS.contains(&arch_variant);
    os_ok && variant_ok
}

fn is_target_entry(key: &str, entry: &serde_json::Value) -> bool {
    if bail_entry(entry) || !target_minor_ok(entry) {
        return false;
    }
    if key.contains("+debug") {
        return false;
    }
    let url = entry.get("url").and_then(|u| u.as_str()).unwrap_or("");
    url.contains("install_only_stripped") && platform_match(entry)
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct PlatformKey {
    minor: u64,
    os: String,
    arch_family: String,
    arch_variant: String,
    libc: String,
    build_variant: String,
}

fn group_by_platform(
    entries: &[(String, serde_json::Value)],
) -> HashMap<String, serde_json::Value> {
    let mut groups: HashMap<PlatformKey, Vec<&(String, serde_json::Value)>> =
        HashMap::new();

    for item in entries {
        let entry = &item.1;
        let Some(arch) = entry.get("arch") else {
            continue;
        };
        let arch_variant = arch
            .get("variant")
            .and_then(|v| v.as_str())
            .unwrap_or("base");
        let build_variant = entry
            .get("variant")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let key = PlatformKey {
            minor: entry.get("minor").and_then(|v| v.as_u64()).unwrap_or(0),
            os: entry
                .get("os")
                .and_then(|o| o.as_str())
                .unwrap_or("")
                .to_string(),
            arch_family: arch
                .get("family")
                .and_then(|f| f.as_str())
                .unwrap_or("")
                .to_string(),
            arch_variant: arch_variant.to_string(),
            libc: entry
                .get("libc")
                .and_then(|l| l.as_str())
                .unwrap_or("")
                .to_string(),
            build_variant,
        };
        groups.entry(key).or_default().push(item);
    }

    let mut result = HashMap::new();
    for (_group_key, mut items) in groups {
        items.sort_by(|a, b| {
            let a_build =
                a.1.get("build").and_then(|v| v.as_str()).unwrap_or("");
            let b_build =
                b.1.get("build").and_then(|v| v.as_str()).unwrap_or("");
            b_build.cmp(a_build)
        });
        let best = items[0];
        result.insert(best.0.clone(), best.1.clone());
    }
    result
}

async fn should_skip_existing(dest: &Path, expected: &Option<String>) -> bool {
    if !dest.exists() {
        return false;
    }
    match expected {
        Some(e) => {
            let dest = dest.to_path_buf();
            let e = e.clone();
            match tokio::task::spawn_blocking(move || {
                crate::store::DownloadStore::hash_file(&dest)
                    .map(|a| a.to_lowercase() == e.to_lowercase())
            })
            .await
            {
                Ok(Ok(skip)) => skip,
                Ok(Err(_)) | Err(_) => false,
            }
        }
        None => true,
    }
}

fn verify_sha256(
    bytes: &[u8],
    expected: &str,
    filename: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let actual = hex_digest(hasher.finalize().as_slice());
    if actual.to_lowercase() != expected.to_lowercase() {
        return Err(format!("sha256 校验失败: {filename}").into());
    }
    Ok(())
}

async fn fetch_and_verify(
    client: &Client,
    entry: &PythonBuildEntry,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let bytes = client.get(&entry.url).send().await?.bytes().await?;
    if let Some(e) = &entry.sha256 {
        verify_sha256(&bytes, e, &entry.filename)?;
    }
    Ok(bytes.to_vec())
}

pub async fn download_python_build(
    client: &Client,
    entry: &PythonBuildEntry,
    dest_dir: &Path,
) -> Result<(PathBuf, bool), Box<dyn std::error::Error>> {
    let dest = dest_dir.join(&entry.filename);
    if should_skip_existing(&dest, &entry.sha256).await {
        return Ok((dest, false));
    }
    let bytes = fetch_and_verify(client, entry).await?;
    let tmp = dest_dir.join(format!("{}.tmp", entry.filename));
    tokio::fs::write(&tmp, &bytes).await?;
    if let Err(e) = tokio::fs::rename(&tmp, &dest).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(format!("重命名失败: {e}").into());
    }
    Ok((dest, true))
}

async fn download_one_build(
    client: &Client,
    entry: &PythonBuildEntry,
    dir: &Path,
) {
    let result = download_python_build(client, entry, dir).await;
    match result {
        Ok((_, true)) => info!("  [OK] {}", entry.filename),
        Err(e) => tracing::warn!("  [FAIL] {}: {e}", entry.filename),
        _ => {}
    }
}

pub async fn download_python_builds_batch(
    client: &Client,
    repo: &Path,
    workers: usize,
) -> Result<Vec<PythonBuildEntry>, Box<dyn std::error::Error>> {
    let entries = fetch_python_builds(client).await?;
    let dir = repo.join("python-builds");
    std::fs::create_dir_all(&dir)?;
    use futures::{StreamExt, stream};
    stream::iter(&entries)
        .map(|entry| {
            let dir = dir.clone();
            async move {
                download_one_build(client, entry, &dir).await;
            }
        })
        .buffer_unordered(workers)
        .collect::<Vec<_>>()
        .await;
    Ok(entries)
}

pub fn build_python_builds_index(
    entries: &[PythonBuildEntry],
    repo: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut meta = serde_json::Map::new();
    for entry in entries {
        let mut e = entry.raw.clone();
        e["url"] = serde_json::Value::String(format!(
            "/python-builds/{}",
            entry.filename
        ));
        // uv treats Some("") prerelease as a prerelease → skip stable builds.
        if e.get("prerelease").and_then(|v| v.as_str()) == Some("") {
            e["prerelease"] = serde_json::Value::Null;
        }
        meta.insert(entry.key.clone(), e);
    }
    std::fs::write(
        repo.join("python-builds/index.json"),
        serde_json::to_string_pretty(&meta)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_python_build_entry_builder() {
        let e = PythonBuildEntry::builder()
            .key("k".to_string())
            .url("u".to_string())
            .filename("f".to_string())
            .sha256(Some("s".to_string()))
            .raw(serde_json::json!({"url":"u","sha256":"s"}))
            .build();
        assert_eq!(e.key, "k");
        assert_eq!(e.sha256, Some("s".to_string()));

        let url = "https://e/p%2B3.tar.gz";
        let p = PythonBuildEntry::builder()
            .key("e".to_string())
            .url(url.to_string())
            .filename(
                url.rfind('/')
                    .map(|p| url[p + 1..].replace("%2B", "+").to_string())
                    .unwrap_or_default(),
            )
            .raw(serde_json::json!({"url":url}))
            .build();
        assert_eq!(p.filename, "p+3.tar.gz");

        let x = PythonBuildEntry::builder()
            .key("x".to_string())
            .url("".to_string())
            .filename("".to_string())
            .raw(serde_json::json!({}))
            .build();
        assert_eq!(x.url, "");
        assert_eq!(x.sha256, None);
    }
}
