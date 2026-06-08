use std::io::{Cursor, Read};
use std::path::Path;

/// Maximum number of entries allowed in a wheel zip archive during metadata
/// extraction. This prevents zip bombs with huge central directories from
/// consuming excessive CPU.
pub const MAX_ZIP_ENTRIES: usize = 10_000;

/// Maximum uncompressed size allowed for a METADATA file.
pub const MAX_METADATA_BYTES: u64 = 10 * 1024 * 1024;

/// Extract `Requires-Dist` entries from a local wheel file.
///
/// `expected_dist_info_path` must be the exact METADATA entry path derived
/// from the wheel filename (e.g. `my_pkg-1.0.dist-info/METADATA`) to
/// prevent a malicious archive from injecting a forged METADATA entry.
pub fn extract_requires_dist_from_wheel(
    path: &Path,
    expected_dist_info_path: &str,
) -> Result<Vec<String>, String> {
    let file = std::fs::File::open(path)
        .map_err(|e| format!("打开 whl 文件失败 {}: {e}", path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("zip 解包失败: {e}"))?;
    let metadata = find_metadata(&mut archive, expected_dist_info_path)?;
    Ok(parse_requires_dist(&metadata))
}

/// Extract `Requires-Dist` entries from wheel bytes (for prefetched remote wheels).
pub fn extract_requires_dist_from_bytes(
    bytes: &[u8],
    expected_dist_info_path: &str,
) -> Result<Vec<String>, String> {
    let reader = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| format!("zip 解包失败: {e}"))?;
    let metadata = find_metadata(&mut archive, expected_dist_info_path)?;
    Ok(parse_requires_dist(&metadata))
}

fn find_metadata<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    expected_dist_info_path: &str,
) -> Result<String, String> {
    let len = archive.len();
    if len > MAX_ZIP_ENTRIES {
        return Err(format!("whl 中 zip 条目过多: {len} > {MAX_ZIP_ENTRIES}"));
    }
    for index in 0..len {
        let mut file = archive
            .by_index(index)
            .map_err(|e| format!("zip 读取条目失败: {e}"))?;
        if file.name() != expected_dist_info_path {
            continue;
        }
        return read_metadata_limited(&mut file);
    }
    Err(format!(
        "whl 中未找到预期的 METADATA 条目: {expected_dist_info_path}"
    ))
}

fn read_metadata_limited(
    file: &mut zip::read::ZipFile<'_>,
) -> Result<String, String> {
    let mut limited = file.take(MAX_METADATA_BYTES);
    let mut content = String::new();
    let bytes_read = limited
        .read_to_string(&mut content)
        .map_err(|e| format!("读取 METADATA 失败: {e}"))?;
    if bytes_read as u64 >= MAX_METADATA_BYTES {
        return Err(format!(
            "METADATA 文件过大: 达到 {MAX_METADATA_BYTES} 字节上限"
        ));
    }
    Ok(content)
}

fn append_continuation(cur: &mut String, line: &str) {
    cur.push(' ');
    cur.push_str(line.trim_start());
}

fn maybe_append_continuation(current: &mut Option<String>, line: &str) {
    if let Some(cur) = current.as_mut() {
        append_continuation(cur, line);
    }
}

fn flush_requires_dist(result: &mut Vec<String>, current: &mut Option<String>) {
    let Some(cur) = current.take() else {
        return;
    };
    if let Some(value) = requires_dist_value(&cur) {
        result.push(value);
    }
}

/// Parse RFC 822 style `Requires-Dist` headers, handling case-insensitive
/// field names and line continuations (lines starting with whitespace
/// continue the previous header value).
fn parse_requires_dist(metadata: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current: Option<String> = None;

    for line in metadata.lines() {
        if line.starts_with(char::is_whitespace) {
            maybe_append_continuation(&mut current, line);
            continue;
        }

        flush_requires_dist(&mut result, &mut current);

        if !line.is_empty() {
            current = Some(line.to_string());
        }
    }

    flush_requires_dist(&mut result, &mut current);
    result
}

fn requires_dist_value(line: &str) -> Option<String> {
    let (key, value) = line.split_once(':')?;
    if key.trim().eq_ignore_ascii_case("Requires-Dist") {
        Some(value.trim().to_string())
    } else {
        None
    }
}

fn is_direct_url_req(req: &pep508_rs::Requirement) -> bool {
    matches!(req.version_or_url, Some(pep508_rs::VersionOrUrl::Url(_)))
}

fn try_redact_whole_url(token: &str) -> Option<String> {
    if url::Url::parse(token).is_ok() {
        Some(crate::filters::redact_url_for_display(token))
    } else {
        None
    }
}

fn try_redact_scheme_url(token: &str) -> Option<String> {
    let pos = token.find("://")?;
    let url_start = token[..pos]
        .rfind(|c: char| {
            !c.is_ascii_alphanumeric() && c != '+' && c != '-' && c != '.'
        })
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let candidate = &token[url_start..];
    let mut result = token[..url_start].to_string();
    result.push_str(&crate::filters::redact_url_for_display(candidate));
    Some(result)
}

fn try_redact_at_url(token: &str) -> Option<String> {
    if !token.contains('@') {
        return None;
    }
    let with_scheme = format!("https://{token}");
    if url::Url::parse(&with_scheme).is_ok() {
        Some(crate::filters::redact_url_for_display(&with_scheme))
    } else {
        Some("<无法解析的 URL>".to_string())
    }
}

fn strip_query_fragment(token: &str) -> Option<String> {
    if !token.contains('?') && !token.contains('#') {
        return None;
    }
    let end = token.find(['?', '#']).unwrap_or(token.len());
    Some(token[..end].to_string())
}

pub(crate) fn redact_url_in_token(token: &str) -> String {
    try_redact_whole_url(token)
        .or_else(|| try_redact_scheme_url(token))
        .or_else(|| try_redact_at_url(token))
        .or_else(|| strip_query_fragment(token))
        .unwrap_or_else(|| token.to_string())
}

pub fn safe_requires_dist_summary(line: &str) -> String {
    // Redact any URL-looking substrings so that tokens/credentials in
    // direct URL requirements do not end up in logs.
    let mut result = String::new();
    for word in line.split_whitespace() {
        result.push_str(&redact_url_in_token(word));
        result.push(' ');
    }
    if result.ends_with(' ') {
        result.pop();
    }
    result
}

fn push_package_name(
    names: &mut Vec<String>,
    line: &str,
    req: &pep508_rs::Requirement,
) {
    if is_direct_url_req(req) {
        tracing::warn!(
            "跳过直接 URL 依赖（请手动加入 packages）: {}",
            safe_requires_dist_summary(line)
        );
        return;
    }
    names.push(crate::filters::normalize_package_name(req.name.as_ref()));
}

/// Extract bare package names from a list of `Requires-Dist` lines.
/// Extra/marker information is intentionally ignored; we want the dependency
/// to be mirrored regardless of conditions so the offline mirror is as
/// complete as possible.
///
/// Direct URL requirements (`pkg @ https://...`) are skipped with a warning
/// because the resolver cannot pull them from PyPI. Users should add such
/// URLs explicitly to the `packages` list.
pub fn extract_package_names(requires_dist: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    for line in requires_dist {
        match line.parse::<pep508_rs::Requirement>() {
            Ok(req) => push_package_name(&mut names, line, &req),
            Err(e) => {
                tracing::warn!(
                    "无法解析 Requires-Dist，已跳过: {} (错误: {})",
                    safe_requires_dist_summary(line),
                    e.message
                );
            }
        }
    }
    names
}
