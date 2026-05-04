"""增量打包/导入端到端回归.

新设计要点(对照 fluttering-doodling-dahl 计划):
- create_incremental_package 新签名,manifest 简化为 created_at + stats
- import-incremental 现算 sha256 写库,新增 --strict
- 联合 early return:simple + python-builds 同时为空则不产 archive
"""

from __future__ import annotations

import hashlib
import json
import tarfile
from pathlib import Path

import pytest

from pip_mirror.cli import _cmd_import_incremental
from pip_mirror.downloader import FileInfo
from pip_mirror.packager import create_incremental_package
from pip_mirror.sqlite_store import DownloadStore

WHEEL_BYTES = b"PK\x05\x06" + b"\x00" * 18
META_BYTES = b"Metadata-Version: 2.1\nName: demo-pkg\nVersion: 1.0\n"


def _hashof(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _seed_repo(repo: Path) -> FileInfo:
    """在 repo 里造一个 wheel + .metadata,返回 FileInfo(不预写 store)."""
    repo.mkdir(parents=True, exist_ok=True)
    pkg_dir = repo / "simple" / "demo-pkg"
    pkg_dir.mkdir(parents=True)

    wheel = pkg_dir / "demo_pkg-1.0-py3-none-any.whl"
    wheel.write_bytes(WHEEL_BYTES)
    meta = pkg_dir / "demo_pkg-1.0-py3-none-any.whl.metadata"
    meta.write_bytes(META_BYTES)

    return FileInfo(
        filename=wheel.name,
        url="https://example.com/" + wheel.name,
        sha256=_hashof(WHEEL_BYTES),
        size=wheel.stat().st_size,
        package_name="demo-pkg",
        version="1.0",
    )


def _make_args(**overrides):
    """构造一个空 args namespace,补默认字段."""
    defaults = {
        "archive": None,
        "config": None,
        "no_reindex": False,
        "strict": False,
    }
    defaults.update(overrides)
    return type("Args", (), defaults)()


def _patch_config(monkeypatch: pytest.MonkeyPatch, repo: Path) -> None:
    from pip_mirror.config import Config
    fake_cfg = Config.from_dict({"repository_dir": str(repo)})
    monkeypatch.setattr(Config, "load", classmethod(lambda cls, path=None: fake_cfg))


# ============================================================================
#                              create_incremental_package
# ============================================================================


def test_create_incremental_includes_metadata_file(tmp_path: Path) -> None:
    """打包必须把 .whl.metadata 一并塞入,且 manifest schema 简化(无 sha256)."""
    repo = tmp_path / "repo"
    out_dir = tmp_path / "out"
    fi = _seed_repo(repo)

    archive = create_incremental_package(
        simple_files=[fi],
        python_builds_files=[],
        python_builds_index=None,
        repository_dir=repo,
        output_dir=out_dir,
    )
    assert archive is not None and archive.exists()

    with tarfile.open(archive, "r:gz") as tar:
        names = tar.getnames()
        assert "simple/demo-pkg/demo_pkg-1.0-py3-none-any.whl" in names
        assert "simple/demo-pkg/demo_pkg-1.0-py3-none-any.whl.metadata" in names

        manifest_text = tar.extractfile(tar.getmember("manifest.json")).read().decode("utf-8")

    manifest = json.loads(manifest_text)
    assert manifest["stats"] == {"simple": 1, "python_builds": 0}
    assert "created_at" in manifest
    # schema 简化后不应再带 files / sha256 字段
    assert "files" not in manifest


def test_create_incremental_with_python_builds(tmp_path: Path) -> None:
    """打包能同时容纳 python-builds 文件和 index.json 快照."""
    repo = tmp_path / "repo"
    out_dir = tmp_path / "out"

    py_dir = repo / "python-builds"
    py_dir.mkdir(parents=True)
    py_tarball = py_dir / (
        "cpython-3.12.5+20240814-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz"
    )
    py_tarball.write_bytes(b"fake-python-build")
    index_json = py_dir / "index.json"
    index_json.write_text('{"cpython-3.12-linux": {"url": "/python-builds/x"}}', encoding="utf-8")

    archive = create_incremental_package(
        simple_files=[],
        python_builds_files=[py_tarball],
        python_builds_index=index_json,
        repository_dir=repo,
        output_dir=out_dir,
    )
    assert archive is not None

    with tarfile.open(archive, "r:gz") as tar:
        names = tar.getnames()
        assert f"python-builds/{py_tarball.name}" in names
        assert "python-builds/index.json" in names
        manifest = json.loads(tar.extractfile(tar.getmember("manifest.json")).read())
        assert manifest["stats"] == {"simple": 0, "python_builds": 1}


def test_create_incremental_returns_none_on_empty(tmp_path: Path) -> None:
    """simple_files 与 python_builds_files 同时为空时不产 archive."""
    repo = tmp_path / "repo"
    repo.mkdir()
    out_dir = tmp_path / "out"

    archive = create_incremental_package(
        simple_files=[],
        python_builds_files=[],
        python_builds_index=None,
        repository_dir=repo,
        output_dir=out_dir,
    )
    assert archive is None
    assert not out_dir.exists() or not list(out_dir.iterdir())


# ============================================================================
#                               import-incremental
# ============================================================================


def test_import_incremental_recomputes_sha256_and_rebuilds_index(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """import 默认行为:解包 → 现算 sha256 写 store → 重建索引,
    生成的 index.html 应当带 data-sha256 与 data-core-metadata."""
    src_repo = tmp_path / "src"
    out_dir = tmp_path / "out"
    fi = _seed_repo(src_repo)
    archive = create_incremental_package(
        simple_files=[fi],
        python_builds_files=[],
        python_builds_index=None,
        repository_dir=src_repo,
        output_dir=out_dir,
    )
    assert archive is not None

    target_repo = tmp_path / "target"
    target_repo.mkdir()
    _patch_config(monkeypatch, target_repo)

    rc = _cmd_import_incremental(_make_args(archive=str(archive)))
    assert rc == 0

    expected_sha = _hashof(WHEEL_BYTES)
    expected_meta_sha = _hashof(META_BYTES)
    store = DownloadStore(target_repo / ".store.db")
    assert store.get_sha256(fi.filename) == expected_sha
    assert store.get_metadata_sha256(fi.filename) == expected_meta_sha

    assert not (target_repo / "manifest.json").exists()

    index_html = (target_repo / "simple" / "demo-pkg" / "index.html").read_text(encoding="utf-8")
    assert f'data-sha256="{expected_sha}"' in index_html
    assert f'data-core-metadata="sha256={expected_meta_sha}"' in index_html

    index_json_path = target_repo / "simple" / "demo-pkg" / "index.json"
    payload = json.loads(index_json_path.read_text(encoding="utf-8"))
    assert payload["files"][0]["hashes"]["sha256"] == expected_sha
    assert payload["files"][0]["dist-info-metadata"]["sha256"] == expected_meta_sha


def test_import_incremental_no_reindex_skips_index(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """--no-reindex 不触发 generate_index,但 store 仍被写入."""
    src_repo = tmp_path / "src"
    out_dir = tmp_path / "out"
    fi = _seed_repo(src_repo)
    archive = create_incremental_package(
        simple_files=[fi],
        python_builds_files=[],
        python_builds_index=None,
        repository_dir=src_repo,
        output_dir=out_dir,
    )
    assert archive is not None

    target_repo = tmp_path / "target"
    target_repo.mkdir()
    _patch_config(monkeypatch, target_repo)

    called: list = []
    import pip_mirror.cli as cli_mod
    monkeypatch.setattr(cli_mod, "generate_index", lambda repo: called.append(repo))

    rc = _cmd_import_incremental(_make_args(archive=str(archive), no_reindex=True))
    assert rc == 0
    assert called == []

    store = DownloadStore(target_repo / ".store.db")
    assert store.get_sha256(fi.filename) == _hashof(WHEEL_BYTES)


def test_import_incremental_default_skips_missing_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """默认宽松模式:解包后某文件被删,sha256 算不出 → WARNING + skip,
    其它文件照常入库,exit 0."""
    src_repo = tmp_path / "src"
    out_dir = tmp_path / "out"

    fi_ok = _seed_repo(src_repo)
    # 再加一个会被破坏的包
    bad_dir = src_repo / "simple" / "broken"
    bad_dir.mkdir(parents=True)
    bad_wheel = bad_dir / "broken-1.0-py3-none-any.whl"
    bad_wheel.write_bytes(b"X" * 50)
    fi_bad = FileInfo(
        filename=bad_wheel.name,
        url="https://example.com/x.whl",
        sha256=_hashof(b"X" * 50),
        size=50,
        package_name="broken",
        version="1.0",
    )
    archive = create_incremental_package(
        simple_files=[fi_ok, fi_bad],
        python_builds_files=[],
        python_builds_index=None,
        repository_dir=src_repo,
        output_dir=out_dir,
    )
    assert archive is not None

    target_repo = tmp_path / "target"
    target_repo.mkdir()
    _patch_config(monkeypatch, target_repo)

    # 注入失败:让 broken/*.whl 的 hash 报错
    import pip_mirror.cli as cli_mod
    real_hash = cli_mod._hash_file

    def fake_hash(p: Path) -> str:
        if "broken" in str(p):
            raise OSError("simulated read failure")
        return real_hash(p)

    monkeypatch.setattr(cli_mod, "_hash_file", fake_hash)

    rc = _cmd_import_incremental(_make_args(archive=str(archive)))
    assert rc == 0  # 默认宽松,跳过坏文件后整体仍成功
    store = DownloadStore(target_repo / ".store.db")
    assert store.get_sha256(fi_ok.filename) == _hashof(WHEEL_BYTES)
    assert store.get_sha256(fi_bad.filename) is None  # 坏文件被跳过


def test_import_incremental_strict_fails_on_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """--strict 下任一文件 hash 失败 → exit 1 + 不重建索引."""
    src_repo = tmp_path / "src"
    out_dir = tmp_path / "out"
    fi = _seed_repo(src_repo)
    archive = create_incremental_package(
        simple_files=[fi],
        python_builds_files=[],
        python_builds_index=None,
        repository_dir=src_repo,
        output_dir=out_dir,
    )
    assert archive is not None

    target_repo = tmp_path / "target"
    target_repo.mkdir()
    _patch_config(monkeypatch, target_repo)

    import pip_mirror.cli as cli_mod
    monkeypatch.setattr(
        cli_mod, "_hash_file",
        lambda p: (_ for _ in ()).throw(OSError("boom")),
    )
    called: list = []
    monkeypatch.setattr(cli_mod, "generate_index", lambda repo: called.append(repo))

    rc = _cmd_import_incremental(_make_args(archive=str(archive), strict=True))
    assert rc == 1
    assert called == [], "strict 失败时不应触发 generate_index"
