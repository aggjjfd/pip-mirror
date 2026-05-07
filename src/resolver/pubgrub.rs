use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use pep440_rs::Version;
use pubgrub::Range;

// ── dep spec helpers ──

pub type DepSpec = (String, String);

pub fn extract_extras(package_ref: &str) -> (String, HashSet<String>) {
    match package_ref.split_once('[') {
        None => (package_ref.to_string(), HashSet::new()),
        Some((name, rest)) => {
            let extras: HashSet<_> = rest
                .strip_suffix(']')
                .unwrap_or(rest)
                .split(',')
                .map(|e| e.trim().to_string())
                .filter(|e| !e.is_empty())
                .collect();
            (name.to_string(), extras)
        }
    }
}

pub fn collect_pkg_extras(
    packages: &[String],
) -> std::collections::HashMap<String, HashSet<String>> {
    let mut pkg_extras = HashMap::new();
    for pkg_ref in packages {
        let (name, extras) = extract_extras(pkg_ref);
        if !extras.is_empty() {
            pkg_extras
                .insert(crate::filters::normalize_package_name(&name), extras);
        }
    }
    pkg_extras
}

pub fn bare_name(package_ref: &str) -> String {
    let name = package_ref.split_once('[').map_or(package_ref, |(n, _)| n);
    crate::filters::normalize_package_name(name)
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
