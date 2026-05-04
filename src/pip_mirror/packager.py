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
from .sqlite_store import DownloadStore

logger = logging.getLogger("pip-mirror")


def create_incremental_package(
    downloaded_files: list[FileInfo],
    repository_dir: Path,
    output_dir: Path,
    compress: bool = True,
) -> Path | None:
    """将本次新增下载的文件打包成增量压缩包.

    内容:
        - simple/<pkg>/<filename>      : wheel / sdist 本体
        - simple/<pkg>/<filename>.metadata : 若是 wheel 且 PEP 658 metadata 存在则一起打入
        - manifest.json                : 文件清单含 sha256 + metadata_sha256

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

    store = DownloadStore(repository_dir / ".store.db")
    metadata_hashes = store.get_all_metadata_hashes()

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
                "metadata_sha256": metadata_hashes.get(f.filename),
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
            normalized = normalize_package_name(file_info.package_name)
            file_path = simple_dir / normalized / file_info.filename

            if not file_path.exists():
                logger.warning(f"文件不存在，跳过: {file_path}")
                continue

            arcname = f"simple/{normalized}/{file_info.filename}"
            tar.add(file_path, arcname=arcname)

            # 一并打包 PEP 658 .whl.metadata(若存在)
            if file_info.filename.endswith(".whl"):
                meta_path = file_path.with_suffix(".whl.metadata")
                if meta_path.exists():
                    tar.add(meta_path, arcname=arcname + ".metadata")

        manifest_bytes = json.dumps(manifest, indent=2, ensure_ascii=False).encode("utf-8")
        manifest_info = tarfile.TarInfo(name="manifest.json")
        manifest_info.size = len(manifest_bytes)
        manifest_info.mtime = time.time()
        tar.addfile(manifest_info, io.BytesIO(manifest_bytes))

    mb = manifest["total_size"] / 1024 / 1024
    logger.info(f"增量包已生成: {archive_path}")
    logger.info(f"包含 {manifest['file_count']} 个文件, 共 {mb:.2f} MB")

    return archive_path
