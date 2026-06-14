use std::path::Path;
use std::sync::Arc;

use futures::{StreamExt, stream};

use super::{
    DownloadOutcome, DownloadResult, FileInfo, should_skip, try_download,
};

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_download_pipeline(
    client: &reqwest::Client,
    repo: &Path,
    files: &[FileInfo],
    prefetched_files: &crate::downloader::PrefetchedFiles,
    include_source: bool,
    download_workers: usize,
    progress: Option<crate::progress::ProgressHandle>,
) -> DownloadResult {
    let mut result = DownloadResult::default();
    let store = open_download_store(repo);
    let pending = collect_pending_downloads(files, include_source, &mut result);
    let outcomes = run_download_tasks(
        client,
        repo,
        &store,
        prefetched_files,
        pending,
        download_workers,
        &progress,
    )
    .await;
    merge_download_outcomes(&mut result, outcomes);
    sort_download_result(&mut result);
    result
}

fn open_download_store(
    repo: &Path,
) -> Option<Arc<crate::store::DownloadStore>> {
    crate::store::DownloadStore::open(&repo.join(".store.db"))
        .ok()
        .map(Arc::new)
}

fn collect_pending_downloads(
    files: &[FileInfo],
    include_source: bool,
    result: &mut DownloadResult,
) -> Vec<FileInfo> {
    let mut pending = Vec::new();
    for fi in files {
        if should_skip(fi, include_source) {
            result.skipped.push(fi.clone());
            continue;
        }
        pending.push(fi.clone());
    }
    pending
}

#[allow(clippy::too_many_arguments)]
async fn run_download_tasks(
    client: &reqwest::Client,
    repo: &Path,
    store: &Option<Arc<crate::store::DownloadStore>>,
    prefetched_files: &crate::downloader::PrefetchedFiles,
    pending: Vec<FileInfo>,
    download_workers: usize,
    progress: &Option<crate::progress::ProgressHandle>,
) -> Vec<DownloadOutcome> {
    stream::iter(pending)
        .map(|fi| {
            let store = store.clone();
            let progress = progress.clone();
            async move {
                try_download(
                    client,
                    &store,
                    prefetched_files,
                    &fi,
                    repo,
                    &progress,
                )
                .await
            }
        })
        .buffer_unordered(download_workers)
        .collect::<Vec<_>>()
        .await
}

fn merge_download_outcomes(
    result: &mut DownloadResult,
    outcomes: Vec<DownloadOutcome>,
) {
    for outcome in outcomes {
        match outcome {
            DownloadOutcome::Skipped(file) => result.skipped.push(file),
            DownloadOutcome::Downloaded(file) => result.downloaded.push(file),
            DownloadOutcome::Failed(file, msg) => {
                result.failed.push((file, msg))
            }
        }
    }
}

fn sort_download_result(result: &mut DownloadResult) {
    result
        .downloaded
        .sort_by(|left, right| left.filename.cmp(&right.filename));
    result
        .skipped
        .sort_by(|left, right| left.filename.cmp(&right.filename));
    result
        .failed
        .sort_by(|left, right| left.0.filename.cmp(&right.0.filename));
}
