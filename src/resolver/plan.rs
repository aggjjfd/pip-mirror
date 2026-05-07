use dashmap::DashMap;
use pep440_rs::Version;

use super::error::ResolveError;
use super::metadata::MetadataCache;
use super::solve::SolveResult;

/// Expand solved versions with adjacent versions on each side.
///
/// For each solved package@version, locate it in the full version list and
/// include `adjacent_versions_per_side` versions to the left and right.
/// The solved version itself is always included.
pub async fn expand_solved_versions(
    all_solutions: &[SolveResult],
    cache: &MetadataCache,
    adjacent_versions_per_side: usize,
    allow_prerelease: bool,
) -> Result<DashMap<String, Vec<Version>>, ResolveError> {
    let result: DashMap<String, Vec<Version>> = DashMap::new();

    for sol in all_solutions {
        for (pkg, ver) in &sol.solved_versions {
            let all_vers = cache.get_all_versions(pkg).await?;
            let filtered = filter_by_prerelease(all_vers, allow_prerelease);
            add_version_with_adjacent(
                &result,
                pkg,
                ver,
                &filtered,
                adjacent_versions_per_side,
            );
        }
    }

    normalize_versions(&result);
    Ok(result)
}

fn filter_by_prerelease(
    versions: Vec<Version>,
    allow_prerelease: bool,
) -> Vec<Version> {
    if allow_prerelease {
        return versions;
    }
    versions
        .into_iter()
        .filter(|v| !v.any_prerelease())
        .collect()
}

fn add_version_with_adjacent(
    result: &DashMap<String, Vec<Version>>,
    pkg: &str,
    ver: &Version,
    filtered: &[Version],
    adjacent: usize,
) {
    let idx = match filtered.iter().position(|v| v == ver) {
        Some(i) => i,
        None => {
            result.entry(pkg.to_string()).or_default().push(ver.clone());
            return;
        }
    };

    let start = idx.saturating_sub(adjacent);
    let end = (idx + adjacent + 1).min(filtered.len());

    let mut entry = result.entry(pkg.to_string()).or_default();
    entry.extend(filtered[start..end].iter().cloned());
}

fn normalize_versions(result: &DashMap<String, Vec<Version>>) {
    for mut entry in result.iter_mut() {
        let v = entry.value_mut();
        v.sort_by(|a, b| b.cmp(a));
        v.dedup();
    }
}
