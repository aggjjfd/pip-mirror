use std::collections::{HashMap, HashSet, VecDeque};

use futures::{StreamExt, stream};
use pep440_rs::Version;
use pubgrub::Range;

use super::eligibility::{SolveContext, candidate_versions_for_package};
use super::error::ResolveError;
use super::markers::ParsedDependency;
use super::pubgrub::spec_to_range;
use super::solve::{
    ActiveExtrasMap, package_extras, parse_version_dependencies,
};

type DependencyKey = (String, Version);

pub(crate) struct DiscoveredClosure {
    pub(crate) dependencies: HashMap<DependencyKey, Vec<ParsedDependency>>,
    pub(crate) discovered_versions: HashSet<DependencyKey>,
}

pub(crate) async fn discover_closure(
    ctx: &SolveContext<'_>,
    root_pkg: &str,
    root_ver: &Version,
    active_extras: &ActiveExtrasMap,
) -> Result<DiscoveredClosure, ResolveError> {
    let mut dependencies = HashMap::new();
    let mut discovered_versions = HashSet::new();
    let mut scheduled_versions =
        HashSet::from([(root_pkg.to_string(), root_ver.clone())]);
    let mut seen_constraints: HashMap<String, Range<Version>> = HashMap::new();
    let mut frontier =
        VecDeque::from([(root_pkg.to_string(), root_ver.clone())]);

    while !frontier.is_empty() {
        let batch = drain_frontier(&mut frontier, &mut discovered_versions);
        let parsed_nodes =
            fetch_dependency_batch(ctx, &batch, active_extras).await?;
        let changed_packages = merge_dependency_ranges(
            parsed_nodes,
            &mut dependencies,
            &mut seen_constraints,
        );
        expand_changed_packages(
            ctx,
            changed_packages,
            &seen_constraints,
            &mut scheduled_versions,
            &mut frontier,
        )
        .await?;
    }

    Ok(DiscoveredClosure {
        dependencies,
        discovered_versions,
    })
}

fn enqueue_if_new(
    key: DependencyKey,
    scheduled: &mut HashSet<DependencyKey>,
    queue: &mut VecDeque<DependencyKey>,
) {
    if scheduled.insert(key.clone()) {
        queue.push_back(key);
    }
}

fn drain_frontier(
    frontier: &mut VecDeque<DependencyKey>,
    discovered_versions: &mut HashSet<DependencyKey>,
) -> Vec<DependencyKey> {
    let batch: Vec<_> = frontier.drain(..).collect();
    discovered_versions.extend(batch.iter().cloned());
    batch
}

async fn fetch_dependency_batch(
    ctx: &SolveContext<'_>,
    batch: &[DependencyKey],
    active_extras: &ActiveExtrasMap,
) -> Result<Vec<(DependencyKey, Vec<ParsedDependency>)>, ResolveError> {
    let results = stream::iter(batch.iter().cloned())
        .map(|(package, version)| async move {
            let extras = package_extras(active_extras, &package);
            let parsed =
                parse_version_dependencies(ctx, &package, &version, &extras)
                    .await?;
            Ok::<_, ResolveError>(((package, version), parsed))
        })
        .buffer_unordered(ctx.metadata_workers)
        .collect::<Vec<_>>()
        .await;
    results.into_iter().collect()
}

fn merge_dependency_ranges(
    parsed_nodes: Vec<(DependencyKey, Vec<ParsedDependency>)>,
    dependencies: &mut HashMap<DependencyKey, Vec<ParsedDependency>>,
    seen_constraints: &mut HashMap<String, Range<Version>>,
) -> Vec<String> {
    let mut changed_packages = HashSet::new();
    for (key, parsed) in parsed_nodes {
        for dependency in &parsed {
            collect_changed_package(
                &mut changed_packages,
                seen_constraints,
                dependency,
            );
        }
        dependencies.insert(key, parsed);
    }
    let mut changed_packages: Vec<_> = changed_packages.into_iter().collect();
    changed_packages.sort();
    changed_packages
}

fn update_seen_constraint(
    seen_constraints: &mut HashMap<String, Range<Version>>,
    dependency: &ParsedDependency,
) -> bool {
    let next = spec_to_range(&dependency.version_spec);
    match seen_constraints.get_mut(&dependency.package_name) {
        Some(range) => {
            let merged = range.union(&next);
            if *range == merged {
                return false;
            }
            *range = merged;
            true
        }
        None => {
            seen_constraints.insert(dependency.package_name.clone(), next);
            true
        }
    }
}

fn collect_changed_package(
    changed_packages: &mut HashSet<String>,
    seen_constraints: &mut HashMap<String, Range<Version>>,
    dependency: &ParsedDependency,
) {
    if !update_seen_constraint(seen_constraints, dependency) {
        return;
    }
    changed_packages.insert(dependency.package_name.clone());
}

async fn expand_changed_packages(
    ctx: &SolveContext<'_>,
    changed_packages: Vec<String>,
    seen_constraints: &HashMap<String, Range<Version>>,
    scheduled_versions: &mut HashSet<DependencyKey>,
    queue: &mut VecDeque<DependencyKey>,
) -> Result<(), ResolveError> {
    let results = stream::iter(changed_packages)
        .map(|package| async move {
            let range = seen_constraints
                .get(&package)
                .expect("changed package must have range")
                .clone();
            let candidates =
                candidate_versions_for_package(ctx, &package, &range).await?;
            Ok::<_, ResolveError>((package, candidates))
        })
        .buffer_unordered(ctx.metadata_workers)
        .collect::<Vec<_>>()
        .await;
    for (package, candidates) in
        results.into_iter().collect::<Result<Vec<_>, _>>()?
    {
        for version in candidates {
            enqueue_if_new(
                (package.clone(), version),
                scheduled_versions,
                queue,
            );
        }
    }
    Ok(())
}
