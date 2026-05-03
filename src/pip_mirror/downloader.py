"""从 PyPI / Mirror 下载包文件.

支持两种模式:
1. JSON API (PyPI 官方): https://pypi.org/pypi/{package}/json
2. Simple Index HTML (所有镜像): https://mirror/pypi/simple/{package}/

下载策略:
- 每个包保留最新 N 个版本
- 先下载所有通过平台过滤的 wheel
- sdist 只在以下情况下载:
  a. 该版本没有任何 wheel 覆盖了目标平台（缺少平台覆盖）
  b. 且该版本是纯 Python 包
- 如果缺少平台覆盖且不是纯 Python 包，发 warning
"""

from __future__ import annotations

import hashlib
import logging
import warnings
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from html.parser import HTMLParser
from pathlib import Path
from typing import Any
from urllib.parse import urljoin

import requests
from packaging.version import parse as parse_version
from tqdm import tqdm

from .filters import (
    is_accepted_wheel,
    is_pure_python_wheel,
    is_source_distribution,
    normalize_package_name,
)
from .sqlite_store import DownloadStore

logger = logging.getLogger("pip-mirror")

# 目标平台定义（用于检查是否全覆盖）
_TARGET_PLATFORMS = {"win32", "win_amd64", "linux_x86_64"}


def _platform_to_target(platform_tag: str) -> set[str]:
    """将 wheel platform tag 映射到目标平台.

    Returns:
        该 wheel 覆盖的目标平台集合
    """
    if platform_tag == "any":
        return set(_TARGET_PLATFORMS)

    # 复合 tag: manylinux_2_28_x86_64.manylinux2014_x86_64
    sub_tags = platform_tag.split(".")
    covered: set[str] = set()

    for sub in sub_tags:
        if sub in ("win32",):
            covered.add("win32")
        elif sub in ("win_amd64",):
            covered.add("win_amd64")
        elif "i686" in sub or "_i686" in sub:
            covered.add("linux_i686")
        elif "x86_64" in sub or "_x86_64" in sub:
            covered.add("linux_x86_64")

    return covered


@dataclass
class FileInfo:
    """包文件信息."""

    filename: str
    url: str
    sha256: str | None = None
    size: int | None = None
    package_name: str = ""
    version: str = ""


@dataclass
class DownloadResult:
    """单次下载运行结果."""

    downloaded: list[FileInfo] = field(default_factory=list)
    skipped: list[FileInfo] = field(default_factory=list)
    failed: list[tuple[FileInfo, str]] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)


class _SimpleIndexParser(HTMLParser):
    """解析 PEP 503 Simple Index HTML 页面，提取文件链接."""

    def __init__(self):
        super().__init__()
        self.links: list[tuple[str, str, str | None]] = []
        self._current_href: str | None = None
        self._current_data: str = ""

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag == "a":
            attr_dict = {k: v for k, v in attrs}
            href = attr_dict.get("href", "")
            self._current_href = href
            self._current_data = ""

    def handle_data(self, data: str) -> None:
        if self._current_href is not None:
            self._current_data += data

    def handle_endtag(self, tag: str) -> None:
        if tag == "a" and self._current_href:
            filename = self._current_data.strip()
            href = self._current_href
            sha256: str | None = None
            if "#sha256=" in href:
                sha256 = href.split("#sha256=")[1]
            if filename and href:
                self.links.append((filename, href, sha256))
            self._current_href = None
            self._current_data = ""


def _extract_version_from_filename(filename: str, package_name: str) -> str | None:
    """从文件名中提取版本号."""
    # wheel 格式
    if filename.endswith(".whl"):
        parts = filename[:-4].split("-")
        if len(parts) >= 5:
            return parts[1]
        return None

    # sdist 格式
    if is_source_distribution(filename):
        base = filename
        for ext in (".tar.gz", ".zip", ".tar.bz2", ".tar.xz"):
            if base.endswith(ext):
                base = base[: -len(ext)]
                break
        pkg_prefixes = [
            package_name.lower(),
            package_name.lower().replace("-", "_"),
            package_name.lower().replace("_", "-"),
        ]
        for prefix in pkg_prefixes:
            if base.lower().startswith(prefix + "-"):
                return base[len(prefix) + 1 :]

    return None


def _fetch_simple_index(
    session: requests.Session, package_name: str, index_url: str,
) -> list[FileInfo]:
    """从 Simple Index HTML 页面获取文件列表.

    如果第一次请求失败，尝试用 _/- 互换的包名重试.
    """
    normalized = normalize_package_name(package_name)
    urls_to_try = [f"{index_url.rstrip('/')}/{normalized}/"]

    # 备选 URL: _ 和 - 互换
    alt = normalized.replace("-", "_") if "-" in normalized else normalized.replace("_", "-")
    if alt != normalized:
        urls_to_try.append(f"{index_url.rstrip('/')}/{alt}/")

    print(f"  [DEBUG] Simple Index URLs: {urls_to_try}")

    last_error: Exception | None = None
    for url in urls_to_try:
        try:
            response = session.get(url, timeout=30)
            response.raise_for_status()

            parser = _SimpleIndexParser()
            parser.feed(response.text)

            result: list[FileInfo] = []
            for filename, href, sha256 in parser.links:
                version = _extract_version_from_filename(filename, package_name)
                full_url = urljoin(url, href)
                result.append(FileInfo(
                    filename=filename, url=full_url, sha256=sha256,
                    package_name=package_name, version=version or "",
                ))

            if result:
                return result
        except Exception as e:
            last_error = e

    if last_error:
        raise last_error
    return []


def _fetch_json_api(
    session: requests.Session, package_name: str, pypi_url: str,
) -> list[FileInfo]:
    """从 PyPI JSON API 获取文件列表."""
    normalized = normalize_package_name(package_name)
    url = f"{pypi_url.rstrip('/')}/pypi/{normalized}/json"
    print(f"  [DEBUG] JSON API URL: {url}")

    response = session.get(url, timeout=30)
    response.raise_for_status()
    data = response.json()

    result: list[FileInfo] = []
    releases = data.get("releases", {})

    for version, files in releases.items():
        for file_data in files:
            filename = file_data.get("filename", "")
            file_url = file_data.get("url", "")
            digests = file_data.get("digests", {})
            sha256 = digests.get("sha256")
            if not sha256 and "#sha256=" in file_url:
                sha256 = file_url.split("#sha256=")[1]
            size = file_data.get("size")

            result.append(FileInfo(
                filename=filename, url=file_url, sha256=sha256,
                size=size, package_name=package_name, version=version,
            ))

    return result


def _is_official_pypi(pypi_url: str) -> bool:
    return "pypi.org" in pypi_url.lower()


def _select_latest_versions(files: list[FileInfo], max_versions: int) -> list[FileInfo]:
    """只保留最新版本的文件."""
    if max_versions <= 0:
        return files

    version_files: dict[str, list[FileInfo]] = {}
    for fi in files:
        version_files.setdefault(fi.version, []).append(fi)

    try:
        sorted_versions = sorted(
            version_files.keys(), key=lambda v: parse_version(v), reverse=True,
        )
    except Exception:
        sorted_versions = sorted(version_files.keys(), reverse=True)

    selected = set(sorted_versions[:max_versions])
    return [fi for fi in files if fi.version in selected]


def _collect_version_files(files: list[FileInfo]) -> dict[str, list[FileInfo]]:
    """按版本分组."""
    result: dict[str, list[FileInfo]] = {}
    for fi in files:
        result.setdefault(fi.version, []).append(fi)
    return result


def download_packages(
    packages: list[str],
    repository_dir: Path,
    pypi_url: str,
    index_url: str,
    include_source: bool,
    workers: int,
    max_versions: int = 3,
    specific_versions: dict[str, list[str]] | None = None,
) -> DownloadResult:
    """下载指定包的文件.

    Args:
        packages: 要下载的包名列表
        repository_dir: 仓库根目录
        pypi_url: PyPI 根 URL
        index_url: Simple Index URL
        include_source: 是否包含源码包（仅在缺少平台覆盖时下载纯 Python sdist）
        workers: 并发下载线程数
        max_versions: 每个包最多保留的最新版本数
        specific_versions: {包名: [版本号列表]} 指定要下载的确切版本，
                          如果提供则忽略 max_versions

    Returns:
        下载结果
    """
    result = DownloadResult()
    store = DownloadStore(repository_dir / ".store.db")
    existing_hashes = store.get_all_hashes()

    files_to_download: list[tuple[FileInfo, Path]] = []

    print(f"开始收集 {len(packages)} 个包的文件信息...")
    print(f"  源: {index_url}")
    if specific_versions:
        print(f"  使用指定版本列表")
    else:
        print(f"  每个包保留最新 {max_versions} 个版本")

    use_json_api = _is_official_pypi(pypi_url)

    with requests.Session() as session:
        for package_name in packages:
            normalized = normalize_package_name(package_name)
            pkg_dir = repository_dir / "simple" / normalized

            files: list[FileInfo] = []

            if use_json_api:
                try:
                    files = _fetch_json_api(session, package_name, pypi_url)
                    print(f"  [OK] {package_name} (JSON API, {len(files)} files)")
                except (requests.HTTPError, requests.RequestException) as e1:
                    try:
                        files = _fetch_simple_index(session, package_name, index_url)
                        print(f"  [OK] {package_name} (Simple Index fallback, {len(files)} files)")
                    except (requests.HTTPError, requests.RequestException) as e2:
                        status = getattr(getattr(e2, "response", None), "status_code", None)
                        if status == 404:
                            print(f"  [ERR] 包不存在: {package_name}")
                        else:
                            print(f"  [ERR] 获取失败 {package_name}: {e2}")
                        continue
            else:
                try:
                    files = _fetch_simple_index(session, package_name, index_url)
                    print(f"  [OK] {package_name} (Simple Index, {len(files)} files)")
                except (requests.HTTPError, requests.RequestException) as e:
                    status = getattr(getattr(e, "response", None), "status_code", None)
                    if status == 404:
                        print(f"  [ERR] 包不存在: {package_name}")
                    else:
                        print(f"  [ERR] 获取失败 {package_name}: {e}")
                    continue

            # 版本过滤
            if specific_versions and package_name in specific_versions:
                allowed = set(specific_versions[package_name])
                files = [fi for fi in files if fi.version in allowed]
                print(f"  [DEBUG] {package_name} 版本过滤后: {len(files)} files, versions={allowed}")
            else:
                files = _select_latest_versions(files, max_versions)
                print(f"  [DEBUG] {package_name} 最新版本过滤后: {len(files)} files")
            version_files = _collect_version_files(files)

            if not version_files:
                print(f"  [DEBUG] {package_name} 无匹配版本文件")
                continue

            for version, vfiles in version_files.items():
                # 判断该版本是否为纯 Python
                has_pure_python = any(is_pure_python_wheel(f.filename) for f in vfiles)

                # 收集该版本已接受的 wheel 覆盖的平台
                covered_platforms: set[str] = set()
                accepted_wheels: list[FileInfo] = []
                sdists: list[FileInfo] = []

                for fi in vfiles:
                    if fi.filename.endswith(".whl"):
                        if is_accepted_wheel(fi.filename):
                            accepted_wheels.append(fi)
                            plat = fi.filename[:-4].split("-")[-1]
                            covered_platforms.update(_platform_to_target(plat))
                    elif is_source_distribution(fi.filename):
                        sdists.append(fi)

                # 先处理 wheel
                for fi in accepted_wheels:
                    existing_sha256 = existing_hashes.get(fi.filename)
                    if existing_sha256 and fi.sha256:
                        if existing_sha256.lower() == fi.sha256.lower():
                            result.skipped.append(fi)
                            continue
                    files_to_download.append((fi, pkg_dir / fi.filename))

                # 再处理 sdist
                if include_source and sdists:
                    if not accepted_wheels:
                        # 该版本没有任何符合平台要求的 wheel，下载 sdist 作为 fallback
                        for fi in sdists:
                            existing_sha256 = existing_hashes.get(fi.filename)
                            if existing_sha256 and fi.sha256:
                                if existing_sha256.lower() == fi.sha256.lower():
                                    result.skipped.append(fi)
                                    continue
                            files_to_download.append((fi, pkg_dir / fi.filename))
                    else:
                        missing = _TARGET_PLATFORMS - covered_platforms
                        if missing:
                            if has_pure_python:
                                # 缺少平台覆盖，但纯 Python wheel 已经覆盖所有平台
                                # 不需要 sdist
                                pass
                            else:
                                # 缺少平台覆盖，但不是纯 Python 包，发 warning
                                missing_list = ", ".join(sorted(missing))
                                msg = (
                                    f"{package_name}=={version}: 缺少平台覆盖 ({missing_list})，"
                                    f"且不是纯 Python 包，无法提供 sdist fallback"
                                )
                                result.warnings.append(msg)
                                print(f"警告: {msg}")

    if not files_to_download:
        print("所有文件已是最新，无需下载")
        return result

    print(f"需要下载 {len(files_to_download)} 个文件")

    # 并发下载，带 tqdm 进度条
    with tqdm(total=len(files_to_download), desc="下载", unit="file") as pbar:
        with ThreadPoolExecutor(max_workers=workers) as executor:
            futures = {
                executor.submit(_download_file, session, fi, dest): fi
                for fi, dest in files_to_download
            }

            for future in as_completed(futures):
                file_info = futures[future]
                try:
                    success, error = future.result()
                    if success:
                        result.downloaded.append(file_info)
                        if file_info.sha256:
                            store.add_file(
                                filename=file_info.filename,
                                package_name=file_info.package_name,
                                version=file_info.version,
                                sha256=file_info.sha256,
                                size=file_info.size,
                            )
                    else:
                        result.failed.append((file_info, error))
                except Exception as e:
                    result.failed.append((file_info, str(e)))
                pbar.update(1)

    print(f"  [DEBUG] 下载汇总: downloaded={len(result.downloaded)}, skipped={len(result.skipped)}, failed={len(result.failed)}")
    pkg_counts = {}
    for fi in result.downloaded:
        pkg_counts[fi.package_name] = pkg_counts.get(fi.package_name, 0) + 1
    if pkg_counts:
        print(f"  [DEBUG] 各包下载数量: {dict(sorted(pkg_counts.items()))}")

    return result


def _download_file(
    session: requests.Session, file_info: FileInfo, dest_path: Path,
) -> tuple[bool, str]:
    """下载单个文件."""
    try:
        dest_path.parent.mkdir(parents=True, exist_ok=True)
        url = file_info.url.split("#")[0]
        response = session.get(url, timeout=120, stream=True)
        response.raise_for_status()

        tmp_path = dest_path.with_suffix(".tmp")
        with open(tmp_path, "wb") as f:
            for chunk in response.iter_content(chunk_size=65536):
                if chunk:
                    f.write(chunk)

        if file_info.sha256:
            actual_hash = hashlib.sha256(tmp_path.read_bytes()).hexdigest()
            if actual_hash.lower() != file_info.sha256.lower():
                tmp_path.unlink(missing_ok=True)
                return False, "hash 校验失败"

        tmp_path.rename(dest_path)
        return True, ""

    except requests.RequestException as e:
        return False, f"网络错误: {e}"
    except OSError as e:
        return False, f"文件错误: {e}"
