use super::pubgrub::{
    Solution, bare_name, collect_pkg_extras, compute_version_windows,
    parse_python_requires, spec_to_range,
};
use crate::downloader::HttpCtx;
use dashmap::DashMap;
use pep440_rs::Version;
use pubgrub::{OfflineDependencyProvider, Range};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{debug, info, warn};
pub struct ResolveParams<'a> {
    pub top_packages: &'a [String],
    pub top_versions: &'a DashMap<String, Vec<Version>>,
    pub pypi_url: &'a str,
    pub max_depth: usize,
    pub max_versions: usize,
    pub allow_prerelease: bool,
}
type CacheType = Arc<DashMap<(String, String), Vec<(String, String)>>>;
async fn fetch_versions(
    http: &HttpCtx<'_>,
    pkg: &str,
    allow_prerelease: bool,
) -> Option<Vec<Version>> {
    crate::resolver::metadata::get_all_versions(http, pkg, allow_prerelease)
        .await
        .ok()
        .filter(|v| !v.is_empty())
}
#[allow(clippy::too_many_arguments)]
async fn explore_pkg_deps(
    http: &HttpCtx<'_>,
    pkg: &str,
    all_vers: &[Version],
    top_versions: &DashMap<String, Vec<Version>>,
    pkg_set: &mut HashSet<String>,
    queue: &mut Vec<(String, usize, HashSet<String>)>,
    depth: usize,
    cache: &CacheType,
    allow_prerelease: bool,
    extras: &HashSet<String>,
) {
    let explore: Vec<Version> = top_versions
        .get(pkg)
        .map(|tv| tv.iter().cloned().collect())
        .unwrap_or_else(|| {
            // Non-top packages: explore latest (stable if !allow_prerelease)
            let mut v = all_vers.to_vec();
            if !allow_prerelease {
                v.retain(|ver| !ver.any_prerelease());
            }
            v.truncate(1);
            v
        });
    for ver in &explore {
        let vs = ver.to_string();
        let Ok(Some(rd)) =
            crate::resolver::metadata::get_requires_dist(http, pkg, &vs).await
        else {
            continue;
        };
        let deps = parse_python_requires(&rd, extras);
        let new_deps: Vec<String> = deps
            .iter()
            .map(|(dn, _)| crate::filters::normalize_package_name(dn))
            .filter(|dn| !pkg_set.contains(dn))
            .collect();
        for dn in &new_deps {
            pkg_set.insert(dn.clone());
            queue.push((dn.clone(), depth + 1, HashSet::new()));
        }
        cache.insert((pkg.to_string(), vs), deps);
    }
}
async fn bfs_collect(
    params: &ResolveParams<'_>,
    http: &HttpCtx<'_>,
    pkg_extras: &HashMap<String, HashSet<String>>,
) -> (HashSet<String>, HashMap<String, Vec<Version>>, CacheType) {
    let top_names: Vec<_> =
        params.top_packages.iter().map(|r| bare_name(r)).collect();
    let mut pkg_set: HashSet<String> = top_names.iter().cloned().collect();
    let mut queue: Vec<_> = top_names
        .iter()
        .map(|n| {
            let extras = pkg_extras.get(n).cloned().unwrap_or_default();
            (n.clone(), 0, extras)
        })
        .collect();
    let mut pkg_versions: HashMap<String, Vec<Version>> = HashMap::new();
    let cache: CacheType = Arc::new(DashMap::new());
    while let Some((pkg, depth, extras)) = queue.pop() {
        let Some(all_vers) =
            fetch_versions(http, &pkg, params.allow_prerelease).await
        else {
            continue;
        };
        pkg_versions.entry(pkg.clone()).or_insert(all_vers.clone());
        if depth < params.max_depth {
            explore_pkg_deps(
                http,
                &pkg,
                &all_vers,
                params.top_versions,
                &mut pkg_set,
                &mut queue,
                depth,
                &cache,
                params.allow_prerelease,
                &extras,
            )
            .await;
        }
    }
    (pkg_set, pkg_versions, cache)
}
struct FetchCtx<'a> {
    http: &'a HttpCtx<'a>,
    cache: &'a CacheType,
    max_versions: usize,
    pkg_extras: &'a HashMap<String, HashSet<String>>,
}
async fn fetch_pkg_deps(
    pkg: String,
    vers: Vec<Version>,
    ctx: &FetchCtx<'_>,
) -> Vec<((String, String), Vec<(String, String)>)> {
    let mut results = Vec::new();
    let pkg_key = pkg.clone();
    let extras = ctx.pkg_extras.get(&pkg).cloned().unwrap_or_default();
    for ver in &vers {
        let vs = ver.to_string();
        let key = (pkg_key.clone(), vs.clone());
        let deps = if let Some(c) = ctx.cache.get(&key) {
            c.clone()
        } else {
            let d = match crate::resolver::metadata::get_requires_dist(
                ctx.http, &pkg, &vs,
            )
            .await
            {
                Ok(Some(rd)) => parse_python_requires(&rd, &extras),
                _ => vec![],
            };
            ctx.cache.insert(key.clone(), d.clone());
            d
        };
        results.push((key, deps));
    }
    results
}
async fn build_deps_map(
    pkg_set: &HashSet<String>,
    pkg_versions: &HashMap<String, Vec<Version>>,
    ctx: &FetchCtx<'_>,
) -> HashMap<(String, String), Vec<(String, String)>> {
    let window = if ctx.max_versions == 0 {
        usize::MAX
    } else {
        ctx.max_versions
    };
    let mut handles = Vec::new();
    for pkg in pkg_set {
        let Some(vers) = pkg_versions.get(pkg) else {
            continue;
        };
        handles.push(fetch_pkg_deps(
            pkg.clone(),
            vers.iter().take(window).cloned().collect(),
            ctx,
        ));
    }
    let mut deps_map = HashMap::new();
    for results in futures::future::join_all(handles).await {
        for (key, deps) in results {
            deps_map.insert(key, deps);
        }
    }
    deps_map
}
fn build_root_range(
    top_pkg: &str,
    tvers: &[Version],
    top_versions: &DashMap<String, Vec<Version>>,
    allow_prerelease: bool,
) -> Range<Version> {
    top_versions
        .get(top_pkg)
        .map(|tv| {
            tv.iter()
                .filter(|v| allow_prerelease || !v.any_prerelease())
                .filter(|v| tvers.contains(v))
                .fold(Range::empty(), |r, v| {
                    r.union(&Range::singleton(v.clone()))
                })
        })
        .unwrap_or_else(Range::full)
}
fn collect_solution(
    solution: impl IntoIterator<Item = (String, Version)>,
) -> Solution {
    let sol: Solution = DashMap::new();
    for (pkg, ver) in solution {
        if pkg != "__root__" {
            sol.insert(pkg, ver);
        }
    }
    sol
}
#[allow(clippy::too_many_arguments, clippy::excessive_nesting)]
fn populate_provider(
    rp: &mut OfflineDependencyProvider<String, Range<Version>>,
    pkg_set: &HashSet<String>,
    pkg_versions: &HashMap<String, Vec<Version>>,
    deps_map: &HashMap<(String, String), Vec<(String, String)>>,
    allow_prerelease: bool,
) {
    for pkg in pkg_set {
        let Some(vers) = pkg_versions.get(pkg) else {
            continue;
        };
        for ver in vers {
            if !allow_prerelease && ver.any_prerelease() {
                continue;
            }
            let cache_key = (pkg.clone(), ver.to_string());
            let Some(deps_raw) = deps_map.get(&cache_key) else {
                continue;
            };
            let deps: Vec<(String, Range<Version>)> = deps_raw
                .iter()
                .filter(|(n, _)| pkg_set.contains(n))
                .map(|(n, s)| (n.clone(), spec_to_range(s)))
                .collect();
            rp.add_dependencies(pkg.clone(), ver.clone(), deps);
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn resolve_one(
    top_pkg: &str,
    pkg_set: &HashSet<String>,
    pkg_versions: &HashMap<String, Vec<Version>>,
    deps_map: &HashMap<(String, String), Vec<(String, String)>>,
    top_versions: &DashMap<String, Vec<Version>>,
    allow_prerelease: bool,
) -> Option<Solution> {
    let tvers = pkg_versions.get(top_pkg)?;
    let root_range =
        build_root_range(top_pkg, tvers, top_versions, allow_prerelease);
    debug!("  {} root range: {:?}", top_pkg, root_range);
    let mut rp = OfflineDependencyProvider::new();
    rp.add_dependencies(
        "__root__".to_string(),
        Version::new([0, 0, 0]),
        vec![(top_pkg.to_string(), root_range)],
    );
    populate_provider(
        &mut rp,
        pkg_set,
        pkg_versions,
        deps_map,
        allow_prerelease,
    );
    pubgrub::resolve(&rp, "__root__".to_string(), Version::new([0, 0, 0]))
        .ok()
        .map(collect_solution)
}
#[allow(clippy::too_many_arguments)]
fn resolve_all(
    top_names: &[String],
    pkg_set: &HashSet<String>,
    pkg_versions: &HashMap<String, Vec<Version>>,
    deps_map: &HashMap<(String, String), Vec<(String, String)>>,
    top_versions: &DashMap<String, Vec<Version>>,
    allow_prerelease: bool,
) -> Vec<Solution> {
    let mut solutions = Vec::new();
    for top_pkg in top_names {
        match resolve_one(
            top_pkg,
            pkg_set,
            pkg_versions,
            deps_map,
            top_versions,
            allow_prerelease,
        ) {
            Some(sol) => solutions.push(sol),
            None => warn!("  {top_pkg} pubgrub UNSAT"),
        }
    }
    solutions
}
pub async fn resolve_dependencies(
    params: &ResolveParams<'_>,
    client: &reqwest::Client,
) -> DashMap<String, Vec<Version>> {
    info!("解析依赖: {} 个顶层包", params.top_packages.len());
    let http = HttpCtx {
        client,
        pypi_url: params.pypi_url,
    };
    let pkg_extras = collect_pkg_extras(params.top_packages);
    let (pkg_set, pkg_versions, req_cache) =
        bfs_collect(params, &http, &pkg_extras).await;
    info!("  收集完成: {} 个包", pkg_set.len());
    let deps_map = build_deps_map(
        &pkg_set,
        &pkg_versions,
        &FetchCtx {
            http: &http,
            cache: &req_cache,
            max_versions: params.max_versions,
            pkg_extras: &pkg_extras,
        },
    )
    .await;
    let top_names: Vec<_> =
        params.top_packages.iter().map(|r| bare_name(r)).collect();
    let all_solutions = resolve_all(
        &top_names,
        &pkg_set,
        &pkg_versions,
        &deps_map,
        params.top_versions,
        params.allow_prerelease,
    );
    info!("  解析完成: {} 个解", all_solutions.len());
    if all_solutions.is_empty() {
        warn!("所有包均无有效解");
        return DashMap::new();
    }
    let av: DashMap<String, Vec<Version>> = pkg_versions.into_iter().collect();
    let result =
        compute_version_windows(&all_solutions, &av, params.max_versions);
    for name in &top_names {
        result.remove(name);
    }
    info!("  依赖解析完成: {} 个包", result.len());
    result
}
