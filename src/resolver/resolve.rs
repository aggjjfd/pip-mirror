use std::collections::HashMap;

use dashmap::DashMap;
use pep440_rs::Version;
use tracing::info;

use crate::downloader::FileInfo;

use super::eligibility::{SolveContext, version_matches_target};
pub use super::error::ResolveError;
use super::metadata::MetadataCache;
use super::plan::expand_solved_versions;
use super::pubgrub::{bare_name, collect_pkg_extras};
use super::solve::solve_one_target;
use super::types::TargetEnv;

pub struct PlanParams<'a> {
    pub top_packages: &'a [String],
    pub pypi_url: &'a str,
    pub top_versions_per_package: usize,
    pub adjacent_versions_per_side: usize,
    pub allow_prerelease: bool,
    pub include_source: bool,
    pub linux_max_glibc: &'a str,
    pub workers: usize,
}

pub struct DependencyPlan {
    pub planned_files: Vec<FileInfo>,
    pub solved_versions: DashMap<String, Vec<Version>>,
}

pub async fn build_dependency_plan(
    params: &PlanParams<'_>,
    client: &reqwest::Client,
) -> Result<DependencyPlan, ResolveError> {
    let cache = MetadataCache::new(
        client.clone(),
        params.pypi_url.to_string(),
        params.workers,
    );
    let top_versions = collect_top_versions(params, &cache).await?;
    let pkg_extras = collect_pkg_extras(params.top_packages);
    let targets = TargetEnv::all_resolution_targets();
    let all_solutions =
        solve_all_targets(params, &cache, &top_versions, &pkg_extras, &targets)
            .await?;
    let expanded = expand_solved_versions(
        &all_solutions,
        &cache,
        params.adjacent_versions_per_side,
        params.allow_prerelease,
    )
    .await?;
    merge_top_versions(&expanded, &top_versions);
    let planned_files =
        collect_planned_files(params, &cache, &expanded).await?;

    info!(
        "依赖规划完成: {} 个解，{} 个文件",
        all_solutions.len(),
        planned_files.len()
    );
    Ok(DependencyPlan {
        planned_files,
        solved_versions: expanded,
    })
}

pub(crate) fn select_top_versions(
    versions: Vec<Version>,
    max_versions: usize,
    allow_prerelease: bool,
) -> Vec<Version> {
    let filtered = versions
        .into_iter()
        .filter(|version| allow_prerelease || !version.any_prerelease());
    if max_versions == 0 {
        return filtered.collect();
    }
    filtered.take(max_versions).collect()
}

async fn collect_top_versions(
    params: &PlanParams<'_>,
    cache: &MetadataCache,
) -> Result<HashMap<String, Vec<Version>>, ResolveError> {
    let mut top_versions = HashMap::new();
    for package_ref in params.top_packages {
        let package = bare_name(package_ref);
        let all_versions = cache.get_all_versions(&package).await?;
        let selected = select_top_versions(
            all_versions,
            params.top_versions_per_package,
            params.allow_prerelease,
        );
        info!("顶层包 {}: 选定 {} 个版本", package, selected.len());
        top_versions.insert(package, selected);
    }
    Ok(top_versions)
}

async fn solve_all_targets(
    params: &PlanParams<'_>,
    cache: &MetadataCache,
    top_versions: &HashMap<String, Vec<Version>>,
    pkg_extras: &HashMap<String, std::collections::HashSet<String>>,
    targets: &[TargetEnv],
) -> Result<Vec<super::solve::SolveResult>, ResolveError> {
    let mut all_solutions = Vec::new();
    for (package, versions) in top_versions {
        let extras = pkg_extras.get(package).cloned().unwrap_or_default();
        for version in versions {
            let mut solutions = solve_version_targets(
                params, cache, package, version, &extras, targets,
            )
            .await?;
            all_solutions.append(&mut solutions);
        }
    }
    Ok(all_solutions)
}

#[allow(clippy::too_many_arguments)]
async fn solve_version_targets(
    params: &PlanParams<'_>,
    cache: &MetadataCache,
    package: &str,
    version: &Version,
    extras: &std::collections::HashSet<String>,
    targets: &[TargetEnv],
) -> Result<Vec<super::solve::SolveResult>, ResolveError> {
    let mut solutions = Vec::new();
    for target in targets {
        let ctx = build_solve_context(params, cache, target);
        if !version_matches_target(&ctx, package, version).await? {
            continue;
        }
        solutions.push(solve_one_target(&ctx, package, version, extras).await?);
    }
    Ok(solutions)
}

fn build_solve_context<'a>(
    params: &'a PlanParams<'a>,
    cache: &'a MetadataCache,
    target: &'a TargetEnv,
) -> SolveContext<'a> {
    SolveContext {
        cache,
        target,
        allow_prerelease: params.allow_prerelease,
        include_source: params.include_source,
        linux_max_glibc: params.linux_max_glibc,
    }
}

fn merge_top_versions(
    expanded: &DashMap<String, Vec<Version>>,
    top_versions: &HashMap<String, Vec<Version>>,
) {
    for (package, versions) in top_versions {
        let mut entry = expanded.entry(package.clone()).or_default();
        for version in versions {
            entry.push(version.clone());
        }
        entry.sort_by(|left, right| right.cmp(left));
        entry.dedup();
    }
}

async fn collect_planned_files(
    params: &PlanParams<'_>,
    cache: &MetadataCache,
    expanded: &DashMap<String, Vec<Version>>,
) -> Result<Vec<FileInfo>, ResolveError> {
    let mut planned_files = Vec::new();
    for entry in expanded.iter() {
        for version in entry.value() {
            let files = cache.get_version_files(entry.key(), version).await?;
            let selected = crate::filters::select_files_for_version(
                &files,
                params.include_source,
                params.linux_max_glibc,
            );
            planned_files.extend(selected);
        }
    }

    let mut seen = std::collections::HashSet::new();
    planned_files.retain(|file| seen.insert(file.filename.clone()));
    Ok(planned_files)
}
