use std::collections::{HashMap, HashSet};

use dashmap::DashMap;
use futures::{StreamExt, stream};
use pep440_rs::Version;

use crate::resolver::error::ResolveError;
use crate::resolver::metadata::MetadataCache;
use crate::resolver::solve::SolveResult;

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
    metadata_workers: usize,
) -> Result<DashMap<String, Vec<Version>>, ResolveError> {
    let result: DashMap<String, Vec<Version>> = DashMap::new();
    let solved_versions = collect_solved_versions(all_solutions);
    let filtered_versions = fetch_filtered_versions(
        cache,
        &solved_versions,
        allow_prerelease,
        metadata_workers,
    )
    .await?;

    for (pkg, versions) in solved_versions {
        let filtered = filtered_versions
            .get(&pkg)
            .expect("solved package must have fetched version list");
        for ver in versions {
            add_version_with_adjacent(
                &result,
                &pkg,
                &ver,
                filtered,
                adjacent_versions_per_side,
            );
        }
    }

    normalize_versions(&result);
    Ok(result)
}

fn collect_solved_versions(
    all_solutions: &[SolveResult],
) -> HashMap<String, HashSet<Version>> {
    let mut solved = HashMap::new();
    for sol in all_solutions {
        for (pkg, ver) in &sol.solved_versions {
            solved
                .entry(pkg.clone())
                .or_insert_with(HashSet::new)
                .insert(ver.clone());
        }
    }
    solved
}

async fn fetch_filtered_versions(
    cache: &MetadataCache,
    solved_versions: &HashMap<String, HashSet<Version>>,
    allow_prerelease: bool,
    metadata_workers: usize,
) -> Result<HashMap<String, Vec<Version>>, ResolveError> {
    let mut packages: Vec<_> = solved_versions.keys().cloned().collect();
    packages.sort();

    let results = stream::iter(packages)
        .map(|package| async move {
            let versions = cache.get_all_versions(&package).await?;
            Ok::<_, ResolveError>((
                package,
                filter_by_prerelease(versions, allow_prerelease),
            ))
        })
        .buffer_unordered(metadata_workers)
        .collect::<Vec<_>>()
        .await;
    results.into_iter().collect()
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
