"""Python HTTP 服务器，提供 PEP 503 Simple Repository 服务."""

from __future__ import annotations

import http.server
import logging
import socketserver
import sys
from datetime import datetime, timezone
from pathlib import Path

from .access_logger import AccessLogger, AccessRecord

logger = logging.getLogger("pip-mirror")


class _RequestHandler(http.server.SimpleHTTPRequestHandler):
    """自定义请求处理器，支持目录索引、跨域和访问日志."""

    def __init__(self, *args, directory: str | None = None, access_logger: AccessLogger | None = None, **kwargs):
        self._serve_dir = directory
        self._access_logger = access_logger
        self._status_code = 200
        self._response_bytes = 0
        super().__init__(*args, directory=directory, **kwargs)

    def send_response(self, code, message=None):
        self._status_code = code
        super().send_response(code, message)

    def end_headers(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        super().end_headers()

    def log_message(self, format: str, *args):
        """覆盖默认日志，使用 AccessLogger 记录到 SQLite."""
        if self._access_logger is None:
            return

        # 提取客户端真实 IP（考虑反向代理）
        client_ip = self.headers.get("X-Forwarded-For", self.client_address[0]).split(",")[0].strip()

        record = AccessRecord(
            timestamp=datetime.now(timezone.utc).isoformat(),
            client_ip=client_ip,
            method=self.command,
            path=self.path,
            status_code=self._status_code,
            user_agent=self.headers.get("User-Agent"),
            bytes_sent=self._response_bytes if self._response_bytes > 0 else None,
            referer=self.headers.get("Referer"),
        )
        self._access_logger.log(record)

        # 同时输出一条 INFO 日志到控制台
        logger.info(f"{client_ip} {self.command} {self.path} {self._status_code}")

    def copyfile(self, source, outputfile):
        """覆盖 copyfile 以统计发送字节数."""
        import shutil
        pos = outputfile.tell() if hasattr(outputfile, "tell") else 0
        shutil.copyfileobj(source, outputfile)
        if hasattr(outputfile, "tell"):
            self._response_bytes = outputfile.tell() - pos

    def do_GET(self):
        """处理 GET 请求并记录."""
        self._status_code = 200
        self._response_bytes = 0
        super().do_GET()


def start_server(host: str, port: int, repository_dir: Path) -> None:
    """启动 HTTP 服务器.

    Args:
        host: 监听地址
        port: 监听端口
        repository_dir: 仓库根目录（其中包含 simple/ 子目录）
    """
    if not repository_dir.exists():
        logger.error(f"仓库目录不存在: {repository_dir}")
        sys.exit(1)

    simple_dir = repository_dir / "simple"
    if not simple_dir.exists():
        logger.warning(f"{simple_dir} 不存在，请先生成索引")

    # 初始化访问日志
    access_log_path = repository_dir / ".access_log.db"
    access_logger = AccessLogger(access_log_path)

    def handler_factory(*args, **kwargs):
        return _RequestHandler(*args, directory=str(repository_dir), access_logger=access_logger, **kwargs)

    with socketserver.ThreadingTCPServer((host, port), handler_factory) as httpd:
        logger.info("PIP 镜像服务器启动")
        logger.info(f"  地址: http://{host}:{port}")
        logger.info(f"  仓库: {repository_dir}")
        logger.info(f"  访问日志: {access_log_path}")
        logger.info(f"  pip 使用: pip install --index-url http://{host}:{port}/simple package")
        logger.info(f"  Python 解释器: UV_PYTHON_DOWNLOADS_JSON_URL=http://{host}:{port}/python-builds/index.json")
        logger.info("  按 Ctrl+C 停止")

        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            logger.info("服务器已停止")
            # 打印访问汇总
            summary = access_logger.get_summary()
            logger.info(f"访问汇总: 总请求={summary['total_requests']}, 成功={summary['successful_requests']}, 独立IP={summary['unique_ips']}")
