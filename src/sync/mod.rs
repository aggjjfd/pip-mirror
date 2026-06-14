use std::path::Path;
use std::time::Duration;

use tracing::info;

use crate::downloader::{
    FileInfo, PrefetchedFiles, download_pkg_files_with_prefetched,
};
use crate::progress::{ProgressHandle, SyncEvent};
use crate::resolver::plan::{
    DependencyPlan, PlanParams, build_dependency_plan,
};
use crate::resolver::resolve::ResolveError;
use crate::store::DownloadStore;

pub mod finalize;
mod plan;
mod record;
pub mod url_wheel;
pub mod url_wheel_download;

pub use finalize::{finalize_mirror, finalize_sync};

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
    progress: Option<ProgressHandle>,
}

async fn run_download_phase_or_dry_run(
    p: &DownloadPhaseParams<'_>,
    dry_run: bool,
) -> Result<Vec<FileInfo>, Box<dyn std::error::Error>> {
    if dry_run {
        log_dry_run(p.pending);
        return Ok(Vec::new());
    }
    execute_download_phase(p).await
}

async fn execute_download_phase(
    p: &DownloadPhaseParams<'_>,
) -> Result<Vec<FileInfo>, Box<dyn std::error::Error>> {
    emit_phase_started(&p.progress, "download", Some(p.pending.len() as u64));

    let result = run_downloads(
        p.config,
        p.client,
        p.repo,
        p.pending,
        p.prefetched,
        p.progress.clone(),
    )
    .await;

    emit_phase_finished(
        &p.progress,
        "download",
        format!(
            "下载 {}，跳过 {}，失败 {}",
            result.downloaded.len(),
            result.skipped.len(),
            result.failed.len()
        ),
    );

    record::record_download_results(p.repo, &result).await?;
    finalize::finalize_sync(
        p.client,
        p.repo,
        p.download_python_builds,
        p.config.download_workers,
        p.progress.clone(),
    )
    .await?;
    Ok(result.downloaded)
}

#[allow(clippy::too_many_arguments)]
pub async fn do_sync(
    config: &crate::config::Config,
    pkgs: &[crate::config::PackageSpec],
    no_deps: bool,
    download_python_builds: bool,
    dry_run: bool,
    progress: Option<ProgressHandle>,
) -> Result<(reqwest::Client, Vec<FileInfo>), Box<dyn std::error::Error>> {
    let repo = &config.repository_dir;
    let client = build_sync_client()?;

    emit_phase_started(&progress, "plan", Some(pkgs.len() as u64));

    let plan =
        create_sync_plan(config, &client, pkgs, no_deps, progress.clone())
            .await?;
    let (pending, prefetched) = prepare_pending_files(repo, plan, &progress)?;
    let params = DownloadPhaseParams {
        config,
        client: &client,
        repo,
        pending: &pending,
        prefetched: &prefetched,
        download_python_builds,
        progress: progress.clone(),
    };
    let downloaded = run_download_phase_or_dry_run(&params, dry_run).await?;
    Ok((client, downloaded))
}

fn emit_phase_started(
    progress: &Option<ProgressHandle>,
    phase: &'static str,
    total: Option<u64>,
) {
    if let Some(p) = progress {
        p.emit(SyncEvent::PhaseStarted { phase, total });
    }
}

fn emit_phase_finished(
    progress: &Option<ProgressHandle>,
    phase: &'static str,
    summary: String,
) {
    if let Some(p) = progress {
        p.emit(SyncEvent::PhaseFinished { phase, summary });
    }
}

pub fn build_sync_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
}

fn prepare_pending_files(
    repo: &Path,
    plan: DependencyPlan,
    progress: &Option<ProgressHandle>,
) -> Result<(Vec<FileInfo>, PrefetchedFiles), Box<dyn std::error::Error>> {
    let planned_count = plan.planned_files.len();
    let pending = filter_incremental_files(repo, plan.planned_files)?;
    let prefetched = filter_prefetched_for_pending(
        pending.as_slice(),
        plan.prefetched_files,
    );
    log_pending_files(pending.len(), planned_count);
    emit_phase_finished(
        progress,
        "plan",
        format!(
            "{} 个待下载（已过滤 {} 个）",
            pending.len(),
            planned_count.saturating_sub(pending.len())
        ),
    );
    Ok((pending, prefetched))
}

fn log_pending_files(pending_count: usize, planned_count: usize) {
    info!(
        "计划下载 {} 个文件，已过滤 {} 个已有文件",
        pending_count,
        planned_count.saturating_sub(pending_count)
    );
}

#[allow(clippy::too_many_arguments)]
async fn run_downloads(
    config: &crate::config::Config,
    client: &reqwest::Client,
    repo: &Path,
    pending: &[FileInfo],
    prefetched: &PrefetchedFiles,
    progress: Option<ProgressHandle>,
) -> crate::downloader::DownloadResult {
    download_pkg_files_with_prefetched(
        client,
        repo,
        pending,
        prefetched,
        config.include_source,
        config.download_workers,
        progress,
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

fn add_url_wheel_to_plan(
    plan: &mut DependencyPlan,
    spec: &crate::config::PackageUrlSpec,
) -> Result<(), ResolveError> {
    let parsed =
        crate::wheel_url::parse_wheel_url(&spec.url, spec.sha256.clone())
            .map_err(|e| {
                ResolveError::Config(format!(
                    "URL whl 解析失败 ({}): {e}",
                    crate::filters::redact_url_for_display(&spec.url)
                ))
            })?;
    let file_info = FileInfo::builder()
        .filename(parsed.filename)
        .url(parsed.url)
        .sha256(parsed.sha256)
        .package_name(parsed.package_name.clone())
        .version(parsed.version.clone())
        .explicit_url(true)
        .build();
    plan.planned_files.push(file_info);
    let version =
        parsed.version.parse::<pep440_rs::Version>().map_err(|_| {
            ResolveError::Config(format!(
                "无法解析 whl 版本: {}",
                parsed.version
            ))
        })?;
    plan.solved_versions
        .entry(parsed.package_name)
        .or_default()
        .push(version);
    Ok(())
}

async fn build_plan(
    config: &crate::config::Config,
    client: &reqwest::Client,
    name_pkgs: &[String],
    no_deps: bool,
    progress: Option<ProgressHandle>,
) -> Result<DependencyPlan, ResolveError> {
    if no_deps {
        return plan::build_top_only_plan(config, client, name_pkgs).await;
    }
    let params = PlanParams {
        top_packages: name_pkgs,
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
    build_dependency_plan(&params, client, progress).await
}

pub async fn create_sync_plan(
    config: &crate::config::Config,
    client: &reqwest::Client,
    pkgs: &[crate::config::PackageSpec],
    no_deps: bool,
    progress: Option<ProgressHandle>,
) -> Result<DependencyPlan, ResolveError> {
    let (mut name_pkgs, url_pkgs) = url_wheel::split_package_specs(pkgs);

    let url_prefetched = url_wheel_download::maybe_collect_url_wheel_deps(
        client,
        &url_pkgs,
        no_deps,
        &mut name_pkgs,
    )
    .await?;

    let mut plan =
        build_plan(config, client, &name_pkgs, no_deps, progress.clone())
            .await?;
    plan.prefetched_files.extend(url_prefetched);

    for spec in &url_pkgs {
        add_url_wheel_to_plan(&mut plan, spec)?;
    }

    url_wheel::dedupe_planned_files(&mut plan.planned_files);
    url_wheel::dedupe_solved_versions(&mut plan.solved_versions);

    Ok(plan)
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
