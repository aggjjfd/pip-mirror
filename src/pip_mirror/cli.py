"""命令行入口."""

from __future__ import annotations

import argparse
import hashlib
import json
import logging
import os
import shutil
import sys
import tarfile
from pathlib import Path

from .access_logger import AccessLogger
from .config import Config, write_example_config
from .dependency_resolver import extract_extras, resolve_dependencies
from .downloader import _extract_version_from_filename, download_packages
from .indexer import generate_index
from .log import setup_logging
from .packager import create_incremental_package
from .python_downloader import sync_python_builds
from .server import start_server
from .sqlite_store import DownloadStore

logger = logging.getLogger("pip-mirror")


# ============================================================================
#                           内部:wheel / python-builds 同步
# ============================================================================


def _sync_wheels(config: Config, packages: list[str], no_deps: bool) -> tuple[list, list[str]]:
    """同步 wheel/sdist,返回 (本次新下载的 FileInfo 列表, 警告列表).

    会抛任何 download_packages 内部未捕获的异常,由调用方 try/except 隔离。
    """
    top_packages = packages
    top_pkg_names = [extract_extras(p)[0] for p in top_packages]

    all_downloaded: list = []
    all_warnings: list[str] = []

    logger.info("")
    logger.info("=" * 50)
    logger.info("第 1/2 步:下载顶层包")
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
        backfill_scan_limit=config.backfill_scan_limit,
    )
    all_downloaded.extend(top_result.downloaded)
    all_warnings.extend(top_result.warnings)

    top_versions: dict[str, list[str]] = {}
    for fi in top_result.downloaded + top_result.skipped:
        if fi.version:
            versions = top_versions.setdefault(fi.package_name, [])
            if fi.version not in versions:
                versions.append(fi.version)

    if not no_deps:
        logger.info("")
        logger.info("=" * 50)
        logger.info("第 2/2 步:解析并下载依赖")
        logger.info("=" * 50)

        dep_versions = resolve_dependencies(
            top_packages=top_packages,
            top_versions=top_versions,
            pypi_url=config.pypi_url,
            workers=config.workers,
            max_depth=5,
            max_versions=config.max_versions,
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
                backfill_scan_limit=config.backfill_scan_limit,
            )
            all_downloaded.extend(dep_result.downloaded)
            all_warnings.extend(dep_result.warnings)

    return all_downloaded, all_warnings


def _sync_python(config: Config) -> tuple[Path, list[Path]]:
    """同步 Python 解释器,返回 (index.json 路径, 本次新下载的 .tar.gz 列表)."""
    return sync_python_builds(
        repository_dir=config.repository_dir,
        workers=config.workers,
    )


# ============================================================================
#                                      命令实现
# ============================================================================


def _cmd_sync(args: argparse.Namespace) -> int:
    """增量同步:wheel/sdist + Python 解释器,产出单一 incremental_*.tar.gz."""
    config = Config.load(Path(args.config) if args.config else None)
    packages = args.packages if args.packages else config.packages

    if not packages:
        logger.error("未指定要同步的包")
        logger.info("请在配置文件中设置 packages,或通过 --packages 参数指定")
        return 1

    wheel_files: list = []
    wheel_warnings: list[str] = []
    wheel_failed = False

    try:
        wheel_files, wheel_warnings = _sync_wheels(config, packages, args.no_deps)
    except Exception as e:
        wheel_failed = True
        logger.exception(f"wheel 同步失败:{e}")

    new_python_files: list[Path] = []
    python_index_path: Path | None = None
    python_failed = False

    try:
        python_index_path, new_python_files = _sync_python(config)
    except Exception as e:
        python_failed = True
        logger.exception(f"Python 解释器同步失败:{e}")

    generate_index(config.repository_dir)

    archive = create_incremental_package(
        simple_files=wheel_files,
        python_builds_files=new_python_files,
        python_builds_index=python_index_path if new_python_files else None,
        repository_dir=config.repository_dir,
        output_dir=config.incremental_dir,
    )
    if archive is None:
        logger.info("no changes:本次没有新文件下载,未产生增量包")

    logger.info("")
    logger.info("=" * 50)
    logger.info("增量同步完成")
    logger.info("=" * 50)

    if wheel_warnings:
        logger.warning(f"wheel 警告 ({len(wheel_warnings)} 条):")
        for w in wheel_warnings:
            logger.warning(f"  ! {w}")

    return 1 if (wheel_failed or python_failed) else 0


def _cmd_sync_full(args: argparse.Namespace) -> int:
    """全量同步:清空仓库 → 重拉 wheel + Python → 打包 mirror.tar.gz + sha256."""
    config = Config.load(Path(args.config) if args.config else None)
    packages = args.packages if args.packages else config.packages

    if not packages:
        logger.error("未指定要同步的包")
        return 1

    repo = config.repository_dir
    simple_dir = repo / "simple"
    python_dir = repo / "python-builds"
    store_db = repo / ".store.db"

    logger.info("=" * 50)
    logger.info("全量同步:清空仓库")
    logger.info("=" * 50)
    if simple_dir.exists():
        shutil.rmtree(simple_dir)
    if python_dir.exists():
        shutil.rmtree(python_dir)
    if store_db.exists():
        store_db.unlink()
    repo.mkdir(parents=True, exist_ok=True)

    wheel_failed = False
    python_failed = False

    try:
        _sync_wheels(config, packages, args.no_deps)
    except Exception as e:
        wheel_failed = True
        logger.exception(f"wheel 同步失败:{e}")

    try:
        _sync_python(config)
    except Exception as e:
        python_failed = True
        logger.exception(f"Python 解释器同步失败:{e}")

    generate_index(repo)

    if wheel_failed or python_failed:
        logger.error("同步阶段存在失败,跳过 mirror.tar.gz 打包")
        return 1

    archive = _pack_full_mirror(repo)
    sha_path = _write_sha256(archive)
    logger.info("")
    logger.info("=" * 50)
    logger.info("全量同步完成")
    logger.info("=" * 50)
    logger.info(f"mirror.tar.gz : {archive} ({archive.stat().st_size / 1024 / 1024:.2f} MB)")
    logger.info(f"mirror.sha256 : {sha_path}")

    return 0


def _pack_full_mirror(repo: Path) -> Path:
    """把 packages/ 整个目录打包到项目根的 mirror.tar.gz(排除 .access_log.db).

    压缩等级由 env `PIP_MIRROR_TAR_COMPRESSION` 控制:
      - 默认或任何其它值:compresslevel=9(本机/内网部署,体积优先)
      - `none`:compresslevel=0,只保留 gzip framing,基本不压缩
        (CI 上 CPU 弱网络强时用,文件名仍是 .tar.gz,外部接口零变化)
    """
    project_root = Path.cwd()
    archive_path = project_root / "mirror.tar.gz"
    if archive_path.exists():
        archive_path.unlink()

    excluded_names = {".access_log.db"}

    def _filter(tarinfo: tarfile.TarInfo) -> tarfile.TarInfo | None:
        leaf = Path(tarinfo.name).name
        if leaf in excluded_names:
            return None
        return tarinfo

    if os.environ.get("PIP_MIRROR_TAR_COMPRESSION", "").lower() == "none":
        compresslevel = 0
    else:
        compresslevel = 9

    logger.info(f"打包 {repo} → {archive_path} (gzip level={compresslevel})")
    with tarfile.open(archive_path, "w:gz", compresslevel=compresslevel) as tar:
        tar.add(repo, arcname=repo.name, filter=_filter)
    return archive_path


def _write_sha256(archive: Path) -> Path:
    """流式算 sha256,写成 sha256sum 兼容格式."""
    hasher = hashlib.sha256()
    with open(archive, "rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            hasher.update(chunk)
    digest = hasher.hexdigest()
    sha_path = archive.parent / "mirror.sha256"
    sha_path.write_text(f"{digest}  {archive.name}\n", encoding="utf-8")
    return sha_path


def _cmd_serve(args: argparse.Namespace) -> int:
    """启动 HTTP 服务."""
    config = Config.load(Path(args.config) if args.config else None)
    host = args.host or config.server_host
    port = args.port or config.server_port

    generate_index(config.repository_dir)
    start_server(host=host, port=port, repository_dir=config.repository_dir)
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


# ============================================================================
#                                  import-incremental
# ============================================================================


class _ImportError(Exception):
    """strict 模式下用于触发整体 fail-fast 的内部异常."""


def _safe_extract_path(repo: Path, member_name: str) -> Path:
    """解析 archive 内的相对路径并校验落在 repo 内,防 path traversal."""
    target = (repo / member_name).resolve()
    repo_resolved = repo.resolve()
    try:
        target.relative_to(repo_resolved)
    except ValueError as e:
        raise _ImportError(f"非法路径(traversal): {member_name}") from e
    return target


def _hash_file(path: Path) -> str:
    """流式算 sha256."""
    hasher = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def _parse_simple_member(name: str) -> tuple[str, str] | None:
    """从 'simple/<pkg>/<filename>' 解析 (pkg, filename),非 simple 路径返回 None."""
    parts = name.split("/")
    if len(parts) != 3 or parts[0] != "simple":
        return None
    return parts[1], parts[2]


def _cmd_import_incremental(args: argparse.Namespace) -> int:
    """合并增量包到本地仓库:解包 → 现算 sha256 写库 → 重建索引.

    默认宽松:单文件失败 → WARNING 跳过其它继续。
    --strict :任一失败 → 整体 fail-fast,exit 1 且不重建索引。
    """
    config = Config.load(Path(args.config) if args.config else None)
    archive = Path(args.archive)
    repo = config.repository_dir.resolve()

    if not archive.exists():
        logger.error(f"增量包不存在: {archive}")
        return 1

    repo.mkdir(parents=True, exist_ok=True)
    logger.info(f"解包 {archive} → {repo}")

    with tarfile.open(archive, "r:*") as tar:
        members = tar.getmembers()
        try:
            for m in members:
                _safe_extract_path(repo, m.name)
        except _ImportError as e:
            logger.error(str(e))
            return 1

        try:
            tar.extractall(repo, filter="data")  # Python 3.12+
        except TypeError:
            tar.extractall(repo)  # Python <3.12

    manifest_path = repo / "manifest.json"
    if manifest_path.exists():
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            stats = manifest.get("stats", {})
            logger.info(
                f"manifest: created_at={manifest.get('created_at')} stats={stats}"
            )
        except Exception as e:
            logger.warning(f"manifest.json 读取失败: {e}")
        manifest_path.unlink()

    store = DownloadStore(repo / ".store.db")
    file_count = 0
    metadata_count = 0
    failed_files: list[str] = []

    simple_entries = [
        (parsed, m)
        for m in members
        if (parsed := _parse_simple_member(m.name)) is not None
        and not parsed[1].endswith(".metadata")
    ]

    for (pkg, filename), _ in simple_entries:
        target_path = repo / "simple" / pkg / filename
        try:
            if not target_path.exists():
                raise _ImportError(f"解包后文件不存在: {target_path}")
            sha256 = _hash_file(target_path)
            version = _extract_version_from_filename(filename, pkg) or ""
            store.add_file(
                filename=filename,
                package_name=pkg,
                version=version,
                sha256=sha256,
                size=target_path.stat().st_size,
            )
            file_count += 1

            meta_path = target_path.with_suffix(target_path.suffix + ".metadata")
            if filename.endswith(".whl") and meta_path.exists():
                meta_sha = _hash_file(meta_path)
                store.set_metadata_sha256(filename, meta_sha)
                metadata_count += 1
        except Exception as e:
            failed_files.append(filename)
            if args.strict:
                logger.error(f"strict 模式失败:{filename}: {e}")
                return 1
            logger.warning(f"跳过 {filename}: {e}")

    logger.info(f"已写入 store: 文件 {file_count} 条, metadata {metadata_count} 条")
    if failed_files:
        logger.warning(f"跳过 {len(failed_files)} 个文件: {failed_files[:5]}...")

    if args.no_reindex:
        logger.info("--no-reindex 指定,跳过索引重建")
    else:
        generate_index(repo)

    return 0


# ============================================================================
#                                       main
# ============================================================================


def main() -> int:
    """CLI 入口."""
    parser = argparse.ArgumentParser(
        prog="pip-mirror",
        description="轻量级私有 PIP 仓库,支持增量与全量两种同步模式",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
四种模式:
  增量更新   pip-mirror sync                    → incremental/incremental_*.tar.gz
  全量更新   pip-mirror sync-full               → mirror.tar.gz + mirror.sha256
  服务启动   pip-mirror serve                   → HTTP server
  内网导入   pip-mirror import-incremental ...  → 合并增量包到本地仓库
        """,
    )
    parser.add_argument(
        "-v", "--verbose", action="store_true", help="显示 DEBUG 级别日志",
    )

    subparsers = parser.add_subparsers(dest="command", help="可用命令")

    sync_parser = subparsers.add_parser(
        "sync",
        help="增量同步:wheel + Python 解释器,产 incremental_*.tar.gz",
    )
    sync_parser.add_argument("-c", "--config", help="配置文件路径(TOML)")
    sync_parser.add_argument("-p", "--packages", nargs="+", help="要同步的包名")
    sync_parser.add_argument("--no-deps", action="store_true", help="不下载依赖")

    full_parser = subparsers.add_parser(
        "sync-full",
        help="全量同步:清空仓库 → 重拉 → 产 mirror.tar.gz + mirror.sha256",
    )
    full_parser.add_argument("-c", "--config", help="配置文件路径(TOML)")
    full_parser.add_argument("-p", "--packages", nargs="+", help="要同步的包名")
    full_parser.add_argument("--no-deps", action="store_true", help="不下载依赖")

    serve_parser = subparsers.add_parser("serve", help="启动 HTTP 服务")
    serve_parser.add_argument("-c", "--config", help="配置文件路径")
    serve_parser.add_argument("--host", help="监听地址")
    serve_parser.add_argument("--port", type=int, help="监听端口")

    access_log_parser = subparsers.add_parser("access-log", help="查看访问日志统计")
    access_log_parser.add_argument("-c", "--config", help="配置文件路径")
    access_log_parser.add_argument(
        "-n", "--limit", type=int, default=20, help="显示最近 N 条记录",
    )

    init_parser = subparsers.add_parser("init", help="生成示例配置文件")
    init_parser.add_argument("-o", "--output", default="pip-mirror.toml", help="输出文件名")
    init_parser.add_argument("-f", "--force", action="store_true", help="覆盖已存在的文件")

    import_parser = subparsers.add_parser(
        "import-incremental",
        help="合并增量包到本地仓库(解包 + 写 .store.db + 重建索引)",
    )
    import_parser.add_argument("archive", help="incremental tar.gz 路径")
    import_parser.add_argument("-c", "--config", help="配置文件路径")
    import_parser.add_argument(
        "--no-reindex", action="store_true", help="跳过自动重建 PEP 503/691 索引",
    )
    import_parser.add_argument(
        "--strict", action="store_true",
        help="任一文件 sha256 计算/写库失败则整体 fail-fast,不重建索引",
    )

    args = parser.parse_args()

    level = logging.DEBUG if args.verbose else logging.INFO
    setup_logging(level)

    if not args.command:
        parser.print_help()
        return 1

    commands = {
        "sync": _cmd_sync,
        "sync-full": _cmd_sync_full,
        "serve": _cmd_serve,
        "access-log": _cmd_access_log,
        "init": _cmd_init,
        "import-incremental": _cmd_import_incremental,
    }

    return commands[args.command](args)


if __name__ == "__main__":
    sys.exit(main())
