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

logger = logging.getLogger("pip-mirror")

from .downloader import FileInfo


def create_incremental_package(
    downloaded_files: list[FileInfo],
    repository_dir: Path,
    output_dir: Path,
    compress: bool = True,
) -> Path | None:
    """将本次新增下载的文件打包成增量压缩包.

    Args:
        downloaded_files: 本次新下载的文件列表
        repository_dir: 仓库根目录
        output_dir: 增量包输出目录
        compress: 是否使用 gzip 压缩（默认 True；GitHub Actions 建议 False）

    Returns:
        生成的压缩包路径，如果没有新增文件则返回 None
    """
    if not downloaded_files:
        logger.info("没有新增文件，跳过增量打包")
        return None

    output_dir.mkdir(parents=True, exist_ok=True)

    timestamp = datetime.now(timezone.utc).strftime("%Y%m%d_%H%M%S")
    ext = ".tar.gz" if compress else ".tar"
    archive_name = f"incremental_{timestamp}{ext}"
    archive_path = output_dir / archive_name

    manifest = {
        "timestamp": timestamp,
        "iso_time": datetime.now(timezone.utc).isoformat(),
        "file_count": len(downloaded_files),
        "total_size": sum(f.size or 0 for f in downloaded_files),
        "files": [
            {
                "package": f.package_name,
                "version": f.version,
                "filename": f.filename,
                "size": f.size,
                "sha256": f.sha256,
            }
            for f in downloaded_files
        ],
    }

    simple_dir = repository_dir / "simple"
    mode = "w:gz" if compress else "w"

    total = len(downloaded_files)
    logger.info(f"增量打包 {total} 个文件...")

    with tarfile.open(archive_path, mode) as tar:
        for file_info in tqdm(downloaded_files, desc="打包", unit="file", total=total):
            normalized = file_info.package_name.lower().replace("_", "-").replace(".", "-")
            file_path = simple_dir / normalized / file_info.filename

            if not file_path.exists():
                logger.warning(f"文件不存在，跳过: {file_path}")
                continue

            arcname = f"simple/{normalized}/{file_info.filename}"
            tar.add(file_path, arcname=arcname)

        manifest_bytes = json.dumps(manifest, indent=2, ensure_ascii=False).encode("utf-8")
        manifest_info = tarfile.TarInfo(name="manifest.json")
        manifest_info.size = len(manifest_bytes)
        manifest_info.mtime = time.time()
        tar.addfile(manifest_info, io.BytesIO(manifest_bytes))

    mb = manifest["total_size"] / 1024 / 1024
    logger.info(f"增量包已生成: {archive_path}")
    logger.info(f"包含 {manifest['file_count']} 个文件, 共 {mb:.2f} MB")

    return archive_path
