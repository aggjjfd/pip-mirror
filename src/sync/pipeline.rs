use crate::config::{Config, PackageSpec};
use crate::http::HttpClient;
use crate::progress::ProgressHandle;
use crate::resolver::resolve::ResolveError;
use crate::sync::phases::{
    DownloadPhase, FinalizePhase, PlanPhase, RecordPhase, SyncOutcome,
    emit_phase_finished, emit_phase_started,
};

#[derive(Debug)]
pub enum SyncError {
    Resolve(ResolveError),
    Other(Box<dyn std::error::Error>),
    Message(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Resolve(e) => write!(f, "{e}"),
            SyncError::Other(e) => write!(f, "{e}"),
            SyncError::Message(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<ResolveError> for SyncError {
    fn from(value: ResolveError) -> Self {
        SyncError::Resolve(value)
    }
}

impl From<Box<dyn std::error::Error>> for SyncError {
    fn from(value: Box<dyn std::error::Error>) -> Self {
        SyncError::Other(value)
    }
}

impl From<String> for SyncError {
    fn from(value: String) -> Self {
        SyncError::Message(value)
    }
}

pub struct SyncPipeline {
    config: Config,
    client: HttpClient,
    pkgs: Vec<PackageSpec>,
    no_deps: bool,
    dry_run: bool,
    download_python_builds: bool,
}

impl SyncPipeline {
    pub fn new(
        config: &Config,
        client: HttpClient,
        pkgs: &[PackageSpec],
    ) -> Self {
        Self {
            config: config.clone(),
            client,
            pkgs: pkgs.to_vec(),
            no_deps: false,
            dry_run: false,
            download_python_builds: false,
        }
    }

    pub fn no_deps(mut self, value: bool) -> Self {
        self.no_deps = value;
        self
    }

    pub fn dry_run(mut self, value: bool) -> Self {
        self.dry_run = value;
        self
    }

    pub fn download_python_builds(mut self, value: bool) -> Self {
        self.download_python_builds = value;
        self
    }

    pub async fn run(
        self,
        progress: Option<ProgressHandle>,
    ) -> Result<SyncOutcome, SyncError> {
        let repo = &self.config.repository_dir;

        emit_phase_started(&progress, "plan", Some(self.pkgs.len() as u64));
        let plan = PlanPhase::run(
            &self.config,
            &self.client,
            &self.pkgs,
            self.no_deps,
            progress.clone(),
        )
        .await?;
        emit_phase_finished(
            &progress,
            "plan",
            format!("{} 个计划文件", plan.planned_files.len()),
        );

        emit_phase_started(
            &progress,
            "download",
            Some(plan.planned_files.len() as u64),
        );
        let result = DownloadPhase::run(
            &self.config,
            &self.client,
            &plan,
            self.dry_run,
            progress.clone(),
        )
        .await?;

        if !self.dry_run {
            RecordPhase::run(repo, &result).await?;
            FinalizePhase::run(
                repo,
                &self.client,
                self.download_python_builds,
                self.config.download_workers,
                progress.clone(),
            )
            .await?;
        }

        Ok(SyncOutcome {
            client: self.client,
            downloaded: result.downloaded,
            skipped: result.skipped,
            failed: result.failed,
        })
    }
}
