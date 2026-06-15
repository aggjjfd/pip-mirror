use std::path::Path;
use std::time::Duration;

use tracing::info;

use crate::indexer::generate_index;
use crate::progress::{ProgressHandle, SyncEvent};
use crate::python_builds::{
    PythonBuildEntry, build_python_builds_index, download_python_builds_batch,
};

pub async fn finalize_sync(
    repo: &std::path::Path,
    download_python_builds: bool,
    download_workers: usize,
    progress: Option<ProgressHandle>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(ref p) = progress {
        p.emit(SyncEvent::PhaseStarted {
            phase: "finalize",
            total: None,
        });
    }

    let python_build_entries = maybe_download_python_builds(
        repo,
        download_python_builds,
        download_workers,
        progress.clone(),
    )
    .await?;

    rebuild_indexes(repo, python_build_entries, progress.clone()).await?;

    if let Some(ref p) = progress {
        p.emit(SyncEvent::PhaseFinished {
            phase: "finalize",
            summary: "索引生成完成".to_string(),
        });
    }

    Ok(())
}

async fn maybe_download_python_builds(
    repo: &Path,
    enabled: bool,
    workers: usize,
    progress: Option<ProgressHandle>,
) -> Result<Option<Vec<PythonBuildEntry>>, Box<dyn std::error::Error>> {
    if !enabled {
        return Ok(None);
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?;
    let entries =
        download_python_builds_batch(&client, repo, workers, progress).await?;
    info!("已下载 Python 解释器，开始生成 python-builds/index.json");
    Ok(Some(entries))
}

async fn rebuild_indexes(
    repo: &Path,
    python_build_entries: Option<Vec<PythonBuildEntry>>,
    progress: Option<ProgressHandle>,
) -> Result<(), Box<dyn std::error::Error>> {
    let repo_clone = repo.to_path_buf();
    tokio::task::spawn_blocking(move || {
        if let Some(entries) = python_build_entries {
            build_python_builds_index(&entries, &repo_clone)
                .map_err(|e| format!("生成 python-builds index 失败: {e}"))?;
        }
        generate_index(&repo_clone, progress);
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("索引生成线程错误: {e}"))??;
    Ok(())
}

pub async fn finalize_mirror(
    repo: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let repo_clone = repo.to_path_buf();
    tokio::task::spawn_blocking(move || {
        generate_index(&repo_clone, None);
        crate::packager::pack_mirror_archive(&repo_clone)
            .map_err(|e| format!("打包镜像失败: {e}"))?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("打包线程错误: {e}"))??;
    Ok(())
}
