"""配置加载与管理."""

from __future__ import annotations

import logging
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Any

logger = logging.getLogger("pip-mirror")


@dataclass(frozen=True)
class Config:
    """应用配置."""

    packages: list[str]
    repository_dir: Path
    pypi_url: str
    index_url: str
    include_source: bool
    workers: int
    server_port: int
    server_host: str
    incremental_dir: Path
    max_versions: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> Config:
        """从字典创建配置."""
        repo_dir = Path(data.get("repository_dir", "./packages")).resolve()
        incremental = Path(data.get("incremental_dir", "./incremental")).resolve()

        return cls(
            packages=data.get("packages", []),
            repository_dir=repo_dir,
            pypi_url=data.get("pypi_url", "https://pypi.org"),
            index_url=data.get("index_url", "https://mirrors.ustc.edu.cn/pypi/simple"),
            include_source=data.get("include_source", True),
            workers=int(data.get("workers", 4)),
            server_port=int(data.get("server_port", 8080)),
            server_host=data.get("server_host", "0.0.0.0"),
            incremental_dir=incremental,
            max_versions=int(data.get("max_versions", 5)),
        )

    @classmethod
    def from_toml(cls, path: Path) -> Config:
        """从 TOML 文件加载配置."""
        try:
            import tomllib  # Python 3.11+
        except ImportError:
            import tomli as tomllib  # Python 3.8-3.10

        with open(path, "rb") as f:
            data = tomllib.load(f)

        return cls.from_dict(data)

    @classmethod
    def from_pyproject(cls) -> Config | None:
        """从 pyproject.toml 的 [tool.pip-mirror] 段加载配置."""
        pyproject = Path("pyproject.toml")
        if not pyproject.exists():
            return None

        try:
            try:
                import tomllib
            except ImportError:
                import tomli as tomllib

            with open(pyproject, "rb") as f:
                data = tomllib.load(f)

            tool_config = data.get("tool", {}).get("pip-mirror", {})
            if not tool_config:
                return None

            return cls.from_dict(tool_config)
        except (OSError, ValueError) as e:
            logger.warning(f"读取 pyproject.toml 失败: {e}")
            return None

    @classmethod
    def load(cls, path: Path | None = None) -> Config:
        """加载配置，优先级: 指定文件 > pyproject.toml > 环境变量 > 默认值."""
        if path:
            if not path.exists():
                raise FileNotFoundError(f"配置文件不存在: {path}")
            logger.info(f"加载配置: {path}")
            return cls.from_toml(path)

        config = cls.from_pyproject()
        if config:
            logger.info("从 pyproject.toml [tool.pip-mirror] 加载配置")
            return config

        env_packages = os.environ.get("PIP_MIRROR_PACKAGES", "")
        if env_packages:
            logger.info("从环境变量加载配置")
            return cls.from_dict({
                "packages": [p.strip() for p in env_packages.split(",") if p.strip()],
            })

        logger.warning("未找到配置，使用默认空配置")
        return cls.from_dict({})


def write_example_config(path: Path) -> None:
    """写入示例配置文件."""
    content = """# PIP Mirror 配置文件
# 支持 Python 3.8+，可放在 pyproject.toml [tool.pip-mirror] 段或独立 TOML 文件

# 要同步的包列表
packages = [
    "requests",
    "numpy",
    "pandas",
    "pillow",
    "rich",
    "packaging",
    "charset-normalizer",
]

# 仓库根目录（生成 PEP 503 索引的位置）
repository_dir = "./packages"

# 增量包输出目录
incremental_dir = "./incremental"

# PyPI JSON API 地址（用于获取包元数据）
pypi_url = "https://pypi.org"

# Simple Index 镜像地址（用于下载文件）
# 推荐国内镜像：
#   - 中科大: https://mirrors.ustc.edu.cn/pypi/simple
#   - 清华:   https://pypi.tuna.tsinghua.edu.cn/simple
#   - 阿里云: https://mirrors.aliyun.com/pypi/simple/
index_url = "https://mirrors.ustc.edu.cn/pypi/simple"

# 是否下载源码包（sdist）
# 注意：只下载纯 Python 包的 sdist，带 C 扩展的只下载预编译 wheel
include_source = true

# 并发下载线程数
workers = 4

# 每个包保留的最新版本数
max_versions = 3

# HTTP 服务配置
server_port = 8080
server_host = "0.0.0.0"
"""
    path.write_text(content, encoding="utf-8")
