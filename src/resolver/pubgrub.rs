use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use dashmap::DashMap;
use pep440_rs::Version;
use pubgrub::OfflineDependencyProvider;
use pubgrub::Ranges;
use tracing::{info, warn};

use super::types::all_targets;

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

// ── pubgrub resolver ──

pub type DepSpec = (String, String); // (package_name, pep440_specifier)

struct ProviderCtx<'a> {
    packages: &'a HashSet<String>,
    top_pkg: &'a str,
    top_ver: &'a Version,
    versions: &'a HashMap<String, Vec<Version>>,
    deps: &'a HashMap<(String, usize), Vec<DepSpec>>,
}

fn dep_to_range(
    dep_name: &str,
    spec: &str,
    versions: &HashMap<String, Vec<Version>>,
) -> Option<(String, Ranges<u32>)> {
    let vers = versions.get(dep_name).map(|v| v.as_slice()).unwrap_or(&[]);
    spec_to_range(vers, spec).map(|r| (dep_name.to_string(), r))
}

fn resolve_dep_list(
    ds: &[DepSpec],
    versions: &HashMap<String, Vec<Version>>,
) -> Vec<(String, Ranges<u32>)> {
    ds.iter()
        .filter_map(|(n, s)| dep_to_range(n, s, versions))
        .collect()
}

fn pkg_deps(
    ctx: &ProviderCtx<'_>,
    pkg: &str,
    idx: usize,
) -> Vec<(String, Ranges<u32>)> {
    ctx.deps
        .get(&(pkg.to_string(), idx))
        .map(|ds| resolve_dep_list(ds, ctx.versions))
        .unwrap_or_default()
}

fn build_provider(
    ctx: &ProviderCtx<'_>,
) -> OfflineDependencyProvider<String, Ranges<u32>> {
    let mut p = OfflineDependencyProvider::new();
    for pkg in ctx.packages {
        let vers = ctx.versions.get(pkg).map(|v| v.as_slice()).unwrap_or(&[]);
        for (idx, _ver) in vers.iter().enumerate() {
            p.add_dependencies(
                pkg.clone(),
                idx.try_into().unwrap_or(0u32),
                pkg_deps(ctx, pkg, idx),
            );
        }
    }
    let tvers = ctx
        .versions
        .get(ctx.top_pkg)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    if let Some(ti) = tvers.iter().position(|v| v == ctx.top_ver) {
        p.add_dependencies(
            "__root__".to_string(),
            0u32,
            vec![(
                ctx.top_pkg.to_string(),
                Ranges::singleton(ti.try_into().unwrap_or(0u32)),
            )],
        );
    }
    p
}

fn parse_specs(spec: &str) -> Option<Vec<pep440_rs::VersionSpecifier>> {
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

/// Convert a PEP 440 specifier to a Ranges<u32> over version indices (descending).
fn spec_to_range(versions: &[Version], spec: &str) -> Option<Ranges<u32>> {
    let specs = match parse_specs(spec) {
        Some(s) => s,
        None => return Some(Ranges::full()),
    };
    let mut range: Option<Ranges<u32>> = None;
    for (idx, ver) in versions.iter().enumerate() {
        if !specs.iter().all(|s| s.contains(ver)) {
            continue;
        }
        let idx_u: u32 = u32::try_from(idx).ok()?;
        let s = Ranges::singleton(idx_u);
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

pub fn bare_name(package_ref: &str) -> String {
    package_ref
        .split_once('[')
        .map_or(package_ref.to_string(), |(n, _)| n.to_string())
}

// ── resolver entry point ──

pub struct ResolveParams<'a> {
    pub top_packages: &'a [String],
    pub top_versions: &'a DashMap<String, Vec<Version>>,
    pub pypi_url: &'a str,
    pub max_versions: usize,
}

pub async fn resolve_dependencies(
    params: &ResolveParams<'_>,
    session: &reqwest::Client,
) -> DashMap<String, Vec<Version>> {
    let targets = all_targets();
    let all_solutions: Vec<Solution> = Vec::new();

    for target in &targets {
        info!("  解析 target {target}");
        // Verify build_provider compiles (real HTTP fetch next step)
        let dummy_versions = HashMap::new();
        let dummy_deps = HashMap::new();
        let dummy_pkgs = HashSet::new();
        let dummy_ver = Version::from_str("1.0.0").unwrap();
        let _provider = build_provider(&ProviderCtx {
            packages: &dummy_pkgs,
            top_pkg: "x",
            top_ver: &dummy_ver,
            versions: &dummy_versions,
            deps: &dummy_deps,
        });
        let _ = (params, session, target);
    }

    warn!("pubgrub resolver stub — returning empty list");
    let _ = all_solutions;
    DashMap::new()
}
