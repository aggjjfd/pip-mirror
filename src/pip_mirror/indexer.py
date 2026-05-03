"""PEP 503 Simple Repository 索引生成."""

from __future__ import annotations

import logging
from pathlib import Path

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


def _generate_package_html(package_name: str, files: list[Path]) -> str:
    """生成单个包的 index.html（文件列表）."""
    lines = []
    for f in sorted(files):
        lines.append(f'    <a href="{f.name}">{f.name}</a><br/>')
    return _PACKAGE_HTML_TEMPLATE.format(
        package_name=package_name,
        links="\n".join(lines),
    )


def generate_index(repository_dir: Path) -> None:
    """生成 PEP 503 规范的 simple index."""
    simple_dir = repository_dir / "simple"
    if not simple_dir.exists():
        logger.info("仓库目录为空，跳过索引生成")
        return

    logger.info("生成 PEP 503 索引...")

    package_names: list[str] = []

    for pkg_dir in sorted(simple_dir.iterdir()):
        if not pkg_dir.is_dir():
            continue

        pkg_name = pkg_dir.name
        package_names.append(pkg_name)

        files = [
            f for f in pkg_dir.iterdir()
            if f.is_file() and not f.name.startswith(".") and not f.name.endswith(".tmp")
        ]

        html = _generate_package_html(pkg_name, files)
        (pkg_dir / "index.html").write_text(html, encoding="utf-8")

    root_html = _generate_index_html(package_names)
    (simple_dir / "index.html").write_text(root_html, encoding="utf-8")

    logger.info(f"索引生成完成: {len(package_names)} 个包")
