pub mod download;
pub mod finalize;
pub mod plan;
pub mod record;

pub use download::DownloadPhase;
pub use finalize::FinalizePhase;
pub use plan::PlanPhase;
pub use record::RecordPhase;

use crate::downloader::FileInfo;
use crate::http::HttpClient;
use crate::progress::{ProgressHandle, SyncEvent};

/// 同步流水线执行结果。
pub struct SyncOutcome {
    pub client: HttpClient,
    pub downloaded: Vec<FileInfo>,
    pub skipped: Vec<FileInfo>,
    pub failed: Vec<(FileInfo, String)>,
}

pub(crate) fn emit_phase_started(
    progress: &Option<ProgressHandle>,
    phase: &'static str,
    total: Option<u64>,
) {
    if let Some(p) = progress {
        p.emit(SyncEvent::PhaseStarted { phase, total });
    }
}

pub(crate) fn emit_phase_finished(
    progress: &Option<ProgressHandle>,
    phase: &'static str,
    summary: String,
) {
    if let Some(p) = progress {
        p.emit(SyncEvent::PhaseFinished { phase, summary });
    }
}
