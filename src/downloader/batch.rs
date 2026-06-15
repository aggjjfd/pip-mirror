use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::{StreamExt, stream};
use reqwest_middleware::ClientWithMiddleware;

use crate::downloader::{
    DownloadResult, FileInfo, PrefetchedFiles, download_file, write_atomic,
};
use crate::filters::{is_accepted_wheel, is_source_distribution};
use crate::http::HttpClient;
use crate::progress::{FileStatus, ProgressHandle, SyncEvent};
use crate::store::DownloadStore;
use sha2::Digest;

pub struct DownloadPolicy {
    pub include_source: bool,
    pub workers: usize,
}

pub struct BatchDownloader {
    client: HttpClient,
    repo: PathBuf,
    store: Option<Arc<DownloadStore>>,
    policy: DownloadPolicy,
    progress: Option<ProgressHandle>,
}

impl BatchDownloader {
    pub fn new(
        client: HttpClient,
        repo: &Path,
        store: Option<DownloadStore>,
        policy: DownloadPolicy,
        progress: Option<ProgressHandle>,
    ) -> Self {
        Self {
            client,
            repo: repo.to_path_buf(),
            store: store.map(Arc::new),
            policy,
            progress,
        }
    }

    pub async fn download(
        &self,
        files: &[FileInfo],
        prefetched: &PrefetchedFiles,
    ) -> DownloadResult {
        let mut result = DownloadResult::default();
        let store = self
            .store
            .clone()
            .or_else(|| open_download_store(&self.repo));
        let pending = collect_pending_downloads(
            files,
            self.policy.include_source,
            &mut result,
        );
        let outcomes = run_download_tasks(
            self.client.inner(),
            &self.repo,
            &store,
            prefetched,
            pending,
            self.policy.workers,
            &self.progress,
        )
        .await;
        merge_download_outcomes(&mut result, outcomes);
        sort_download_result(&mut result);
        result
    }
}

fn open_download_store(repo: &Path) -> Option<Arc<DownloadStore>> {
    let db_path = repo.join(".store.db");
    if let Some(parent) = db_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!("创建 .store.db 父目录失败: {e}");
        return None;
    }
    match DownloadStore::open(&db_path) {
        Ok(store) => Some(Arc::new(store)),
        Err(e) => {
            tracing::warn!("打开或创建 .store.db 失败: {e}");
            None
        }
    }
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
    client: &ClientWithMiddleware,
    repo: &Path,
    store: &Option<Arc<DownloadStore>>,
    prefetched_files: &PrefetchedFiles,
    pending: Vec<FileInfo>,
    download_workers: usize,
    progress: &Option<ProgressHandle>,
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

enum DownloadOutcome {
    Skipped(FileInfo),
    Downloaded(FileInfo),
    Failed(FileInfo, String),
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

#[allow(clippy::too_many_arguments)]
async fn try_download(
    client: &ClientWithMiddleware,
    store: &Option<Arc<DownloadStore>>,
    prefetched_files: &PrefetchedFiles,
    fi: &FileInfo,
    repo: &Path,
    progress: &Option<ProgressHandle>,
) -> DownloadOutcome {
    let dest = repo
        .join("simple")
        .join(&fi.package_name)
        .join(&fi.filename);
    if store.as_ref().is_some_and(|s| {
        s.has_file(&fi.package_name, &fi.filename).unwrap_or(false)
    }) {
        return DownloadOutcome::Skipped(fi.clone());
    }
    if dest.exists() {
        // 文件已存在但数据库里没有记录：补录，避免重复下载后仍然丢失记录。
        if let Some(s) = store {
            s.record_download(fi, &dest).await;
        }
        return DownloadOutcome::Skipped(fi.clone());
    }
    let key = (fi.package_name.clone(), fi.filename.clone());
    if let Some(bytes) = prefetched_files.get(&key) {
        return try_prefetched_write(fi, &dest, bytes, store, progress).await;
    }
    try_network_download(client, fi, &dest, store, progress).await
}

async fn try_prefetched_write(
    fi: &FileInfo,
    dest: &Path,
    bytes: &[u8],
    store: &Option<Arc<DownloadStore>>,
    progress: &Option<ProgressHandle>,
) -> DownloadOutcome {
    if !bytes_match_sha256(fi, bytes) {
        return DownloadOutcome::Failed(
            fi.clone(),
            "预下载文件 hash 校验失败".to_string(),
        );
    }
    let (ok, msg) = write_atomic(dest, bytes).await;
    if ok {
        tracing::debug!("复用预下载文件: {}", fi.filename);
        if let Some(p) = progress {
            p.emit(SyncEvent::FileDone {
                package: fi.package_name.clone(),
                filename: fi.filename.clone(),
                status: FileStatus::Reused,
            });
        }
        if let Some(s) = store {
            s.record_download(fi, dest).await;
        }
        return DownloadOutcome::Downloaded(fi.clone());
    }
    DownloadOutcome::Failed(fi.clone(), msg)
}

async fn try_network_download(
    client: &ClientWithMiddleware,
    fi: &FileInfo,
    dest: &Path,
    store: &Option<Arc<DownloadStore>>,
    progress: &Option<ProgressHandle>,
) -> DownloadOutcome {
    let (ok, msg) = download_file(client, fi, dest).await;
    if ok {
        tracing::debug!("下载完成: {}", fi.filename);
        if let Some(p) = progress {
            p.emit(SyncEvent::FileDone {
                package: fi.package_name.clone(),
                filename: fi.filename.clone(),
                status: FileStatus::Downloaded,
            });
        }
        if let Some(s) = store {
            s.record_download(fi, dest).await;
        }
        DownloadOutcome::Downloaded(fi.clone())
    } else {
        DownloadOutcome::Failed(fi.clone(), msg)
    }
}

fn bytes_match_sha256(fi: &FileInfo, bytes: &[u8]) -> bool {
    fi.sha256.as_ref().is_none_or(|expected| {
        let mut hasher = sha2::Sha256::new();
        hasher.update(bytes);
        let actual = crate::hex_digest(hasher.finalize().as_slice());
        actual.eq_ignore_ascii_case(expected)
    })
}

fn should_skip(fi: &FileInfo, include_source: bool) -> bool {
    if !include_source && is_source_distribution(&fi.filename) {
        return true;
    }
    if fi.filename.ends_with(".whl") && fi.explicit_url {
        return false;
    }
    fi.filename.ends_with(".whl") && !is_accepted_wheel(&fi.filename)
}
