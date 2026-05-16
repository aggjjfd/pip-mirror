use std::str::FromStr;

use dashmap::DashMap;
use futures::{StreamExt, stream};
use pep440_rs::{Version, VersionSpecifier, VersionSpecifiers};
use pubgrub::Range;

use crate::filters::version_is_installable_for_target;

use super::error::ResolveError;
use super::markers::ParsedDependency;
use super::metadata::MetadataCache;
use super::types::TargetEnv;

const MAX_VERSION_MATCH_CHECKS_PER_PACKAGE: usize = 16;

/// Cache key for parsed dependencies of a specific package@version under a
/// given target and extras set.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct ParsedDepsCacheKey {
    pub package: String,
    pub version: Version,
    pub target: TargetEnv,
    pub extras: Vec<String>, // sorted for deterministic hashing
}

pub struct SolveContext<'a> {
    pub cache: &'a MetadataCache,
    pub target: &'a TargetEnv,
    pub allow_prerelease: bool,
    pub include_source: bool,
    pub linux_max_glibc: &'a str,
    pub metadata_workers: usize,
    pub parsed_deps_cache:
        Option<&'a DashMap<ParsedDepsCacheKey, Vec<ParsedDependency>>>,
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
    matches_range: &Range<Version>,
) -> Result<Vec<Version>, ResolveError> {
    let candidate_pool: Vec<Version> =
        build_candidate_pool(ctx, package, matches_range).await?;
    let checked = concurrent_match_checks(ctx, package, candidate_pool).await?;
    Ok(collect_matches(checked))
}

async fn build_candidate_pool(
    ctx: &SolveContext<'_>,
    package: &str,
    matches_range: &Range<Version>,
) -> Result<Vec<Version>, ResolveError> {
    let all_versions = ctx.cache.get_all_versions(package).await?;
    let candidates: Vec<Version> = all_versions
        .into_iter()
        .filter(|version| matches_range.contains(version))
        .collect();

    if ctx.allow_prerelease {
        return Ok(candidates);
    }

    let stable: Vec<Version> = candidates
        .iter()
        .filter(|v| !v.any_prerelease())
        .cloned()
        .collect();

    if !stable.is_empty() {
        Ok(stable)
    } else {
        // 若该包所有可用版本均为 pre-release，仍允许使用（与 uv 行为一致）
        Ok(candidates)
    }
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
        .buffer_unordered(MAX_VERSION_MATCH_CHECKS_PER_PACKAGE)
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
        parse_requires_python_spec(requires_python).map_err(|detail| {
            ResolveError::InvalidRequiresPython {
                package: package.to_string(),
                version: version.clone(),
                spec: requires_python.to_string(),
                detail,
            }
        })?;
    Ok(specifiers.contains(&target_python_version(ctx.target)))
}

fn target_python_version(target: &TargetEnv) -> Version {
    Version::from_str(&target.python_full_version)
        .expect("supported target Python versions must be valid")
}

/// Parse a `requires_python` specifier string, handling `!=X.Y*` wildcard
/// comparisons and `>=X.Y.*` legacy forms that pep440_rs's string parser rejects.
fn parse_requires_python_spec(
    requires_python: &str,
) -> Result<VersionSpecifiers, String> {
    let normalized = super::normalize_legacy_wildcards(requires_python);
    let specifiers = normalized
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(parse_one_specifier)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(VersionSpecifiers::from_iter(specifiers))
}

fn parse_one_specifier(part: &str) -> Result<VersionSpecifier, String> {
    let maybe_star = part.strip_prefix("!=").and_then(|v| v.strip_suffix('*'));
    if let Some(v) = maybe_star {
        let v = v.trim().trim_end_matches('.');
        return Version::from_str(v)
            .map_err(|e| format!("Failed to parse version: {e}"))
            .map(VersionSpecifier::not_equals_star_version);
    }
    VersionSpecifier::from_str(part)
        .map_err(|e| format!("Failed to parse specifier: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_requires_python_with_star_wildcards() {
        let spec =
            parse_requires_python_spec(">=2.7,!=3.0*,!=3.1*,!=3.2*").unwrap();

        assert!(spec.contains(&Version::from_str("2.7.0").unwrap()));
        assert!(spec.contains(&Version::from_str("2.7.18").unwrap()));
        assert!(!spec.contains(&Version::from_str("3.0.0").unwrap()));
        assert!(!spec.contains(&Version::from_str("3.0.1").unwrap()));
        assert!(!spec.contains(&Version::from_str("3.1.0").unwrap()));
        assert!(!spec.contains(&Version::from_str("3.2.5").unwrap()));
        assert!(spec.contains(&Version::from_str("3.3.0").unwrap()));
        assert!(spec.contains(&Version::from_str("3.12.0").unwrap()));
    }

    #[test]
    fn test_parse_requires_python_without_wildcards() {
        let spec = parse_requires_python_spec(">=3.8").unwrap();
        assert!(spec.contains(&Version::from_str("3.8.0").unwrap()));
        assert!(spec.contains(&Version::from_str("3.12.0").unwrap()));
        assert!(!spec.contains(&Version::from_str("3.7.0").unwrap()));
    }

    #[test]
    fn test_parse_requires_python_not_equals_without_star() {
        let spec = parse_requires_python_spec("!=3.8").unwrap();
        assert!(!spec.contains(&Version::from_str("3.8.0").unwrap()));
        assert!(spec.contains(&Version::from_str("3.8.1").unwrap()));
        assert!(spec.contains(&Version::from_str("3.9.0").unwrap()));
    }

    #[test]
    fn test_parse_requires_python_empty_string() {
        let spec = parse_requires_python_spec("").unwrap();
        assert!(spec.contains(&Version::from_str("3.8.0").unwrap()));
        assert!(spec.contains(&Version::from_str("2.7.0").unwrap()));
    }

    #[test]
    fn test_parse_requires_python_invalid_wildcard_version() {
        let result = parse_requires_python_spec("!=abc*");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_requires_python_major_only_wildcard() {
        let spec = parse_requires_python_spec("!=3.*").unwrap();
        assert!(!spec.contains(&Version::from_str("3.0.0").unwrap()));
        assert!(!spec.contains(&Version::from_str("3.0.5").unwrap()));
        assert!(!spec.contains(&Version::from_str("3.1.0").unwrap()));
        assert!(!spec.contains(&Version::from_str("3.8.1").unwrap()));
        assert!(spec.contains(&Version::from_str("2.7.0").unwrap()));
        assert!(spec.contains(&Version::from_str("4.0.0").unwrap()));
    }

    #[test]
    fn test_parse_requires_python_mixed_wildcard_and_normal() {
        let spec = parse_requires_python_spec(">=3.8,!=3.10*,<4").unwrap();
        assert!(!spec.contains(&Version::from_str("3.7.0").unwrap()));
        assert!(spec.contains(&Version::from_str("3.8.0").unwrap()));
        assert!(spec.contains(&Version::from_str("3.9.0").unwrap()));
        assert!(!spec.contains(&Version::from_str("3.10.0").unwrap()));
        assert!(!spec.contains(&Version::from_str("3.10.5").unwrap()));
        assert!(!spec.contains(&Version::from_str("4.0.0").unwrap()));
    }

    #[test]
    fn test_parse_requires_python_error_cases() {
        assert!(parse_requires_python_spec("!=").is_err());
        assert!(parse_requires_python_spec(">=abc").is_err());
        assert!(parse_requires_python_spec("invalid").is_err());
    }

    #[test]
    fn test_parse_requires_python_gte_legacy_wildcard() {
        // nltk 3.6.2 uses >=3.5.*
        let spec = parse_requires_python_spec(">=3.5.*").unwrap();
        assert!(!spec.contains(&Version::from_str("3.4.0").unwrap()));
        assert!(spec.contains(&Version::from_str("3.5.0").unwrap()));
        assert!(spec.contains(&Version::from_str("3.5.1").unwrap()));
        assert!(spec.contains(&Version::from_str("3.12.0").unwrap()));
    }

    #[test]
    fn test_parse_requires_python_lte_legacy_wildcard() {
        let spec = parse_requires_python_spec("<=3.5.*").unwrap();
        assert!(spec.contains(&Version::from_str("3.5.0").unwrap()));
        assert!(!spec.contains(&Version::from_str("3.6.0").unwrap()));
    }
}
