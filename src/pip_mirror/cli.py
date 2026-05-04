"""命令行入口."""

from __future__ import annotations

import argparse
import logging
import sys
from pathlib import Path

from .access_logger import AccessLogger
from .config import Config, write_example_config
from .dependency_resolver import extract_extras, resolve_dependencies
from .downloader import download_packages
from .indexer import generate_index
from .log import setup_logging
from .packager import create_incremental_package
from .python_downloader import sync_python_builds
from .server import start_server

logger = logging.getLogger("pip-mirror")


def _cmd_sync(args: argparse.Namespace) -> int:
    """执行同步命令."""
    config = Config.load(Path(args.config) if args.config else None)

    packages = args.packages if args.packages else config.packages

    if not packages:
        logger.error("未指定要同步的包")
        logger.info("请在配置文件中设置 packages，或通过 --packages 参数指定")
        return 1

    # 区分顶层包（可能包含 extras）和纯包名
    top_packages = packages
    top_pkg_names = [extract_extras(p)[0] for p in top_packages]

    all_downloaded: list = []
    all_warnings: list = []

    # ========== 第一遍：下载顶层包 ==========
    logger.info("")
    logger.info("=" * 50)
    logger.info("第 1/2 步：下载顶层包")
    logger.info("=" * 50)

    top_result = download_packages(
        packages=top_pkg_names,
        repository_dir=config.repository_dir,
        pypi_url=config.pypi_url,
        index_url=config.index_url,
        include_source=config.include_source,
        workers=config.workers,
        max_versions=config.max_versions,
        allow_prerelease=config.allow_prerelease,
    )

    all_downloaded.extend(top_result.downloaded)
    all_warnings.extend(top_result.warnings)

    # 收集顶层包已下载的版本
    top_versions: dict[str, list[str]] = {}
    for fi in top_result.downloaded + top_result.skipped:
        if fi.version:
            versions = top_versions.setdefault(fi.package_name, [])
            if fi.version not in versions:
                versions.append(fi.version)

    # ========== 第二遍：解析并下载依赖 ==========
    if not args.no_deps:
        logger.info("")
        logger.info("=" * 50)
        logger.info("第 2/2 步：解析并下载依赖")
        logger.info("=" * 50)

        dep_versions = resolve_dependencies(
            top_packages=top_packages,
            top_versions=top_versions,
            pypi_url=config.pypi_url,
            workers=config.workers,
            allow_prerelease=config.allow_prerelease,
        )

        if dep_versions:
            dep_names = list(dep_versions.keys())
            dep_result = download_packages(
                packages=dep_names,
                repository_dir=config.repository_dir,
                pypi_url=config.pypi_url,
                index_url=config.index_url,
                include_source=config.include_source,
                workers=config.workers,
                specific_versions=dep_versions,
                allow_prerelease=config.allow_prerelease,
            )

            all_downloaded.extend(dep_result.downloaded)
            all_warnings.extend(dep_result.warnings)

    # ========== 生成索引和增量包 ==========
    generate_index(config.repository_dir)

    if all_downloaded and not args.no_pack:
        create_incremental_package(
            downloaded_files=all_downloaded,
            repository_dir=config.repository_dir,
            output_dir=config.incremental_dir,
            compress=not args.no_compress,
        )

    logger.info("")
    logger.info("=" * 50)
    logger.info("同步完成")
    logger.info("=" * 50)

    if all_warnings:
        logger.warning(f"警告 ({len(all_warnings)} 条):")
        for w in all_warnings:
            logger.warning(f"  ! {w}")

    return 0


def _cmd_serve(args: argparse.Namespace) -> int:
    """执行服务命令."""
    config = Config.load(Path(args.config) if args.config else None)

    host = args.host or config.server_host
    port = args.port or config.server_port

    generate_index(config.repository_dir)
    start_server(host=host, port=port, repository_dir=config.repository_dir)
    return 0


def _cmd_sync_python(args: argparse.Namespace) -> int:
    """同步 Python 解释器."""
    config = Config.load(Path(args.config) if args.config else None)

    workers = args.workers if args.workers else config.workers

    index_path = sync_python_builds(
        repository_dir=config.repository_dir,
        workers=workers,
    )

    logger.info("")
    logger.info("=" * 50)
    logger.info("Python 解释器同步完成")
    logger.info("=" * 50)
    logger.info(f"index.json: {index_path}")
    logger.info("")
    logger.info("内网 uv 使用方式:")
    logger.info("  export UV_PYTHON_DOWNLOADS_JSON_URL=http://<服务器>:<端口>/python-builds/index.json")
    logger.info("  uv python install 3.12")

    return 0


def _cmd_access_log(args: argparse.Namespace) -> int:
    """查看访问日志统计."""
    config = Config.load(Path(args.config) if args.config else None)

    db_path = config.repository_dir / ".access_log.db"
    if not db_path.exists():
        logger.warning(f"访问日志数据库不存在: {db_path}")
        logger.info("请先启动服务器并接收一些请求")
        return 1

    access_logger = AccessLogger(db_path)
    summary = access_logger.get_summary()

    logger.info("=" * 50)
    logger.info("访问日志统计")
    logger.info("=" * 50)
    logger.info(f"总请求数:   {summary['total_requests']}")
    logger.info(f"成功请求:   {summary['successful_requests']}")
    logger.info(f"独立 IP 数: {summary['unique_ips']}")
    logger.info("")

    top_ips = access_logger.get_top_ips(10)
    if top_ips:
        logger.info("--- 下载量最多的 IP ---")
        for ip, count in top_ips:
            logger.info(f"  {ip}: {count} 次")
        logger.info("")

    top_paths = access_logger.get_top_paths(15)
    if top_paths:
        logger.info("--- 下载量最多的文件 ---")
        for path, count in top_paths:
            logger.info(f"  {path}: {count} 次")
        logger.info("")

    records = access_logger.get_stats(args.limit)
    if records:
        logger.info(f"--- 最近 {len(records)} 条访问记录 ---")
        for r in records:
            ts = r['timestamp'][:19] if len(r['timestamp']) > 19 else r['timestamp']
            logger.info(f"  [{ts}] {r['client_ip']} {r['method']} {r['path']} {r['status_code']}")

    return 0


def _cmd_init(args: argparse.Namespace) -> int:
    """初始化示例配置."""
    config_path = Path(args.output)

    if config_path.exists() and not args.force:
        logger.warning(f"文件已存在: {config_path}")
        logger.info("使用 --force 覆盖")
        return 1

    write_example_config(config_path)
    logger.info(f"示例配置已生成: {config_path}")
    return 0


def _cmd_import_incremental(args: argparse.Namespace) -> int:
    """从 incremental tar.gz 合并到本地仓库:解包 + 写 .store.db + 重建索引."""
    import json
    import tarfile

    from .sqlite_store import DownloadStore

    config = Config.load(Path(args.config) if args.config else None)
    archive = Path(args.archive)
    repo = config.repository_dir

    if not archive.exists():
        logger.error(f"增量包不存在: {archive}")
        return 1

    repo.mkdir(parents=True, exist_ok=True)
    logger.info(f"解包 {archive} → {repo}")

    with tarfile.open(archive, "r:*") as tar:
        try:
            tar.extractall(repo, filter="data")  # Python 3.12+
        except TypeError:
            tar.extractall(repo)  # Python <3.12

    manifest_path = repo / "manifest.json"
    if not manifest_path.exists():
        logger.error(f"增量包缺 manifest.json: {archive}")
        return 1

    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    entries = manifest.get("files", [])

    store = DownloadStore(repo / ".store.db")
    file_count = 0
    metadata_count = 0
    for entry in entries:
        if not entry.get("sha256"):
            logger.warning(f"跳过 {entry.get('filename')}: 缺 sha256")
            continue
        store.add_file(
            filename=entry["filename"],
            package_name=entry["package"],
            version=entry["version"],
            sha256=entry["sha256"],
            size=entry.get("size"),
        )
        file_count += 1
        meta_sha = entry.get("metadata_sha256")
        if meta_sha:
            store.set_metadata_sha256(entry["filename"], meta_sha)
            metadata_count += 1

    manifest_path.unlink()  # 不污染 repository_dir
    logger.info(f"已写入 store: 文件 {file_count} 条, metadata {metadata_count} 条")

    if args.no_reindex:
        logger.info("--no-reindex 指定,跳过索引重建")
    else:
        from .indexer import generate_index
        generate_index(repo)

    return 0


def main() -> int:
    """CLI 入口."""
    parser = argparse.ArgumentParser(
        prog="pip-mirror",
        description="轻量级私有 PIP 仓库，支持增量同步",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
示例:
  pip-mirror sync                          # 同步配置文件中的包
  pip-mirror sync --packages requests numpy # 同步指定包
  pip-mirror sync --no-deps                # 不同步依赖
  pip-mirror serve --port 8080             # 启动 HTTP 服务
  pip-mirror init                          # 生成示例配置
  pip-mirror import-incremental incr.tar.gz # 内网合并增量包
        """,
    )

    subparsers = parser.add_subparsers(dest="command", help="可用命令")

    sync_parser = subparsers.add_parser("sync", help="从 PyPI 同步包")
    sync_parser.add_argument("-c", "--config", help="配置文件路径（TOML 格式）")
    sync_parser.add_argument("-p", "--packages", nargs="+", help="要同步的包名")
    sync_parser.add_argument("--no-deps", action="store_true", help="不下载依赖")
    sync_parser.add_argument("--no-pack", action="store_true", help="跳过增量打包")
    sync_parser.add_argument(
        "--no-compress", action="store_true",
        help="增量包不压缩（纯 tar，适合 GitHub Actions）",
    )

    sync_python_parser = subparsers.add_parser("sync-python", help="同步 Python 解释器")
    sync_python_parser.add_argument("-c", "--config", help="配置文件路径")
    sync_python_parser.add_argument("--workers", type=int, help="并发下载线程数")

    serve_parser = subparsers.add_parser("serve", help="启动 HTTP 服务")
    serve_parser.add_argument("-c", "--config", help="配置文件路径")
    serve_parser.add_argument("--host", help="监听地址")
    serve_parser.add_argument("--port", type=int, help="监听端口")

    access_log_parser = subparsers.add_parser("access-log", help="查看访问日志统计")
    access_log_parser.add_argument("-c", "--config", help="配置文件路径")
    access_log_parser.add_argument("-n", "--limit", type=int, default=20, help="显示最近 N 条记录")

    init_parser = subparsers.add_parser("init", help="生成示例配置文件")
    init_parser.add_argument("-o", "--output", default="pip-mirror.toml", help="输出文件名")
    init_parser.add_argument("-f", "--force", action="store_true", help="覆盖已存在的文件")

    import_parser = subparsers.add_parser(
        "import-incremental",
        help="将 incremental tar.gz 合并到本地仓库(写 .store.db 并重建索引)",
    )
    import_parser.add_argument("archive", help="incremental tar.gz 路径")
    import_parser.add_argument("-c", "--config", help="配置文件路径")
    import_parser.add_argument(
        "--no-reindex", action="store_true", help="跳过自动重建 PEP 503/691 索引",
    )

    parser.add_argument(
        "-v", "--verbose", action="store_true",
        help="显示 DEBUG 级别日志",
    )

    args = parser.parse_args()

    level = logging.DEBUG if args.verbose else logging.INFO
    setup_logging(level)

    if not args.command:
        parser.print_help()
        return 1

    commands = {
        "sync": _cmd_sync,
        "sync-python": _cmd_sync_python,
        "serve": _cmd_serve,
        "access-log": _cmd_access_log,
        "init": _cmd_init,
        "import-incremental": _cmd_import_incremental,
    }

    return commands[args.command](args)


if __name__ == "__main__":
    sys.exit(main())
