use url::Url;

use crate::config::PackageUrlSpec;
use crate::downloader::{Downloadable, DownloadableItem, ExplicitWheel};
use crate::resolver::resolve::ResolveError;

/// Maximum wheel size allowed for remote metadata extraction (200 MiB).
/// Wheels larger than this must have their dependencies declared manually.
pub const MAX_REMOTE_WHEEL_BYTES: u64 = 200 * 1024 * 1024;

pub fn split_package_specs(
    pkgs: &[crate::config::PackageSpec],
) -> (Vec<String>, Vec<crate::config::PackageUrlSpec>) {
    let mut names = Vec::new();
    let mut urls = Vec::new();
    for p in pkgs {
        match p {
            crate::config::PackageSpec::Name(n) => names.push(n.clone()),
            crate::config::PackageSpec::Url(u) => urls.push(u.clone()),
        }
    }
    (names, urls)
}

/// Build a [`DownloadableItem::Explicit`] from a user-supplied URL wheel spec.
pub fn explicit_wheel_from_spec(
    spec: &PackageUrlSpec,
) -> Result<DownloadableItem, ResolveError> {
    let parsed =
        crate::wheel_url::parse_wheel_url(&spec.url, spec.sha256.clone())
            .map_err(|e| {
                ResolveError::Config(format!(
                    "URL whl 解析失败 ({}): {e}",
                    crate::redact::redact_url_for_display(&spec.url)
                ))
            })?;
    let wheel = ExplicitWheel {
        filename: parsed.filename,
        url: parsed.url,
        sha256: parsed.sha256,
        package_name: parsed.package_name,
        version: parsed.version,
    };
    Ok(DownloadableItem::Explicit(wheel))
}

fn apply_duplicate_resolution(
    files: &[DownloadableItem],
    prev_idx: usize,
    new_idx: usize,
    chosen: &mut std::collections::HashMap<(String, String), usize>,
    key: (String, String),
) {
    let prev = &files[prev_idx];
    let new = &files[new_idx];
    if prev.is_explicit_url() {
        // Existing explicit URL takes precedence; drop the duplicate.
        return;
    }
    if new.is_explicit_url() {
        tracing::warn!(
            "显式 URL wheel 覆盖同名 PyPI 结果: {} {} (url: {})",
            new.package_name(),
            new.filename(),
            crate::redact::redact_url_for_display(new.source_url())
        );
        chosen.insert(key, new_idx);
        return;
    }
    tracing::warn!(
        "忽略重复的 wheel 文件: {} {} (url: {})",
        new.package_name(),
        new.filename(),
        crate::redact::redact_url_for_display(new.source_url())
    );
}

pub fn dedupe_planned_files(files: &mut Vec<DownloadableItem>) {
    let mut chosen: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();
    for (idx, fi) in files.iter().enumerate() {
        let key = (fi.package_name().to_string(), fi.filename().to_string());
        if let Some(&prev_idx) = chosen.get(&key) {
            apply_duplicate_resolution(files, prev_idx, idx, &mut chosen, key);
            continue;
        }
        chosen.insert(key, idx);
    }
    let allowed: std::collections::HashSet<usize> =
        chosen.into_values().collect();
    let mut idx = 0_usize;
    files.retain(|_| {
        let keep = allowed.contains(&idx);
        idx += 1;
        keep
    });
}

pub fn dedupe_solved_versions(
    solved: &mut dashmap::DashMap<String, Vec<pep440_rs::Version>>,
) {
    for mut entry in solved.iter_mut() {
        entry.value_mut().sort_by(|a, b| b.cmp(a));
        entry.value_mut().dedup();
    }
}

pub fn resolve_file_url(
    url_str: &str,
) -> Result<std::path::PathBuf, ResolveError> {
    let url = Url::parse(url_str).map_err(|e| {
        ResolveError::Config(format!(
            "无效的 file URL ({}): {e}",
            crate::redact::redact_url_for_display(url_str)
        ))
    })?;
    url.to_file_path().map_err(|_| {
        ResolveError::Config(format!(
            "无法将 file URL 转换为路径: {}",
            crate::redact::redact_url_for_display(url_str)
        ))
    })
}

async fn read_local_wheel_bytes(
    url: &str,
    path: &std::path::Path,
) -> Result<Vec<u8>, ResolveError> {
    let metadata = tokio::fs::metadata(path).await.map_err(|e| {
        ResolveError::Config(format!(
            "读取本地文件失败 {}: {e}",
            path.display()
        ))
    })?;
    if metadata.len() > MAX_REMOTE_WHEEL_BYTES {
        return Err(ResolveError::Config(format!(
            "本地 whl 文件过大 ({}): 超过 {} 字节；请手动声明其依赖",
            crate::redact::redact_url_for_display(url),
            MAX_REMOTE_WHEEL_BYTES
        )));
    }
    tokio::fs::read(path).await.map_err(|e| {
        ResolveError::Config(format!(
            "读取本地文件失败 {}: {e}",
            path.display()
        ))
    })
}

async fn extract_requires_dist_blocking(
    bytes: Vec<u8>,
    url: String,
    expected: String,
) -> Result<Vec<String>, ResolveError> {
    tokio::task::spawn_blocking(move || {
        crate::wheel_metadata::extract_requires_dist_from_bytes(
            &bytes, &expected,
        )
        .map_err(|e| {
            ResolveError::Config(format!(
                "读取 {} 的 METADATA 失败: {e}",
                crate::redact::redact_url_for_display(&url)
            ))
        })
    })
    .await
    .map_err(|e| ResolveError::Config(format!("本地 whl 解析线程错误: {e}")))?
}

pub async fn read_local_wheel_deps(
    url: &str,
    expected_dist_info_path: &str,
    expected_sha256: Option<&str>,
) -> Result<Vec<String>, ResolveError> {
    let path = resolve_file_url(url)?;
    let bytes = read_local_wheel_bytes(url, &path).await?;
    if expected_sha256
        .as_ref()
        .is_some_and(|exp| !sha256_matches(&bytes, exp))
    {
        return Err(ResolveError::Config(format!(
            "{} 的 sha256 校验失败",
            crate::redact::redact_url_for_display(url)
        )));
    }
    extract_requires_dist_blocking(
        bytes,
        url.to_string(),
        expected_dist_info_path.to_string(),
    )
    .await
}

pub use crate::sync::url_wheel_download::*;
