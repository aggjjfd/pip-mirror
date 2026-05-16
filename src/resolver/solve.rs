use pep440_rs::Version;
use pubgrub::{
    DefaultStringReporter, OfflineDependencyProvider, PubGrubError, Range,
    Reporter as _, SelectedDependencies,
};
use std::collections::{HashMap, HashSet, VecDeque};
use type_state_builder::TypeStateBuilder;

use super::discovery::DiscoveryEngine;
use super::eligibility::{ParsedDepsCacheKey, SolveContext};
use super::error::ResolveError;
use super::markers::{ParsedDependency, parse_requires_dist};
use super::pubgrub::spec_to_range;
use super::{ActiveExtrasMap, package_extras};

const ROOT_PACKAGE: &str = "__root__";

pub type SolvedVersions = HashMap<String, Version>;

#[derive(Clone)]
pub struct SolveResult {
    pub solved_versions: SolvedVersions,
    pub active_extras: ActiveExtrasMap,
}

#[derive(TypeStateBuilder)]
#[builder(impl_into)]
struct SolveFixpointParams<'a> {
    #[builder(required)]
    ctx: &'a SolveContext<'a>,
    #[builder(required)]
    root_pkg: &'a str,
    #[builder(required)]
    root_ver: &'a Version,
    #[builder(required)]
    root_extras: &'a HashSet<String>,
}

fn reprocess_changed_packages(
    engine: &mut DiscoveryEngine,
    active_extras: &ActiveExtrasMap,
    next_active_extras: &ActiveExtrasMap,
    solved_versions: &SolvedVersions,
) {
    for (pkg, new_extras) in next_active_extras {
        let old_extras = active_extras.get(pkg);
        if old_extras != Some(new_extras)
            && let Some(version) = solved_versions.get(pkg)
        {
            engine.reprocess(pkg, version);
        }
    }
}

async fn run_iteration(
    p: &SolveFixpointParams<'_>,
    engine: &DiscoveryEngine,
) -> Result<(SolvedVersions, ActiveExtrasMap), ResolveError> {
    let solved_versions =
        solve_discovered_graph(p.ctx, p.root_pkg, p.root_ver, engine)?;
    let next_active_extras = collect_solution_extras(
        p.ctx,
        p.root_pkg,
        p.root_extras,
        &solved_versions,
    )
    .await?;
    Ok((solved_versions, next_active_extras))
}

async fn solve_fixpoint(
    p: &SolveFixpointParams<'_>,
    engine: &mut DiscoveryEngine,
    active_extras: &mut ActiveExtrasMap,
) -> Result<SolveResult, ResolveError> {
    loop {
        let (solved_versions, next_active_extras) =
            run_iteration(p, engine).await?;

        if next_active_extras == *active_extras {
            return Ok(SolveResult {
                solved_versions,
                active_extras: next_active_extras,
            });
        }

        reprocess_changed_packages(
            engine,
            active_extras,
            &next_active_extras,
            &solved_versions,
        );

        *active_extras = next_active_extras;
        engine.run(p.ctx, active_extras).await?;
    }
}

pub async fn solve_one_target(
    ctx: &SolveContext<'_>,
    root_pkg: &str,
    root_ver: &Version,
    root_extras: &HashSet<String>,
) -> Result<SolveResult, ResolveError> {
    let root_pkg = crate::filters::normalize_package_name(root_pkg);
    let mut active_extras = initial_active_extras(&root_pkg, root_extras);

    let mut engine =
        DiscoveryEngine::with_root(root_pkg.clone(), root_ver.clone());
    engine.run(ctx, &active_extras).await?;

    let params = SolveFixpointParams::builder()
        .ctx(ctx)
        .root_pkg(&*root_pkg)
        .root_ver(root_ver)
        .root_extras(root_extras)
        .build();
    solve_fixpoint(&params, &mut engine, &mut active_extras).await
}

fn initial_active_extras(
    root_pkg: &str,
    root_extras: &HashSet<String>,
) -> ActiveExtrasMap {
    let mut active_extras = ActiveExtrasMap::new();
    active_extras.insert(root_pkg.to_string(), root_extras.clone());
    active_extras
}

pub(crate) async fn parse_version_dependencies(
    ctx: &SolveContext<'_>,
    package: &str,
    version: &Version,
    extras: &HashSet<String>,
) -> Result<Vec<ParsedDependency>, ResolveError> {
    let mut extras_vec: Vec<String> = extras.iter().cloned().collect();
    extras_vec.sort();
    let key = ParsedDepsCacheKey {
        package: package.to_string(),
        version: version.clone(),
        target: ctx.target.clone(),
        extras: extras_vec,
    };

    if let Some(cache) = ctx.parsed_deps_cache
        && let Some(cached) = cache.get(&key)
    {
        return Ok(cached.clone());
    }

    let requires_dist = ctx.cache.get_requires_dist(package, version).await?;
    let result = parse_requires_dist(&requires_dist, extras, ctx.target)?;

    if let Some(cache) = ctx.parsed_deps_cache {
        cache.insert(key, result.clone());
    }

    Ok(result)
}

fn solve_discovered_graph(
    ctx: &SolveContext<'_>,
    root_pkg: &str,
    root_ver: &Version,
    engine: &DiscoveryEngine,
) -> Result<SolvedVersions, ResolveError> {
    let mut provider = OfflineDependencyProvider::new();
    provider.add_dependencies(
        ROOT_PACKAGE.to_string(),
        synthetic_root_version(),
        vec![(root_pkg.to_string(), Range::singleton(root_ver.clone()))],
    );

    for (package, version) in &engine.discovered_versions {
        let deps = engine
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
