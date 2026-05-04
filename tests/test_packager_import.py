"""增量打包/导入端到端回归: packager 含 .metadata + manifest 含 metadata_sha256;
import-incremental 能解包并把记录写入 .store.db,索引重建后 sha256 出现在 index.html.
"""

from __future__ import annotations

import json
import tarfile
from pathlib import Path

import pytest

from pip_mirror.cli import _cmd_import_incremental
from pip_mirror.downloader import FileInfo
from pip_mirror.packager import create_incremental_package
from pip_mirror.sqlite_store import DownloadStore


def _seed_repo(repo: Path) -> tuple[FileInfo, str, str]:
    """在 repo 里造一个 wheel + .metadata + .store.db 行,返回 (FileInfo, sha256, meta_sha256)."""
    repo.mkdir(parents=True, exist_ok=True)
    pkg_dir = repo / "simple" / "demo-pkg"
    pkg_dir.mkdir(parents=True)

    wheel = pkg_dir / "demo_pkg-1.0-py3-none-any.whl"
    wheel.write_bytes(b"PK\x05\x06" + b"\x00" * 18)  # 任意非空内容,sha 校验不在测试范围

    meta = pkg_dir / "demo_pkg-1.0-py3-none-any.whl.metadata"
    meta.write_bytes(b"Metadata-Version: 2.1\nName: demo-pkg\nVersion: 1.0\n")

    sha = "a" * 64
    meta_sha = "b" * 64
    store = DownloadStore(repo / ".store.db")
    store.add_file(
        filename=wheel.name,
        package_name="demo-pkg",
        version="1.0",
        sha256=sha,
        size=wheel.stat().st_size,
    )
    store.set_metadata_sha256(wheel.name, meta_sha)

    fi = FileInfo(
        filename=wheel.name,
        url="https://example.com/" + wheel.name,
        sha256=sha,
        size=wheel.stat().st_size,
        package_name="demo-pkg",
        version="1.0",
    )
    return fi, sha, meta_sha


def test_create_incremental_includes_metadata_file_and_hash(tmp_path: Path) -> None:
    """create_incremental_package 必须把 .whl.metadata 一并打入 tar,
    并且 manifest.json 的对应条目带上 metadata_sha256 字段."""
    repo = tmp_path / "repo"
    out_dir = tmp_path / "out"
    fi, sha, meta_sha = _seed_repo(repo)

    archive = create_incremental_package([fi], repo, out_dir, compress=True)
    assert archive is not None and archive.exists()

    with tarfile.open(archive, "r:gz") as tar:
        names = tar.getnames()
        assert "simple/demo-pkg/demo_pkg-1.0-py3-none-any.whl" in names
        assert "simple/demo-pkg/demo_pkg-1.0-py3-none-any.whl.metadata" in names, (
            f"PEP 658 .metadata 文件未被打包: {names}"
        )

        manifest_member = tar.getmember("manifest.json")
        manifest_text = tar.extractfile(manifest_member).read().decode("utf-8")

    manifest = json.loads(manifest_text)
    assert len(manifest["files"]) == 1
    entry = manifest["files"][0]
    assert entry["sha256"] == sha
    assert entry["metadata_sha256"] == meta_sha


def test_import_incremental_writes_store_and_rebuilds_index(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """import-incremental 端到端:打包→拷到全新 repo→import→.store.db 含记录,
    生成的 index.html 含 data-sha256/data-core-metadata 属性."""
    src_repo = tmp_path / "src"
    out_dir = tmp_path / "out"
    fi, sha, meta_sha = _seed_repo(src_repo)
    archive = create_incremental_package([fi], src_repo, out_dir, compress=True)
    assert archive is not None

    # 全新空仓库
    target_repo = tmp_path / "target"
    target_repo.mkdir()

    # _cmd_import_incremental 通过 Config.load() 读 repository_dir,
    # 这里用 monkeypatch 把 Config.load 强制返回指向 target_repo 的 Config
    from pip_mirror.config import Config
    fake_cfg = Config.from_dict({"repository_dir": str(target_repo)})
    monkeypatch.setattr(Config, "load", classmethod(lambda cls, path=None: fake_cfg))

    args = type("Args", (), {
        "archive": str(archive),
        "config": None,
        "no_reindex": False,
    })()
    rc = _cmd_import_incremental(args)
    assert rc == 0

    # store 应当含一条记录
    store = DownloadStore(target_repo / ".store.db")
    assert store.get_sha256(fi.filename) == sha
    assert store.get_metadata_sha256(fi.filename) == meta_sha

    # manifest.json 不应残留在 repo 根
    assert not (target_repo / "manifest.json").exists()

    # 索引重建后,index.html 应当带 data-sha256 与 data-core-metadata 属性
    index_html = (target_repo / "simple" / "demo-pkg" / "index.html").read_text(encoding="utf-8")
    assert f'data-sha256="{sha}"' in index_html
    assert f'data-core-metadata="sha256={meta_sha}"' in index_html

    # PEP 691 JSON 也应有 hashes 字段
    index_json_path = target_repo / "simple" / "demo-pkg" / "index.json"
    payload = json.loads(index_json_path.read_text(encoding="utf-8"))
    assert payload["files"][0]["hashes"]["sha256"] == sha
    assert payload["files"][0]["dist-info-metadata"]["sha256"] == meta_sha


def test_import_incremental_no_reindex_skips_index_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """--no-reindex 时不应跑 generate_index;.store.db 仍写入."""
    src_repo = tmp_path / "src"
    out_dir = tmp_path / "out"
    fi, sha, _ = _seed_repo(src_repo)
    archive = create_incremental_package([fi], src_repo, out_dir, compress=True)
    assert archive is not None

    target_repo = tmp_path / "target"
    target_repo.mkdir()

    from pip_mirror.config import Config
    fake_cfg = Config.from_dict({"repository_dir": str(target_repo)})
    monkeypatch.setattr(Config, "load", classmethod(lambda cls, path=None: fake_cfg))

    called = []
    import pip_mirror.indexer as indexer_mod
    monkeypatch.setattr(
        indexer_mod, "generate_index", lambda repo: called.append(repo),
    )

    args = type("Args", (), {
        "archive": str(archive),
        "config": None,
        "no_reindex": True,
    })()
    rc = _cmd_import_incremental(args)
    assert rc == 0
    assert called == [], "--no-reindex 仍触发了 generate_index"

    store = DownloadStore(target_repo / ".store.db")
    assert store.get_sha256(fi.filename) == sha
