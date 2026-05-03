"""PEP 503 Simple Repository 索引生成，支持 PEP 658 metadata 和 PEP 691 JSON API."""

from __future__ import annotations

import json
import logging
from pathlib import Path

from .sqlite_store import DownloadStore

logger = logging.getLogger("pip-mirror")


_INDEX_HTML_TEMPLATE = """<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Simple Index</title>
</head>
<body>
{links}
</body>
</html>
"""

_PACKAGE_HTML_TEMPLATE = """<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Links for {package_name}</title>
</head>
<body>
<h1>Links for {package_name}</h1>
{links}
</body>
</html>
"""


def _generate_index_html(package_names: list[str]) -> str:
    """生成根 index.html（包列表）."""
    lines = []
    for name in sorted(package_names):
        lines.append(f'    <a href="{name}/">{name}</a><br/>')
    return _INDEX_HTML_TEMPLATE.format(links="\n".join(lines))


def _generate_index_json(package_names: list[str]) -> str:
    """生成根 index.json（PEP 691）."""
    data = {
        "meta": {"api-version": "1.0"},
        "projects": [{"name": name} for name in sorted(package_names)],
    }
    return json.dumps(data, indent=2, ensure_ascii=False)


def _generate_package_html(
    package_name: str,
    files: list[Path],
    hashes: dict[str, str],
    metadata_hashes: dict[str, str],
) -> str:
    """生成单个包的 index.html，带 PEP 658 data- 属性."""
    lines = []
    for f in sorted(files):
        attrs = f'href="{f.name}"'
        sha256 = hashes.get(f.name)
        if sha256:
            attrs += f' data-sha256="{sha256}"'
        meta_sha256 = metadata_hashes.get(f.name)
        if meta_sha256 and f.name.endswith(".whl"):
            attrs += f' data-core-metadata="sha256={meta_sha256}"'
            attrs += f' data-dist-info-metadata="sha256={meta_sha256}"'
        lines.append(f"    <a {attrs}>{f.name}</a><br/>")
    return _PACKAGE_HTML_TEMPLATE.format(
        package_name=package_name,
        links="\n".join(lines),
    )


def _generate_package_json(
    package_name: str,
    files: list[Path],
    hashes: dict[str, str],
    metadata_hashes: dict[str, str],
) -> str:
    """生成单个包的 index.json（PEP 691）."""
    file_entries = []
    for f in sorted(files):
        entry: dict = {
            "filename": f.name,
            "url": f.name,
            "hashes": {},
        }
        sha256 = hashes.get(f.name)
        if sha256:
            entry["hashes"]["sha256"] = sha256
        meta_sha256 = metadata_hashes.get(f.name)
        if meta_sha256 and f.name.endswith(".whl"):
            entry["dist-info-metadata"] = {"sha256": meta_sha256}
        file_entries.append(entry)

    data = {
        "meta": {"api-version": "1.0"},
        "name": package_name,
        "files": file_entries,
    }
    return json.dumps(data, indent=2, ensure_ascii=False)


def generate_index(repository_dir: Path) -> None:
    """生成 PEP 503 / PEP 691 规范的 simple index."""
    simple_dir = repository_dir / "simple"
    if not simple_dir.exists():
        logger.info("仓库目录为空，跳过索引生成")
        return

    logger.info("生成 PEP 503 / PEP 691 索引...")

    store = DownloadStore(repository_dir / ".store.db")
    hashes = store.get_all_hashes()
    metadata_hashes = store.get_all_metadata_hashes()

    package_names: list[str] = []

    for pkg_dir in sorted(simple_dir.iterdir()):
        if not pkg_dir.is_dir():
            continue

        pkg_name = pkg_dir.name
        package_names.append(pkg_name)

        files = [
            f for f in pkg_dir.iterdir()
            if f.is_file()
            and not f.name.startswith(".")
            and not f.name.endswith(".tmp")
            and not f.name.endswith(".metadata")
        ]

        html = _generate_package_html(pkg_name, files, hashes, metadata_hashes)
        (pkg_dir / "index.html").write_text(html, encoding="utf-8")

        json_content = _generate_package_json(pkg_name, files, hashes, metadata_hashes)
        (pkg_dir / "index.json").write_text(json_content, encoding="utf-8")

    root_html = _generate_index_html(package_names)
    (simple_dir / "index.html").write_text(root_html, encoding="utf-8")

    root_json = _generate_index_json(package_names)
    (simple_dir / "index.json").write_text(root_json, encoding="utf-8")

    logger.info(f"索引生成完成: {len(package_names)} 个包")
