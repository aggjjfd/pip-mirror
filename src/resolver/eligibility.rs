use std::str::FromStr;

use futures::{StreamExt, stream};
use pep440_rs::{Version, VersionSpecifiers};

use crate::filters::version_is_installable_for_target;

use super::error::ResolveError;
use super::metadata::MetadataCache;
use super::types::TargetEnv;

const VERSION_MATCH_CONCURRENCY: usize = 16;

pub struct SolveContext<'a> {
    pub cache: &'a MetadataCache,
    pub target: &'a TargetEnv,
    pub allow_prerelease: bool,
    pub include_source: bool,
    pub linux_max_glibc: &'a str,
}

pub async fn version_matches_target(
    ctx: &SolveContext<'_>,
    package: &str,
    version: &Version,
) -> Result<bool, ResolveError> {
    let files = ctx.cache.get_version_files(package, version).await?;
    if !version_is_installable_for_target(
        &files,
        ctx.target,
        ctx.include_source,
        ctx.linux_max_glibc,
    ) {
        return Ok(false);
    }

    let Some(spec) = ctx.cache.get_requires_python(package, version).await?
    else {
        return Ok(true);
    };
    requires_python_matches_target(package, version, &spec, ctx)
}

pub async fn candidate_versions_for_package(
    ctx: &SolveContext<'_>,
    package: &str,
    matches_range: impl Fn(&Version) -> bool,
) -> Result<Vec<Version>, ResolveError> {
    let candidate_pool: Vec<Version> =
        build_candidate_pool(ctx, package, matches_range).await?;
    let checked = concurrent_match_checks(ctx, package, candidate_pool).await?;
    Ok(collect_matches(checked))
}

async fn build_candidate_pool(
    ctx: &SolveContext<'_>,
    package: &str,
    matches_range: impl Fn(&Version) -> bool,
) -> Result<Vec<Version>, ResolveError> {
    Ok(ctx
        .cache
        .get_all_versions(package)
        .await?
        .into_iter()
        .filter(|version| ctx.allow_prerelease || !version.any_prerelease())
        .filter(matches_range)
        .collect())
}

async fn concurrent_match_checks(
    ctx: &SolveContext<'_>,
    package: &str,
    candidate_pool: Vec<Version>,
) -> Result<Vec<(Version, bool)>, ResolveError> {
    let package = package.to_string();
    let results = stream::iter(candidate_pool)
        .map(|version| {
            let package = package.clone();
            async move {
                let matches =
                    version_matches_target(ctx, &package, &version).await?;
                Ok::<_, ResolveError>((version, matches))
            }
        })
        .buffer_unordered(VERSION_MATCH_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    results.into_iter().collect()
}

fn collect_matches(checked: Vec<(Version, bool)>) -> Vec<Version> {
    checked
        .into_iter()
        .filter(|(_, matches)| *matches)
        .map(|(version, _)| version)
        .collect()
}

fn requires_python_matches_target(
    package: &str,
    version: &Version,
    requires_python: &str,
    ctx: &SolveContext<'_>,
) -> Result<bool, ResolveError> {
    let specifiers =
        VersionSpecifiers::from_str(requires_python).map_err(|err| {
            ResolveError::InvalidRequiresPython {
                package: package.to_string(),
                version: version.clone(),
                spec: requires_python.to_string(),
                detail: err.to_string(),
            }
        })?;
    Ok(specifiers.contains(&target_python_version(ctx.target)))
}

fn target_python_version(target: &TargetEnv) -> Version {
    Version::from_str(&target.python_full_version)
        .expect("supported target Python versions must be valid")
}
