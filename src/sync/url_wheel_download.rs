use futures::StreamExt;

use crate::downloader::PrefetchedFiles;
use crate::hex_digest;
use crate::http::{HttpClient, HttpError};
use crate::resolver::resolve::ResolveError;
use crate::sync::url_wheel::{MAX_REMOTE_WHEEL_BYTES, read_local_wheel_deps};

pub fn sha256_matches(bytes: &[u8], expected: &str) -> bool {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let actual = hex_digest(hasher.finalize().as_slice());
    actual.eq_ignore_ascii_case(expected)
}

fn check_content_length(url: &str, len: u64) -> Result<(), ResolveError> {
    if len > MAX_REMOTE_WHEEL_BYTES {
        return Err(ResolveError::Config(format!(
            "whl 文件过大 ({}): {} > {} 字节；请手动声明其依赖",
            crate::filters::redact_url_for_display(url),
            len,
            MAX_REMOTE_WHEEL_BYTES
        )));
    }
    Ok(())
}

fn check_sha256(
    url: &str,
    bytes: &[u8],
    expected: Option<&str>,
) -> Result<(), HttpError> {
    if expected.is_some_and(|exp| !sha256_matches(bytes, exp)) {
        return Err(HttpError::Sha256Mismatch {
            url: url.to_string(),
        });
    }
    Ok(())
}

fn append_chunk(
    bytes: &mut Vec<u8>,
    url: &str,
    chunk: Result<bytes::Bytes, HttpError>,
) -> Result<(), ResolveError> {
    let chunk = chunk.map_err(|e| {
        ResolveError::Config(format!(
            "读取 {} 响应失败: {e}",
            crate::filters::redact_url_for_display(url)
        ))
    })?;
    if bytes.len() as u64 + chunk.len() as u64 > MAX_REMOTE_WHEEL_BYTES {
        return Err(ResolveError::Config(format!(
            "whl 文件过大 ({}): 超过 {} 字节；请手动声明其依赖",
            crate::filters::redact_url_for_display(url),
            MAX_REMOTE_WHEEL_BYTES
        )));
    }
    bytes.extend_from_slice(&chunk);
    Ok(())
}

async fn read_stream_to_vec(
    client: &HttpClient,
    url: &str,
) -> Result<Vec<u8>, ResolveError> {
    let (content_length, mut stream) = client
        .get_stream(url)
        .await
        .map_err(|e| ResolveError::Config(e.to_string()))?;
    if let Some(len) = content_length {
        check_content_length(url, len)?;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        append_chunk(&mut bytes, url, chunk)?;
    }
    Ok(bytes)
}

pub async fn download_wheel_bytes(
    client: &HttpClient,
    url: &str,
    expected_sha256: Option<&str>,
) -> Result<Vec<u8>, ResolveError> {
    let bytes = read_stream_to_vec(client, url).await?;
    check_sha256(url, &bytes, expected_sha256)
        .map_err(|e| ResolveError::Config(e.to_string()))?;
    Ok(bytes)
}

fn extract_requires_dist_from_bytes_err(
    url: &str,
    bytes: &[u8],
    expected_dist_info_path: &str,
) -> Result<Vec<String>, ResolveError> {
    crate::wheel_metadata::extract_requires_dist_from_bytes(
        bytes,
        expected_dist_info_path,
    )
    .map_err(|e| {
        ResolveError::Config(format!(
            "读取 {} 的 METADATA 失败: {e}",
            crate::filters::redact_url_for_display(url)
        ))
    })
}

async fn spawn_extract_requires_dist(
    url: String,
    bytes: Vec<u8>,
    expected: String,
) -> Result<Vec<String>, ResolveError> {
    tokio::task::spawn_blocking(move || {
        extract_requires_dist_from_bytes_err(&url, &bytes, &expected)
    })
    .await
    .map_err(|e| ResolveError::Config(format!("解析 wheel 元数据失败: {e}")))?
}

pub fn merge_unique_dep_names(into: &mut Vec<String>, names: Vec<String>) {
    for name in names {
        if !into.contains(&name) {
            into.push(name);
        }
    }
}

async fn process_single_url_wheel(
    client: &HttpClient,
    spec: &crate::config::PackageUrlSpec,
    prefetched: &mut PrefetchedFiles,
) -> Result<Vec<String>, ResolveError> {
    let parsed =
        crate::wheel_url::parse_wheel_url(&spec.url, spec.sha256.clone())
            .map_err(|e| {
                ResolveError::Config(format!(
                    "URL whl 解析失败 ({}): {e}",
                    crate::filters::redact_url_for_display(&spec.url)
                ))
            })?;

    let is_file_url = spec
        .url
        .get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case("file://"));
    let expected_path = parsed.dist_info_dir;

    let requires_dist = if is_file_url {
        read_local_wheel_deps(&spec.url, &expected_path, spec.sha256.as_deref())
            .await?
    } else {
        let bytes =
            download_wheel_bytes(client, &spec.url, spec.sha256.as_deref())
                .await?;
        let dist = spawn_extract_requires_dist(
            spec.url.clone(),
            bytes.clone(),
            expected_path,
        )
        .await?;
        prefetched.insert((parsed.package_name, parsed.filename), bytes);
        dist
    };

    Ok(crate::wheel_metadata::extract_package_names(&requires_dist))
}

/// Extract dependency package names from explicit URL wheels and prefetch
/// remote wheels so the download phase can reuse them.
pub async fn collect_url_wheel_deps(
    client: &HttpClient,
    url_pkgs: &[crate::config::PackageUrlSpec],
) -> Result<(Vec<String>, PrefetchedFiles), ResolveError> {
    let mut dep_names = Vec::new();
    let mut prefetched = PrefetchedFiles::new();
    let mut seen: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();

    for spec in url_pkgs {
        let parsed = crate::wheel_url::parse_wheel_url(&spec.url, None)
            .map_err(|e| {
                ResolveError::Config(format!(
                    "URL whl 解析失败 ({}): {e}",
                    crate::filters::redact_url_for_display(&spec.url)
                ))
            })?;
        let key = (parsed.package_name, parsed.filename);
        if !seen.insert(key.clone()) {
            continue;
        }
        let names =
            process_single_url_wheel(client, spec, &mut prefetched).await?;
        merge_unique_dep_names(&mut dep_names, names);
    }

    Ok((dep_names, prefetched))
}

pub async fn maybe_collect_url_wheel_deps(
    client: &HttpClient,
    url_pkgs: &[crate::config::PackageUrlSpec],
    no_deps: bool,
    name_pkgs: &mut Vec<String>,
) -> Result<PrefetchedFiles, ResolveError> {
    if no_deps || url_pkgs.is_empty() {
        return Ok(PrefetchedFiles::new());
    }
    let (url_dep_names, prefetched) =
        collect_url_wheel_deps(client, url_pkgs).await?;
    merge_unique_dep_names(name_pkgs, url_dep_names);
    Ok(prefetched)
}
