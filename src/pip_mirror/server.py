"""Python HTTP 服务器，提供 PEP 503 Simple Repository 服务."""

from __future__ import annotations

import http.server
import socketserver
import sys
from pathlib import Path


class _RequestHandler(http.server.SimpleHTTPRequestHandler):
    """自定义请求处理器，支持目录索引和跨域."""

    def __init__(self, *args, directory: str | None = None, **kwargs):
        self._serve_dir = directory
        super().__init__(*args, directory=directory, **kwargs)

    def end_headers(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        super().end_headers()

    def log_message(self, format: str, *args):
        pass


def start_server(host: str, port: int, repository_dir: Path) -> None:
    """启动 HTTP 服务器.

    Args:
        host: 监听地址
        port: 监听端口
        repository_dir: 仓库根目录（其中包含 simple/ 子目录）
    """
    if not repository_dir.exists():
        print(f"仓库目录不存在: {repository_dir}", file=sys.stderr)
        sys.exit(1)

    simple_dir = repository_dir / "simple"
    if not simple_dir.exists():
        print(f"警告: {simple_dir} 不存在，请先生成索引", file=sys.stderr)

    def handler_factory(*args, **kwargs):
        return _RequestHandler(*args, directory=str(repository_dir), **kwargs)

    with socketserver.ThreadingTCPServer((host, port), handler_factory) as httpd:
        print("PIP 镜像服务器启动")
        print(f"  地址: http://{host}:{port}")
        print(f"  仓库: {repository_dir}")
        print(f"  pip 使用: pip install --index-url http://{host}:{port}/simple package")
        print("  按 Ctrl+C 停止")

        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\n服务器已停止")
