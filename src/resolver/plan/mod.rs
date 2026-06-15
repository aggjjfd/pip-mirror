use std::{collections::HashMap, sync::Arc};

use dashmap::DashMap;
use futures::{StreamExt, stream};
use pep440_rs::Version;
use tracing::{debug, info};

use crate::downloader::FileInfo;
use crate::http::HttpClient;
use crate::progress::{ProgressHandle, SyncEvent};

use super::eligibility::ParsedDepsCacheKey;
use super::error::ResolveError;
use super::markers::ParsedDependency;
use super::metadata::MetadataCache;
use super::pubgrub::{bare_name, collect_pkg_extras};
use super::resolve::{build_solve_jobs, solve_all_targets};
use super::solve::SolveResult;
use super::solve_cache::{SolveCaches, SolveResultCache};
use super::types::TargetEnv;

mod expand;
use expand::expand_solved_versions;

pub struct PlanParams<'a> {
    pub top_packages: &'a [String],
    pub pypi_urls: &'a [String],
    pub top_versions_per_package: usize,
    pub adjacent_versions_per_side: usize,
    pub allow_prerelease: bool,
    pub include_source: bool,
    pub linux_max_glibc: &'a str,
    pub resolve_workers: usize,
    pub metadata_workers: usize,
    pub targets: Vec<TargetEnv>,
}

#[derive(Debug)]
pub struct DependencyPlan {
    pub planned_files: Vec<FileInfo>,
    pub prefetched_files: HashMap<(String, String), Vec<u8>>,
    pub solved_versions: DashMap<String, Vec<Version>>,
}

pub async fn build_dependency_plan(
    params: &PlanParams<'_>,
    client: &HttpClient,
    progress: Option<ProgressHandle>,
) -> Result<DependencyPlan, ResolveError> {
    let (plan, solution_count) =
        build_dependency_plan_inner(params, client, &progress).await?;

    if let Some(ref p) = progress {
        p.emit(SyncEvent::PhaseFinished {
            phase: "resolve",
            summary: format!(
                "{} 个解，{} 个文件",
                solution_count,
                plan.planned_files.len()
            ),
        });
    }

    Ok(plan)
}

async fn build_dependency_plan_inner(
    params: &PlanParams<'_>,
    client: &HttpClient,
    progress: &Option<ProgressHandle>,
) -> Result<(DependencyPlan, usize), ResolveError> {
    let base_url = params
        .pypi_urls
        .first()
        .map(String::as_str)
        .unwrap_or("https://pypi.org")
        .to_string();
    let cache =
        MetadataCache::new(client.clone(), base_url, params.metadata_workers);
    let top_versions = collect_top_versions(params, &cache).await?;
    let pkg_extras = collect_pkg_extras(params.top_packages);
    let targets = if params.targets.is_empty() {
        TargetEnv::all_resolution_targets()
    } else {
        params.targets.clone()
    };

    let jobs = build_solve_jobs(&top_versions, &pkg_extras, &targets);
    if let Some(p) = progress {
        p.emit(SyncEvent::PhaseStarted {
            phase: "resolve",
            total: Some(jobs.len() as u64),
        });
    }

    let solve_cache = Arc::new(SolveResultCache::new());
    let parsed_deps_cache: Arc<
        DashMap<ParsedDepsCacheKey, Vec<ParsedDependency>>,
    > = Arc::new(DashMap::new());
    let caches = SolveCaches::builder()
        .meta(&cache)
        .solve(&*solve_cache)
        .parsed(&*parsed_deps_cache)
        .build();
    let all_solutions =
        solve_all_targets(params, &caches, &jobs, progress).await?;
    let plan = build_plan_from_solutions(
        params,
        &cache,
        &top_versions,
        &all_solutions,
    )
    .await?;

    info!(
        "依赖规划完成: {} 个解，{} 个文件",
        all_solutions.len(),
        plan.planned_files.len()
    );
    Ok((plan, all_solutions.len()))
}

async fn build_plan_from_solutions(
    params: &PlanParams<'_>,
    cache: &MetadataCache,
    top_versions: &HashMap<String, Vec<Version>>,
    all_solutions: &[SolveResult],
) -> Result<DependencyPlan, ResolveError> {
    let expanded = expand_solved_versions(
        all_solutions,
        cache,
        params.adjacent_versions_per_side,
        params.allow_prerelease,
        params.metadata_workers,
    )
    .await?;
    merge_top_versions(&expanded, top_versions);
    let planned_files = collect_planned_files(params, cache, &expanded).await?;
    let prefetched_files = super::build_requires::collect_prefetched_sdists(
        cache,
        &planned_files,
        params.include_source,
        params.metadata_workers,
    )
    .await?;
    Ok(DependencyPlan {
        planned_files,
        prefetched_files,
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
        .filter(|v| allow_prerelease || !v.any_prerelease());
    if max_versions == 0 {
        filtered.collect()
    } else {
        filtered.take(max_versions).collect()
    }
}

async fn collect_top_versions(
    params: &PlanParams<'_>,
    cache: &MetadataCache,
) -> Result<HashMap<String, Vec<Version>>, ResolveError> {
    let results = stream::iter(params.top_packages.iter())
        .map(|package_ref| async move {
            let package = bare_name(package_ref);
            let all_versions = cache.get_all_versions(&package).await?;
            let selected = select_top_versions(
                all_versions,
                params.top_versions_per_package,
                params.allow_prerelease,
            );
            debug!("顶层包 {}: 选定 {} 个版本", package, selected.len());
            Ok::<_, ResolveError>((package, selected))
        })
        .buffer_unordered(params.resolve_workers)
        .collect::<Vec<_>>()
        .await;
    let mut top_versions =
        results.into_iter().collect::<Result<Vec<_>, _>>()?;
    top_versions.sort_by(|(l, _), (r, _)| l.cmp(r));
    Ok(top_versions.into_iter().collect())
}

fn merge_top_versions(
    expanded: &DashMap<String, Vec<Version>>,
    top_versions: &HashMap<String, Vec<Version>>,
) {
    for (pkg, versions) in top_versions {
        let mut entry = expanded.entry(pkg.clone()).or_default();
        for version in versions {
            entry.push(version.clone());
        }
        entry.sort_by(|a, b| b.cmp(a));
        entry.dedup();
    }
}

async fn collect_planned_files(
    params: &PlanParams<'_>,
    cache: &MetadataCache,
    expanded: &DashMap<String, Vec<Version>>,
) -> Result<Vec<FileInfo>, ResolveError> {
    let targets = if params.targets.is_empty() {
        super::types::TargetEnv::all_resolution_targets()
    } else {
        params.targets.clone()
    };
    let target_ref = &targets;
    let results = stream::iter(build_file_jobs(expanded))
        .map(|(index, package, version)| async move {
            let files = cache.get_version_files(&package, &version).await?;
            Ok::<_, ResolveError>((
                index,
                crate::filters::select_files_for_version(
                    &files,
                    target_ref,
                    params.include_source,
                    params.linux_max_glibc,
                ),
            ))
        })
        .buffer_unordered(params.metadata_workers)
        .collect::<Vec<_>>()
        .await;
    let mut collected = results.into_iter().collect::<Result<Vec<_>, _>>()?;
    collected.sort_by_key(|(i, _)| *i);
    let mut planned_files: Vec<_> =
        collected.into_iter().flat_map(|(_, s)| s).collect();
    planned_files.sort_by(|l, r| l.filename.cmp(&r.filename));
    planned_files.dedup_by(|l, r| l.filename == r.filename);
    Ok(planned_files)
}

fn build_file_jobs(
    expanded: &DashMap<String, Vec<Version>>,
) -> Vec<(usize, String, Version)> {
    let mut pkgs: Vec<_> = expanded
        .iter()
        .map(|e| (e.key().clone(), e.value().clone()))
        .collect();
    pkgs.sort_by(|(l, _), (r, _)| l.cmp(r));
    pkgs.into_iter()
        .flat_map(|(p, vs)| vs.into_iter().map(move |v| (p.clone(), v)))
        .enumerate()
        .map(|(i, (p, v))| (i, p, v))
        .collect()
}
