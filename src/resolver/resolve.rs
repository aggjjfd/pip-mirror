use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use dashmap::DashMap;
use pep440_rs::Version;
use pubgrub::OfflineDependencyProvider;
use pubgrub::Ranges;
use tracing::{info, warn};

use super::pubgrub::{
    Solution, bare_name, compute_version_windows, dep_to_range,
    parse_python_requires,
};
use crate::downloader::HttpCtx;

pub struct ResolveParams<'a> {
    pub top_packages: &'a [String],
    pub top_versions: &'a DashMap<String, Vec<Version>>,
    pub pypi_url: &'a str,
    pub max_depth: usize,
    pub max_versions: usize,
}

type CacheType = Arc<DashMap<(String, String), Vec<(String, String)>>>;

async fn fetch_versions(http: &HttpCtx<'_>, pkg: &str) -> Option<Vec<Version>> {
    crate::downloader::get_all_versions(http, pkg)
        .await
        .ok()
        .filter(|v| !v.is_empty())
}

struct ExploreCtx<'a, 'b> {
    pkg: &'a str,
    all_vers: &'a [Version],
    params: &'a ResolveParams<'b>,
    http: &'a HttpCtx<'a>,
    pkg_set: &'a mut HashSet<String>,
    queue: &'a mut Vec<(String, usize)>,
    depth: usize,
    req_cache: &'a CacheType,
}

impl ExploreCtx<'_, '_> {
    fn register_deps(&mut self, deps: &[(String, String)]) {
        let new_deps: Vec<String> = deps
            .iter()
            .map(|(dn, _)| crate::filters::normalize_package_name(dn))
            .filter(|dn| !self.pkg_set.contains(dn))
            .collect();
        for dn in new_deps {
            self.pkg_set.insert(dn.clone());
            self.queue.push((dn.clone(), self.depth + 1));
        }
    }
}

async fn explore_pkg_deps(mut ctx: ExploreCtx<'_, '_>) {
    let explore: Vec<Version> = ctx
        .params
        .top_versions
        .get(ctx.pkg)
        .map(|tv| tv.iter().cloned().collect())
        .unwrap_or_else(|| ctx.all_vers.iter().take(1).cloned().collect());
    for ver in &explore {
        let vs = ver.to_string();
        let Ok(Some(rd)) =
            crate::downloader::get_requires_dist(ctx.http, ctx.pkg, &vs).await
        else {
            continue;
        };
        let deps = parse_python_requires(&rd);
        ctx.register_deps(&deps);
        ctx.req_cache.insert((ctx.pkg.to_string(), vs), deps);
    }
}

async fn bfs_collect(
    params: &ResolveParams<'_>,
    http: &HttpCtx<'_>,
) -> (HashSet<String>, HashMap<String, Vec<Version>>, CacheType) {
    let top_names: Vec<_> =
        params.top_packages.iter().map(|r| bare_name(r)).collect();
    let mut pkg_set: HashSet<String> = top_names.iter().cloned().collect();
    let mut queue: Vec<_> = top_names.iter().map(|n| (n.clone(), 0)).collect();
    let mut pkg_versions: HashMap<String, Vec<Version>> = HashMap::new();
    let req_cache: CacheType = Arc::new(DashMap::new());

    while let Some((pkg, depth)) = queue.pop() {
        let Some(all_vers) = fetch_versions(http, &pkg).await else {
            continue;
        };
        pkg_versions.entry(pkg.clone()).or_insert(all_vers.clone());
        if depth < params.max_depth {
            explore_pkg_deps(ExploreCtx {
                pkg: &pkg,
                all_vers: &all_vers,
                params,
                http,
                pkg_set: &mut pkg_set,
                queue: &mut queue,
                depth,
                req_cache: &req_cache,
            })
            .await;
        }
    }
    (pkg_set, pkg_versions, req_cache)
}

struct FetchCtx<'a> {
    http: &'a HttpCtx<'a>,
    cache: &'a CacheType,
}

async fn fetch_pkg_deps(
    pkg: String,
    vers: Vec<Version>,
    ctx: &FetchCtx<'_>,
) -> Vec<((String, usize), Vec<(String, String)>)> {
    let mut results = Vec::new();
    for (idx, ver) in vers.iter().enumerate() {
        let vs = ver.to_string();
        let key = (pkg.clone(), vs.clone());
        let deps = if let Some(c) = ctx.cache.get(&key) {
            c.clone()
        } else {
            let d =
                match crate::downloader::get_requires_dist(ctx.http, &pkg, &vs)
                    .await
                {
                    Ok(Some(rd)) => parse_python_requires(&rd),
                    _ => vec![],
                };
            ctx.cache.insert(key, d.clone());
            d
        };
        results.push(((pkg.clone(), idx), deps));
    }
    results
}

async fn build_deps_map(
    pkg_set: &HashSet<String>,
    pkg_versions: &HashMap<String, Vec<Version>>,
    ctx: &FetchCtx<'_>,
) -> HashMap<(String, usize), Vec<(String, String)>> {
    let mut handles = Vec::new();
    for pkg in pkg_set {
        let Some(vers) = pkg_versions.get(pkg) else {
            continue;
        };
        handles.push(fetch_pkg_deps(pkg.clone(), vers.clone(), ctx));
    }
    let mut deps_map = HashMap::new();
    for results in futures::future::join_all(handles).await {
        for (key, deps) in results {
            deps_map.insert(key, deps);
        }
    }
    deps_map
}

fn collect_solution(
    solution: impl IntoIterator<Item = (String, u32)>,
    pkg_versions: &HashMap<String, Vec<Version>>,
) -> Solution {
    let sol: Solution = DashMap::new();
    for (pkg, vi) in solution {
        if pkg == "__root__" {
            continue;
        }
        if let Some(vers) = pkg_versions.get(&pkg)
            && let Some(v) = vers.get(usize::try_from(vi).unwrap_or(0))
        {
            sol.insert(pkg, v.clone());
        }
    }
    sol
}

struct ResolveOneCtx<'a> {
    top_pkg: &'a str,
    pkg_set: &'a HashSet<String>,
    pkg_versions: &'a HashMap<String, Vec<Version>>,
    deps_map: &'a HashMap<(String, usize), Vec<(String, String)>>,
    top_versions: &'a DashMap<String, Vec<Version>>,
}

fn dep_list_to_ranges(
    ds: &[(String, String)],
    pkg_versions: &HashMap<String, Vec<Version>>,
) -> Vec<(String, Ranges<u32>)> {
    ds.iter()
        .filter_map(|(n, s)| dep_to_range(n, s, pkg_versions))
        .collect()
}

fn populate_provider(
    rp: &mut OfflineDependencyProvider<String, Ranges<u32>>,
    ctx: &ResolveOneCtx<'_>,
) {
    for pkg in ctx.pkg_set {
        let Some(vers) = ctx.pkg_versions.get(pkg) else {
            continue;
        };
        for (idx, _v) in vers.iter().enumerate() {
            let deps: Vec<_> = ctx
                .deps_map
                .get(&(pkg.clone(), idx))
                .map(|ds| dep_list_to_ranges(ds, ctx.pkg_versions))
                .unwrap_or_default();
            rp.add_dependencies(
                pkg.clone(),
                idx.try_into().unwrap_or(0u32),
                deps,
            );
        }
    }
}

fn resolve_one(ctx: &ResolveOneCtx<'_>) -> Option<Solution> {
    let tvers = ctx.pkg_versions.get(ctx.top_pkg).or_else(|| {
        warn!("  {} not in pkg_versions", ctx.top_pkg);
        None
    })?;
    let top_ver = ctx
        .top_versions
        .get(ctx.top_pkg)
        .and_then(|tv| tv.first().cloned())
        .or_else(|| {
            warn!("  {} not in top_versions, using latest", ctx.top_pkg);
            None
        })
        .unwrap_or_else(|| tvers[0].clone());
    let Some(top_idx) = tvers.iter().position(|v| *v == top_ver) else {
        warn!(
            "  {} top_ver {} not found in versions ({} vers)",
            ctx.top_pkg,
            top_ver,
            tvers.len(),
        );
        return None;
    };

    let mut rp = OfflineDependencyProvider::new();
    rp.add_dependencies(
        "__root__".to_string(),
        0u32,
        vec![(
            ctx.top_pkg.to_string(),
            Ranges::singleton(top_idx.try_into().unwrap_or(0u32)),
        )],
    );
    populate_provider(&mut rp, ctx);
    pubgrub::resolve(&rp, "__root__".to_string(), 0u32)
        .ok()
        .map(|s| collect_solution(s, ctx.pkg_versions))
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
    let (pkg_set, pkg_versions, req_cache) = bfs_collect(params, &http).await;
    info!("  收集完成: {} 个包", pkg_set.len());
    info!(
        "  top packages in pkg_versions: {:?}",
        params
            .top_packages
            .iter()
            .filter_map(|n| {
                let b = bare_name(n);
                pkg_versions
                    .get(&b)
                    .map(|v| format!("{b}({} vers)", v.len()))
            })
            .collect::<Vec<_>>()
    );
    let deps_map = build_deps_map(
        &pkg_set,
        &pkg_versions,
        &FetchCtx {
            http: &http,
            cache: &req_cache,
        },
    )
    .await;
    let top_names: Vec<_> =
        params.top_packages.iter().map(|r| bare_name(r)).collect();
    let base_ctx = ResolveOneCtx {
        top_pkg: "",
        pkg_set: &pkg_set,
        pkg_versions: &pkg_versions,
        deps_map: &deps_map,
        top_versions: params.top_versions,
    };
    let mut all_solutions: Vec<Solution> = Vec::new();
    for top_pkg in &top_names {
        let ctx = ResolveOneCtx {
            top_pkg,
            ..base_ctx
        };
        match resolve_one(&ctx) {
            Some(sol) => all_solutions.push(sol),
            None => warn!("  {top_pkg} pubgrub UNSAT"),
        }
    }

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
