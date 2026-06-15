use std::path::Path;

use crate::http::HttpClient;
use crate::progress::ProgressHandle;
use crate::sync::finalize;
use crate::sync::pipeline::SyncError;

pub struct FinalizePhase;

impl FinalizePhase {
    pub async fn run(
        repo: &Path,
        client: &HttpClient,
        download_python_builds: bool,
        download_workers: usize,
        progress: Option<ProgressHandle>,
    ) -> Result<(), SyncError> {
        finalize::finalize_sync(
            repo,
            client,
            download_python_builds,
            download_workers,
            progress,
        )
        .await?;
        Ok(())
    }
}
