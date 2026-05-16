use std::path::Path;
use std::time::Duration;

use tracing::info;

use crate::downloader::{
    FileInfo, PrefetchedFiles, download_pkg_files_with_prefetched,
};
use crate::indexer::generate_index;
use crate::python_builds::{
    PythonBuildEntry, build_python_builds_index, download_python_builds_batch,
};
use crate::resolver::resolve::{
    DependencyPlan, PlanParams, ResolveError, build_dependency_plan,
};
use crate::store::DownloadStore;

mod plan;
mod record;

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

fn log_dry_run(pending: &[FileInfo]) {
    info!(
        "Dry-run 依赖解析完成，待下载文件清单（{} 个）:",
        pending.len()
    );
    for fi in pending {
        info!("  {}  {}  {}", fi.package_name, fi.version, fi.filename);
    }
}

struct DownloadPhaseParams<'a> {
    config: &'a crate::config::Config,
    client: &'a reqwest::Client,
    repo: &'a Path,
    pending: &'a [FileInfo],
    prefetched: &'a PrefetchedFiles,
    download_python_builds: bool,
}

async fn execute_download_phase(
    p: &DownloadPhaseParams<'_>,
) -> Result<Vec<FileInfo>, Box<dyn std::error::Error>> {
    let result =
        run_downloads(p.config, p.client, p.repo, p.pending, p.prefetched)
            .await;
    record::record_download_results(p.repo, &result).await?;
    finalize_sync(
        p.client,
        p.repo,
        p.download_python_builds,
        p.config.download_workers,
    )
    .await?;
    Ok(result.downloaded)
}

pub async fn do_sync(
    config: &crate::config::Config,
    pkgs: &[String],
    no_deps: bool,
    download_python_builds: bool,
    dry_run: bool,
) -> Result<(reqwest::Client, Vec<FileInfo>), Box<dyn std::error::Error>> {
    let repo = &config.repository_dir;
    let client = build_sync_client()?;
    let plan = create_sync_plan(config, &client, pkgs, no_deps).await?;
    let (pending, prefetched) = prepare_pending_files(repo, plan)?;
    if dry_run {
        log_dry_run(&pending);
        return Ok((client, Vec::new()));
    }
    let downloaded = execute_download_phase(&DownloadPhaseParams {
        config,
        client: &client,
        repo,
        pending: &pending,
        prefetched: &prefetched,
        download_python_builds,
    })
    .await?;
    Ok((client, downloaded))
}

fn build_sync_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
}

fn prepare_pending_files(
    repo: &Path,
    plan: DependencyPlan,
) -> Result<(Vec<FileInfo>, PrefetchedFiles), Box<dyn std::error::Error>> {
    let planned_count = plan.planned_files.len();
    let pending = filter_incremental_files(repo, plan.planned_files)?;
    let prefetched = filter_prefetched_for_pending(
        pending.as_slice(),
        plan.prefetched_files,
    );
    log_pending_files(pending.len(), planned_count);
    Ok((pending, prefetched))
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
    prefetched: &PrefetchedFiles,
) -> crate::downloader::DownloadResult {
    download_pkg_files_with_prefetched(
        client,
        repo,
        pending,
        prefetched,
        config.include_source,
        config.download_workers,
    )
    .await
}

fn filter_prefetched_for_pending(
    pending: &[FileInfo],
    prefetched_files: PrefetchedFiles,
) -> PrefetchedFiles {
    let mut result = PrefetchedFiles::new();
    for file in pending {
        let key = (file.package_name.clone(), file.filename.clone());
        if let Some(bytes) = prefetched_files.get(&key) {
            result.insert(key, bytes.clone());
        }
    }
    result
}

async fn create_sync_plan(
    config: &crate::config::Config,
    client: &reqwest::Client,
    pkgs: &[String],
    no_deps: bool,
) -> Result<DependencyPlan, ResolveError> {
    if no_deps {
        return plan::build_top_only_plan(config, client, pkgs).await;
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
        targets: crate::resolver::types::TargetEnv::from_specs(&config.targets),
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

async fn finalize_sync(
    client: &reqwest::Client,
    repo: &std::path::Path,
    download_python_builds: bool,
    download_workers: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let python_build_entries = maybe_download_python_builds(
        client,
        repo,
        download_python_builds,
        download_workers,
    )
    .await?;
    rebuild_indexes(repo, python_build_entries).await?;
    Ok(())
}

async fn maybe_download_python_builds(
    client: &reqwest::Client,
    repo: &Path,
    enabled: bool,
    workers: usize,
) -> Result<Option<Vec<PythonBuildEntry>>, Box<dyn std::error::Error>> {
    if !enabled {
        return Ok(None);
    }
    let entries = download_python_builds_batch(client, repo, workers).await?;
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
