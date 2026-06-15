use std::path::Path;

use crate::downloader::DownloadResult;
use crate::sync::pipeline::SyncError;
use crate::sync::record;

pub struct RecordPhase;

impl RecordPhase {
    pub async fn run(
        repo: &Path,
        result: &DownloadResult,
    ) -> Result<(), SyncError> {
        record::record_download_results(repo, result).await?;
        Ok(())
    }
}
