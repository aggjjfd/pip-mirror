use std::collections::HashSet;
use std::str::FromStr;

use crate::resolver::types::TargetEnv;
use pep440_rs::VersionSpecifiers;

pub mod file;
pub use file::ResolvedFile;

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
    files: &[ResolvedFile],
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
    files: &[ResolvedFile],
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
    fi: &ResolvedFile,
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
    files: &[ResolvedFile],
    targets: &[TargetEnv],
    include_source: bool,
    max_glibc: &str,
) -> Vec<ResolvedFile> {
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

/// 解析后的包引用，包含归一化名称、extras 集合和可选版本约束。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPackageRef {
    pub name: String,
    pub extras: HashSet<String>,
    pub version_spec: Option<String>,
}

/// 从包引用字符串中提取 `[extras]`，返回剩余包名部分（含版本约束）和 extras 集合。
fn parse_extras_and_rest(
    raw: &str,
) -> Result<(String, HashSet<String>), String> {
    let Some((name, rest)) = raw.split_once('[') else {
        return Ok((raw.to_string(), HashSet::new()));
    };
    let (extras_str, after_bracket) = rest
        .split_once(']')
        .ok_or_else(|| "缺少右括号 ']'".to_string())?;
    if !after_bracket.is_empty()
        && !after_bracket.starts_with(['>', '<', '=', '!', '~'])
    {
        return Err(format!("']' 后存在非版本约束内容: {after_bracket}"));
    }
    let extras = extras_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Ok((format!("{name}{after_bracket}"), extras))
}

/// 判断字符是否属于包名字符（字母、数字、`_`、`.`、`-`）。
fn is_package_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'
}

/// 把包名部分拆成名称和可选版本约束字符串。
fn split_name_and_spec(
    name_part: &str,
) -> Result<(&str, Option<&str>), String> {
    match name_part.find(|c: char| !is_package_name_char(c)) {
        None => Ok((name_part, None)),
        Some(0) => Err("包名缺失".to_string()),
        Some(idx) => {
            let (name, spec) = name_part.split_at(idx);
            Ok((name, Some(spec.trim())))
        }
    }
}

/// 校验版本约束字符串：不能为空、不能包含空格，且必须是合法的 PEP 440 约束。
fn validate_version_spec_str(spec: &str) -> Result<(), String> {
    if spec.is_empty() {
        return Err("版本约束不能为空".to_string());
    }
    if spec.contains(' ') {
        return Err("版本约束中不允许空格".to_string());
    }
    VersionSpecifiers::from_str(spec)
        .map(|_| ())
        .map_err(|err| format!("无效版本约束: {err}"))
}

/// 解析配置中的包引用字符串。
///
/// 支持格式：
/// - `numpy`
/// - `markitdown[pptx,docx]`
/// - `numpy==2.5.0`
/// - `geopandas[all]==5.0.0`
/// - `numpy>=1.20,<2.0`
///
/// 版本操作符两侧不允许空格。
pub fn parse_package_ref(raw: &str) -> Result<ParsedPackageRef, String> {
    if raw.is_empty() {
        return Err("包引用不能为空".to_string());
    }

    let (name_part, extras) = parse_extras_and_rest(raw)?;
    let (name, spec) = split_name_and_spec(&name_part)
        .map_err(|reason| format!("{reason}: {raw}"))?;
    if let Some(s) = spec {
        validate_version_spec_str(s)?;
    }

    Ok(ParsedPackageRef {
        name: normalize_package_name(name),
        extras,
        version_spec: spec.map(|s| s.to_string()),
    })
}
