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

async fn cmd_sync_d(
    c: Option<PathBuf>,
    p: Option<Vec<String>>,
    nd: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    cmd_sync(c.as_deref(), p, nd).await
}
async fn cmd_sync_full_d(
    c: Option<PathBuf>,
    p: Option<Vec<String>>,
    nd: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    cmd_sync_full(c.as_deref(), p, nd).await
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
    } = cmd
    {
        return cmd_sync_d(config, packages, no_deps).await;
    }
    try_sync_full(cmd).await
}
async fn try_sync_full(cmd: Command) -> Result<(), Box<dyn std::error::Error>> {
    if let Command::SyncFull {
        config,
        packages,
        no_deps,
    } = cmd
    {
        return cmd_sync_full_d(config, packages, no_deps).await;
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
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    pip_mirror::logging::init(cli.verbose);
    try_sync(cli.command).await
}

// ── command implementations ──

async fn cmd_sync(
    config_path: Option<&std::path::Path>,
    packages: Option<Vec<String>>,
    _no_deps: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = pip_mirror::config::Config::load(config_path)?;
    info!(
        "增量同步: {} 个包",
        packages.unwrap_or(config.packages).len()
    );
    info!("TODO: sync wheels + python builds → incremental package");
    Ok(())
}

fn clean_repo(
    repo: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
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

async fn download_pkg_files(
    client: &reqwest::Client,
    repo: &std::path::Path,
    files: &[pip_mirror::downloader::FileInfo],
) {
    for fi in files {
        let dest = repo
            .join("simple")
            .join(&fi.package_name)
            .join(&fi.filename);
        let _ = pip_mirror::downloader::download_file(client, fi, &dest).await;
    }
}

struct SyncCtx<'a> {
    http: &'a pip_mirror::downloader::HttpCtx<'a>,
    repo: &'a std::path::Path,
}

async fn download_dep_versions(
    http: &pip_mirror::downloader::HttpCtx<'_>,
    repo: &std::path::Path,
    deps: &dashmap::DashMap<String, Vec<pep440_rs::Version>>,
) {
    let dep_list: Vec<String> = deps
        .iter()
        .map(|e| {
            let vers: Vec<_> =
                e.value().iter().map(|v| v.to_string()).collect();
            format!("  {}: [{}]", e.key(), vers.join(", "))
        })
        .collect();
    info!("依赖包清单 ({} 个):", dep_list.len());
    for line in &dep_list {
        info!("{line}");
    }
    for entry in deps.iter() {
        let pkg = entry.key();
        let vers = entry.value();
        let vers_set: std::collections::HashSet<String> =
            vers.iter().map(|v| v.to_string()).collect();
        let Ok(files) = pip_mirror::downloader::fetch_json_api(http, pkg).await
        else {
            tracing::warn!("  [FAIL] 依赖包 {pkg}: 获取元数据失败");
            continue;
        };
        let selected: Vec<_> = files
            .into_iter()
            .filter(|f| vers_set.contains(&f.version))
            .collect();
        for fi in &selected {
            info!("  → {} {} [{}]", fi.package_name, fi.version, fi.filename);
        }
        download_pkg_files(http.client, repo, &selected).await;
        info!("  [OK] {pkg}: {} 个文件", selected.len());
    }
}

async fn sync_top_packages(
    ctx: &SyncCtx<'_>,
    pkgs: &[String],
    max_versions: usize,
) -> dashmap::DashMap<String, Vec<pep440_rs::Version>> {
    let top_versions = dashmap::DashMap::new();
    for pkg in pkgs {
        let Ok(files) =
            pip_mirror::downloader::fetch_json_api(ctx.http, pkg).await
        else {
            tracing::warn!("  [FAIL] {pkg}: 获取数据失败");
            continue;
        };
        let selected = pip_mirror::downloader::select_latest_versions(
            &files,
            max_versions,
        );
        let mut vers: Vec<pep440_rs::Version> = selected
            .iter()
            .filter_map(|f| f.version.parse().ok())
            .collect();
        vers.sort_by(|a, b| b.cmp(a));
        vers.dedup();
        top_versions
            .insert(pip_mirror::resolver::pubgrub::bare_name(pkg), vers);
        info!("  [OK] {pkg}: {} files", selected.len());
        download_pkg_files(ctx.http.client, ctx.repo, &selected).await;
    }
    top_versions
}

async fn finalize_mirror(
    client: &reqwest::Client,
    repo: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let entries =
        pip_mirror::python_builds::download_python_builds_batch(client, repo)
            .await?;
    pip_mirror::python_builds::build_python_builds_index(&entries, repo)?;
    pip_mirror::indexer::generate_index(repo);
    pip_mirror::packager::pack_mirror_archive(repo)?;
    Ok(())
}

async fn cmd_sync_full(
    config_path: Option<&std::path::Path>,
    packages: Option<Vec<String>>,
    no_deps: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = pip_mirror::config::Config::load(config_path)?;
    let pkgs = packages.unwrap_or(config.packages);
    info!("全量同步: {} 个包", pkgs.len());
    let repo = &config.repository_dir;

    clean_repo(repo)?;

    let client = reqwest::Client::new();
    let http = pip_mirror::downloader::HttpCtx {
        client: &client,
        pypi_url: &config.pypi_url,
    };
    let top_versions = sync_top_packages(
        &SyncCtx { http: &http, repo },
        &pkgs,
        config.max_versions,
    )
    .await;

    if !no_deps && !top_versions.is_empty() {
        let params = pip_mirror::resolver::resolve::ResolveParams {
            top_packages: &pkgs,
            top_versions: &top_versions,
            pypi_url: &config.pypi_url,
            max_depth: config.max_depth,
            max_versions: config.max_versions,
            allow_prerelease: config.allow_prerelease,
        };
        let deps = pip_mirror::resolver::resolve::resolve_dependencies(
            &params, &client,
        )
        .await;
        if !deps.is_empty() {
            download_dep_versions(&http, repo, &deps).await;
        }
    }

    finalize_mirror(&client, repo).await
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

fn cmd_import_incremental(
    args: ImportIncrementalArgs<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = (args.no_reindex, args.strict);
    let config = pip_mirror::config::Config::load(args.config_path)?;
    info!(
        "解包 {} → {}",
        args.archive.display(),
        config.repository_dir.display()
    );
    info!("TODO: import-incremental full logic");
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
