use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use futures::{StreamExt, stream};
use pep440_rs::Version;
use tracing::info;

use crate::downloader::FileInfo;
use crate::progress::{ProgressHandle, SyncEvent};

use super::eligibility::SolveContext;
pub use super::error::ResolveError;
use super::markers::ParsedDependency;
use super::metadata::MetadataCache;
use super::plan::expand_solved_versions;
use super::pubgrub::{bare_name, collect_pkg_extras};
use super::solve_cache::{
    SolveCaches, SolveResultCache, prefilter_solve_jobs, run_solve_job,
};
use super::types::TargetEnv;

pub struct PlanParams<'a> {
    pub top_packages: &'a [String],
    pub pypi_url: &'a str,
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
    client: &reqwest::Client,
    progress: Option<ProgressHandle>,
) -> Result<DependencyPlan, ResolveError> {
    if let Some(ref p) = progress {
        p.emit(SyncEvent::PhaseStarted {
            phase: "resolve",
            total: Some(params.top_packages.len() as u64),
        });
    }

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
    client: &reqwest::Client,
    progress: &Option<ProgressHandle>,
) -> Result<(DependencyPlan, usize), ResolveError> {
    let cache = MetadataCache::new(
        client.clone(),
        params.pypi_url.to_string(),
        params.metadata_workers,
    );
    let top_versions = collect_top_versions(params, &cache, progress).await?;
    let pkg_extras = collect_pkg_extras(params.top_packages);
    let targets = if params.targets.is_empty() {
        TargetEnv::all_resolution_targets()
    } else {
        params.targets.clone()
    };
    let solve_cache = Arc::new(SolveResultCache::new());
    let parsed_deps_cache: Arc<
        DashMap<super::eligibility::ParsedDepsCacheKey, Vec<ParsedDependency>>,
    > = Arc::new(DashMap::new());
    let caches = SolveCaches::builder()
        .meta(&cache)
        .solve(&*solve_cache)
        .parsed(&*parsed_deps_cache)
        .build();
    let all_solutions = solve_all_targets(
        params,
        &caches,
        &top_versions,
        &pkg_extras,
        &targets,
        progress,
    )
    .await?;
    let expanded = expand_solved_versions(
        &all_solutions,
        &cache,
        params.adjacent_versions_per_side,
        params.allow_prerelease,
        params.metadata_workers,
    )
    .await?;
    merge_top_versions(&expanded, &top_versions);
    let planned_files =
        collect_planned_files(params, &cache, &expanded).await?;
    let prefetched_files = super::build_requires::collect_prefetched_sdists(
        &cache,
        &planned_files,
        params.include_source,
        params.metadata_workers,
    )
    .await?;

    info!(
        "依赖规划完成: {} 个解，{} 个文件",
        all_solutions.len(),
        planned_files.len()
    );
    Ok((
        DependencyPlan {
            planned_files,
            prefetched_files,
            solved_versions: expanded,
        },
        all_solutions.len(),
    ))
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

fn emit_top_version_progress(
    progress: &Option<ProgressHandle>,
    completed: &AtomicU64,
    package: &str,
) {
    if let Some(p) = progress {
        let current = completed.fetch_add(1, Ordering::Relaxed) + 1;
        p.emit(SyncEvent::PhaseProgress {
            phase: "resolve",
            current,
            message: package.to_string(),
        });
    }
}

async fn collect_top_versions(
    params: &PlanParams<'_>,
    cache: &MetadataCache,
    progress: &Option<ProgressHandle>,
) -> Result<HashMap<String, Vec<Version>>, ResolveError> {
    let completed = Arc::new(AtomicU64::new(0));
    let results = stream::iter(params.top_packages.iter())
        .map(|package_ref| {
            let completed = Arc::clone(&completed);
            async move {
                let package = bare_name(package_ref);
                let all_versions = cache.get_all_versions(&package).await?;
                let selected = select_top_versions(
                    all_versions,
                    params.top_versions_per_package,
                    params.allow_prerelease,
                );
                info!("顶层包 {}: 选定 {} 个版本", package, selected.len());
                emit_top_version_progress(progress, &completed, &package);
                Ok::<_, ResolveError>((package, selected))
            }
        })
        .buffer_unordered(params.resolve_workers)
        .collect::<Vec<_>>()
        .await;
    let mut top_versions =
        results.into_iter().collect::<Result<Vec<_>, _>>()?;
    top_versions.sort_by(|(l, _), (r, _)| l.cmp(r));
    Ok(top_versions.into_iter().collect())
}

#[allow(clippy::too_many_arguments)]
async fn solve_all_targets(
    params: &PlanParams<'_>,
    caches: &SolveCaches<'_>,
    top_versions: &HashMap<String, Vec<Version>>,
    pkg_extras: &HashMap<String, HashSet<String>>,
    targets: &[TargetEnv],
    _progress: &Option<ProgressHandle>,
) -> Result<Vec<super::solve::SolveResult>, ResolveError> {
    let jobs = build_solve_jobs(top_versions, pkg_extras, targets);
    let jobs = prefilter_solve_jobs(params, caches.meta, jobs).await?;
    let mut solved = stream::iter(jobs)
        .map(|job| async move { run_solve_job(params, caches, &job).await })
        .buffer_unordered(params.resolve_workers)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    solved.sort_by_key(|(index, _)| *index);
    Ok(solved.into_iter().filter_map(|(_, r)| r).collect())
}

pub(crate) struct SolveJob {
    pub(crate) index: usize,
    pub(crate) package: Arc<str>,
    pub(crate) version: Version,
    pub(crate) extras: Arc<HashSet<String>>,
    pub(crate) target: Arc<TargetEnv>,
}

fn push_jobs_for_version(
    jobs: &mut Vec<SolveJob>,
    pkg: &str,
    version: &Version,
    extras: &Arc<HashSet<String>>,
    targets: &[Arc<TargetEnv>],
) {
    let pkg_arc: Arc<str> = Arc::from(pkg);
    for target in targets {
        jobs.push(SolveJob {
            index: jobs.len(),
            package: Arc::clone(&pkg_arc),
            version: version.clone(),
            extras: Arc::clone(extras),
            target: Arc::clone(target),
        });
    }
}

fn build_solve_jobs(
    top_versions: &HashMap<String, Vec<Version>>,
    pkg_extras: &HashMap<String, HashSet<String>>,
    targets: &[TargetEnv],
) -> Vec<SolveJob> {
    let targets_arc: Vec<Arc<TargetEnv>> =
        targets.iter().cloned().map(Arc::new).collect();
    let mut packages: Vec<_> = top_versions.iter().collect();
    packages.sort_by_key(|(p, _)| *p);

    let mut jobs = Vec::new();
    for (pkg, versions) in packages {
        let extras = Arc::new(pkg_extras.get(pkg).cloned().unwrap_or_default());
        for version in versions {
            push_jobs_for_version(
                &mut jobs,
                pkg,
                version,
                &extras,
                &targets_arc,
            );
        }
    }
    jobs
}

pub(crate) fn build_solve_context<'a>(
    params: &'a PlanParams<'a>,
    cache: &'a MetadataCache,
    target: &'a TargetEnv,
    parsed_deps_cache: Option<
        &'a DashMap<
            super::eligibility::ParsedDepsCacheKey,
            Vec<ParsedDependency>,
        >,
    >,
) -> SolveContext<'a> {
    SolveContext {
        cache,
        target,
        allow_prerelease: params.allow_prerelease,
        include_source: params.include_source,
        linux_max_glibc: params.linux_max_glibc,
        metadata_workers: params.metadata_workers,
        parsed_deps_cache,
    }
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

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
