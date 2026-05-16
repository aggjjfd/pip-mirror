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
        /// 仅解析依赖，不下载文件
        #[arg(long)]
        dry_run: bool,
    },
    /// 全量同步: 清空仓库 → 重拉 → 产 mirror.tar.gz + mirror.sha256
    SyncFull {
        #[arg(short = 'c', long)]
        config: Option<PathBuf>,
        #[arg(short, long, num_args = 1..)]
        packages: Option<Vec<String>>,
        #[arg(long)]
        no_deps: bool,
        /// 仅解析依赖，不下载文件
        #[arg(long)]
        dry_run: bool,
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
    /// 生成示例配置文件
    Init {
        #[arg(short = 'o', long, default_value = "pip-mirror.toml")]
        output: PathBuf,
    },
}

async fn cmd_sync_d(
    c: Option<PathBuf>,
    p: Option<Vec<String>>,
    nd: bool,
    dr: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    cmd_sync(c.as_deref(), p, nd, dr).await
}
async fn cmd_sync_full_d(
    c: Option<PathBuf>,
    p: Option<Vec<String>>,
    nd: bool,
    dr: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    cmd_sync_full(c.as_deref(), p, nd, dr).await
}
async fn cmd_serve_d(
    c: Option<PathBuf>,
    h: Option<String>,
    p: Option<u16>,
) -> Result<(), Box<dyn std::error::Error>> {
    cmd_serve(c.as_deref(), h, p).await
}

async fn try_sync(cmd: Command) -> Result<(), Box<dyn std::error::Error>> {
    if let Command::Sync {
        config,
        packages,
        no_deps,
        dry_run,
    } = cmd
    {
        return cmd_sync_d(config, packages, no_deps, dry_run).await;
    }
    try_sync_full(cmd).await
}
async fn try_sync_full(cmd: Command) -> Result<(), Box<dyn std::error::Error>> {
    if let Command::SyncFull {
        config,
        packages,
        no_deps,
        dry_run,
    } = cmd
    {
        return cmd_sync_full_d(config, packages, no_deps, dry_run).await;
    }
    try_serve(cmd).await
}
async fn try_serve(cmd: Command) -> Result<(), Box<dyn std::error::Error>> {
    if let Command::Serve { config, host, port } = cmd {
        return cmd_serve_d(config, host, port).await;
    }
    try_import(cmd)
}
fn try_import(cmd: Command) -> Result<(), Box<dyn std::error::Error>> {
    if let Command::ImportIncremental {
        archive,
        config,
        no_reindex,
        strict,
    } = cmd
    {
        return cmd_import_incremental(ImportIncrementalArgs {
            archive: &archive,
            config_path: config.as_deref(),
            no_reindex,
            strict,
        });
    }
    try_access(cmd)
}
fn try_access(cmd: Command) -> Result<(), Box<dyn std::error::Error>> {
    if let Command::AccessLog { config, limit } = cmd {
        return cmd_access_log(config.as_deref(), limit);
    }
    try_init(cmd)
}
fn try_init(cmd: Command) -> Result<(), Box<dyn std::error::Error>> {
    if let Command::Init { output } = cmd {
        return cmd_init(&output);
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    pip_mirror::logging::init(cli.verbose);
    try_sync(cli.command).await
}

// ── command implementations ──

fn log_incremental_archive(archive: &std::path::Path) {
    info!(
        "增量包: {} ({:.2} MB)",
        archive.display(),
        pip_mirror::sync::archive_mb(archive)
    );
}

async fn cmd_sync(
    config_path: Option<&std::path::Path>,
    packages: Option<Vec<String>>,
    no_deps: bool,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = pip_mirror::config::Config::load(config_path)?;
    let pkgs = packages.unwrap_or_else(|| config.packages.clone());
    info!("增量同步: {} 个包", pkgs.len());
    if !dry_run {
        std::fs::create_dir_all(&config.repository_dir)?;
    }
    let (_client, downloaded) =
        pip_mirror::sync::do_sync(&config, &pkgs, no_deps, true, dry_run).await?;
    if dry_run {
        return Ok(());
    }
    std::fs::create_dir_all(&config.incremental_dir)?;
    if let Some(a) = pip_mirror::packager::build_incremental_package_async(
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

async fn cmd_sync_full(
    config_path: Option<&std::path::Path>,
    packages: Option<Vec<String>>,
    no_deps: bool,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = pip_mirror::config::Config::load(config_path)?;
    let pkgs = packages.unwrap_or_else(|| config.packages.clone());
    info!("全量同步: {} 个包", pkgs.len());
    if !dry_run {
        pip_mirror::sync::clean_repo(&config.repository_dir)?;
    }
    let (client, _downloaded) =
        pip_mirror::sync::do_sync(&config, &pkgs, no_deps, true, dry_run).await?;
    if dry_run {
        return Ok(());
    }
    pip_mirror::sync::finalize_mirror(&client, &config.repository_dir).await
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

struct ImportIncrementalArgs<'a> {
    archive: &'a std::path::Path,
    config_path: Option<&'a std::path::Path>,
    no_reindex: bool,
    strict: bool,
}

#[allow(unused_variables)]
fn cmd_import_incremental(
    args: ImportIncrementalArgs<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = pip_mirror::config::Config::load(args.config_path)?;
    if args.strict {
        info!("严格模式: 校验增量包完整性");
    }
    info!(
        "解包 {} → {}",
        args.archive.display(),
        config.repository_dir.display()
    );
    let f = std::fs::File::open(args.archive)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(f));
    tar.unpack(&config.repository_dir)?;
    if !args.no_reindex {
        pip_mirror::indexer::generate_index(&config.repository_dir);
    }
    info!("导入完成");
    Ok(())
}

fn print_access_ips(logger: &pip_mirror::access_log::AccessLogger) {
    for (ip, count) in logger.get_top_ips(10) {
        info!("  {ip}: {count} 次");
    }
}

fn print_access_recent(
    logger: &pip_mirror::access_log::AccessLogger,
    limit: usize,
) {
    for r in logger.get_recent(limit) {
        info!(
            "  [{}] {} {} {} {}",
            r.timestamp, r.client_ip, r.method, r.path, r.status_code
        );
    }
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
    let s = logger.get_summary();
    info!(
        "总请求数: {}\n成功请求: {}\n独立 IP 数: {}",
        s.total_requests, s.successful_requests, s.unique_ips
    );
    print_access_ips(&logger);
    print_access_recent(&logger, limit);
    Ok(())
}

const INIT_TEMPLATE: &str = r#"packages = [
    "requests",
    "gradio",
    "markitdown[pptx,docx,xls,xlsx,pdf]",
]
repository_dir = "./packages"
incremental_dir = "./incremental"
pypi_url = "https://pypi.org"
index_url = "https://mirrors.ustc.edu.cn/pypi/simple"
include_source = false
resolve_workers = 8
metadata_workers = 32
download_workers = 8
top_versions_per_package = 5
adjacent_versions_per_side = 2
allow_prerelease = false
linux_max_glibc = "2.39"
server_host = "127.0.0.1"
server_port = 8080
"#;

fn cmd_init(
    output: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if output.exists() {
        return Err(format!("文件已存在: {}", output.display()).into());
    }
    std::fs::write(output, INIT_TEMPLATE)?;
    info!("示例配置已生成: {}", output.display());
    Ok(())
}
