"""F3 回归: server 在响应 /python-builds/index.json 时,把相对 url 改写为 absolute URL."""

from __future__ import annotations

import json
import socket
import threading
import time
import urllib.request
from pathlib import Path
from socketserver import ThreadingTCPServer

import pytest

from pip_mirror.access_logger import AccessLogger
from pip_mirror.server import _RequestHandler


def _free_port() -> int:
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()
    return port


def _start_server(repository_dir: Path) -> tuple[ThreadingTCPServer, threading.Thread, int]:
    port = _free_port()
    access_logger = AccessLogger(repository_dir / ".access_log.db")

    def factory(*args, **kwargs):
        return _RequestHandler(
            *args,
            directory=str(repository_dir),
            access_logger=access_logger,
            **kwargs,
        )

    httpd = ThreadingTCPServer(("127.0.0.1", port), factory)
    thread = threading.Thread(target=httpd.serve_forever, daemon=True)
    thread.start()
    time.sleep(0.05)
    return httpd, thread, port


def test_python_builds_index_url_rewritten_to_absolute(tmp_path: Path) -> None:
    """磁盘 index.json 里 url 是相对路径,GET 后应被改写为以 http:// 开头的绝对 URL.

    同时验证非 url 字段(sha256)不被丢失或改写,Content-Type 是 application/json。
    """
    pb_dir = tmp_path / "python-builds"
    pb_dir.mkdir(parents=True)
    raw = {
        "cpython-3.12.4-x86_64-linux-gnu": {
            "url": "/python-builds/cpython-3.12.4.tar.gz",
            "sha256": "deadbeef",
            "kind": "cpython",
        },
        "cpython-3.11.10-x86_64-linux-gnu": {
            "url": "/python-builds/cpython-3.11.10.tar.gz",
            "sha256": "cafef00d",
            "kind": "cpython",
        },
    }
    (pb_dir / "index.json").write_text(json.dumps(raw), encoding="utf-8")

    httpd, _, port = _start_server(tmp_path)
    try:
        with urllib.request.urlopen(
            f"http://127.0.0.1:{port}/python-builds/index.json"
        ) as resp:
            content_type = resp.headers.get("Content-Type", "")
            data = json.loads(resp.read().decode("utf-8"))
    finally:
        httpd.shutdown()
        httpd.server_close()

    assert "application/json" in content_type, f"unexpected Content-Type: {content_type!r}"

    for key, entry in data.items():
        assert entry["url"].startswith("http://"), f"{key} url not absolute: {entry['url']}"
        assert entry["url"].endswith(raw[key]["url"]), f"{key} suffix lost: {entry['url']}"
        assert f":{port}" in entry["url"], f"{key} port lost: {entry['url']}"
        assert entry["sha256"] == raw[key]["sha256"], (
            f"{key} sha256 dropped or mutated by url-rewrite: "
            f"got {entry.get('sha256')!r}, want {raw[key]['sha256']!r}"
        )
        assert entry["kind"] == raw[key]["kind"], f"{key} kind dropped"


def test_python_builds_index_404_when_missing(tmp_path: Path) -> None:
    """没有 python-builds/index.json 时返回 404,而不是 500."""
    httpd, _, port = _start_server(tmp_path)
    try:
        with pytest.raises(urllib.error.HTTPError) as info:
            urllib.request.urlopen(
                f"http://127.0.0.1:{port}/python-builds/index.json"
            )
        assert info.value.code == 404
    finally:
        httpd.shutdown()
        httpd.server_close()
