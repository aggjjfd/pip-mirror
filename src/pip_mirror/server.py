"""Python HTTP 服务器，提供 PEP 503 / PEP 691 Simple Repository 服务."""

from __future__ import annotations

import http.server
import json
import logging
import socketserver
import sys
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from .access_logger import AccessLogger, AccessRecord

logger = logging.getLogger("pip-mirror")

_JSON_CONTENT_TYPE = "application/vnd.pypi.simple.v1+json"
_HTML_CONTENT_TYPE = "application/vnd.pypi.simple.v1+html"


class _RequestHandler(http.server.SimpleHTTPRequestHandler):
    """自定义请求处理器，支持目录索引、跨域、访问日志和内容协商."""

    def __init__(
        self,
        *args,
        directory: str | None = None,
        access_logger: AccessLogger | None = None,
        **kwargs,
    ):
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

    def _client_ip(self) -> str:
        """提取客户端真实 IP（考虑反向代理）."""
        forwarded = self.headers.get("X-Forwarded-For")
        if forwarded:
            return forwarded.split(",")[0].strip()
        return self.client_address[0]

    def log_message(self, format: str, *args):
        """覆盖默认日志，使用 AccessLogger 记录到 SQLite."""
        if self._access_logger is None:
            return

        record = AccessRecord(
            timestamp=datetime.now(timezone.utc).isoformat(),
            client_ip=self._client_ip(),
            method=self.command,
            path=self.path,
            status_code=self._status_code,
            user_agent=self.headers.get("User-Agent"),
            bytes_sent=self._response_bytes if self._response_bytes > 0 else None,
            referer=self.headers.get("Referer"),
        )
        self._access_logger.log(record)
        logger.info(f"{record.client_ip} {self.command} {self.path} {self._status_code}")

    def copyfile(self, source, outputfile):
        """覆盖 copyfile 以统计发送字节数."""
        import shutil

        pos = outputfile.tell() if hasattr(outputfile, "tell") else 0
        shutil.copyfileobj(source, outputfile)
        if hasattr(outputfile, "tell"):
            self._response_bytes = outputfile.tell() - pos

    def _wants_json(self) -> bool:
        """判断客户端是否请求 JSON 格式（PEP 691）."""
        # 1. 检查 URL 参数 ?format=...
        parsed = urlparse(self.path)
        query = parse_qs(parsed.query)
        fmt = query.get("format", [""])[0]
        if _JSON_CONTENT_TYPE in fmt:
            return True

        # 2. 检查 Accept 头
        accept = self.headers.get("Accept", "")
        return _JSON_CONTENT_TYPE in accept

    def _is_simple_api_path(self, path: str) -> bool:
        """判断路径是否为 Simple API 端点."""
        stripped = path.rstrip("/")
        return stripped == "/simple" or stripped.startswith("/simple/")

    def _serve_json(self, filepath: Path) -> None:
        """提供 JSON 索引文件."""
        if not filepath.exists():
            self.send_error(404, "JSON index not found")
            return

        content = filepath.read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", _JSON_CONTENT_TYPE)
        self.send_header("Content-Length", str(len(content)))
        self.end_headers()
        self.wfile.write(content)
        self._response_bytes = len(content)

    def _translate_path_to_json(self, path: str) -> Path | None:
        """将 Simple API 路径翻译为 JSON 文件路径."""
        if not self._serve_dir:
            return None

        base = Path(self._serve_dir)
        stripped = path.split("?")[0].rstrip("/")

        if stripped == "/simple":
            return base / "simple" / "index.json"

        if stripped.startswith("/simple/"):
            # /simple/pkg/ -> simple/pkg/index.json
            parts = stripped[len("/simple/"):].split("/")
            if len(parts) >= 1 and parts[0]:
                pkg_name = parts[0]
                return base / "simple" / pkg_name / "index.json"

        return None

    def do_GET(self):
        """处理 GET 请求，支持 PEP 691 内容协商."""
        self._status_code = 200
        self._response_bytes = 0

        parsed_path = urlparse(self.path).path

        if parsed_path == "/python-builds/index.json":
            self._serve_python_builds_index()
            return

        if self._is_simple_api_path(parsed_path) and self._wants_json():
            json_path = self._translate_path_to_json(self.path)
            if json_path:
                self._serve_json(json_path)
                return

        # 默认：调用父类处理（返回 HTML 或文件）
        super().do_GET()

    def _public_base_url(self) -> str:
        """从 Host header 推导 absolute base URL,fallback 到绑定地址."""
        host = self.headers.get("Host")
        if host:
            return f"http://{host}"
        bind_host, bind_port = self.server.server_address[:2]
        return f"http://{bind_host}:{bind_port}"

    def _serve_python_builds_index(self) -> None:
        """读取磁盘 python-builds/index.json,把相对 url 改写为绝对 URL 后返回."""
        if not self._serve_dir:
            self.send_error(404)
            return

        filepath = Path(self._serve_dir) / "python-builds" / "index.json"
        if not filepath.exists():
            self.send_error(404, "python-builds index not found")
            return

        try:
            data = json.loads(filepath.read_text(encoding="utf-8"))
        except (OSError, ValueError) as e:
            self.send_error(500, f"failed to parse index.json: {e}")
            return

        base = self._public_base_url()
        for entry in data.values():
            url = entry.get("url", "")
            if url.startswith("/"):
                entry["url"] = f"{base}{url}"

        body = json.dumps(data, indent=2, ensure_ascii=False).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        self._response_bytes = len(body)


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
        return _RequestHandler(
            *args, directory=str(repository_dir), access_logger=access_logger, **kwargs
        )

    with socketserver.ThreadingTCPServer((host, port), handler_factory) as httpd:
        logger.info("PIP 镜像服务器启动")
        logger.info(f"  地址: http://{host}:{port}")
        logger.info(f"  仓库: {repository_dir}")
        logger.info(f"  访问日志: {access_log_path}")
        logger.info(f"  pip 使用: pip install --index-url http://{host}:{port}/simple package")
        logger.info(
            f"  Python 解释器: UV_PYTHON_DOWNLOADS_JSON_URL="
            f"http://{host}:{port}/python-builds/index.json"
        )
        logger.info("  按 Ctrl+C 停止")

        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            logger.info("服务器已停止")
            summary = access_logger.get_summary()
            logger.info(
                f"访问汇总: 总请求={summary['total_requests']}, "
                f"成功={summary['successful_requests']}, 独立IP={summary['unique_ips']}"
            )
