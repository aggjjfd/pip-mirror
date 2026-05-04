use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tracing::info;

#[derive(Parser)]
#[command(name = "pip-mirror", about = "轻量级私有 PIP 仓库")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Command {
    /// 增量同步: wheel + Python 解释器, 产 incremental_*.tar.gz
    Sync {
        #[arg(short = 'c', long)]
        config: Option<PathBuf>,
        #[arg(short, long, num_args = 1..)]
        packages: Option<Vec<String>>,
        #[arg(long)]
        no_deps: bool,
    },
    /// 全量同步: 清空仓库 → 重拉 → 产 mirror.tar.gz + mirror.sha256
    SyncFull {
        #[arg(short = 'c', long)]
        config: Option<PathBuf>,
        #[arg(short, long, num_args = 1..)]
        packages: Option<Vec<String>>,
        #[arg(long)]
        no_deps: bool,
    },
    /// 启动 HTTP 服务 (PEP 503/691)
    Serve {
        #[arg(short = 'c', long)]
        config: Option<PathBuf>,
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        port: Option<u16>,
    },
    /// 合并增量包到本地仓库
    ImportIncremental {
        archive: PathBuf,
        #[arg(short = 'c', long)]
        config: Option<PathBuf>,
        #[arg(long)]
        no_reindex: bool,
        #[arg(long)]
        strict: bool,
    },
    /// 查看访问日志统计
    AccessLog {
        #[arg(short = 'c', long)]
        config: Option<PathBuf>,
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    pip_mirror::logging::init(cli.verbose);

    match cli.command {
        Command::Sync { config, packages, no_deps } => {
            cmd_sync(config.as_deref(), packages, no_deps).await?;
        }
        Command::SyncFull { config, packages, no_deps } => {
            cmd_sync_full(config.as_deref(), packages, no_deps).await?;
        }
        Command::Serve { config, host, port } => {
            cmd_serve(config.as_deref(), host, port).await?;
        }
        Command::ImportIncremental { archive, config, no_reindex, strict } => {
            cmd_import_incremental(&archive, config.as_deref(), no_reindex, strict)?;
        }
        Command::AccessLog { config, limit } => {
            cmd_access_log(config.as_deref(), limit)?;
        }
    }

    Ok(())
}

// ── command implementations ──

async fn cmd_sync(
    config_path: Option<&std::path::Path>,
    packages: Option<Vec<String>>,
    _no_deps: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = pip_mirror::config::Config::load(config_path)?;
    let pkgs = packages.unwrap_or(config.packages);
    info!("增量同步: {} 个包", pkgs.len());
    info!("TODO: sync wheels + python builds → incremental package");
    Ok(())
}

async fn cmd_sync_full(
    config_path: Option<&std::path::Path>,
    packages: Option<Vec<String>>,
    _no_deps: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = pip_mirror::config::Config::load(config_path)?;
    let pkgs = packages.unwrap_or(config.packages);
    info!("全量同步: {} 个包", pkgs.len());

    let repo = &config.repository_dir;
    let simple_dir = repo.join("simple");
    let python_dir = repo.join("python-builds");
    let store_db = repo.join(".store.db");

    if simple_dir.exists() {
        std::fs::remove_dir_all(&simple_dir)?;
    }
    if python_dir.exists() {
        std::fs::remove_dir_all(&python_dir)?;
    }
    if store_db.exists() {
        std::fs::remove_file(&store_db)?;
    }
    std::fs::create_dir_all(repo)?;

    info!("TODO: download top packages + resolve deps + download deps (M2-M4)");

    let client = reqwest::Client::new();
    let entries = pip_mirror::python_builds::fetch_python_builds(&client).await?;
    let output_dir = repo.join("python-builds");
    std::fs::create_dir_all(&output_dir)?;

    for entry in &entries {
        match pip_mirror::python_builds::download_python_build(&client, entry, &output_dir).await {
            Ok((_, downloaded)) => {
                if downloaded {
                    info!("  [OK] {}", entry.filename);
                }
            }
            Err(e) => {
                tracing::warn!("  [FAIL] {}: {e}", entry.filename);
            }
        }
    }

    let mut local_metadata = serde_json::Map::new();
    for entry in &entries {
        let mut e = serde_json::json!({ "url": format!("/python-builds/{}", entry.filename) });
        if let Some(sha) = &entry.sha256 {
            e["sha256"] = serde_json::Value::String(sha.clone());
        }
        local_metadata.insert(entry.key.clone(), e);
    }
    let index_path = output_dir.join("index.json");
    std::fs::write(&index_path, serde_json::to_string_pretty(&local_metadata)?)?;

    pip_mirror::indexer::generate_index(repo);

    let compression = match std::env::var("PIP_MIRROR_TAR_COMPRESSION").as_deref() {
        Ok("none") => flate2::Compression::none(),
        _ => flate2::Compression::best(),
    };
    let archive = std::env::current_dir()?.join("mirror.tar.gz");
    pip_mirror::downloader::pack_full_mirror(repo, &archive, compression)?;
    let sha_path = pip_mirror::packager::write_sha256(&archive)?;

    let size_mb = std::fs::metadata(&archive)?.len() as f64 / 1024.0 / 1024.0;
    info!("mirror.tar.gz : {} ({size_mb:.2} MB)", archive.display());
    info!("mirror.sha256 : {}", sha_path.display());

    Ok(())
}

async fn cmd_serve(
    config_path: Option<&std::path::Path>,
    host: Option<String>,
    port: Option<u16>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = pip_mirror::config::Config::load(config_path)?;
    let h = host.unwrap_or(config.server_host);
    let p = port.unwrap_or(config.server_port);
    pip_mirror::server::start_server(&h, p, config.repository_dir).await?;
    Ok(())
}

fn cmd_import_incremental(
    archive: &std::path::Path,
    config_path: Option<&std::path::Path>,
    _no_reindex: bool,
    _strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = pip_mirror::config::Config::load(config_path)?;
    info!("解包 {} → {}", archive.display(), config.repository_dir.display());
    info!("TODO: import-incremental full logic");
    Ok(())
}

fn cmd_access_log(
    config_path: Option<&std::path::Path>,
    limit: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = pip_mirror::config::Config::load(config_path)?;
    let db_path = config.repository_dir.join(".access_log.db");
    if !db_path.exists() {
        tracing::warn!("访问日志数据库不存在: {}", db_path.display());
        return Ok(());
    }

    let logger = pip_mirror::access_log::AccessLogger::open(&db_path)?;
    let summary = logger.get_summary();
    info!("总请求数: {}", summary.total_requests);
    info!("成功请求: {}", summary.successful_requests);
    info!("独立 IP 数: {}", summary.unique_ips);

    let top_ips = logger.get_top_ips(10);
    if !top_ips.is_empty() {
        info!("下载量最多的 IP:");
        for (ip, count) in top_ips {
            info!("  {ip}: {count} 次");
        }
    }

    let records = logger.get_recent(limit);
    if !records.is_empty() {
        info!("最近 {limit} 条访问记录:");
        for r in records {
            info!(
                "  [{}] {} {} {} {}",
                r.timestamp, r.client_ip, r.method, r.path, r.status_code
            );
        }
    }

    Ok(())
}
