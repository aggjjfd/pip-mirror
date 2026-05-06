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

/// 获取当前运行时环境变量值（PEP 508 规范）。
fn env_value(var: &str) -> Option<String> {
    match var {
        "sys_platform" | "platform_system" => {
            Some(std::env::consts::OS.to_lowercase())
        }
        "platform_machine" => Some(std::env::consts::ARCH.to_lowercase()),
        "os_name" => Some(if std::env::consts::OS == "windows" {
            "nt".to_string()
        } else {
            "posix".to_string()
        }),
        _ => None,
    }
}

/// 评估简单条件 `(变量 ==/!= 值)`。
/// 返回 Some(true)  表示当前平台明确满足。
/// 返回 Some(false) 表示当前平台明确不满足。
/// 返回 None        表示无法评估（保守保留）。
fn eval_simple_condition(cond: &str) -> Option<bool> {
    for op in ["==", "!="] {
        let Some((var, val)) = cond.split_once(op) else {
            continue;
        };
        let current = env_value(var.trim())?;
        let expected = val
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_lowercase();
        let matches = current == expected;
        return Some(if op == "==" { matches } else { !matches });
    }
    None
}

fn any_part_keeps(marker: &str) -> bool {
    marker
        .split(" or ")
        .any(|p| eval_simple_condition(p.trim()) == Some(true))
}

fn any_part_skips(marker: &str) -> bool {
    marker
        .split(" and ")
        .any(|p| eval_simple_condition(p.trim()) == Some(false))
}

/// 判断平台标记在当前运行时是否明确不满足（应跳过）。
/// 支持 `and` / `or` 组合。
fn skip_by_platform(marker: &str) -> bool {
    let m = marker.trim().to_lowercase();

    if m.contains(" or ") {
        return !any_part_keeps(&m);
    }
    if m.contains(" and ") {
        return any_part_skips(&m);
    }
    eval_simple_condition(&m) == Some(false)
}

fn should_skip_marker(marker: &str) -> bool {
    let m = marker.trim().to_lowercase();
    // extras 由调用方通过 extras 参数控制，这里跳过
    m.contains("extra ==")
        || m.contains("extra!=")
        || m.contains("extra in ")
        || m.contains("extra not in")
        || skip_by_platform(marker)
}

pub fn parse_python_requires(lines: &[String]) -> Vec<DepSpec> {
    let mut deps = Vec::new();
    for line in lines {
        let req = line.split(';').next().unwrap_or(line).trim();
        let marker = line.split(';').nth(1);
        if marker.is_some_and(should_skip_marker) {
            continue;
        }
        let (name, spec) = split_name_spec(req);
        let clean = crate::filters::normalize_package_name(
            name.split('[').next().unwrap_or(name),
        );
        deps.push((clean.to_string(), spec.to_string()));
    }
    deps
}
