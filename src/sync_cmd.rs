use std::path::Path;

use tracing::info;

use crate::config::validator::{ConfigError, ConfigValidator};
use crate::config::{Config, PackageSpec};
use crate::downloader::DownloadableItem;

fn log_incremental_archive(archive: &Path) {
    info!(
        "增量包: {} ({:.2} MB)",
        archive.display(),
        crate::sync::archive_mb(archive)
    );
}

async fn do_incremental_pack(
    config: &Config,
    downloaded: Vec<DownloadableItem>,
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

pub fn cli_packages_to_specs(
    packages: Option<Vec<String>>,
) -> Result<Vec<PackageSpec>, String> {
    let Some(p) = packages else {
        return Ok(Vec::new());
    };
    for name in &p {
        if ConfigValidator::looks_like_url(name) {
            return Err(
                ConfigError::UrlMistakenForName(name.to_string()).to_string()
            );
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

fn build_sync_client(
    config: &Config,
) -> Result<crate::http::HttpClient, Box<dyn std::error::Error>> {
    Ok(crate::http::HttpClient::builder()
        .with_timeout(300)
        .with_mirrors(config.effective_mirrors())?
        .build()?)
}

async fn run_sync_pipeline(
    config: &Config,
    pkgs: &[PackageSpec],
    no_deps: bool,
    dry_run: bool,
    progress: crate::progress::ProgressHandle,
) -> Result<crate::sync::phases::SyncOutcome, Box<dyn std::error::Error>> {
    let client = build_sync_client(config)?;
    crate::sync::SyncPipeline::new(config, client, pkgs)
        .no_deps(no_deps)
        .dry_run(dry_run)
        .download_python_builds(true)
        .run(Some(progress))
        .await
        .map_err(Into::into)
}

async fn perform_sync(
    config: Config,
    pkgs: Vec<PackageSpec>,
    no_deps: bool,
    dry_run: bool,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::progress::run_with_progress(verbose, |progress| async move {
        let outcome = run_sync_pipeline(
            &config,
            &pkgs,
            no_deps,
            dry_run,
            progress.clone(),
        )
        .await?;

        if !dry_run {
            do_incremental_pack(&config, outcome.downloaded).await?;
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

async fn run_sync_full_inner(
    config: Config,
    pkgs: Vec<PackageSpec>,
    no_deps: bool,
    dry_run: bool,
    progress: crate::progress::ProgressHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    if !dry_run {
        crate::sync::clean_repo(&config.repository_dir)?;
    }
    let _outcome =
        run_sync_pipeline(&config, &pkgs, no_deps, dry_run, progress).await?;
    if !dry_run {
        crate::sync::finalize_mirror(&config.repository_dir).await?;
    }
    Ok(())
}

async fn perform_sync_full(
    config: Config,
    pkgs: Vec<PackageSpec>,
    no_deps: bool,
    dry_run: bool,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::progress::run_with_progress(verbose, |progress| async move {
        run_sync_full_inner(config, pkgs, no_deps, dry_run, progress).await
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

/// 已知的顶层路径名（无顶层文件夹的归档直接使用这些）
const KNOWN_TOP_LEVEL: &[&str] = &["simple", "python-builds", ".store.db"];

/// 剥离归档条目的顶层文件夹前缀。
///
/// 如果第一层是已知路径名（simple/python-builds/.store.db），保持原路径；
/// 否则剥掉第一层目录。返回 `None` 表示该条目是顶层文件夹自身，应跳过。
fn strip_top_level(
    path: &std::path::Path,
    stripped_prefix: &mut Option<String>,
) -> Option<std::path::PathBuf> {
    let first = path.iter().next()?;
    let name = first.to_string_lossy().to_string();

    if KNOWN_TOP_LEVEL.contains(&name.as_str()) {
        return Some(path.to_path_buf());
    }

    // 有顶层文件夹，剥掉第一层
    if stripped_prefix.is_none() {
        *stripped_prefix = Some(name.clone());
        info!("检测到顶层文件夹, 导入时自动剥离: {}", name);
    }

    let stripped: std::path::PathBuf = path.iter().skip(1).collect();
    if stripped.as_os_str().is_empty() {
        None
    } else {
        Some(stripped)
    }
}

/// 解包单个归档条目到目标目录
fn unpack_entry(
    entry: &mut tar::Entry<'_, Box<dyn std::io::Read>>,
    repo_dir: &std::path::Path,
    stripped_prefix: &mut Option<String>,
) -> Result<(), String> {
    let path = entry
        .path()
        .map_err(|e| format!("获取条目路径失败: {e}"))?
        .to_path_buf();

    let Some(dest_rel) = strip_top_level(&path, stripped_prefix) else {
        return Ok(());
    };

    let dest = repo_dir.join(&dest_rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录失败: {e}"))?;
    }
    entry
        .unpack(&dest)
        .map(|_| ())
        .map_err(|e| format!("解包失败: {e}"))
}

/// 遍历归档并逐条解包到 repo_dir
fn unpack_entries(
    archive: &std::path::Path,
    repo_dir: &std::path::Path,
) -> Result<(), String> {
    let reader = crate::packager::open_archive_reader(archive)
        .map_err(|e| format!("打开增量包失败: {e}"))?;
    let mut tar = tar::Archive::new(reader);
    let mut stripped_prefix: Option<String> = None;

    for entry in tar
        .entries()
        .map_err(|e| format!("读取归档条目失败: {e}"))?
    {
        let mut entry = entry.map_err(|e| format!("读取条目失败: {e}"))?;
        unpack_entry(&mut entry, repo_dir, &mut stripped_prefix)?;
    }
    Ok(())
}

async fn unpack_archive(
    archive: &std::path::Path,
    repo_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let archive = archive.to_path_buf();
    let repo_dir = repo_dir.to_path_buf();
    tokio::task::spawn_blocking(move || unpack_entries(&archive, &repo_dir))
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
