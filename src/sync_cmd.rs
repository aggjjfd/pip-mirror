use std::path::Path;

use tracing::info;

use crate::config::{Config, PackageSpec};
use crate::downloader::FileInfo;

fn log_incremental_archive(archive: &Path) {
    info!(
        "增量包: {} ({:.2} MB)",
        archive.display(),
        crate::sync::archive_mb(archive)
    );
}

async fn do_incremental_pack(
    config: &Config,
    downloaded: Vec<FileInfo>,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(&config.incremental_dir)?;
    if let Some(a) = crate::packager::build_incremental_package_async(
        &config.repository_dir,
        &downloaded,
        &config.incremental_dir,
    )
    .await?
    {
        log_incremental_archive(&a);
    }
    Ok(())
}

fn looks_like_url(s: &str) -> bool {
    let lower = s.trim().to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("file://")
}

pub fn cli_packages_to_specs(
    packages: Option<Vec<String>>,
) -> Result<Vec<PackageSpec>, String> {
    let Some(p) = packages else {
        return Ok(Vec::new());
    };
    for name in &p {
        if looks_like_url(name) {
            let safe = crate::filters::redact_url_for_display(name);
            return Err(format!(
                "命令行包名 `{safe}` 看起来像 URL。如需指定 whl URL，请使用配置文件中的 `{{ url = \"{safe}\" }}` 表格式。"
            ));
        }
    }
    Ok(p.into_iter().map(PackageSpec::Name).collect())
}

pub fn load_packages(
    config: &Config,
    packages: Option<Vec<String>>,
) -> Result<Vec<PackageSpec>, Box<dyn std::error::Error>> {
    if let Some(p) = packages {
        Ok(cli_packages_to_specs(Some(p))?)
    } else {
        Ok(config.packages.clone())
    }
}

async fn perform_sync(
    config: Config,
    pkgs: Vec<PackageSpec>,
    no_deps: bool,
    dry_run: bool,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::progress::run_with_progress(verbose, |progress| async move {
        let (_client, downloaded) = crate::sync::do_sync(
            &config,
            &pkgs,
            no_deps,
            true,
            dry_run,
            Some(progress.clone()),
        )
        .await?;

        if !dry_run {
            do_incremental_pack(&config, downloaded).await?;
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .await
}

pub async fn cmd_sync(
    config_path: Option<&Path>,
    packages: Option<Vec<String>>,
    no_deps: bool,
    dry_run: bool,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load(config_path)?;
    let pkgs = load_packages(&config, packages)?;
    info!("增量同步: {} 个包", pkgs.len());
    std::fs::create_dir_all(&config.repository_dir)?;
    perform_sync(config, pkgs, no_deps, dry_run, verbose).await
}

async fn perform_sync_full(
    config: Config,
    pkgs: Vec<PackageSpec>,
    no_deps: bool,
    dry_run: bool,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::progress::run_with_progress(verbose, |progress| async move {
        if dry_run {
            let (_client, _downloaded) = crate::sync::do_sync(
                &config,
                &pkgs,
                no_deps,
                true,
                true,
                Some(progress.clone()),
            )
            .await?;
            return Ok::<(), Box<dyn std::error::Error>>(());
        }
        crate::sync::clean_repo(&config.repository_dir)?;
        let (client, _downloaded) = crate::sync::do_sync(
            &config,
            &pkgs,
            no_deps,
            true,
            false,
            Some(progress.clone()),
        )
        .await?;
        crate::sync::finalize_mirror(&client, &config.repository_dir).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .await
}

pub async fn cmd_sync_full(
    config_path: Option<&Path>,
    packages: Option<Vec<String>>,
    no_deps: bool,
    dry_run: bool,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load(config_path)?;
    let pkgs = load_packages(&config, packages)?;
    info!("全量同步: {} 个包", pkgs.len());
    perform_sync_full(config, pkgs, no_deps, dry_run, verbose).await
}

fn emit_import_started(
    progress: &crate::progress::ProgressHandle,
    message: &str,
) {
    progress.emit(crate::progress::SyncEvent::PhaseStarted {
        phase: "import",
        total: None,
    });
    progress.emit(crate::progress::SyncEvent::PhaseProgress {
        phase: "import",
        current: 0,
        message: message.to_string(),
    });
}

fn emit_import_finished(
    progress: &crate::progress::ProgressHandle,
    summary: &str,
) {
    progress.emit(crate::progress::SyncEvent::PhaseFinished {
        phase: "import",
        summary: summary.to_string(),
    });
}

async fn unpack_archive(
    archive: &std::path::Path,
    repo_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let archive = archive.to_path_buf();
    let repo_dir = repo_dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let reader = crate::packager::open_archive_reader(&archive)
            .map_err(|e| format!("打开增量包失败: {e}"))?;
        let mut tar = tar::Archive::new(reader);
        tar.unpack(&repo_dir)
            .map_err(|e| format!("解包失败: {e}"))?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("解包任务异常: {e}"))?
    .map_err(|e| -> Box<dyn std::error::Error> { e.into() })
}

async fn perform_import_incremental(
    config: Config,
    archive: std::path::PathBuf,
    no_reindex: bool,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::progress::run_with_progress(verbose, |progress| async move {
        emit_import_started(&progress, &format!("解包 {}", archive.display()));
        unpack_archive(&archive, &config.repository_dir).await?;
        emit_import_finished(&progress, "解包完成");

        if !no_reindex {
            crate::indexer::generate_index(
                &config.repository_dir,
                Some(progress.clone()),
            );
        }

        emit_import_finished(&progress, "导入完成");
        info!("导入完成");
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .await
}

pub async fn cmd_import_incremental(
    archive: &Path,
    config_path: Option<&Path>,
    no_reindex: bool,
    strict: bool,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load(config_path)?;
    if strict {
        info!("严格模式: 校验增量包完整性");
    }
    info!(
        "解包 {} → {}",
        archive.display(),
        config.repository_dir.display()
    );
    perform_import_incremental(
        config,
        archive.to_path_buf(),
        no_reindex,
        verbose,
    )
    .await
}
