use std::collections::HashSet;

use dashmap::DashMap;
use futures::{StreamExt, stream};
use pep440_rs::Version;
use tracing::info;
use type_state_builder::TypeStateBuilder;

use super::eligibility::{ParsedDepsCacheKey, version_matches_target};
use super::error::ResolveError;
use super::markers::ParsedDependency;
use super::metadata::MetadataCache;
use super::resolve::{PlanParams, SolveJob, build_solve_context};
use super::solve::{SolveResult, solve_one_target};
use super::types::TargetEnv;

/// Cache key for memoizing `solve_one_target` results.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct SolveCacheKey {
    pub(crate) package: String,
    pub(crate) version: Version,
    pub(crate) target: TargetEnv,
    pub(crate) extras: Vec<String>,
}

pub(crate) type SolveResultCache = DashMap<SolveCacheKey, SolveResult>;

#[derive(TypeStateBuilder)]
#[builder(impl_into)]
pub(crate) struct SolveCaches<'a> {
    #[builder(required)]
    pub(crate) meta: &'a MetadataCache,
    #[builder(required)]
    pub(crate) solve: &'a SolveResultCache,
    #[builder(required)]
    pub(crate) parsed: &'a DashMap<ParsedDepsCacheKey, Vec<ParsedDependency>>,
}

pub(crate) fn solve_cache_key(
    package: &str,
    version: &Version,
    target: &TargetEnv,
    extras: &HashSet<String>,
) -> SolveCacheKey {
    let mut extras_vec: Vec<String> = extras.iter().cloned().collect();
    extras_vec.sort();
    SolveCacheKey {
        package: package.to_string(),
        version: version.clone(),
        target: target.clone(),
        extras: extras_vec,
    }
}

async fn do_actual_solve(
    params: &PlanParams<'_>,
    caches: &SolveCaches<'_>,
    job: &SolveJob,
    cache_key: SolveCacheKey,
) -> Result<(usize, Option<SolveResult>), ResolveError> {
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

pub(crate) async fn run_solve_job(
    params: &PlanParams<'_>,
    caches: &SolveCaches<'_>,
    job: &SolveJob,
) -> Result<(usize, Option<SolveResult>), ResolveError> {
    let cache_key =
        solve_cache_key(&job.package, &job.version, &job.target, &job.extras);
    if let Some(cached) = caches.solve.get(&cache_key) {
        info!(
            "求解命中缓存: {}@{} -> {}",
            job.package, job.version, job.target
        );
        return Ok((job.index, Some(cached.clone())));
    }
    do_actual_solve(params, caches, job, cache_key).await
}

pub(crate) async fn prefilter_solve_jobs(
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
        .filter(|(_, c)| *c)
        .map(|(j, _)| j)
        .collect())
}
