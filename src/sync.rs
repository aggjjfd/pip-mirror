use std::path::Path;
use std::time::Duration;

use dashmap::DashMap;
use pep440_rs::Version;
use tracing::info;

use crate::downloader::{FileInfo, download_pkg_files};
use crate::indexer::generate_index;
use crate::python_builds::{
    PythonBuildEntry, build_python_builds_index, download_python_builds_batch,
};
use crate::resolver::metadata::MetadataCache;
use crate::resolver::pubgrub::bare_name;
use crate::resolver::resolve::{
    DependencyPlan, PlanParams, ResolveError, build_dependency_plan,
    select_top_versions,
};
use crate::store::DownloadStore;

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

pub async fn do_sync(
    config: &crate::config::Config,
    pkgs: &[String],
    no_deps: bool,
    download_python_builds: bool,
) -> Result<(reqwest::Client, Vec<FileInfo>), Box<dyn std::error::Error>> {
    let repo = &config.repository_dir;
    let client = build_sync_client()?;
    let plan = create_sync_plan(config, &client, pkgs, no_deps).await?;
    let pending = prepare_pending_files(repo, plan)?;
    let result = run_downloads(config, &client, repo, &pending).await;
    record_download_results(repo, &result).await?;
    finalize_sync(&client, repo, download_python_builds).await?;
    Ok((client, result.downloaded))
}

fn build_sync_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
}

fn prepare_pending_files(
    repo: &Path,
    plan: DependencyPlan,
) -> Result<Vec<FileInfo>, Box<dyn std::error::Error>> {
    let planned_count = plan.planned_files.len();
    let pending = filter_incremental_files(repo, plan.planned_files)?;
    log_pending_files(pending.len(), planned_count);
    Ok(pending)
}

fn log_pending_files(pending_count: usize, planned_count: usize) {
    info!(
        "计划下载 {} 个文件，已过滤 {} 个已有文件",
        pending_count,
        planned_count.saturating_sub(pending_count)
    );
}

async fn run_downloads(
    config: &crate::config::Config,
    client: &reqwest::Client,
    repo: &Path,
    pending: &[FileInfo],
) -> crate::downloader::DownloadResult {
    download_pkg_files(
        client,
        repo,
        pending,
        config.include_source,
        config.download_workers,
    )
    .await
}

async fn create_sync_plan(
    config: &crate::config::Config,
    client: &reqwest::Client,
    pkgs: &[String],
    no_deps: bool,
) -> Result<DependencyPlan, ResolveError> {
    if no_deps {
        return build_top_only_plan(config, client, pkgs).await;
    }
    let params = PlanParams {
        top_packages: pkgs,
        pypi_url: &config.pypi_url,
        top_versions_per_package: config.top_versions_per_package,
        adjacent_versions_per_side: config.adjacent_versions_per_side,
        allow_prerelease: config.allow_prerelease,
        include_source: config.include_source,
        linux_max_glibc: &config.linux_max_glibc,
        resolve_workers: config.resolve_workers,
        metadata_workers: config.metadata_workers,
    };
    build_dependency_plan(&params, client).await
}

fn filter_incremental_files(
    repo: &std::path::Path,
    planned_files: Vec<FileInfo>,
) -> Result<Vec<FileInfo>, Box<dyn std::error::Error>> {
    if !repo.join(".store.db").exists() {
        return Ok(planned_files);
    }
    let store = DownloadStore::open(&repo.join(".store.db"))?;
    Ok(store.filter_missing_files(&planned_files)?)
}

async fn record_download_results(
    repo: &std::path::Path,
    result: &crate::downloader::DownloadResult,
) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = repo.join(".store.db");
    let store = DownloadStore::open(&db_path)?;
    for fi in &result.downloaded {
        let dest = repo.join(&fi.package_name).join(&fi.filename);
        store.record_download(fi, &dest).await;
    }
    for (fi, err) in &result.failed {
        tracing::warn!("  [FAIL] {} {}: {}", fi.package_name, fi.filename, err);
    }
    Ok(())
}

async fn finalize_sync(
    client: &reqwest::Client,
    repo: &std::path::Path,
    download_python_builds: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let python_build_entries =
        maybe_download_python_builds(client, repo, download_python_builds)
            .await?;
    rebuild_indexes(repo, python_build_entries).await?;
    Ok(())
}

async fn maybe_download_python_builds(
    client: &reqwest::Client,
    repo: &Path,
    enabled: bool,
) -> Result<Option<Vec<PythonBuildEntry>>, Box<dyn std::error::Error>> {
    if !enabled {
        return Ok(None);
    }
    let entries = download_python_builds_batch(client, repo).await?;
    info!("已下载 Python 解释器，开始生成 python-builds/index.json");
    Ok(Some(entries))
}

async fn rebuild_indexes(
    repo: &Path,
    python_build_entries: Option<Vec<PythonBuildEntry>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let repo_clone = repo.to_path_buf();
    tokio::task::spawn_blocking(move || {
        if let Some(entries) = python_build_entries {
            build_python_builds_index(&entries, &repo_clone)
                .map_err(|e| format!("生成 python-builds index 失败: {e}"))?;
        }
        generate_index(&repo_clone);
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("索引生成线程错误: {e}"))??;
    Ok(())
}

async fn build_top_only_plan(
    config: &crate::config::Config,
    client: &reqwest::Client,
    pkgs: &[String],
) -> Result<DependencyPlan, ResolveError> {
    let cache = MetadataCache::new(
        client.clone(),
        config.pypi_url.clone(),
        config.metadata_workers,
    );
    let mut planned_files = Vec::new();
    let solved_versions: DashMap<String, Vec<Version>> = DashMap::new();

    for pkg in pkgs {
        let package = bare_name(pkg);
        let selected_versions = select_top_versions(
            cache.get_all_versions(&package).await?,
            config.top_versions_per_package,
            config.allow_prerelease,
        );

        solved_versions.insert(package.clone(), selected_versions.clone());
        for version in selected_versions {
            let files = cache.get_version_files(&package, &version).await?;
            let selected = crate::filters::select_files_for_version(
                &files,
                config.include_source,
                &config.linux_max_glibc,
            );
            planned_files.extend(selected);
        }
    }

    let mut seen = std::collections::HashSet::new();
    planned_files.retain(|fi| seen.insert(fi.filename.clone()));

    Ok(DependencyPlan {
        planned_files,
        solved_versions,
    })
}

pub async fn finalize_mirror(
    _client: &reqwest::Client,
    repo: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let repo_clone = repo.to_path_buf();
    tokio::task::spawn_blocking(move || {
        generate_index(&repo_clone);
        crate::packager::pack_mirror_archive(&repo_clone)
            .map_err(|e| format!("打包镜像失败: {e}"))?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("打包线程错误: {e}"))??;
    Ok(())
}
