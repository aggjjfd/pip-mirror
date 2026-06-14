#[derive(Debug, Clone, PartialEq)]
pub enum FileStatus {
    Downloaded,
    Reused,
    Skipped,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SyncEvent {
    PhaseStarted {
        phase: &'static str,
        total: Option<u64>,
    },
    PhaseProgress {
        phase: &'static str,
        current: u64,
        message: String,
    },
    PhaseFinished {
        phase: &'static str,
        summary: String,
    },
    FileDone {
        package: String,
        filename: String,
        status: FileStatus,
    },
    Error {
        message: String,
    },
    Warning {
        message: String,
    },
}
