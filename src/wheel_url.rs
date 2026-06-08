use std::path::PathBuf;
use std::str::FromStr;

use percent_encoding::percent_decode_str;
use url::Url;

#[derive(Debug, Clone)]
pub struct ParsedUrlWheel {
    pub url: String,
    pub filename: String,
    pub package_name: String,
    pub version: String,
    pub sha256: Option<String>,
    /// The expected `.dist-info` directory name inside the wheel archive,
    /// derived from the raw distribution name and version in the wheel
    /// filename. Used to locate the correct METADATA entry securely.
    pub dist_info_dir: String,
}

/// Parse a URL pointing to a `.whl` file and extract package metadata.
///
/// Supported schemes: `http`, `https`, `file`.
/// The filename must conform to PEP 427:
/// `{distribution}-{version}(-{build})?-{python}-{abi}-{platform}.whl`
fn check_scheme(scheme: &str) -> Result<(), String> {
    if scheme != "http" && scheme != "https" && scheme != "file" {
        return Err(format!("不支持的 URL scheme: {scheme}"));
    }
    Ok(())
}

fn extract_filename(url: &str) -> Result<String, String> {
    let parsed = Url::parse(url).map_err(|e| format!("无效的 URL: {e}"))?;
    check_scheme(parsed.scheme())?;

    let path = percent_decode_str(parsed.path())
        .decode_utf8()
        .map_err(|e| format!("URL path 包含非法 UTF-8 编码: {e}"))?
        .to_string();
    let filename = PathBuf::from(&path)
        .file_name()
        .ok_or("URL 中没有文件名")?
        .to_string_lossy()
        .to_string();
    Ok(filename)
}

pub fn parse_wheel_url(
    url: &str,
    sha256: Option<String>,
) -> Result<ParsedUrlWheel, String> {
    let filename = extract_filename(url)?;

    if !filename.to_ascii_lowercase().ends_with(".whl") {
        return Err(format!("URL 必须指向 .whl 文件: {filename}"));
    }

    let (package_name, version, dist_info_dir) =
        parse_wheel_filename(&filename)?;

    Ok(ParsedUrlWheel {
        url: url.to_string(),
        filename,
        package_name,
        version,
        sha256,
        dist_info_dir,
    })
}

fn is_valid_build_tag(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_digit() {
        return false;
    }
    let mut saw_letter = false;
    for c in chars {
        match c {
            _ if c.is_ascii_digit() && saw_letter => return false,
            _ if c.is_ascii_digit() => {}
            _ if c.is_ascii_lowercase() => saw_letter = true,
            _ => return false,
        }
    }
    true
}

fn try_build_tag_form(parts: &[&str]) -> Option<(String, String, String)> {
    if parts.len() < 6 {
        return None;
    }
    let build_part = parts[parts.len() - 4];
    let version_idx = parts.len() - 5;
    let version_candidate = parts[version_idx];
    if !is_valid_build_tag(build_part)
        || pep440_rs::Version::from_str(version_candidate).is_err()
    {
        return None;
    }
    let package_name_raw = parts[..version_idx].join("-");
    let dist_info_dir =
        format!("{package_name_raw}-{version_candidate}.dist-info/METADATA");
    Some((
        crate::filters::normalize_package_name(&package_name_raw),
        version_candidate.to_string(),
        dist_info_dir,
    ))
}

fn try_no_build_tag_form(parts: &[&str]) -> Option<(String, String, String)> {
    if parts.len() < 5 {
        return None;
    }
    let version_idx = parts.len() - 4;
    let version_candidate = parts[version_idx];
    if pep440_rs::Version::from_str(version_candidate).is_err() {
        return None;
    }
    let package_name_raw = parts[..version_idx].join("-");
    let dist_info_dir =
        format!("{package_name_raw}-{version_candidate}.dist-info/METADATA");
    Some((
        crate::filters::normalize_package_name(&package_name_raw),
        version_candidate.to_string(),
        dist_info_dir,
    ))
}

fn parse_wheel_filename(
    filename: &str,
) -> Result<(String, String, String), String> {
    if !filename.to_ascii_lowercase().ends_with(".whl") {
        return Err(format!("无法按 PEP 427 解析 wheel 文件名: {filename}"));
    }
    let suffix_len = ".whl".len();
    let stem = &filename[..filename.len() - suffix_len];
    let parts: Vec<&str> = stem.split('-').collect();

    if let Some(result) = try_build_tag_form(&parts) {
        return Ok(result);
    }
    if let Some(result) = try_no_build_tag_form(&parts) {
        return Ok(result);
    }

    Err(format!("无法从 wheel 文件名中解析出版本: {filename}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_http_wheel_url() {
        let parsed = parse_wheel_url(
            "https://example.com/mypkg-1.0-py3-none-any.whl",
            None,
        )
        .unwrap();
        assert_eq!(parsed.package_name, "mypkg");
        assert_eq!(parsed.version, "1.0");
        assert_eq!(parsed.filename, "mypkg-1.0-py3-none-any.whl");
    }

    #[test]
    fn test_parse_file_wheel_url() {
        let parsed = parse_wheel_url(
            "file:///opt/wheels/my_pkg-2.1.0-cp38-cp38-manylinux_2_17_x86_64.whl",
            Some("abc".to_string()),
        )
        .unwrap();
        assert_eq!(parsed.package_name, "my-pkg");
        assert_eq!(parsed.version, "2.1.0");
        assert_eq!(
            parsed.filename,
            "my_pkg-2.1.0-cp38-cp38-manylinux_2_17_x86_64.whl"
        );
        assert_eq!(parsed.sha256, Some("abc".to_string()));
    }

    #[test]
    fn test_parse_wheel_with_build_tag() {
        let parsed = parse_wheel_url(
            "https://example.com/foo-1.0-1-py3-none-any.whl",
            None,
        )
        .unwrap();
        assert_eq!(parsed.package_name, "foo");
        assert_eq!(parsed.version, "1.0");
    }

    #[test]
    fn test_parse_wheel_with_alphabetic_build_tag() {
        let parsed = parse_wheel_url(
            "https://example.com/foo-1.0-1a-py3-none-any.whl",
            None,
        )
        .unwrap();
        assert_eq!(parsed.package_name, "foo");
        assert_eq!(parsed.version, "1.0");
    }

    #[test]
    fn test_parse_wheel_with_complex_version() {
        let parsed = parse_wheel_url(
            "https://example.com/foo-1.0a1-cp38-cp38-win_amd64.whl",
            None,
        )
        .unwrap();
        assert_eq!(parsed.package_name, "foo");
        assert_eq!(parsed.version, "1.0a1");
    }

    #[test]
    fn test_rejects_non_wheel() {
        assert!(
            parse_wheel_url("https://example.com/foo.tar.gz", None).is_err()
        );
    }

    #[test]
    fn test_rejects_bad_filename() {
        assert!(
            parse_wheel_url("https://example.com/too-few-parts.whl", None)
                .is_err()
        );
    }

    #[test]
    fn test_rejects_unparseable_version() {
        assert!(
            parse_wheel_url(
                "https://example.com/foo-notaversion-py3-none-any.whl",
                None
            )
            .is_err()
        );
    }

    #[test]
    fn test_parse_wheel_with_multi_digit_build_tag() {
        let parsed = parse_wheel_url(
            "https://example.com/foo-1.0-10-py3-none-any.whl",
            None,
        )
        .unwrap();
        assert_eq!(parsed.package_name, "foo");
        assert_eq!(parsed.version, "1.0");
    }

    #[test]
    fn test_parse_wheel_with_multi_digit_letter_build_tag() {
        let parsed = parse_wheel_url(
            "https://example.com/foo-1.0-10a-py3-none-any.whl",
            None,
        )
        .unwrap();
        assert_eq!(parsed.package_name, "foo");
        assert_eq!(parsed.version, "1.0");
    }

    #[test]
    fn test_rejects_invalid_build_tag_with_symbol() {
        // PEP 427 build tags only allow digits followed by lowercase letters.
        assert!(
            parse_wheel_url(
                "https://example.com/foo-1.0-+1-py3-none-any.whl",
                None
            )
            .is_err()
        );
    }

    #[test]
    fn test_rejects_unsupported_scheme() {
        assert!(
            parse_wheel_url("ftp://example.com/foo-1.0-py3-none-any.whl", None)
                .is_err()
        );
    }

    #[test]
    fn test_parse_wheel_url_with_query_and_fragment() {
        let parsed = parse_wheel_url(
            "https://example.com/foo-1.0-py3-none-any.whl?token=abc#frag",
            None,
        )
        .unwrap();
        assert_eq!(parsed.filename, "foo-1.0-py3-none-any.whl");
        assert_eq!(parsed.package_name, "foo");
        assert_eq!(parsed.version, "1.0");
    }

    #[test]
    fn test_parse_wheel_url_with_percent_encoded_filename() {
        let parsed = parse_wheel_url(
            "file:///opt/wheels/foo%20bar-1.0-py3-none-any.whl",
            None,
        )
        .unwrap();
        assert_eq!(parsed.filename, "foo bar-1.0-py3-none-any.whl");
        // normalize_package_name does not collapse spaces; it only replaces
        // underscores/dots/hyphens and lowercases.
        assert_eq!(parsed.package_name, "foo bar");
        assert_eq!(parsed.version, "1.0");
    }
}
