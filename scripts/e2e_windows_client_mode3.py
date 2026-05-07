#!/usr/bin/env python3
from __future__ import annotations

import os
import sqlite3
import subprocess
import sys
import tarfile
import time
from pathlib import Path
from typing import TextIO
from urllib.error import URLError
from urllib.request import urlopen

SERVER_READY_TIMEOUT_SECONDS = 60
HTTP_TIMEOUT_SECONDS = 5
LOG_TAIL_LINES = 100
SERVER_STOP_TIMEOUT_SECONDS = 10
MIN_PYTHON_BUILD_HITS = 1

REQUESTS_PACKAGE = "requests"
MANAGED_PYTHON_VERSION = "3.12"

STAGE_DIR = Path(r"C:\stage")
CLIENT_VENV = Path(r"C:\client")
UV_PYTHONS_DIR = Path(r"C:\uv-pythons")
STAGE_TOML = Path("stage.toml")
SERVER_LOG = Path("server.log")
SERVER_ERR = Path("server.err")
SERVER_PID = Path("server.pid")
MIRROR_TARBALL = Path("mirror.tar.gz")
ACCESS_LOG_DB = STAGE_DIR / "packages" / ".access_log.db"

GREEN = "\033[32m"
YELLOW = "\033[33m"
RED = "\033[31m"
RESET = "\033[0m"


def log_info(message: str) -> None:
    print(f"{GREEN}[INFO]{RESET} {message}", flush=True)


def log_warn(message: str) -> None:
    print(f"{YELLOW}[WARN]{RESET} {message}", flush=True)


def log_error(message: str) -> None:
    print(f"{RED}[ERROR]{RESET} {message}", flush=True)


def run_checked(args: list[str], env: dict[str, str] | None = None) -> None:
    log_info(f"run: {' '.join(args)}")
    completed = subprocess.run(args, check=False, text=True, env=env)
    if completed.returncode != 0:
        raise RuntimeError(f"command failed ({completed.returncode}): {' '.join(args)}")


def run_capture(args: list[str], env: dict[str, str] | None = None) -> str:
    log_info(f"run(capture): {' '.join(args)}")
    completed = subprocess.run(
        args,
        check=False,
        text=True,
        env=env,
        capture_output=True,
    )
    if completed.returncode != 0:
        if completed.stdout:
            print(completed.stdout, end="")
        if completed.stderr:
            print(completed.stderr, end="", file=sys.stderr)
        raise RuntimeError(f"command failed ({completed.returncode}): {' '.join(args)}")
    return completed.stdout.strip()


def require_path(path: Path, hint: str) -> None:
    if not path.exists():
        raise RuntimeError(f"{hint}: {path}")


def prepare_stage() -> None:
    log_info("extract mirror.tar.gz to C:\\stage")
    STAGE_DIR.mkdir(parents=True, exist_ok=True)
    require_path(MIRROR_TARBALL, "mirror tarball missing")
    with tarfile.open(MIRROR_TARBALL, "r:gz") as archive:
        archive.extractall(STAGE_DIR)

    required_paths = [
        STAGE_DIR / "packages" / "simple" / "requests" / "index.html",
        STAGE_DIR / "packages" / ".store.db",
        STAGE_DIR / "packages" / "python-builds" / "index.json",
    ]
    for required in required_paths:
        require_path(required, "required file missing after extract")

    simple_dir = STAGE_DIR / "packages" / "simple"
    head_entries = [p.name for p in sorted(simple_dir.iterdir(), key=lambda x: x.name.lower())[:10]]
    log_info(f"staged simple/ (head 10): {head_entries}")


def write_stage_toml(mirror_host: str, mirror_port: int) -> None:
    contents = (
        f'packages = ["{REQUESTS_PACKAGE}"]\n'
        'repository_dir = "C:/stage/packages"\n'
        f'server_host = "{mirror_host}"\n'
        f"server_port = {mirror_port}\n"
    )
    STAGE_TOML.write_text(contents, encoding="utf-8")
    log_info(f"generated {STAGE_TOML}")


def wait_for_server(mirror_host: str, mirror_port: int) -> None:
    url = f"http://{mirror_host}:{mirror_port}/simple/requests/"
    for second in range(SERVER_READY_TIMEOUT_SECONDS):
        try:
            with urlopen(url, timeout=HTTP_TIMEOUT_SECONDS):
                log_info(f"server up after {second} s")
                return
        except URLError:
            time.sleep(1)
    raise RuntimeError("server did not come up within 60 s")


def validate_charset_wheel() -> None:
    site_packages = CLIENT_VENV / "Lib" / "site-packages"
    dist_infos = sorted(site_packages.glob("charset_normalizer-*.dist-info"))
    if not dist_infos:
        raise RuntimeError("charset_normalizer dist-info not found in C:\\client")

    wheel_meta = dist_infos[0] / "WHEEL"
    require_path(wheel_meta, "WHEEL meta missing")
    lines = wheel_meta.read_text(encoding="utf-8", errors="replace").splitlines()
    tag_lines = [line for line in lines if line.startswith("Tag:") and ("win_amd64" in line or "win32" in line)]
    if not tag_lines:
        log_warn("----- WHEEL contents -----")
        for line in lines:
            print(line)
        raise RuntimeError("installed charset_normalizer is NOT a Windows wheel")

    log_info("charset_normalizer installed wheel tag(s):")
    for line in tag_lines:
        print(f"  {line}")


def check_python_build_hits() -> None:
    require_path(ACCESS_LOG_DB, "access log db missing")
    conn = sqlite3.connect(ACCESS_LOG_DB)
    try:
        query = "SELECT COUNT(*) FROM access_log WHERE path LIKE '/python-builds/%';"
        hits = int(conn.execute(query).fetchone()[0])
    finally:
        conn.close()
    log_info(f"python-builds hits in mirror access_log: {hits}")
    if hits < MIN_PYTHON_BUILD_HITS:
        raise RuntimeError("access_log has zero /python-builds/ hits")


def tail_file(path: Path) -> None:
    if not path.exists():
        log_warn(f"{path} not found")
        return
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    for line in lines[-LOG_TAIL_LINES:]:
        print(line)


def start_server(bin_path: Path) -> tuple[subprocess.Popen[str], TextIO, TextIO]:
    stdout_handle = SERVER_LOG.open("w", encoding="utf-8")
    stderr_handle = SERVER_ERR.open("w", encoding="utf-8")
    proc = subprocess.Popen(
        [str(bin_path), "serve", "-c", str(STAGE_TOML)],
        stdout=stdout_handle,
        stderr=stderr_handle,
        text=True,
    )
    SERVER_PID.write_text(str(proc.pid), encoding="ascii")
    return proc, stdout_handle, stderr_handle


def stop_server(
    proc: subprocess.Popen[str] | None,
    stdout_handle: TextIO | None,
    stderr_handle: TextIO | None,
) -> None:
    if stderr_handle:
        stderr_handle.close()
    if stdout_handle:
        stdout_handle.close()
    if proc and proc.poll() is None:
        log_info("stop pip-mirror serve")
        proc.terminate()
        try:
            proc.wait(timeout=SERVER_STOP_TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired:
            proc.kill()
    log_info("===== tail server.log =====")
    tail_file(SERVER_LOG)
    log_info("===== tail server.err =====")
    tail_file(SERVER_ERR)


def install_client_requests(mirror_host: str, mirror_port: int, env: dict[str, str]) -> None:
    run_checked(["uv", "venv", str(CLIENT_VENV)], env=env)
    env["VIRTUAL_ENV"] = str(CLIENT_VENV)
    run_checked(
        [
            "uv",
            "pip",
            "install",
            "--index-url",
            f"http://{mirror_host}:{mirror_port}/simple",
            REQUESTS_PACKAGE,
        ],
        env=env,
    )
    run_checked(
        [r"C:\client\Scripts\python.exe", "-c", "import requests; print('client requests', requests.__version__)"],
        env=env,
    )
    validate_charset_wheel()


def install_managed_python(mirror_host: str, mirror_port: int, env: dict[str, str]) -> None:
    env["UV_PYTHON_DOWNLOADS_JSON_URL"] = f"http://{mirror_host}:{mirror_port}/python-builds/index.json"
    env["UV_PYTHON_INSTALL_DIR"] = str(UV_PYTHONS_DIR)
    env["UV_PYTHON_PREFERENCE"] = "only-managed"
    run_checked(["uv", "python", "install", MANAGED_PYTHON_VERSION], env=env)
    pybin = run_capture(["uv", "python", "find", MANAGED_PYTHON_VERSION], env=env)
    log_info(f"uv python find {MANAGED_PYTHON_VERSION} -> {pybin}")
    run_checked([pybin, "--version"], env=env)
    check_python_build_hits()


def run_mode3() -> None:
    mirror_host = os.environ.get("MIRROR_HOST", "127.0.0.1")
    mirror_port = int(os.environ.get("MIRROR_PORT", "18080"))

    prepare_stage()
    write_stage_toml(mirror_host, mirror_port)

    bin_path = Path.cwd() / "target" / "release" / "pip-mirror.exe"
    require_path(bin_path, "pip-mirror.exe missing")

    env = os.environ.copy()
    proc: subprocess.Popen[str] | None = None
    stdout_handle: TextIO | None = None
    stderr_handle: TextIO | None = None
    try:
        log_info("start pip-mirror serve")
        proc, stdout_handle, stderr_handle = start_server(bin_path)
        wait_for_server(mirror_host, mirror_port)
        install_client_requests(mirror_host, mirror_port, env)
        install_managed_python(mirror_host, mirror_port, env)
    finally:
        stop_server(proc, stdout_handle, stderr_handle)


def main() -> int:
    try:
        run_mode3()
        return 0
    except Exception as exc:  # noqa: BLE001
        log_error(str(exc))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
