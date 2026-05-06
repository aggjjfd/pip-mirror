use std::collections::HashSet;
use std::str::FromStr;

use dashmap::DashMap;
use pep440_rs::Version;
use pubgrub::Range;

/// Result of a per-target dependency resolution.
pub type Solution = DashMap<String, Version>;

// ── version window helpers ──

struct EntryCtx<'a> {
    entry: dashmap::mapref::multiple::RefMulti<'a, String, Version>,
    all_versions: &'a DashMap<String, Vec<Version>>,
    result: &'a DashMap<String, Vec<Version>>,
    half: usize,
}

fn process_solution_entry(ctx: EntryCtx<'_>) {
    let pkg = ctx.entry.key();
    let sol_ver = ctx.entry.value();
    let Some(versions) = ctx.all_versions.get(pkg) else {
        return;
    };
    let mut set = ctx.result.entry(pkg.clone()).or_default();
    let Some(idx) = versions.iter().position(|v| v == sol_ver) else {
        set.push(sol_ver.clone());
        return;
    };
    let start = idx.saturating_sub(ctx.half);
    let end = (idx + ctx.half + 1).min(versions.len());
    set.extend(versions[start..end].iter().cloned());
}

pub fn compute_version_windows(
    target_solutions: &[Solution],
    all_versions: &DashMap<String, Vec<Version>>,
    max_versions: usize,
) -> DashMap<String, Vec<Version>> {
    let half = max_versions / 2;
    let result: DashMap<String, Vec<Version>> = DashMap::new();
    for sol in target_solutions {
        for entry in sol.iter() {
            process_solution_entry(EntryCtx {
                entry,
                all_versions,
                result: &result,
                half,
            });
        }
    }
    for mut entry in result.iter_mut() {
        let v = entry.value_mut();
        v.sort_by(|a, b| b.cmp(a));
        v.dedup();
        v.retain(|ver| !ver.any_prerelease());
    }
    result
}

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

pub fn bare_name(package_ref: &str) -> String {
    let name = package_ref.split_once('[').map_or(package_ref, |(n, _)| n);
    crate::filters::normalize_package_name(name)
}

/// Split a PEP 508 requirement (marker already stripped) into (package_name, version_spec).
fn split_name_spec(req: &str) -> (&str, &str) {
    let rest = req.trim();
    let name_end = rest
        .find(|c: char| !c.is_alphanumeric() && !matches!(c, '-' | '_' | '.'))
        .unwrap_or(rest.len());
    let after_name = if rest.as_bytes().get(name_end) == Some(&b'[') {
        rest[name_end + 1..]
            .find(']')
            .map(|i| name_end + i + 2)
            .unwrap_or(rest.len())
    } else {
        name_end
    };
    (&rest[..after_name], rest[after_name..].trim())
}

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
    } else if release.len() == 2 {
        let upper = bump_release(release, 0);
        Range::higher_than(ver).intersection(&Range::strictly_lower_than(upper))
    } else {
        Range::higher_than(ver)
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

fn has_platform_marker(line: &str) -> bool {
    let Some((_, marker)) = line.split_once(';') else {
        return false;
    };
    let m = marker.trim();
    m.contains("extra")
        || m.contains("sys_platform")
        || m.contains("os_name")
        || m.contains("platform_machine")
        || m.contains("platform_system")
}

pub fn parse_python_requires(lines: &[String]) -> Vec<DepSpec> {
    let mut deps = Vec::new();
    for line in lines {
        if has_platform_marker(line) {
            continue;
        }
        let req = line.split(';').next().unwrap_or(line).trim();
        let (name, spec) = split_name_spec(req);
        let clean = crate::filters::normalize_package_name(
            name.split('[').next().unwrap_or(name),
        );
        deps.push((clean.to_string(), spec.to_string()));
    }
    deps
}
