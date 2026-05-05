use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use dashmap::DashMap;
use pep440_rs::Version;
use pubgrub::Ranges;

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
    let Some(idx) = versions.iter().position(|v| v == sol_ver) else {
        ctx.result
            .entry(pkg.to_string())
            .or_default()
            .push(sol_ver.clone());
        return;
    };
    let start = idx.saturating_sub(ctx.half);
    let end = (idx + ctx.half + 1).min(versions.len());
    let mut set = ctx.result.entry(pkg.clone()).or_default();
    for v in &versions[start..end] {
        if !set.contains(v) {
            set.push(v.clone());
        }
    }
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
        let v: &mut Vec<Version> = entry.value_mut();
        v.sort_by(|a, b| b.cmp(a));
        v.dedup();
    }
    result
}

// ── pubgrub solver ──

pub type DepSpec = (String, String);

pub fn dep_to_range(
    dep_name: &str,
    spec: &str,
    versions: &HashMap<String, Vec<Version>>,
) -> Option<(String, Ranges<u32>)> {
    let vers = versions.get(dep_name).map(|v| v.as_slice()).unwrap_or(&[]);
    spec_to_range(vers, spec).map(|r| (dep_name.to_string(), r))
}

pub fn parse_specs(spec: &str) -> Option<Vec<pep440_rs::VersionSpecifier>> {
    if spec.is_empty() {
        return None;
    }
    let specs: Vec<_> = spec
        .split(',')
        .filter_map(|s| {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                pep440_rs::VersionSpecifier::from_str(t).ok()
            }
        })
        .collect();
    if specs.is_empty() { None } else { Some(specs) }
}

pub fn spec_to_range(versions: &[Version], spec: &str) -> Option<Ranges<u32>> {
    let specs = match parse_specs(spec) {
        Some(s) => s,
        None => return Some(Ranges::full()),
    };
    let mut range: Option<Ranges<u32>> = None;
    for (idx, ver) in versions.iter().enumerate() {
        if !specs.iter().all(|s| s.contains(ver)) {
            continue;
        }
        let s = Ranges::singleton(u32::try_from(idx).ok()?);
        range = Some(match range {
            None => s,
            Some(r) => r.union(&s),
        });
    }
    range
}

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
    package_ref
        .split_once('[')
        .map_or(package_ref.to_string(), |(n, _)| n.to_string())
}

pub fn parse_python_requires(lines: &[String]) -> Vec<DepSpec> {
    let mut deps = Vec::new();
    for line in lines {
        if line.contains("extra ==") {
            continue;
        }
        let req = line.split(';').next().unwrap_or(line).trim();
        let (name, spec) = match req.split_once(' ') {
            Some((n, s)) => (n.to_string(), s.to_string()),
            None => (req.to_string(), String::new()),
        };
        let clean = name.split('[').next().unwrap_or(&name).to_string();
        deps.push((clean, spec));
    }
    deps
}
