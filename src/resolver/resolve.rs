use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::progress::{ProgressHandle, SyncEvent};
use dashmap::DashMap;
use futures::{StreamExt, stream};
use pep440_rs::Version;

use super::eligibility::SolveContext;
pub use super::error::ResolveError;
use super::markers::ParsedDependency;
use super::metadata::MetadataCache;
pub use super::plan::{DependencyPlan, PlanParams, build_dependency_plan};
use super::solve_cache::{SolveCaches, prefilter_solve_jobs, run_solve_job};
use super::types::TargetEnv;

fn emit_solve_progress(
    progress: &Option<ProgressHandle>,
    completed: &AtomicU64,
    job: &SolveJob,
) {
    let Some(p) = progress else {
        return;
    };
    let current = completed.fetch_add(1, Ordering::Relaxed) + 1;
    p.emit(SyncEvent::PhaseProgress {
        phase: "resolve",
        current,
        message: format!("{}@{} -> {}", job.package, job.version, job.target),
    });
}

fn emit_solve_done(progress: &Option<ProgressHandle>, total: u64) {
    if let Some(p) = progress {
        p.emit(SyncEvent::PhaseProgress {
            phase: "resolve",
            current: total,
            message: "依赖求解完成".to_string(),
        });
    }
}

pub(crate) async fn solve_all_targets(
    params: &PlanParams<'_>,
    caches: &SolveCaches<'_>,
    jobs: &[SolveJob],
    progress: &Option<ProgressHandle>,
) -> Result<Vec<super::solve::SolveResult>, ResolveError> {
    let jobs = prefilter_solve_jobs(params, caches.meta, jobs.to_vec()).await?;
    let completed = Arc::new(AtomicU64::new(0));
    let total_jobs = jobs.len() as u64;

    let mut solved = stream::iter(jobs)
        .map(|job| {
            let progress = progress.clone();
            let completed = Arc::clone(&completed);
            async move {
                let result = run_solve_job(params, caches, &job).await;
                emit_solve_progress(&progress, &completed, &job);
                result
            }
        })
        .buffer_unordered(params.resolve_workers)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    solved.sort_by_key(|(index, _)| *index);
    emit_solve_done(progress, total_jobs);

    Ok(solved.into_iter().filter_map(|(_, r)| r).collect())
}

#[derive(Clone)]
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

pub(crate) fn build_solve_jobs(
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

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
