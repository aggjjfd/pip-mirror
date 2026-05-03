"""下载 Python 解释器（python-build-standalone）并生成 uv 兼容的 index.json."""

from __future__ import annotations

import hashlib
import json
import logging
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from urllib.parse import unquote

import requests
from tqdm import tqdm

logger = logging.getLogger("pip-mirror")

_UV_METADATA_URL = (
    "https://raw.githubusercontent.com/astral-sh/uv/main/crates/uv-python/download-metadata.json"
)
_TARGET_MINORS = {8, 9, 10, 11, 12, 13, 14}


def _fetch_uv_metadata(session: requests.Session) -> dict:
    """下载 uv 内置的 Python 元数据 JSON."""
    logger.info(f"获取 uv metadata: {_UV_METADATA_URL}")
    resp = session.get(_UV_METADATA_URL, timeout=60)
    resp.raise_for_status()
    return resp.json()


def _is_target_entry(key: str, entry: dict) -> bool:
    """判断是否为目标平台条目."""
    if entry.get("prerelease"):
        return False
    if entry.get("major") != 3:
        return False
    if entry.get("minor") not in _TARGET_MINORS:
        return False
    if "+debug" in key:
        return False

    url = entry.get("url", "")
    if "install_only_stripped" not in url:
        return False

    os_name = entry.get("os", "")
    arch_family = entry.get("arch", {}).get("family", "")
    libc = entry.get("libc", "")

    if os_name == "windows" and arch_family in ("x86_64", "i686") and libc == "none":
        return True
    if os_name == "linux" and arch_family == "x86_64" and libc == "gnu":
        return True
    return False


def _group_by_platform(entries: list[tuple[str, dict]]) -> dict:
    """按 (minor, os, arch_family, arch_variant, libc) 分组，每组只保留最新 build."""
    groups: dict[tuple, list[tuple[str, dict]]] = {}
    for key, entry in entries:
        arch = entry.get("arch", {})
        variant = arch.get("variant")
        variant_str = variant if variant else "base"
        group_key = (
            entry.get("minor"),
            entry.get("os"),
            arch.get("family"),
            variant_str,
            entry.get("libc"),
        )
        groups.setdefault(group_key, []).append((key, entry))

    result = {}
    for group_key, items in groups.items():
        items.sort(key=lambda x: x[1].get("build", ""), reverse=True)
        best_key, best_entry = items[0]
        result[best_key] = best_entry
    return result


def _filename_from_url(url: str) -> str:
    """从 URL 中提取文件名并解码 URL 编码."""
    return unquote(url.split("/")[-1])


def _download_one(
    session: requests.Session,
    url: str,
    dest: Path,
    expected_sha256: str | None,
) -> bool:
    """下载单个文件，存在且 sha256 匹配则跳过.

    Returns:
        True 如果文件已存在且匹配（跳过），或下载成功
        False 如果下载/校验失败
    """
    if dest.exists() and expected_sha256:
        actual = hashlib.sha256(dest.read_bytes()).hexdigest()
        if actual.lower() == expected_sha256.lower():
            return True

    try:
        resp = session.get(url, timeout=300, stream=True)
        resp.raise_for_status()

        tmp = dest.with_suffix(".tmp")
        hasher = hashlib.sha256()
        with open(tmp, "wb") as f:
            for chunk in resp.iter_content(chunk_size=65536):
                if chunk:
                    f.write(chunk)
                    hasher.update(chunk)

        if expected_sha256:
            actual = hasher.hexdigest()
            if actual.lower() != expected_sha256.lower():
                logger.error(f"sha256 校验失败: {dest.name}")
                tmp.unlink(missing_ok=True)
                return False

        tmp.rename(dest)
        return True
    except Exception as e:
        logger.error(f"下载失败 {url}: {e}")
        return False


def sync_python_builds(
    repository_dir: Path,
    workers: int = 4,
) -> Path:
    """同步 Python 解释器并生成 index.json.

    Args:
        repository_dir: 仓库根目录
        workers: 并发下载线程数

    Returns:
        生成的 index.json 路径
    """
    output_dir = repository_dir / "python-builds"
    output_dir.mkdir(parents=True, exist_ok=True)

    with requests.Session() as session:
        metadata = _fetch_uv_metadata(session)

        # 过滤目标条目
        target_entries = [
            (key, entry) for key, entry in metadata.items() if _is_target_entry(key, entry)
        ]
        logger.info(f"过滤后目标条目: {len(target_entries)}")

        # 每组只保留最新 build
        latest = _group_by_platform(target_entries)
        logger.info(f"去重后最新 build: {len(latest)}")

        # 准备下载任务
        tasks = []
        for key, entry in latest.items():
            url = entry["url"]
            filename = _filename_from_url(url)
            dest = output_dir / filename
            sha256 = entry.get("sha256")
            tasks.append((key, url, dest, sha256))

        # 并发下载
        total = len(tasks)
        downloaded = 0
        skipped = 0
        failed = 0

        with tqdm(total=total, desc="下载 Python 解释器", unit="file") as pbar:
            with ThreadPoolExecutor(max_workers=workers) as executor:
                futures = {
                    executor.submit(_download_one, session, url, dest, sha): (key, dest)
                    for key, url, dest, sha in tasks
                }

                for future in as_completed(futures):
                    key, dest = futures[future]
                    try:
                        success = future.result()
                        if success:
                            if dest.exists() and dest.stat().st_size > 0:
                                downloaded += 1
                            else:
                                skipped += 1
                        else:
                            failed += 1
                    except Exception as e:
                        logger.error(f"异常 {key}: {e}")
                        failed += 1
                    pbar.update(1)

    logger.info(
        f"Python 解释器同步完成: 下载={downloaded}, 跳过={skipped}, 失败={failed}"
    )

    # 生成 index.json（修改 url 指向本地）
    local_metadata = {}
    for key, entry in latest.items():
        local_entry = dict(entry)
        filename = _filename_from_url(entry["url"])
        local_entry["url"] = f"/python-builds/{filename}"
        local_metadata[key] = local_entry

    index_path = output_dir / "index.json"
    index_path.write_text(
        json.dumps(local_metadata, indent=2, ensure_ascii=False),
        encoding="utf-8",
    )
    logger.info(f"index.json 已生成: {index_path} ({len(local_metadata)} 个条目)")

    return index_path
