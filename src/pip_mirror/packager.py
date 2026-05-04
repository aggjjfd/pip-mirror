"""增量打包功能."""

from __future__ import annotations

import io
import json
import logging
import tarfile
import time
from datetime import datetime, timezone
from pathlib import Path

from tqdm import tqdm

from .downloader import FileInfo
from .filters import normalize_package_name

logger = logging.getLogger("pip-mirror")


def create_incremental_package(
    simple_files: list[FileInfo],
    python_builds_files: list[Path],
    python_builds_index: Path | None,
    repository_dir: Path,
    output_dir: Path,
) -> Path | None:
    """将本次新增的下载文件打包成增量 tar.gz.

    内容布局(均相对于 archive 根):
        simple/<pkg>/<filename>          : wheel / sdist 本体
        simple/<pkg>/<filename>.metadata : 对应 PEP 658 metadata(若存在)
        python-builds/<filename>         : 新下载的 Python 解释器 .tar.gz
        python-builds/index.json         : 仅当本次有新解释器时附带
        manifest.json                    : 摘要(created_at + stats)

    联合 early-return:simple_files 与 python_builds_files 同时为空 → 返回 None,
    不产 archive。

    Args:
        simple_files: 本次新下载的 wheel/sdist 列表
        python_builds_files: 本次新下载的 Python 解释器文件路径列表
        python_builds_index: 仅当 python_builds_files 非空时传入,会被一并打包
        repository_dir: 仓库根目录(用于解析源文件位置)
        output_dir: 增量包输出目录

    Returns:
        生成的 tar.gz 路径;无新文件时返回 None
    """
    if not simple_files and not python_builds_files:
        logger.info("没有新增文件,跳过增量打包")
        return None

    output_dir.mkdir(parents=True, exist_ok=True)

    timestamp = datetime.now(timezone.utc).strftime("%Y%m%d_%H%M%S")
    archive_path = output_dir / f"incremental_{timestamp}.tar.gz"

    manifest = {
        "created_at": datetime.now(timezone.utc).isoformat(),
        "stats": {
            "simple": len(simple_files),
            "python_builds": len(python_builds_files),
        },
    }

    simple_dir = repository_dir / "simple"
    total = len(simple_files) + len(python_builds_files)
    logger.info(f"增量打包 {total} 个文件...")

    with tarfile.open(archive_path, "w:gz") as tar:
        for file_info in tqdm(simple_files, desc="打包 simple", unit="file"):
            normalized = normalize_package_name(file_info.package_name)
            file_path = simple_dir / normalized / file_info.filename

            if not file_path.exists():
                logger.warning(f"文件不存在,跳过: {file_path}")
                continue

            arcname = f"simple/{normalized}/{file_info.filename}"
            tar.add(file_path, arcname=arcname)

            if file_info.filename.endswith(".whl"):
                meta_path = file_path.with_suffix(".whl.metadata")
                if meta_path.exists():
                    tar.add(meta_path, arcname=arcname + ".metadata")

        for build_path in tqdm(python_builds_files, desc="打包 python-builds", unit="file"):
            if not build_path.exists():
                logger.warning(f"文件不存在,跳过: {build_path}")
                continue
            arcname = f"python-builds/{build_path.name}"
            tar.add(build_path, arcname=arcname)

        if python_builds_files and python_builds_index and python_builds_index.exists():
            tar.add(python_builds_index, arcname="python-builds/index.json")

        manifest_bytes = json.dumps(manifest, indent=2, ensure_ascii=False).encode("utf-8")
        manifest_info = tarfile.TarInfo(name="manifest.json")
        manifest_info.size = len(manifest_bytes)
        manifest_info.mtime = int(time.time())
        tar.addfile(manifest_info, io.BytesIO(manifest_bytes))

    archive_mb = archive_path.stat().st_size / 1024 / 1024
    logger.info(
        f"增量包已生成: {archive_path} "
        f"(simple={manifest['stats']['simple']}, "
        f"python_builds={manifest['stats']['python_builds']}, "
        f"size={archive_mb:.2f} MB)"
    )

    return archive_path
