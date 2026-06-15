use std::path::Path;

use tracing::info;

use crate::config::Config;
use crate::downloader::{
    BatchDownloader, DownloadPolicy, DownloadResult, Downloadable,
    DownloadableItem, PrefetchedFiles,
};
use crate::http::HttpClient;
use crate::progress::{ProgressHandle, SyncEvent};
use crate::resolver::plan::DependencyPlan;
use crate::store::DownloadStore;
use crate::sync::pipeline::SyncError;

pub struct DownloadPhase;

impl DownloadPhase {
    pub async fn run(
        config: &Config,
        client: &HttpClient,
        plan: &DependencyPlan,
        dry_run: bool,
        progress: Option<ProgressHandle>,
    ) -> Result<DownloadResult, SyncError> {
        let repo = &config.repository_dir;
        let (pending, prefetched, store) =
            prepare_pending_files(repo, plan, &progress)?;

        if dry_run {
            log_dry_run(&pending);
            return Ok(DownloadResult::default());
        }

        let policy = DownloadPolicy {
            include_source: config.include_source,
            workers: config.download_workers,
        };
        let downloader = BatchDownloader::new(
            client.clone(),
            repo,
            store,
            policy,
            progress.clone(),
        );
        let result = downloader.download(&pending, &prefetched).await;

        crate::sync::phases::emit_phase_finished(
            &progress,
            "download",
            format!(
                "下载 {}，跳过 {}，失败 {}",
                result.downloaded.len(),
                result.skipped.len(),
                result.failed.len()
            ),
        );

        Ok(result)
    }
}

fn prepare_pending_files(
    repo: &Path,
    plan: &DependencyPlan,
    progress: &Option<ProgressHandle>,
) -> Result<
    (
        Vec<DownloadableItem>,
        PrefetchedFiles,
        Option<DownloadStore>,
    ),
    SyncError,
> {
    let planned_count = plan.planned_files.len();
    let store = open_store(repo)?;
    let pending = if let Some(s) = &store {
        s.filter_missing_files(&plan.planned_files)
            .map_err(|e| SyncError::Other(Box::new(e)))?
    } else {
        plan.planned_files.clone()
    };
    let prefetched = filter_prefetched_for_pending(
        pending.as_slice(),
        &plan.prefetched_files,
    );
    log_pending_files(pending.len(), planned_count);
    if let Some(p) = progress {
        p.emit(SyncEvent::PhaseProgress {
            phase: "download",
            current: pending.len() as u64,
            message: format!(
                "已过滤 {} 个已有文件",
                planned_count.saturating_sub(pending.len())
            ),
        });
    }
    Ok((pending, prefetched, store))
}

fn open_store(repo: &Path) -> Result<Option<DownloadStore>, SyncError> {
    let db_path = repo.join(".store.db");
    if !db_path.exists() {
        return Ok(None);
    }
    Ok(Some(
        DownloadStore::open(&db_path)
            .map_err(|e| SyncError::Other(Box::new(e)))?,
    ))
}

fn log_pending_files(pending_count: usize, planned_count: usize) {
    info!(
        "计划下载 {} 个文件，已过滤 {} 个已有文件",
        pending_count,
        planned_count.saturating_sub(pending_count)
    );
}

fn filter_prefetched_for_pending(
    pending: &[DownloadableItem],
    prefetched_files: &PrefetchedFiles,
) -> PrefetchedFiles {
    let mut result = PrefetchedFiles::new();
    for file in pending {
        let key =
            (file.package_name().to_string(), file.filename().to_string());
        if let Some(bytes) = prefetched_files.get(&key) {
            result.insert(key, bytes.clone());
        }
    }
    result
}

fn log_dry_run(pending: &[DownloadableItem]) {
    info!(
        "Dry-run 依赖解析完成，待下载文件清单（{} 个）：",
        pending.len()
    );
    for fi in pending {
        info!(
            "  {}  {}  {}",
            fi.package_name(),
            fi.version(),
            fi.filename()
        );
    }
}
