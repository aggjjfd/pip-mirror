"""测试 _backfill_one_target 纯函数:对每个 missing target 沿老版本回溯."""

from __future__ import annotations

from pip_mirror.downloader import (
    FileInfo,
    _backfill_one_target,
)


def _wheel(name: str, ver: str, plat: str) -> FileInfo:
    """构造 wheel FileInfo,文件名 {name}-{ver}-cp312-cp312-{plat}.whl;plat=any 用 py3-none-any."""
    if plat == "any":
        filename = f"{name}-{ver}-py3-none-any.whl"
    else:
        filename = f"{name}-{ver}-cp312-cp312-{plat}.whl"
    return FileInfo(
        filename=filename,
        url=f"https://example.com/{filename}",
        sha256="0" * 64,
        package_name=name,
        version=ver,
    )


def _group(files: list[FileInfo]) -> dict[str, list[FileInfo]]:
    g: dict[str, list[FileInfo]] = {}
    for fi in files:
        if fi.version:
            g.setdefault(fi.version, []).append(fi)
    return g


def test_backfill_finds_old_win32():
    """selected 5 版无 win32,旧 0.9 有 win32 → extra 含 0.9 的 win32 wheel."""
    older = [
        _wheel("p", "0.9", "linux_x86_64"),
        _wheel("p", "0.9", "win_amd64"),
        _wheel("p", "0.9", "win32"),
    ]
    extra, is_pre = _backfill_one_target(
        "win32", ["0.9"], _group(older)
    )
    assert extra is not None
    assert any(fi.filename.endswith("-win32.whl") for fi in extra)
    assert all(fi.version == "0.9" for fi in extra)
    assert is_pre is False


def test_backfill_no_history_returns_none():
    """整个历史无 win32 → 返回 None."""
    older = [
        _wheel("p", "0.9", "linux_x86_64"),
        _wheel("p", "0.9", "win_amd64"),
        _wheel("p", "0.5", "linux_x86_64"),
    ]
    extra, is_pre = _backfill_one_target(
        "win32", ["0.9", "0.5"], _group(older)
    )
    assert extra is None
    assert is_pre is False


def test_backfill_multi_target_independent():
    """win32 命中 0.8,linux_x86_64 命中 0.5 → 各自独立回溯."""
    older = [
        _wheel("p", "0.9", "win_amd64"),
        _wheel("p", "0.8", "win_amd64"),
        _wheel("p", "0.8", "win32"),
        _wheel("p", "0.5", "win_amd64"),
        _wheel("p", "0.5", "linux_x86_64"),
    ]
    extra_win32, _ = _backfill_one_target("win32", ["0.9", "0.8", "0.5"], _group(older))
    extra_linux, _ = _backfill_one_target("linux_x86_64", ["0.9", "0.8", "0.5"], _group(older))
    assert extra_win32 is not None
    assert any(fi.filename.endswith("-win32.whl") for fi in extra_win32)
    assert extra_linux is not None
    assert any(fi.filename.endswith("-linux_x86_64.whl") for fi in extra_linux)


def test_backfill_respects_older_versions_order():
    """older_versions 是降序,应命中第一个符合条件的版本."""
    older = [
        _wheel("p", "0.9", "linux_x86_64"),
        _wheel("p", "0.8", "win32"),
        _wheel("p", "0.7", "win32"),
    ]
    extra, _ = _backfill_one_target("win32", ["0.9", "0.8", "0.7"], _group(older))
    assert extra is not None
    assert all(fi.version == "0.8" for fi in extra)
