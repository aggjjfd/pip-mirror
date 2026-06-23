use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use pep440_rs::Version;
use pubgrub::Range;

use crate::filters::{
    ParsedPackageRef, normalize_package_name, parse_package_ref,
};

// ── dep spec helpers ──

pub type DepSpec = (String, String);

pub fn extract_extras(package_ref: &str) -> (String, HashSet<String>) {
    match parse_package_ref(package_ref) {
        Ok(parsed) => (parsed.name, parsed.extras),
        Err(_) => (package_ref.to_string(), HashSet::new()),
    }
}

pub fn collect_pkg_refs(
    packages: &[String],
) -> HashMap<String, ParsedPackageRef> {
    let mut pkg_refs = HashMap::new();
    for pkg_ref in packages {
        if let Ok(parsed) = parse_package_ref(pkg_ref) {
            pkg_refs.insert(parsed.name.clone(), parsed);
        }
    }
    pkg_refs
}

pub fn bare_name(package_ref: &str) -> String {
    let name = package_ref.split_once('[').map_or(package_ref, |(n, _)| n);
    normalize_package_name(name)
}

// ── version range helpers ──

fn split_operator(s: &str) -> Option<(&str, &str)> {
    for (prefix, canonical) in &[
        (">=", ">="),
        ("<=", "<="),
        ("!=", "!="),
        ("~=", "~="),
        ("==", "=="),
        (">", ">"),
        ("<", "<"),
        ("=", "=="),
    ] {
        if let Some(rest) = s.trim().strip_prefix(prefix) {
            return Some((canonical, rest.trim()));
        }
    }
    None
}

fn bump_release(release: &[u64], idx: usize) -> Version {
    let mut parts = if idx < release.len() {
        release[..=idx].to_vec()
    } else {
        let mut v = release.to_vec();
        v.resize(idx + 1, 0);
        v
    };
    parts[idx] += 1;
    Version::from_str(
        &parts
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join("."),
    )
    .unwrap_or_else(|_| Version::new([0, 0, 0]))
}

pub fn compatible_range(ver: Version) -> Range<Version> {
    let release = ver.release();
    if release.len() >= 3 {
        let upper = bump_release(release, release.len() - 2);
        Range::higher_than(ver).intersection(&Range::strictly_lower_than(upper))
    } else {
        let upper = bump_release(release, 0);
        Range::higher_than(ver).intersection(&Range::strictly_lower_than(upper))
    }
}

fn op_to_range(op: &str, ver: Version) -> Range<Version> {
    match op {
        ">=" => Range::higher_than(ver),
        ">" => Range::strictly_higher_than(ver),
        "<=" => Range::lower_than(ver),
        "<" => Range::strictly_lower_than(ver),
        "==" => Range::singleton(ver),
        "~=" => compatible_range(ver),
        _ => Range::full(),
    }
}

pub fn spec_to_range(spec: &str) -> Range<Version> {
    if spec.is_empty() {
        return Range::full();
    }
    let mut range = Range::full();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((op, ver_str)) = split_operator(part) else {
            continue;
        };
        let Ok(ver) = Version::from_str(ver_str) else {
            continue;
        };
        if op == "!=" {
            range = range.intersection(
                &Range::strictly_lower_than(ver.clone())
                    .union(&Range::strictly_higher_than(ver)),
            );
            continue;
        }
        range = range.intersection(&op_to_range(op, ver));
    }
    range
}

/// 严格校验用户指定的版本约束字符串。
///
/// 与 `spec_to_range` 不同：遇到任何无效操作符或无效版本号都会返回 Err，
/// 而不是静默跳过。用于配置加载阶段，避免用户写错约束被当作无约束处理。
pub fn validate_version_spec(spec: &str) -> Result<(), String> {
    if spec.is_empty() {
        return Err("版本约束不能为空".to_string());
    }
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err("版本约束中有多余逗号".to_string());
        }
        let Some((op, ver_str)) = split_operator(part) else {
            return Err(format!("无效版本操作符: {part}"));
        };
        let ver_str = ver_str.trim();
        if ver_str.is_empty() {
            return Err(format!("{op} 后缺少版本号"));
        }
        if Version::from_str(ver_str).is_err() {
            return Err(format!("无效版本号: {ver_str}"));
        }
    }
    Ok(())
}
