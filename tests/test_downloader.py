"""F1 + F4 + F5 回归: downloader 内部 helper 与主流程关键路径."""

from __future__ import annotations

import zipfile
from dataclasses import asdict
from pathlib import Path

import pytest
import requests

from pip_mirror import downloader as downloader_mod
from pip_mirror.downloader import (
    DownloadResult,
    FileInfo,
    _drop_prerelease_files,
    _enqueue_or_skip,
    _log_fetch_error,
    download_packages,
)


def _fi(filename: str = "pkg-1.0-py3-none-any.whl", sha: str | None = "abc") -> FileInfo:
    return FileInfo(
        filename=filename,
        url=f"https://example.com/{filename}",
        sha256=sha,
        size=10,
        package_name="pkg",
        version="1.0",
    )


def test_enqueue_or_skip_skips_when_hash_matches(tmp_path: Path) -> None:
    fi = _fi(sha="ABC")
    existing = {"pkg-1.0-py3-none-any.whl": "abc"}
    queue: list[tuple[FileInfo, Path]] = []
    result = DownloadResult()

    _enqueue_or_skip(fi, existing, tmp_path, queue, result)

    assert queue == []
    assert result.skipped == [fi]


def test_enqueue_or_skip_enqueues_when_hash_differs(tmp_path: Path) -> None:
    fi = _fi(sha="abc")
    existing = {"pkg-1.0-py3-none-any.whl": "different"}
    queue: list[tuple[FileInfo, Path]] = []
    result = DownloadResult()

    _enqueue_or_skip(fi, existing, tmp_path, queue, result)

    assert result.skipped == []
    assert queue == [(fi, tmp_path / fi.filename)]


def test_enqueue_or_skip_enqueues_when_no_existing_record(tmp_path: Path) -> None:
    fi = _fi(sha="abc")
    queue: list[tuple[FileInfo, Path]] = []
    result = DownloadResult()

    _enqueue_or_skip(fi, {}, tmp_path, queue, result)

    assert result.skipped == []
    assert queue == [(fi, tmp_path / fi.filename)]


class _FakeResp:
    def __init__(self, status_code: int) -> None:
        self.status_code = status_code


def test_log_fetch_error_404(caplog) -> None:
    exc = requests.HTTPError()
    exc.response = _FakeResp(404)  # type: ignore[attr-defined]
    with caplog.at_level("ERROR", logger="pip-mirror"):
        _log_fetch_error("requests", exc)
    assert any("包不存在" in rec.message and "requests" in rec.message for rec in caplog.records)


def test_log_fetch_error_other(caplog) -> None:
    exc = requests.ConnectionError("network down")
    with caplog.at_level("ERROR", logger="pip-mirror"):
        _log_fetch_error("requests", exc)
    assert any("获取失败" in rec.message and "requests" in rec.message for rec in caplog.records)


def test_file_info_dataclass_roundtrip() -> None:
    """sanity: FileInfo dataclass 字段稳定,避免重命名时引入下游问题."""
    fi = _fi()
    payload = asdict(fi)
    assert payload["filename"] == fi.filename
    assert payload["package_name"] == "pkg"
    assert payload["version"] == "1.0"


def test_metadata_lands_in_correct_package_dir(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """F1 端到端: 多包同步,每个包的 .whl.metadata 必须落在自己包目录,而不是最后一个包目录.

    回滚 F1(把 wheel_path 改回循环外残留的 pkg_dir)时,本测试会失败 —
    因为 pkg-a 的 metadata 会被尝试写到 simple/pkg-b/ 下。
    """
    repo = tmp_path / "packages"

    def fake_fetch_json(session, package_name, pypi_url):
        wheel_name = f"{package_name.replace('-', '_')}-1.0-py3-none-any.whl"
        return [
            FileInfo(
                filename=wheel_name,
                url=f"https://pypi.org/files/{wheel_name}",
                sha256="a" * 64,
                size=100,
                package_name=package_name,
                version="1.0",
            )
        ]

    def fake_download_file(session, file_info: FileInfo, dest_path: Path):
        dest_path.parent.mkdir(parents=True, exist_ok=True)
        dist_info_dir = (
            f"{file_info.package_name.replace('-', '_')}-{file_info.version}.dist-info"
        )
        with zipfile.ZipFile(dest_path, "w") as zf:
            zf.writestr(
                f"{dist_info_dir}/METADATA",
                f"Metadata-Version: 2.1\nName: {file_info.package_name}\n"
                f"Version: {file_info.version}\n",
            )
        return True, ""

    monkeypatch.setattr(downloader_mod, "_fetch_json_api", fake_fetch_json)
    monkeypatch.setattr(downloader_mod, "_download_file", fake_download_file)

    result = download_packages(
        packages=["pkg-a", "pkg-b"],
        repository_dir=repo,
        pypi_url="https://pypi.org",
        index_url="https://pypi.org/simple",
        include_source=False,
        workers=2,
    )

    assert len(result.downloaded) == 2, f"expected 2 wheels downloaded, got {result.downloaded}"

    for name in ("pkg-a", "pkg-b"):
        wheel_name = f"{name.replace('-', '_')}-1.0-py3-none-any.whl"
        wheel = repo / "simple" / name / wheel_name
        meta = repo / "simple" / name / f"{wheel_name}.metadata"
        assert wheel.exists(), f"wheel missing for {name}: {wheel}"
        assert meta.exists(), f"PEP 658 metadata missing for {name}: {meta}"
        text = meta.read_text(encoding="utf-8")
        assert f"Name: {name}" in text, (
            f"metadata of {name} contained wrong package name — F1 regression. text={text!r}"
        )


# ---------- prerelease 过滤行为 ----------


def _wheel(name: str, version: str) -> FileInfo:
    fname = f"{name.replace('-', '_')}-{version}-py3-none-any.whl"
    return FileInfo(
        filename=fname,
        url=f"https://example.com/{fname}",
        sha256="a" * 64,
        size=10,
        package_name=name,
        version=version,
    )


def test_drop_prerelease_files_removes_rc_alpha_beta_dev() -> None:
    files = [
        _wheel("pkg", "1.0.0"),
        _wheel("pkg", "2.0.0rc1"),
        _wheel("pkg", "3.0.0a1"),
        _wheel("pkg", "4.0.0b2"),
        _wheel("pkg", "5.0.0.dev1"),
        _wheel("pkg", "1.5.0"),
    ]
    kept = _drop_prerelease_files(files)
    assert {fi.version for fi in kept} == {"1.0.0", "1.5.0"}


def test_drop_prerelease_files_keeps_post_release() -> None:
    files = [_wheel("pkg", "1.0.0"), _wheel("pkg", "1.0.0.post1")]
    kept = _drop_prerelease_files(files)
    assert {fi.version for fi in kept} == {"1.0.0", "1.0.0.post1"}


def test_download_packages_filters_prerelease_by_default(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """默认 allow_prerelease=False: 顶层包路径过滤掉 rc / dev 版本."""
    repo = tmp_path / "packages"
    versions = ["1.0.0", "2.0.0rc1", "3.0.0", "3.1.0a1"]

    def fake_fetch_json(session, package_name, pypi_url):
        return [_wheel(package_name, v) for v in versions]

    def fake_download_file(session, file_info: FileInfo, dest_path: Path):
        dest_path.parent.mkdir(parents=True, exist_ok=True)
        dist_info = (
            f"{file_info.package_name.replace('-', '_')}-{file_info.version}.dist-info"
        )
        with zipfile.ZipFile(dest_path, "w") as zf:
            zf.writestr(
                f"{dist_info}/METADATA",
                f"Metadata-Version: 2.1\nName: {file_info.package_name}\n"
                f"Version: {file_info.version}\n",
            )
        return True, ""

    monkeypatch.setattr(downloader_mod, "_fetch_json_api", fake_fetch_json)
    monkeypatch.setattr(downloader_mod, "_download_file", fake_download_file)

    result = download_packages(
        packages=["pkg-x"],
        repository_dir=repo,
        pypi_url="https://pypi.org",
        index_url="https://pypi.org/simple",
        include_source=False,
        workers=2,
        max_versions=10,
    )

    downloaded_versions = {fi.version for fi in result.downloaded}
    assert downloaded_versions == {"1.0.0", "3.0.0"}, (
        f"应只下载正式版, got {downloaded_versions}"
    )


def test_download_packages_allow_prerelease_keeps_all(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """allow_prerelease=True: 不过滤."""
    repo = tmp_path / "packages"
    versions = ["1.0.0", "2.0.0rc1"]

    def fake_fetch_json(session, package_name, pypi_url):
        return [_wheel(package_name, v) for v in versions]

    def fake_download_file(session, file_info: FileInfo, dest_path: Path):
        dest_path.parent.mkdir(parents=True, exist_ok=True)
        dist_info = (
            f"{file_info.package_name.replace('-', '_')}-{file_info.version}.dist-info"
        )
        with zipfile.ZipFile(dest_path, "w") as zf:
            zf.writestr(
                f"{dist_info}/METADATA",
                f"Metadata-Version: 2.1\nName: {file_info.package_name}\n"
                f"Version: {file_info.version}\n",
            )
        return True, ""

    monkeypatch.setattr(downloader_mod, "_fetch_json_api", fake_fetch_json)
    monkeypatch.setattr(downloader_mod, "_download_file", fake_download_file)

    result = download_packages(
        packages=["pkg-x"],
        repository_dir=repo,
        pypi_url="https://pypi.org",
        index_url="https://pypi.org/simple",
        include_source=False,
        workers=2,
        max_versions=10,
        allow_prerelease=True,
    )

    assert {fi.version for fi in result.downloaded} == {"1.0.0", "2.0.0rc1"}


def test_download_packages_falls_back_when_only_prereleases(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, caplog: pytest.LogCaptureFixture,
) -> None:
    """fallback: 包仅有预发行版 → 保留全部 + 打 WARNING."""
    repo = tmp_path / "packages"
    versions = ["1.0.0rc1", "2.0.0a1"]

    def fake_fetch_json(session, package_name, pypi_url):
        return [_wheel(package_name, v) for v in versions]

    def fake_download_file(session, file_info: FileInfo, dest_path: Path):
        dest_path.parent.mkdir(parents=True, exist_ok=True)
        dist_info = (
            f"{file_info.package_name.replace('-', '_')}-{file_info.version}.dist-info"
        )
        with zipfile.ZipFile(dest_path, "w") as zf:
            zf.writestr(
                f"{dist_info}/METADATA",
                f"Metadata-Version: 2.1\nName: {file_info.package_name}\n"
                f"Version: {file_info.version}\n",
            )
        return True, ""

    monkeypatch.setattr(downloader_mod, "_fetch_json_api", fake_fetch_json)
    monkeypatch.setattr(downloader_mod, "_download_file", fake_download_file)

    with caplog.at_level("WARNING", logger="pip-mirror"):
        result = download_packages(
            packages=["only-pre"],
            repository_dir=repo,
            pypi_url="https://pypi.org",
            index_url="https://pypi.org/simple",
            include_source=False,
            workers=2,
            max_versions=10,
        )

    assert {fi.version for fi in result.downloaded} == {"1.0.0rc1", "2.0.0a1"}
    assert any(
        "only-pre" in rec.message and "仅有预发行版" in rec.message and "回退" in rec.message
        for rec in caplog.records
    ), f"应有 fallback warning 日志, got: {[r.message for r in caplog.records]}"


def test_download_packages_specific_versions_path_unaffected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """specific_versions 路径不被 prerelease 过滤(尊重显式指定 + resolve 已过滤过)."""
    repo = tmp_path / "packages"
    versions = ["1.0.0", "2.0.0rc1"]

    def fake_fetch_json(session, package_name, pypi_url):
        return [_wheel(package_name, v) for v in versions]

    def fake_download_file(session, file_info: FileInfo, dest_path: Path):
        dest_path.parent.mkdir(parents=True, exist_ok=True)
        dist_info = (
            f"{file_info.package_name.replace('-', '_')}-{file_info.version}.dist-info"
        )
        with zipfile.ZipFile(dest_path, "w") as zf:
            zf.writestr(
                f"{dist_info}/METADATA",
                f"Metadata-Version: 2.1\nName: {file_info.package_name}\n"
                f"Version: {file_info.version}\n",
            )
        return True, ""

    monkeypatch.setattr(downloader_mod, "_fetch_json_api", fake_fetch_json)
    monkeypatch.setattr(downloader_mod, "_download_file", fake_download_file)

    result = download_packages(
        packages=["pkg-x"],
        repository_dir=repo,
        pypi_url="https://pypi.org",
        index_url="https://pypi.org/simple",
        include_source=False,
        workers=2,
        specific_versions={"pkg-x": ["2.0.0rc1"]},
    )

    assert {fi.version for fi in result.downloaded} == {"2.0.0rc1"}
