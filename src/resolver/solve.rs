use pep440_rs::Version;
use pubgrub::{
    DefaultStringReporter, OfflineDependencyProvider, PubGrubError, Range,
    Reporter as _, SelectedDependencies,
};
use std::collections::{HashMap, HashSet, VecDeque};

use super::eligibility::{SolveContext, candidate_versions_for_package};
use super::error::ResolveError;
use super::markers::{ParsedDependency, parse_requires_dist};
use super::pubgrub::spec_to_range;

const ROOT_PACKAGE: &str = "__root__";

type DependencyKey = (String, Version);
pub type ActiveExtrasMap = HashMap<String, HashSet<String>>;
pub type SolvedVersions = HashMap<String, Version>;

pub struct SolveResult {
    pub solved_versions: SolvedVersions,
    pub active_extras: ActiveExtrasMap,
}

struct DiscoveredClosure {
    dependencies: HashMap<DependencyKey, Vec<ParsedDependency>>,
    discovered_versions: HashSet<DependencyKey>,
}

pub async fn solve_one_target(
    ctx: &SolveContext<'_>,
    root_pkg: &str,
    root_ver: &Version,
    root_extras: &HashSet<String>,
) -> Result<SolveResult, ResolveError> {
    let root_pkg = crate::filters::normalize_package_name(root_pkg);
    let mut active_extras = initial_active_extras(&root_pkg, root_extras);

    loop {
        let closure =
            discover_closure(ctx, &root_pkg, root_ver, &active_extras).await?;
        let solved_versions =
            solve_discovered_graph(ctx, &root_pkg, root_ver, &closure)?;
        let next_active_extras = collect_solution_extras(
            ctx,
            &root_pkg,
            root_extras,
            &solved_versions,
        )
        .await?;

        if next_active_extras == active_extras {
            return Ok(SolveResult {
                solved_versions,
                active_extras: next_active_extras,
            });
        }
        active_extras = next_active_extras;
    }
}

fn initial_active_extras(
    root_pkg: &str,
    root_extras: &HashSet<String>,
) -> ActiveExtrasMap {
    let mut active_extras = ActiveExtrasMap::new();
    active_extras.insert(root_pkg.to_string(), root_extras.clone());
    active_extras
}

async fn discover_closure(
    ctx: &SolveContext<'_>,
    root_pkg: &str,
    root_ver: &Version,
    active_extras: &ActiveExtrasMap,
) -> Result<DiscoveredClosure, ResolveError> {
    let mut dependencies = HashMap::new();
    let mut discovered_versions = HashSet::new();
    let mut scheduled_versions =
        HashSet::from([(root_pkg.to_string(), root_ver.clone())]);
    let mut seen_constraints: HashMap<String, Vec<Range<Version>>> =
        HashMap::new();
    let mut queue = VecDeque::from([(root_pkg.to_string(), root_ver.clone())]);

    while let Some((package, version)) = queue.pop_front() {
        if !discovered_versions.insert((package.clone(), version.clone())) {
            continue;
        }

        let extras = package_extras(active_extras, &package);
        let parsed =
            parse_version_dependencies(ctx, &package, &version, &extras)
                .await?;
        enqueue_dependency_candidates(
            ctx,
            &parsed,
            &mut seen_constraints,
            &mut scheduled_versions,
            &mut queue,
        )
        .await?;
        dependencies.insert((package, version), parsed);
    }

    Ok(DiscoveredClosure {
        dependencies,
        discovered_versions,
    })
}

fn package_extras(
    active_extras: &ActiveExtrasMap,
    package: &str,
) -> HashSet<String> {
    active_extras.get(package).cloned().unwrap_or_default()
}

async fn parse_version_dependencies(
    ctx: &SolveContext<'_>,
    package: &str,
    version: &Version,
    extras: &HashSet<String>,
) -> Result<Vec<ParsedDependency>, ResolveError> {
    let requires_dist = ctx.cache.get_requires_dist(package, version).await?;
    Ok(parse_requires_dist(&requires_dist, extras, ctx.target)?)
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

async fn enqueue_dependency_candidates(
    ctx: &SolveContext<'_>,
    dependencies: &[ParsedDependency],
    seen_constraints: &mut HashMap<String, Vec<Range<Version>>>,
    scheduled_versions: &mut HashSet<DependencyKey>,
    queue: &mut VecDeque<DependencyKey>,
) -> Result<(), ResolveError> {
    for dependency in dependencies {
        let ranges = seen_constraints
            .entry(dependency.package_name.clone())
            .or_default();
        ranges.push(spec_to_range(&dependency.version_spec));

        let candidates = candidate_versions_for_package(
            ctx,
            &dependency.package_name,
            |version| ranges.iter().any(|range| range.contains(version)),
        )
        .await?;
        for version in candidates {
            let key = (dependency.package_name.clone(), version);
            enqueue_if_new(key, scheduled_versions, queue);
        }
    }
    Ok(())
}

fn solve_discovered_graph(
    ctx: &SolveContext<'_>,
    root_pkg: &str,
    root_ver: &Version,
    closure: &DiscoveredClosure,
) -> Result<SolvedVersions, ResolveError> {
    let mut provider = OfflineDependencyProvider::new();
    provider.add_dependencies(
        ROOT_PACKAGE.to_string(),
        synthetic_root_version(),
        vec![(root_pkg.to_string(), Range::singleton(root_ver.clone()))],
    );

    for (package, version) in &closure.discovered_versions {
        let deps = closure
            .dependencies
            .get(&(package.clone(), version.clone()))
            .cloned()
            .unwrap_or_default();
        let dependency_ranges = deps.into_iter().map(|dependency| {
            (
                dependency.package_name,
                spec_to_range(&dependency.version_spec),
            )
        });
        provider.add_dependencies(
            package.clone(),
            version.clone(),
            dependency_ranges,
        );
    }

    let root_version = synthetic_root_version();
    let solution =
        pubgrub::resolve(&provider, ROOT_PACKAGE.to_string(), root_version);
    extract_solution(ctx, root_pkg, root_ver, solution)
}

type PubGrubSolution = Result<
    SelectedDependencies<OfflineDependencyProvider<String, Range<Version>>>,
    PubGrubError<OfflineDependencyProvider<String, Range<Version>>>,
>;

fn extract_solution(
    ctx: &SolveContext<'_>,
    root_pkg: &str,
    root_ver: &Version,
    solution: PubGrubSolution,
) -> Result<SolvedVersions, ResolveError> {
    match solution {
        Ok(solution) => Ok(solution
            .into_iter()
            .filter(|(package, _)| package != ROOT_PACKAGE)
            .collect()),
        Err(PubGrubError::NoSolution(tree)) => Err(ResolveError::NoSolution {
            package: root_pkg.to_string(),
            version: root_ver.clone(),
            target: ctx.target.to_string(),
            detail: DefaultStringReporter::report(&tree),
        }),
        Err(err) => Err(ResolveError::NoSolution {
            package: root_pkg.to_string(),
            version: root_ver.clone(),
            target: ctx.target.to_string(),
            detail: err.to_string(),
        }),
    }
}

fn synthetic_root_version() -> Version {
    Version::new([0, 0, 0])
}

async fn collect_solution_extras(
    ctx: &SolveContext<'_>,
    root_pkg: &str,
    root_extras: &HashSet<String>,
    solved_versions: &SolvedVersions,
) -> Result<ActiveExtrasMap, ResolveError> {
    let mut active_extras = initial_active_extras(root_pkg, root_extras);
    let mut processed: HashMap<String, HashSet<String>> = HashMap::new();
    let mut queue = VecDeque::from([root_pkg.to_string()]);

    while let Some(package) = queue.pop_front() {
        let Some(version) = solved_versions.get(&package) else {
            continue;
        };
        let current_extras = package_extras(&active_extras, &package);
        if processed.get(&package) == Some(&current_extras) {
            continue;
        }
        processed.insert(package.clone(), current_extras.clone());

        let deps =
            parse_version_dependencies(ctx, &package, version, &current_extras)
                .await?;
        schedule_selected_dependencies(
            deps,
            solved_versions,
            &mut active_extras,
            &mut queue,
        );
    }

    Ok(active_extras)
}

fn schedule_selected_dependencies(
    dependencies: Vec<ParsedDependency>,
    solved_versions: &SolvedVersions,
    active_extras: &mut ActiveExtrasMap,
    queue: &mut VecDeque<String>,
) {
    for dependency in dependencies {
        if !solved_versions.contains_key(&dependency.package_name) {
            continue;
        }

        queue.push_back(dependency.package_name.clone());
        if dependency.extras.is_empty() {
            continue;
        }

        let entry = active_extras
            .entry(dependency.package_name.clone())
            .or_default();
        let old_len = entry.len();
        entry.extend(dependency.extras);
        if entry.len() > old_len {
            queue.push_back(dependency.package_name);
        }
    }
}
