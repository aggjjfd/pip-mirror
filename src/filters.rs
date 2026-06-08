use std::collections::HashSet;

use url::Url;

use crate::downloader::FileInfo;
use crate::resolver::types::TargetEnv;

/// Strip query, fragment and userinfo from a URL for safe logging/errors.
///
/// If the string cannot be parsed as a URL, returns a placeholder instead of
/// echoing the original input, so malformed URLs that contain credentials are
/// not leaked in error messages.
pub fn redact_url_for_display(url: &str) -> String {
    match Url::parse(url) {
        Ok(mut parsed) => {
            parsed.set_query(None);
            parsed.set_fragment(None);
            parsed.set_username("").ok();
            parsed.set_password(None).ok();
            if parsed.host_str().is_none()
                && parsed.scheme() != "file"
                && url.contains('@')
            {
                return "<无法解析的 URL>".to_string();
            }
            parsed.to_string()
        }
        Err(_) => "<无法解析的 URL>".to_string(),
    }
}

static REJECTED_SUBSTRINGS: &[&str] = &[
    "aarch64",
    "arm64",
    "armv",
    "arm_",
    "armhf",
    "armel",
    "arm32",
    "musllinux",
    "macosx",
    "s390x",
    "ppc64le",
    "ppc64",
    "riscv64",
    "wasm32",
];

/// Parse a manylinux tag into its (major, minor) glibc version.
///
/// Supported forms:
/// - manylinux1_x86_64        → (2, 5)
/// - manylinux2010_x86_64     → (2, 12)
/// - manylinux2014_x86_64     → (2, 17)
/// - manylinux_2_28_x86_64    → (2, 28)
/// - manylinux_2_39_x86_64    → (2, 39)
fn parse_manylinux_glibc(tag: &str) -> Option<(u32, u32)> {
    if tag.contains("manylinux1_") {
        return Some((2, 5));
    }
    if tag.contains("manylinux2010_") {
        return Some((2, 12));
    }
    if tag.contains("manylinux2014_") {
        return Some((2, 17));
    }
    // manylinux_2_X_y → (2, X)
    if let Some(start) = tag.find("manylinux_2_") {
        let rest = &tag[start + "manylinux_2_".len()..];
        if let Some(end) = rest.find('_') {
            let minor: u32 = rest[..end].parse().ok()?;
            return Some((2, minor));
        }
    }
    None
}

fn parse_glibc_version(s: &str) -> Option<(u32, u32)> {
    let (major, minor) = s.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

const DEFAULT_MAX_GLIBC: (u32, u32) = (2, 39);
const TARGET_LINUX_X86_64: &str = "linux_x86_64";
const TARGET_WIN32: &str = "win32";
const TARGET_WIN_AMD64: &str = "win_amd64";

/// Check whether a wheel filename is acceptable under the given glibc limit.
///
/// A wheel is accepted if any of its platform tags is:
/// - "any"
/// - "win32" / "win_amd64"
/// - "linux_x86_64"
/// - a manylinux_x86_64 tag with glibc <= max_glibc
///
/// A wheel is rejected if any tag contains a rejected substring
/// (e.g. musllinux, aarch64, macosx).
fn parse_and_filter_tags(filename: &str) -> Option<Vec<&str>> {
    let sub_tags = parse_wheel_platform(filename)?;
    if sub_tags
        .iter()
        .any(|tag| REJECTED_SUBSTRINGS.iter().any(|r| tag.contains(r)))
    {
        return None;
    }
    Some(sub_tags)
}

fn is_accepted_wheel_impl(filename: &str, max_glibc: (u32, u32)) -> bool {
    let Some(sub_tags) = parse_and_filter_tags(filename) else {
        return false;
    };
    sub_tags.iter().any(|tag| {
        matches!(*tag, "any" | "win32" | "win_amd64" | "linux_x86_64")
            || (tag.contains("manylinux")
                && tag.contains("x86_64")
                && parse_manylinux_glibc(tag)
                    .is_some_and(|glibc| glibc <= max_glibc))
    })
}

/// Check whether a wheel is globally acceptable (using default max glibc 2.39).
pub fn is_accepted_wheel(filename: &str) -> bool {
    is_accepted_wheel_impl(filename, DEFAULT_MAX_GLIBC)
}

/// Check whether a wheel is acceptable under a custom glibc limit.
pub fn is_accepted_wheel_with_glibc(filename: &str, max_glibc: &str) -> bool {
    let glibc = parse_glibc_version(max_glibc).unwrap_or(DEFAULT_MAX_GLIBC);
    is_accepted_wheel_impl(filename, glibc)
}

/// Parse the platform tag(s) from a wheel filename.
/// Returns `None` if the file is not a `.whl`.
mod wheel_abi;
pub use wheel_abi::{parse_wheel_platform, python_abi_matches_target};

/// Map a wheel platform sub-tag → set of target platform names.
fn map_sub_tag(sub: &str, covered: &mut HashSet<&'static str>) {
    match sub {
        "win32" => {
            covered.insert("win32");
        }
        "win_amd64" => {
            covered.insert("win_amd64");
        }
        s if s.contains("x86_64") || s.contains("_x86_64") => {
            covered.insert("linux_x86_64");
        }
        _ => {}
    }
}

pub fn platform_to_target(tag: &str) -> HashSet<&'static str> {
    if tag == "any" {
        return ["win32", "win_amd64", "linux_x86_64"].into();
    }
    let mut covered = HashSet::new();
    for sub in tag.split('.') {
        map_sub_tag(sub, &mut covered);
    }
    covered
}

pub fn is_pure_python_wheel(filename: &str) -> bool {
    let Some(sub_tags) = parse_wheel_platform(filename) else {
        return false;
    };
    sub_tags.contains(&"any")
}

pub fn is_source_distribution(filename: &str) -> bool {
    filename.ends_with(".tar.gz")
        || filename.ends_with(".zip")
        || filename.ends_with(".tar.bz2")
        || filename.ends_with(".tar.xz")
}

pub fn sdist_fallback_allowed(
    files: &[FileInfo],
    include_source: bool,
) -> bool {
    include_source && files.iter().any(|f| is_source_distribution(&f.filename))
}

fn target_platform_key(target: &TargetEnv) -> Option<&'static str> {
    match (target.sys_platform(), target.platform_machine()) {
        ("linux", "x86_64") => Some(TARGET_LINUX_X86_64),
        ("win32", "x86") => Some(TARGET_WIN32),
        ("win32", "AMD64") => Some(TARGET_WIN_AMD64),
        _ => None,
    }
}

fn tag_matches_target(
    tag: &str,
    target_key: &str,
    max_glibc: (u32, u32),
) -> bool {
    match target_key {
        TARGET_LINUX_X86_64 => {
            matches!(tag, "any" | "linux_x86_64")
                || (tag.contains("manylinux")
                    && tag.contains("x86_64")
                    && parse_manylinux_glibc(tag)
                        .is_some_and(|glibc| glibc <= max_glibc))
        }
        TARGET_WIN32 => matches!(tag, "any" | "win32"),
        TARGET_WIN_AMD64 => matches!(tag, "any" | "win_amd64"),
        _ => false,
    }
}

fn wheel_matches_target(
    filename: &str,
    target_key: &str,
    max_glibc: (u32, u32),
) -> bool {
    let Some(sub_tags) = parse_and_filter_tags(filename) else {
        return false;
    };
    sub_tags
        .iter()
        .any(|tag| tag_matches_target(tag, target_key, max_glibc))
}

fn push_normalized(result: &mut String, ch: char, prev_was_sep: &mut bool) {
    let lc = ch.to_ascii_lowercase();
    if lc == '_' || lc == '.' || lc == '-' {
        if !*prev_was_sep {
            *prev_was_sep = true;
        }
        return;
    }
    if *prev_was_sep && !result.is_empty() {
        result.push('-');
    }
    result.push(lc);
    *prev_was_sep = false;
}

pub fn normalize_package_name(name: &str) -> String {
    let bare = name.split_once('[').map_or(name, |(n, _)| n);
    let mut result = String::with_capacity(bare.len());
    let mut prev_was_sep = true;
    for ch in bare.chars() {
        push_normalized(&mut result, ch, &mut prev_was_sep);
    }
    result
}

pub fn wheel_is_installable_for_target(
    filename: &str,
    target: &TargetEnv,
    max_glibc: &str,
) -> bool {
    if !python_abi_matches_target(filename, target) {
        return false;
    }
    let Some(target_key) = target_platform_key(target) else {
        return false;
    };
    let glibc = parse_glibc_version(max_glibc).unwrap_or(DEFAULT_MAX_GLIBC);
    wheel_matches_target(filename, target_key, glibc)
}

pub fn version_is_installable_for_target(
    files: &[FileInfo],
    target: &TargetEnv,
    include_source: bool,
    max_glibc: &str,
) -> bool {
    files.iter().any(|file| {
        file.filename.ends_with(".whl")
            && wheel_is_installable_for_target(
                &file.filename,
                target,
                max_glibc,
            )
    }) || sdist_fallback_allowed(files, include_source)
}

// ── file selection helpers ──

fn wheel_is_download_candidate(
    fi: &FileInfo,
    targets: &[TargetEnv],
    max_glibc: &str,
    glibc: (u32, u32),
) -> bool {
    fi.filename.ends_with(".whl")
        && is_accepted_wheel_impl(&fi.filename, glibc)
        && targets.iter().any(|target| {
            wheel_is_installable_for_target(&fi.filename, target, max_glibc)
        })
}

/// Select files for a single package@version under the given policy.
///
/// Returns the kept wheels; if no wheel is kept and `include_source` is true,
/// falls back to the sdist.
pub fn select_files_for_version(
    files: &[FileInfo],
    targets: &[TargetEnv],
    include_source: bool,
    max_glibc: &str,
) -> Vec<FileInfo> {
    let glibc = parse_glibc_version(max_glibc).unwrap_or(DEFAULT_MAX_GLIBC);

    let mut kept_wheels = Vec::new();
    for fi in files {
        if wheel_is_download_candidate(fi, targets, max_glibc, glibc) {
            kept_wheels.push(fi.clone());
        }
    }

    if !kept_wheels.is_empty() {
        // Deduplicate by filename (a wheel may have multiple tags).
        let mut seen = HashSet::new();
        kept_wheels.retain(|fi| seen.insert(fi.filename.clone()));
        return kept_wheels;
    }

    if !include_source {
        return Vec::new();
    }
    if sdist_fallback_allowed(files, include_source) {
        return files
            .iter()
            .filter(|f| is_source_distribution(&f.filename))
            .cloned()
            .collect();
    }
    Vec::new()
}
