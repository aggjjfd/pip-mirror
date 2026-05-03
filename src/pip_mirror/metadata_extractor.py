"""从 wheel 中提取 Core Metadata (METADATA)."""

from __future__ import annotations

import hashlib
import logging
import zipfile
from pathlib import Path

logger = logging.getLogger("pip-mirror")


def extract_wheel_metadata(wheel_path: Path) -> tuple[bytes, str] | None:
    """从 wheel 中提取 METADATA 文件内容和 sha256.

    Args:
        wheel_path: wheel 文件路径

    Returns:
        (metadata_bytes, sha256_hex) 或 None（提取失败）
    """
    if not wheel_path.exists():
        return None

    try:
        with zipfile.ZipFile(wheel_path, "r") as zf:
            # 精确匹配 <pkg>-<ver>.dist-info/METADATA,避免子目录或子串误判
            metadata_name = None
            for name in zf.namelist():
                parts = name.split("/")
                if (
                    len(parts) == 2
                    and parts[1] == "METADATA"
                    and parts[0].endswith(".dist-info")
                ):
                    metadata_name = name
                    break

            if not metadata_name:
                logger.debug(f"未找到 METADATA: {wheel_path.name}")
                return None

            content = zf.read(metadata_name)
            sha256 = hashlib.sha256(content).hexdigest()
            return content, sha256

    except zipfile.BadZipFile:
        logger.warning(f"损坏的 wheel 文件: {wheel_path}")
        return None
    except Exception as e:
        logger.warning(f"提取 METADATA 失败 {wheel_path.name}: {e}")
        return None
