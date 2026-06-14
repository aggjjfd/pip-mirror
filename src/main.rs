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
    /// 增量同步: wheel + Python 解释器, 产 incremental_*.tar.zst
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
    /// 全量同步: 清空仓库 → 重拉 → 产 mirror.tar.zst + mirror.sha256
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
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    cmd_sync(c.as_deref(), p, nd, dr, verbose).await
}
async fn cmd_sync_full_d(
    c: Option<PathBuf>,
    p: Option<Vec<String>>,
    nd: bool,
    dr: bool,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    cmd_sync_full(c.as_deref(), p, nd, dr, verbose).await
}
async fn cmd_serve_d(
    c: Option<PathBuf>,
    h: Option<String>,
    p: Option<u16>,
) -> Result<(), Box<dyn std::error::Error>> {
    cmd_serve(c.as_deref(), h, p).await
}

async fn try_sync(
    cmd: Command,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Command::Sync {
        config,
        packages,
        no_deps,
        dry_run,
    } = cmd
    {
        return cmd_sync_d(config, packages, no_deps, dry_run, verbose).await;
    }
    try_sync_full(cmd, verbose).await
}
async fn try_sync_full(
    cmd: Command,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Command::SyncFull {
        config,
        packages,
        no_deps,
        dry_run,
    } = cmd
    {
        return cmd_sync_full_d(config, packages, no_deps, dry_run, verbose)
            .await;
    }
    try_serve(cmd, verbose).await
}
async fn try_serve(
    cmd: Command,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Command::Serve { config, host, port } = cmd {
        return cmd_serve_d(config, host, port).await;
    }
    try_import(cmd, verbose).await
}
async fn try_import(
    cmd: Command,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Command::ImportIncremental {
        archive,
        config,
        no_reindex,
        strict,
    } = cmd
    {
        return cmd_import_incremental(
            &archive,
            config.as_deref(),
            no_reindex,
            strict,
            verbose,
        )
        .await;
    }
    try_access(cmd).await
}
async fn try_access(cmd: Command) -> Result<(), Box<dyn std::error::Error>> {
    if let Command::AccessLog { config, limit } = cmd {
        return cmd_access_log(config.as_deref(), limit);
    }
    try_init(cmd).await
}
async fn try_init(cmd: Command) -> Result<(), Box<dyn std::error::Error>> {
    if let Command::Init { output } = cmd {
        return cmd_init(&output);
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    pip_mirror::logging::init(cli.verbose);
    let verbose = cli.verbose;
    try_sync(cli.command, verbose).await
}

// ── command implementations ──

use pip_mirror::sync_cmd::{cmd_import_incremental, cmd_sync, cmd_sync_full};

async fn cmd_serve(
    config_path: Option<&std::path::Path>,
    host: Option<String>,
    port: Option<u16>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = pip_mirror::config::Config::load(config_path)?;
    let h = host.unwrap_or(config.server_host);
    let p = port.unwrap_or(config.server_port);
    pip_mirror::server::start_server(
        &h,
        p,
        config.repository_dir,
        config.targets,
    )
    .await?;
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

#[cfg(test)]
mod tests {
    use pip_mirror::sync_cmd::cli_packages_to_specs;

    #[test]
    fn test_cli_packages_to_specs_redacts_url_in_error() {
        let url = "https://user:pass@example.com/foo.whl?token=secret";
        let err = cli_packages_to_specs(Some(vec![url.to_string()]))
            .expect_err("should fail");
        assert!(
            !err.contains("user:pass"),
            "error leaked credentials: {err}"
        );
        assert!(!err.contains("token=secret"), "error leaked token: {err}");
        assert!(
            err.contains("example.com/foo.whl"),
            "error should keep host/path for user context: {err}"
        );
    }
}
