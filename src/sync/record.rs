use std::path::Path;

use futures::{StreamExt, stream};

use crate::downloader::FileInfo;
use crate::store::DownloadStore;

const HASH_CONCURRENCY: usize = 8;

pub async fn record_download_results(
    repo: &Path,
    result: &crate::downloader::DownloadResult,
) -> Result<(), Box<dyn std::error::Error>> {
    let records_data = stream::iter(result.downloaded.iter())
        .map(|fi| {
            let dest = repo
                .join("simple")
                .join(&fi.package_name)
                .join(&fi.filename);
            async move {
                let sha256 = hash_file_async(&dest, fi.sha256.clone()).await;
                let size =
                    tokio::fs::metadata(&dest).await.ok().map(|m| m.len());
                (
                    fi.filename.clone(),
                    fi.package_name.clone(),
                    fi.version.clone(),
                    sha256,
                    size,
                )
            }
        })
        .buffer_unordered(HASH_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;

    let db_path = repo.join(".store.db");
    let store = DownloadStore::open(&db_path)?;

    insert_records_batch(&store, &records_data);

    record_skipped_files(&store, repo, &result.skipped).await;

    for (fi, err) in &result.failed {
        tracing::warn!("  [FAIL] {} {}: {}", fi.package_name, fi.filename, err);
    }
    Ok(())
}

async fn record_skipped_files(
    store: &DownloadStore,
    repo: &Path,
    skipped: &[FileInfo],
) {
    let missing = match store.filter_missing_files(skipped) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("查询 skipped 文件失败: {e}");
            return;
        }
    };

    let records_data: Vec<_> = stream::iter(missing)
        .map(|fi| {
            let dest = repo
                .join("simple")
                .join(&fi.package_name)
                .join(&fi.filename);
            try_hash_file(dest, fi)
        })
        .buffer_unordered(HASH_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .flatten()
        .collect();

    insert_records_batch(store, &records_data);
}

async fn try_hash_file(
    dest: std::path::PathBuf,
    fi: FileInfo,
) -> Option<(String, String, String, String, Option<u64>)> {
    if !dest.exists() {
        return None;
    }
    let sha256 = hash_file_async(&dest, fi.sha256.clone()).await;
    let size = tokio::fs::metadata(&dest).await.ok().map(|m| m.len());
    Some((fi.filename, fi.package_name, fi.version, sha256, size))
}

async fn hash_file_async(dest: &Path, known_sha256: Option<String>) -> String {
    match known_sha256 {
        Some(h) => h,
        None => {
            let dest = dest.to_path_buf();
            let hr = tokio::task::spawn_blocking(move || {
                DownloadStore::hash_file(&dest)
            })
            .await;
            DownloadStore::handle_hash_result(hr)
        }
    }
}

fn insert_records_batch(
    store: &DownloadStore,
    records_data: &[(String, String, String, String, Option<u64>)],
) {
    let records: Vec<crate::store::FileRecord> = records_data
        .iter()
        .map(|(filename, package_name, version, sha256, size)| {
            crate::store::FileRecord {
                filename,
                package_name,
                version,
                sha256,
                size: *size,
            }
        })
        .collect();

    if let Err(e) = store.add_files_batch(&records) {
        tracing::warn!("批量写入 .store.db 失败: {e}");
    }
}
