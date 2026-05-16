use std::collections::{HashMap, HashSet, VecDeque};

use futures::{StreamExt, stream};
use pep440_rs::Version;
use pubgrub::Range;

use super::eligibility::{SolveContext, candidate_versions_for_package};
use super::error::ResolveError;
use super::markers::ParsedDependency;
use super::pubgrub::spec_to_range;
use super::solve::parse_version_dependencies;
use super::{ActiveExtrasMap, package_extras};

type DependencyKey = (String, Version);

pub(crate) struct DiscoveryEngine {
    pub(crate) dependencies: HashMap<DependencyKey, Vec<ParsedDependency>>,
    pub(crate) discovered_versions: HashSet<DependencyKey>,
    scheduled_versions: HashSet<DependencyKey>,
    seen_constraints: HashMap<String, Range<Version>>,
    frontier: VecDeque<DependencyKey>,
}

impl DiscoveryEngine {
    pub(crate) fn with_root(root_pkg: String, root_ver: Version) -> Self {
        let mut scheduled = HashSet::new();
        let mut frontier = VecDeque::new();
        let key = (root_pkg, root_ver);
        scheduled.insert(key.clone());
        frontier.push_back(key);
        Self {
            dependencies: HashMap::new(),
            discovered_versions: HashSet::new(),
            scheduled_versions: scheduled,
            seen_constraints: HashMap::new(),
            frontier,
        }
    }

    pub(crate) async fn run(
        &mut self,
        ctx: &SolveContext<'_>,
        active_extras: &ActiveExtrasMap,
    ) -> Result<(), ResolveError> {
        while !self.frontier.is_empty() {
            let batch = self.drain_frontier();
            let parsed_nodes =
                fetch_dependency_batch(ctx, &batch, active_extras).await?;
            let changed_packages = merge_dependency_ranges(
                parsed_nodes,
                &mut self.dependencies,
                &mut self.seen_constraints,
            );
            expand_changed_packages(
                ctx,
                changed_packages,
                &self.seen_constraints,
                &mut self.scheduled_versions,
                &mut self.frontier,
            )
            .await?;
        }
        Ok(())
    }

    fn drain_frontier(&mut self) -> Vec<DependencyKey> {
        let batch: Vec<_> = self.frontier.drain(..).collect();
        self.discovered_versions.extend(batch.iter().cloned());
        batch
    }

    pub(crate) fn reprocess(&mut self, package: &str, version: &Version) {
        let key = (package.to_string(), version.clone());
        self.dependencies.remove(&key);
        if self.discovered_versions.remove(&key) {
            self.scheduled_versions.remove(&key);
        }
        if self.scheduled_versions.insert(key.clone()) {
            self.frontier.push_back(key);
        }
        self.seen_constraints.remove(package);
    }
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
