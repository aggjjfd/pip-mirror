use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use dashmap::DashMap;
use futures::{StreamExt, stream};
use pep440_rs::Version;
use tracing::info;

use crate::downloader::FileInfo;
use type_state_builder::TypeStateBuilder;

use super::eligibility::{
    ParsedDepsCacheKey, SolveContext, version_matches_target,
};
pub use super::error::ResolveError;
use super::metadata::MetadataCache;
use super::plan::expand_solved_versions;
use super::pubgrub::{bare_name, collect_pkg_extras};
use super::solve::{SolveResult, solve_one_target};
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

pub struct DependencyPlan {
    pub planned_files: Vec<FileInfo>,
    pub prefetched_files: HashMap<(String, String), Vec<u8>>,
    pub solved_versions: DashMap<String, Vec<Version>>,
}

/// Cache key for memoizing `solve_one_target` results.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct SolveCacheKey {
    package: String,
    version: Version,
    target: TargetEnv,
    extras: Vec<String>, // sorted for deterministic hashing
}

type SolveResultCache = DashMap<SolveCacheKey, SolveResult>;

#[derive(TypeStateBuilder)]
#[builder(impl_into)]
struct SolveCaches<'a> {
    #[builder(required)]
    meta: &'a MetadataCache,
    #[builder(required)]
    solve: &'a SolveResultCache,
    #[builder(required)]
    parsed: &'a DashMap<ParsedDepsCacheKey, Vec<super::markers::ParsedDependency>>,
}

pub async fn build_dependency_plan(
    params: &PlanParams<'_>,
    client: &reqwest::Client,
) -> Result<DependencyPlan, ResolveError> {
    let cache = MetadataCache::new(
        client.clone(),
        params.pypi_url.to_string(),
        params.metadata_workers,
    );
    let top_versions = collect_top_versions(params, &cache).await?;
    let pkg_extras = collect_pkg_extras(params.top_packages);
    let targets = if params.targets.is_empty() {
        TargetEnv::all_resolution_targets()
    } else {
        params.targets.clone()
    };
    let solve_cache = Arc::new(SolveResultCache::new());
    let parsed_deps_cache = Arc::new(DashMap::new());
    let caches = SolveCaches::builder()
        .meta(&cache)
        .solve(&*solve_cache)
        .parsed(&*parsed_deps_cache)
        .build();
    let all_solutions =
        solve_all_targets(params, &caches, &top_versions, &pkg_extras, &targets)
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
    let results = stream::iter(params.top_packages.iter())
        .map(|package_ref| async move {
            let package = bare_name(package_ref);
            let all_versions = cache.get_all_versions(&package).await?;
            let selected = select_top_versions(
                all_versions,
                params.top_versions_per_package,
                params.allow_prerelease,
            );
            info!("顶层包 {}: 选定 {} 个版本", package, selected.len());
            Ok::<_, ResolveError>((package, selected))
        })
        .buffer_unordered(params.resolve_workers)
        .collect::<Vec<_>>()
        .await;
    let mut top_versions =
        results.into_iter().collect::<Result<Vec<_>, _>>()?;
    top_versions.sort_by(|(left, _), (right, _)| left.cmp(right));
    Ok(top_versions.into_iter().collect())
}

async fn solve_all_targets(
    params: &PlanParams<'_>,
    caches: &SolveCaches<'_>,
    top_versions: &HashMap<String, Vec<Version>>,
    pkg_extras: &HashMap<String, std::collections::HashSet<String>>,
    targets: &[TargetEnv],
) -> Result<Vec<super::solve::SolveResult>, ResolveError> {
    let jobs = build_solve_jobs(top_versions, pkg_extras, targets);
    let jobs = prefilter_solve_jobs(params, caches.meta, jobs).await?;
    let results = stream::iter(jobs)
        .map(|job| async move {
            run_solve_job(params, caches, &job).await
        })
        .buffer_unordered(params.resolve_workers)
        .collect::<Vec<_>>()
        .await;
    let mut solved = results.into_iter().collect::<Result<Vec<_>, _>>()?;
    solved.sort_by_key(|(index, _)| *index);
    Ok(solved
        .into_iter()
        .filter_map(|(_, result)| result)
        .collect())
}

async fn run_solve_job(
    params: &PlanParams<'_>,
    caches: &SolveCaches<'_>,
    job: &SolveJob,
) -> Result<(usize, Option<super::solve::SolveResult>), ResolveError> {
    let mut extras_vec: Vec<String> = job.extras.iter().cloned().collect();
    extras_vec.sort();
    let cache_key = SolveCacheKey {
        package: job.package.clone(),
        version: job.version.clone(),
        target: job.target.clone(),
        extras: extras_vec,
    };

    if let Some(cached) = caches.solve.get(&cache_key) {
        info!(
            "求解命中缓存: {}@{} -> {}",
            job.package, job.version, job.target
        );
        return Ok((job.index, Some(cached.clone())));
    }

    let ctx = build_solve_context(
        params,
        caches.meta,
        &job.target,
        Some(caches.parsed),
    );
    if !version_matches_target(&ctx, &job.package, &job.version).await? {
        return Ok((job.index, None));
    }
    info!(
        "开始求解: {}@{} -> {}",
        job.package, job.version, job.target
    );
    let result =
        solve_one_target(&ctx, &job.package, &job.version, &job.extras).await;
    if let Err(ResolveError::NoSolution { detail, .. }) = &result {
        tracing::warn!(
            "  ! 求解跳过: {}@{} -> {} 无可用依赖解: {}",
            job.package,
            job.version,
            job.target,
            detail
        );
        return Ok((job.index, None));
    }
    let solved = result?;
    info!(
        "求解完成: {}@{} -> {}",
        job.package, job.version, job.target
    );
    caches.solve.insert(cache_key, solved.clone());
    Ok((job.index, Some(solved)))
}

struct SolveJob {
    index: usize,
    package: String,
    version: Version,
    extras: HashSet<String>,
    target: TargetEnv,
}

fn build_solve_jobs(
    top_versions: &HashMap<String, Vec<Version>>,
    pkg_extras: &HashMap<String, HashSet<String>>,
    targets: &[TargetEnv],
) -> Vec<SolveJob> {
    let mut packages: Vec<_> = top_versions.iter().collect();
    packages.sort_by_key(|(package, _)| *package);

    packages
        .into_iter()
        .flat_map(|(package, versions)| {
            let extras = pkg_extras.get(package).cloned().unwrap_or_default();
            versions.iter().cloned().flat_map(move |version| {
                let package = package.clone();
                let extras = extras.clone();
                targets.iter().cloned().map(move |target| SolveJob {
                    index: 0,
                    package: package.clone(),
                    version: version.clone(),
                    extras: extras.clone(),
                    target,
                })
            })
        })
        .enumerate()
        .map(|(index, mut job)| {
            job.index = index;
            job
        })
        .collect()
}

async fn prefilter_solve_jobs(
    params: &PlanParams<'_>,
    cache: &MetadataCache,
    jobs: Vec<SolveJob>,
) -> Result<Vec<SolveJob>, ResolveError> {
    let results = stream::iter(jobs)
        .map(|job| async move {
            let ctx = build_solve_context(params, cache, &job.target, None);
            let compatible =
                version_matches_target(&ctx, &job.package, &job.version)
                    .await?;
            Ok::<_, ResolveError>((job, compatible))
        })
        .buffer_unordered(params.metadata_workers)
        .collect::<Vec<_>>()
        .await;

    Ok(results
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|(_, compat)| *compat)
        .map(|(job, _)| job)
        .collect())
}

fn build_solve_context<'a>(
    params: &'a PlanParams<'a>,
    cache: &'a MetadataCache,
    target: &'a TargetEnv,
    parsed_deps_cache: Option<
        &'a DashMap<ParsedDepsCacheKey, Vec<super::markers::ParsedDependency>>,
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
    let targets = if params.targets.is_empty() {
        super::types::TargetEnv::all_resolution_targets()
    } else {
        params.targets.clone()
    };
    let jobs = build_file_jobs(expanded);
    let results = stream::iter(jobs)
        .map(|(index, package, version)| {
            let targets = &targets;
            async move {
                let files = cache.get_version_files(&package, &version).await?;
                let selected = crate::filters::select_files_for_version(
                    &files,
                    targets,
                    params.include_source,
                    params.linux_max_glibc,
                );
                Ok::<_, ResolveError>((index, selected))
            }
        })
        .buffer_unordered(params.metadata_workers)
        .collect::<Vec<_>>()
        .await;
    let mut planned_files = Vec::new();
    let mut collected = results.into_iter().collect::<Result<Vec<_>, _>>()?;
    collected.sort_by_key(|(index, _)| *index);
    for (_, selected) in collected {
        planned_files.extend(selected);
    }
    planned_files.sort_by(|left, right| left.filename.cmp(&right.filename));
    planned_files.dedup_by(|left, right| left.filename == right.filename);
    Ok(planned_files)
}

fn build_file_jobs(
    expanded: &DashMap<String, Vec<Version>>,
) -> Vec<(usize, String, Version)> {
    let mut packages: Vec<_> = expanded
        .iter()
        .map(|entry| (entry.key().clone(), entry.value().clone()))
        .collect();
    packages.sort_by(|(left, _), (right, _)| left.cmp(right));

    packages
        .into_iter()
        .flat_map(|(package, versions)| {
            versions
                .into_iter()
                .map(move |version| (package.clone(), version))
        })
        .enumerate()
        .map(|(index, (package, version))| (index, package, version))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use pep440_rs::Version;

    use super::{SolveCacheKey, SolveResult, SolveResultCache};
    use crate::resolver::types::TargetEnv;

    #[test]
    fn test_solve_cache_key_hash_and_eq() {
        let target = TargetEnv::all_resolution_targets()[0].clone();
        let key1 = SolveCacheKey {
            package: "demo".to_string(),
            version: Version::new([1, 0, 0]),
            target: target.clone(),
            extras: vec!["feature".to_string()],
        };
        let key2 = SolveCacheKey {
            package: "demo".to_string(),
            version: Version::new([1, 0, 0]),
            target,
            extras: vec!["feature".to_string()],
        };
        assert_eq!(key1, key2);

        let cache = SolveResultCache::new();
        cache.insert(
            key1,
            SolveResult {
                solved_versions: HashMap::new(),
                active_extras: HashMap::new(),
            },
        );
        assert!(cache.contains_key(&key2));
    }
}
