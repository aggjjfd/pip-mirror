use std::collections::HashSet;

use dashmap::DashMap;
use pep440_rs::Version;
use tracing::warn;

use super::types::TargetEnv;

/// Result of a per-target dependency resolution.
pub type Solution = DashMap<String, Version>;

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
            .entry(pkg.clone())
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

/// Compute version windows around all target solutions.
/// For each package, collects all solution versions across targets,
/// and for each solution version, takes `[idx - half, idx + half]`
/// windows in the sorted all-versions list. All windows are unioned.
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

    // dedup and keep descending
    for mut entry in result.iter_mut() {
        let versions: &mut Vec<Version> = entry.value_mut();
        versions.sort_by(|a, b| b.cmp(a));
        versions.dedup();
    }

    result
}

/// Extract package name and extras from a reference like "markitdown[pptx,docx]".
pub fn extract_extras(package_ref: &str) -> (String, HashSet<String>) {
    match package_ref.split_once('[') {
        None => (package_ref.to_string(), HashSet::new()),
        Some((name, rest)) => {
            let extras_part = rest.strip_suffix(']').unwrap_or(rest);
            let extras: HashSet<_> = extras_part
                .split(',')
                .map(|e| e.trim().to_string())
                .filter(|e| !e.is_empty())
                .collect();
            (name.to_string(), extras)
        }
    }
}

/// Strip the extras suffix to get the bare package name.
pub fn bare_name(package_ref: &str) -> String {
    match package_ref.split_once('[') {
        None => package_ref.to_string(),
        Some((name, _)) => name.to_string(),
    }
}

// ── placeholder resolver stub ──

pub struct ResolveParams<'a> {
    pub top_packages: &'a [String],
    pub top_versions: &'a DashMap<String, Vec<Version>>,
    pub pypi_url: &'a str,
    pub workers: usize,
    pub max_depth: usize,
    pub max_versions: usize,
    pub allow_prerelease: bool,
}

/// Resolve dependencies for all targets, returning computed version windows.
///
/// This is a **stub** that returns empty results. The full pubgrub-based
/// resolver will be implemented once the pubgrub API integration is complete.
pub fn resolve_dependencies(params: &ResolveParams<'_>) -> DashMap<String, Vec<Version>> {
    let _ = params.top_versions;
    let _ = params.top_packages;
    warn!("pubgrub resolver not yet implemented — returning empty dependency list");
    DashMap::new()
}

/// Parse a requires_dist list for a specific target environment.
pub fn parse_requires_dist(
    requires_dist: &[String],
    _extras: &HashSet<String>,
    _target: &TargetEnv,
) -> Vec<(String, String)> {
    // Stub: return empty deps
    let _ = requires_dist;
    vec![]
}
