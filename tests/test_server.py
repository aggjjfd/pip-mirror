"""F3 回归: server 在响应 /python-builds/index.json 时,把相对 url 改写为 absolute URL."""

from __future__ import annotations

import hashlib
import json
import socket
import sqlite3
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


def test_binary_download_full_body(tmp_path: Path) -> None:
    """回归: copyfile 必须把整个文件写出去,Content-Length 与实际 body 字节数必须一致.

    历史故障: copyfile 早期实现先调 outputfile.tell() 统计字节数,但 socket 包装的
    BufferedWriter 调 tell 抛 io.UnsupportedOperation,导致 200 头已发但 body 0
    字节,客户端拿到 'end of file before message length reached'。这个测试用一个
    >64KB 文件(覆盖多次 read/write 循环)+ sha256 校验 body 完整性。
    """
    pb_dir = tmp_path / "python-builds"
    pb_dir.mkdir(parents=True)
    payload = (b"PIPMIRROR" * 130_000)[:1_048_576]  # 正好 1 MiB,跨多次 64 KiB 读循环
    target = pb_dir / "cpython-fake.tar.gz"
    target.write_bytes(payload)
    expected_sha = hashlib.sha256(payload).hexdigest()

    httpd, _, port = _start_server(tmp_path)
    try:
        with urllib.request.urlopen(
            f"http://127.0.0.1:{port}/python-builds/cpython-fake.tar.gz"
        ) as resp:
            content_length = resp.headers.get("Content-Length")
            body = resp.read()
    finally:
        httpd.shutdown()
        httpd.server_close()

    assert content_length == str(len(payload)), (
        f"Content-Length 与文件大小不匹配: header={content_length}, "
        f"file={len(payload)}"
    )
    assert len(body) == len(payload), (
        f"body 字节数与 Content-Length 不一致: body={len(body)}, "
        f"expected={len(payload)}"
    )
    assert hashlib.sha256(body).hexdigest() == expected_sha, (
        "body 内容哈希与磁盘文件不一致(可能被截断或乱序)"
    )

    # access_log 应记录正确的 bytes_sent
    with sqlite3.connect(tmp_path / ".access_log.db") as conn:
        rows = conn.execute(
            "SELECT path, status_code, bytes_sent FROM access_log "
            "WHERE path = '/python-builds/cpython-fake.tar.gz'"
        ).fetchall()
    assert rows, "access_log 没记录到这次下载"
    _, status, bytes_sent = rows[-1]
    assert status == 200
    assert bytes_sent == len(payload), (
        f"access_log.bytes_sent 与实际不符: got {bytes_sent}, "
        f"expected {len(payload)}"
    )
