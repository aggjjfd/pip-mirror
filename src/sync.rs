use std::path::Path;

use dashmap::DashMap;
use pep440_rs::Version;
use tracing::info;

use crate::downloader::{
    FileInfo, HttpCtx, download_pkg_files, fetch_json_api,
};
use crate::indexer::generate_index;
use crate::packager::pack_mirror_archive;
use crate::python_builds::download_python_builds_batch;
use crate::resolver::pubgrub::bare_name;
use crate::resolver::resolve::resolve_dependencies;

pub fn archive_mb(p: &Path) -> f64 {
    std::fs::metadata(p)
        .map(|m| m.len() as f64 / 1048576.0)
        .unwrap_or(0.0)
}

pub fn clean_repo(repo: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for sub in &["simple", "python-builds"] {
        let dir = repo.join(sub);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
    }
    let db = repo.join(".store.db");
    if db.exists() {
        std::fs::remove_file(&db)?;
    }
    std::fs::create_dir_all(repo)?;
    Ok(())
}

pub struct SyncCtx<'a> {
    pub http: &'a HttpCtx<'a>,
    pub repo: &'a Path,
    pub allow_prerelease: bool,
}

pub async fn do_sync(
    config: &crate::config::Config,
    pkgs: &[String],
    no_deps: bool,
    download_python_builds: bool,
) -> Result<(reqwest::Client, Vec<FileInfo>), Box<dyn std::error::Error>> {
    let repo = &config.repository_dir;
    let client = reqwest::Client::new();
    let http = HttpCtx {
        client: &client,
        pypi_url: &config.pypi_url,
    };
    let (top_versions, mut downloaded) = sync_top_packages(
        &SyncCtx {
            http: &http,
            repo,
            allow_prerelease: config.allow_prerelease,
        },
        pkgs,
        config.max_versions,
    )
    .await;
    if !no_deps && !top_versions.is_empty() {
        let params = crate::resolver::resolve::ResolveParams {
            top_packages: pkgs,
            top_versions: &top_versions,
            pypi_url: &config.pypi_url,
            max_depth: config.max_depth,
            max_versions: config.max_versions,
            allow_prerelease: config.allow_prerelease,
        };
        let deps = resolve_dependencies(&params, &client).await;
        if !deps.is_empty() {
            let d = download_dep_versions(
                &http,
                repo,
                &deps,
                config.include_source,
            )
            .await;
            downloaded.extend(d);
        }
    }
    if download_python_builds {
        download_python_builds_batch(&client, repo).await?;
    }
    generate_index(repo);
    Ok((client, downloaded))
}

async fn download_dep_versions(
    http: &HttpCtx<'_>,
    repo: &Path,
    deps: &DashMap<String, Vec<Version>>,
    include_source: bool,
) -> Vec<FileInfo> {
    let dep_list: Vec<String> = deps
        .iter()
        .map(|e| {
            let vers: Vec<_> =
                e.value().iter().map(|v| v.to_string()).collect();
            format!("  {}: [{}]", e.key(), vers.join(", "))
        })
        .collect();
    info!("依赖包清单 ({} 个):", dep_list.len());
    for line in &dep_list {
        info!("{line}");
    }
    let mut downloaded = Vec::new();
    for entry in deps.iter() {
        let pkg = entry.key();
        let vers = entry.value();
        let vers_set: std::collections::HashSet<String> =
            vers.iter().map(|v| v.to_string()).collect();
        let Ok(files) = fetch_json_api(http, pkg).await else {
            tracing::warn!("  [FAIL] 依赖包 {pkg}: 获取元数据失败");
            continue;
        };
        let selected: Vec<_> = files
            .into_iter()
            .filter(|f| vers_set.contains(&f.version))
            .collect();
        for fi in &selected {
            info!("  → {} {} [{}]", fi.package_name, fi.version, fi.filename);
        }
        let d =
            download_pkg_files(http.client, repo, &selected, include_source)
                .await;
        downloaded.extend(d);
        info!("  [OK] {pkg}: {} 个文件", selected.len());
    }
    downloaded
}

async fn sync_top_packages(
    ctx: &SyncCtx<'_>,
    pkgs: &[String],
    max_versions: usize,
) -> (DashMap<String, Vec<Version>>, Vec<FileInfo>) {
    let top_versions = DashMap::new();
    let mut downloaded = Vec::new();
    for pkg in pkgs {
        let Ok(files) = fetch_json_api(ctx.http, pkg).await else {
            tracing::warn!("  [FAIL] {pkg}: 获取数据失败");
            continue;
        };
        let selected = crate::downloader::select_latest_versions(
            &files,
            max_versions,
            ctx.allow_prerelease,
        );
        let mut vers: Vec<Version> = selected
            .iter()
            .filter_map(|f| f.version.parse().ok())
            .collect();
        vers.sort_by(|a, b| b.cmp(a));
        vers.dedup();
        top_versions.insert(bare_name(pkg), vers);
        info!("  [OK] {pkg}: {} files", selected.len());
        let d = download_pkg_files(ctx.http.client, ctx.repo, &selected, true)
            .await;
        downloaded.extend(d);
    }
    (top_versions, downloaded)
}

pub async fn finalize_mirror(
    client: &reqwest::Client,
    repo: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let entries = download_python_builds_batch(client, repo).await?;
    crate::python_builds::build_python_builds_index(&entries, repo)?;
    generate_index(repo);
    pack_mirror_archive(repo)?;
    Ok(())
}
