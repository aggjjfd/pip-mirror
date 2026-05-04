"""测试 _backfill_platform_coverage 纯函数:对每个 missing target 沿老版本回溯."""

from __future__ import annotations

from pip_mirror.downloader import (
    FileInfo,
    _backfill_platform_coverage,
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


def test_backfill_finds_old_win32():
    """selected 5 版无 win32,旧 0.9 有 win32 → extra 含 0.9 的 win32 wheel."""
    selected = [
        _wheel("p", "1.5", "linux_x86_64"),
        _wheel("p", "1.5", "win_amd64"),
        _wheel("p", "1.4", "linux_x86_64"),
        _wheel("p", "1.4", "win_amd64"),
    ]
    older = [
        _wheel("p", "0.9", "linux_x86_64"),
        _wheel("p", "0.9", "win_amd64"),
        _wheel("p", "0.9", "win32"),
    ]
    extra, warnings = _backfill_platform_coverage(
        "p", selected, selected + older, spec="", allow_prerelease=False,
    )
    assert any(fi.filename.endswith("-win32.whl") for fi in extra), (
        f"应回溯到 0.9 的 win32 wheel,实际 extras={[fi.filename for fi in extra]}"
    )
    assert all(fi.version == "0.9" for fi in extra)
    assert warnings == []


def test_backfill_no_history_warns():
    """整个历史无 win32 → extra 空,warning 含 win32 + 放弃."""
    selected = [
        _wheel("p", "1.0", "linux_x86_64"),
        _wheel("p", "1.0", "win_amd64"),
    ]
    older = [
        _wheel("p", "0.9", "linux_x86_64"),
        _wheel("p", "0.9", "win_amd64"),
        _wheel("p", "0.5", "linux_x86_64"),
    ]
    extra, warnings = _backfill_platform_coverage(
        "p", selected, selected + older, spec="", allow_prerelease=False,
    )
    assert extra == []
    assert any("win32" in w and "放弃" in w for w in warnings), (
        f"应有 win32 放弃 warning,实际 warnings={warnings}"
    )


def test_backfill_multi_target_different_versions():
    """win32 命中 0.8,linux_x86_64 命中 0.5 → extra 含两个版本."""
    selected = [_wheel("p", "1.0", "win_amd64")]
    older = [
        _wheel("p", "0.9", "win_amd64"),
        _wheel("p", "0.8", "win_amd64"),
        _wheel("p", "0.8", "win32"),
        _wheel("p", "0.5", "win_amd64"),
        _wheel("p", "0.5", "linux_x86_64"),
    ]
    extra, warnings = _backfill_platform_coverage(
        "p", selected, selected + older, spec="", allow_prerelease=False,
    )
    versions = {fi.version for fi in extra}
    assert "0.8" in versions, f"win32 应命中 0.8,extras={[fi.filename for fi in extra]}"
    assert "0.5" in versions, f"linux_x86_64 应命中 0.5,extras={[fi.filename for fi in extra]}"
    assert any(fi.filename.endswith("-win32.whl") for fi in extra)
    assert any(fi.filename.endswith("-linux_x86_64.whl") for fi in extra)
    assert warnings == []


def test_backfill_respects_spec():
    """spec='>=0.9' 阻止扫到 0.5 上的 win32 → extra 空,warning 命中."""
    selected = [
        _wheel("p", "1.0", "linux_x86_64"),
        _wheel("p", "1.0", "win_amd64"),
    ]
    older = [
        _wheel("p", "0.9", "linux_x86_64"),
        _wheel("p", "0.9", "win_amd64"),
        _wheel("p", "0.5", "win32"),
    ]
    extra, warnings = _backfill_platform_coverage(
        "p", selected, selected + older, spec=">=0.9", allow_prerelease=False,
    )
    assert extra == [], f"spec 应过滤掉 0.5,实际 extras={[fi.filename for fi in extra]}"
    assert any("win32" in w for w in warnings)


def test_backfill_skip_pure_python():
    """selected 含 any wheel → 直接 return [],sdist fallback 路径已能救."""
    selected = [_wheel("p", "1.0", "any")]
    older = [
        _wheel("p", "0.5", "win32"),
        _wheel("p", "0.5", "linux_x86_64"),
        _wheel("p", "0.5", "win_amd64"),
    ]
    extra, warnings = _backfill_platform_coverage(
        "p", selected, selected + older, spec="", allow_prerelease=False,
    )
    assert extra == []
    assert warnings == []
