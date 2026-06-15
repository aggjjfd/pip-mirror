use std::path::Path;

use crate::http::HttpClient;

pub mod finalize;
pub mod phases;
pub mod pipeline;
mod plan;
mod record;
pub mod url_wheel;
pub mod url_wheel_download;

pub use finalize::{finalize_mirror, finalize_sync};
pub use phases::SyncOutcome;
pub use pipeline::{SyncError, SyncPipeline};

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

pub fn build_sync_client(
    mirrors: Vec<String>,
) -> Result<HttpClient, Box<dyn std::error::Error>> {
    Ok(HttpClient::builder()
        .with_timeout(300)
        .with_mirrors(mirrors)?
        .build()?)
}
