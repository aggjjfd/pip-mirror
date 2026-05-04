use std::collections::{HashMap, HashSet};

use dashmap::DashMap;
use pep440_rs::Version;
use pubgrub::OfflineDependencyProvider;
use pubgrub::Ranges;
use tracing::{info, warn};

use super::pubgrub::{
    ProviderCtx, Solution, bare_name, build_provider, compute_version_windows,
    dep_to_range, parse_python_requires,
};
use super::types::all_targets;

pub struct ResolveParams<'a> {
    pub top_packages: &'a [String],
    pub top_versions: &'a DashMap<String, Vec<Version>>,
    pub pypi_url: &'a str,
    pub max_depth: usize,
    pub max_versions: usize,
}

type CacheType = HashMap<(String, String), Vec<(String, String)>>;

async fn fetch_versions(
    client: &reqwest::Client,
    pkg: &str,
    pypi: &str,
) -> Option<Vec<Version>> {
    crate::downloader::get_all_versions(client, pkg, pypi)
        .await
        .ok()
        .filter(|v| !v.is_empty())
}

struct ExploreCtx<'a, 'b> {
    pkg: &'a str,
    all_vers: &'a [Version],
    params: &'a ResolveParams<'b>,
    client: &'a reqwest::Client,
    pkg_set: &'a mut HashSet<String>,
    queue: &'a mut Vec<(String, usize)>,
    depth: usize,
    req_cache: &'a mut CacheType,
}

async fn explore_pkg_deps(ctx: ExploreCtx<'_, '_>) {
    let explore: Vec<Version> = ctx.params.top_versions.get(ctx.pkg)
        .map(|tv| tv.iter().cloned().collect())
        .unwrap_or_else(|| ctx.all_vers.iter().take(1).cloned().collect());
    for ver in &explore {
        let vs = ver.to_string();
        let Ok(Some(rd)) = crate::downloader::get_requires_dist(ctx.client, ctx.pkg, &vs, ctx.params.pypi_url).await else { continue };
        let deps = parse_python_requires(&rd);
        for (dn, _) in &deps {
            let dn = crate::filters::normalize_package_name(dn);
            if !ctx.pkg_set.contains(&dn) { ctx.pkg_set.insert(dn.clone()); ctx.queue.push((dn.clone(), ctx.depth + 1)); }
        }
        ctx.req_cache.insert((ctx.pkg.to_string(), vs), deps);
    }
}

async fn bfs_collect(
    params: &ResolveParams<'_>,
    client: &reqwest::Client,
) -> (HashSet<String>, HashMap<String, Vec<Version>>, CacheType) {
    let top_names: Vec<_> =
        params.top_packages.iter().map(|r| bare_name(r)).collect();
    let mut pkg_set: HashSet<String> = top_names.iter().cloned().collect();
    let mut queue: Vec<_> = top_names.iter().map(|n| (n.clone(), 0)).collect();
    let mut pkg_versions: HashMap<String, Vec<Version>> = HashMap::new();
    let mut req_cache = HashMap::new();

    while let Some((pkg, depth)) = queue.pop() {
        let Some(all_vers) =
            fetch_versions(client, &pkg, params.pypi_url).await
        else {
            continue;
        };
        pkg_versions.entry(pkg.clone()).or_insert(all_vers.clone());
        if depth < params.max_depth {
            explore_pkg_deps(ExploreCtx { pkg: &pkg, all_vers: &all_vers, params, client, pkg_set: &mut pkg_set, queue: &mut queue, depth, req_cache: &mut req_cache }).await;
        }
    }
    (pkg_set, pkg_versions, req_cache)
}

async fn get_deps(
    pkg: &str,
    ver: &str,
    req_cache: &mut CacheType,
    client: &reqwest::Client,
    pypi: &str,
) -> Vec<(String, String)> {
    let key = (pkg.to_string(), ver.to_string());
    if let Some(cached) = req_cache.get(&key) {
        return cached.clone();
    }
    let deps = match crate::downloader::get_requires_dist(
        client, pkg, ver, pypi,
    )
    .await
    {
        Ok(Some(rd)) => parse_python_requires(&rd),
        _ => vec![],
    };
    req_cache.insert(key, deps.clone());
    deps
}

async fn build_deps_map(
    pkg_set: &HashSet<String>,
    pkg_versions: &HashMap<String, Vec<Version>>,
    req_cache: &mut CacheType,
    client: &reqwest::Client,
    pypi: &str,
) -> HashMap<(String, usize), Vec<(String, String)>> {
    let mut deps_map = HashMap::new();
    for pkg in pkg_set {
        let Some(vers) = pkg_versions.get(pkg) else {
            continue;
        };
        for (idx, ver) in vers.iter().enumerate() {
            let vs = ver.to_string();
            deps_map.insert(
                (pkg.clone(), idx),
                get_deps(pkg, &vs, req_cache, client, pypi).await,
            );
        }
    }
    deps_map
}

fn pkg_dep_ranges(
    pkg: &str,
    idx: usize,
    deps_map: &HashMap<(String, usize), Vec<(String, String)>>,
    pkg_versions: &HashMap<String, Vec<Version>>,
) -> Vec<(String, Ranges<u32>)> {
    deps_map
        .get(&(pkg.to_string(), idx))
        .map(|ds| {
            ds.iter()
                .filter_map(|(n, s)| dep_to_range(n, s, pkg_versions))
                .collect()
        })
        .unwrap_or_default()
}

fn add_pkg_to_provider(
    pkg: &str,
    vers: &[Version],
    deps_map: &HashMap<(String, usize), Vec<(String, String)>>,
    pkg_versions: &HashMap<String, Vec<Version>>,
    rp: &mut OfflineDependencyProvider<String, Ranges<u32>>,
) {
    for (idx, _v) in vers.iter().enumerate() {
        rp.add_dependencies(
            pkg.to_string(),
            idx.try_into().unwrap_or(0u32),
            pkg_dep_ranges(pkg, idx, deps_map, pkg_versions),
        );
    }
}

fn collect_solution(
    solution: impl IntoIterator<Item = (String, u32)>,
    pkg_versions: &HashMap<String, Vec<Version>>,
) -> Solution {
    let mut sol: Solution = DashMap::new();
    for (pkg, vi) in solution {
        if pkg == "__root__" {
            continue;
        }
        if let Some(vers) = pkg_versions.get(&pkg) {
            if let Some(v) = vers.get(usize::try_from(vi).unwrap_or(0)) {
                sol.insert(pkg, v.clone());
            }
        }
    }
    sol
}

fn resolve_one(
    top_pkg: &str,
    pkg_set: &HashSet<String>,
    pkg_versions: &HashMap<String, Vec<Version>>,
    deps_map: &HashMap<(String, usize), Vec<(String, String)>>,
    top_versions: &DashMap<String, Vec<Version>>,
) -> Option<Solution> {
    let tvers = pkg_versions.get(top_pkg)?;
    let top_ver = top_versions
        .get(top_pkg)
        .and_then(|tv| tv.first().cloned())
        .unwrap_or_else(|| tvers[0].clone());
    let top_idx = tvers.iter().position(|v| *v == top_ver)?;

    let ctx = ProviderCtx {
        packages: pkg_set,
        top_pkg,
        top_ver: &top_ver,
        versions: pkg_versions,
        deps: deps_map,
    };
    let _provider = build_provider(&ctx);

    let mut rp = OfflineDependencyProvider::new();
    rp.add_dependencies(
        "__root__".to_string(),
        0u32,
        vec![(
            top_pkg.to_string(),
            Ranges::singleton(top_idx.try_into().unwrap_or(0u32)),
        )],
    );
    for pkg in pkg_set {
        let Some(vers) = pkg_versions.get(pkg) else {
            continue;
        };
        add_pkg_to_provider(pkg, vers, deps_map, pkg_versions, &mut rp);
    }
    pubgrub::resolve(&rp, "__root__".to_string(), 0u32).ok().map(|s| collect_solution(s, pkg_versions))
}

pub async fn resolve_dependencies(
    params: &ResolveParams<'_>,
    client: &reqwest::Client,
) -> DashMap<String, Vec<Version>> {
    info!("解析依赖: {} 个顶层包", params.top_packages.len());
    let (pkg_set, pkg_versions, mut req_cache) =
        bfs_collect(params, client).await;
    info!("  收集完成: {} 个包", pkg_set.len());
    let deps_map = build_deps_map(
        &pkg_set,
        &pkg_versions,
        &mut req_cache,
        client,
        params.pypi_url,
    )
    .await;
    let top_names: Vec<_> =
        params.top_packages.iter().map(|r| bare_name(r)).collect();
    let targets = all_targets();
    let mut all_solutions: Vec<Solution> = Vec::new();

    for target in &targets {
        for top_pkg in &top_names {
            if let Some(sol) = resolve_one(
                top_pkg,
                &pkg_set,
                &pkg_versions,
                &deps_map,
                params.top_versions,
            ) {
                all_solutions.push(sol);
            } else {
                warn!("  {target} / {top_pkg} pubgrub UNSAT");
            }
        }
    }

    info!("  解析完成: {} 个解", all_solutions.len());
    if all_solutions.is_empty() {
        warn!("所有 target 均无有效解");
        return DashMap::new();
    }

    let av: DashMap<String, Vec<Version>> = pkg_versions.into_iter().collect();
    let mut result =
        compute_version_windows(&all_solutions, &av, params.max_versions);
    for name in &top_names {
        result.remove(name);
    }
    info!("  依赖解析完成: {} 个包", result.len());
    result
}
