use std::collections::HashSet;

use crate::downloader::FileInfo;
use crate::resolver::types::TargetEnv;

// ── platform rejection ──

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

fn sub_tag_rejected(sub: &str) -> bool {
    REJECTED_SUBSTRINGS.iter().any(|r| sub.contains(r))
}

// ── glibc version parsing ──

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
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 2 {
        return None;
    }
    let major: u32 = parts[0].parse().ok()?;
    let minor: u32 = parts[1].parse().ok()?;
    Some((major, minor))
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
fn is_accepted_wheel_impl(filename: &str, max_glibc: (u32, u32)) -> bool {
    let Some(sub_tags) = parse_wheel_platform(filename) else {
        return false;
    };

    // Reject if any tag contains a banned substring.
    if sub_tags.iter().any(|t| sub_tag_rejected(t)) {
        return false;
    }

    // Accept if any tag is explicitly allowed.
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

// ── wheel platform parsing ──

/// Parse the platform tag(s) from a wheel filename.
/// Returns `None` if the file is not a `.whl`.
pub fn parse_wheel_platform(filename: &str) -> Option<Vec<&str>> {
    if !filename.ends_with(".whl") {
        return None;
    }
    let stem = &filename[..filename.len() - 4];
    let parts: Vec<&str> = stem.split('-').collect();
    (parts.len() >= 5).then(|| parts[parts.len() - 1].split('.').collect())
}

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

fn target_platform_key(target: &TargetEnv) -> Option<&'static str> {
    match (
        target.sys_platform.as_str(),
        target.platform_machine.as_str(),
    ) {
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
    let Some(sub_tags) = parse_wheel_platform(filename) else {
        return false;
    };
    if sub_tags.iter().any(|tag| sub_tag_rejected(tag)) {
        return false;
    }
    sub_tags
        .iter()
        .any(|tag| tag_matches_target(tag, target_key, max_glibc))
}

pub fn normalize_package_name(name: &str) -> String {
    let bare = name.split_once('[').map_or(name, |(n, _)| n);
    bare.to_lowercase()
        .replace(['_', '.'], "-")
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

pub fn wheel_is_installable_for_target(
    filename: &str,
    target: &TargetEnv,
    max_glibc: &str,
) -> bool {
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
    if files.iter().any(|file| {
        file.filename.ends_with(".whl")
            && wheel_is_installable_for_target(
                &file.filename,
                target,
                max_glibc,
            )
    }) {
        return true;
    }

    include_source
        && files
            .iter()
            .any(|file| is_source_distribution(&file.filename))
}

// ── file selection helpers ──

/// Select files for a single package@version under the given policy.
///
/// Returns the kept wheels; if no wheel is kept and `include_source` is true,
/// falls back to the sdist.
pub fn select_files_for_version(
    files: &[FileInfo],
    include_source: bool,
    max_glibc: &str,
) -> Vec<FileInfo> {
    let glibc = parse_glibc_version(max_glibc).unwrap_or(DEFAULT_MAX_GLIBC);

    let mut kept_wheels = Vec::new();
    for fi in files {
        if fi.filename.ends_with(".whl")
            && is_accepted_wheel_impl(&fi.filename, glibc)
        {
            kept_wheels.push(fi.clone());
        }
    }

    if !kept_wheels.is_empty() {
        // Deduplicate by filename (a wheel may have multiple tags).
        let mut seen = HashSet::new();
        kept_wheels.retain(|fi| seen.insert(fi.filename.clone()));
        return kept_wheels;
    }

    if include_source {
        files
            .iter()
            .filter(|fi| is_source_distribution(&fi.filename))
            .cloned()
            .collect()
    } else {
        Vec::new()
    }
}
