"""F8 回归: extract_extras 是 dependency_resolver 的公开 API."""

from __future__ import annotations

from typing import Any
from unittest.mock import MagicMock, patch

import pytest

from pip_mirror.dependency_resolver import (
    TargetEnv,
    _compute_version_windows,
    _filter_versions,
    _get_all_versions,
    _parse_requires_dist,
    extract_extras,
    resolve_dependencies,
)


def test_extract_extras_no_brackets() -> None:
    name, extras = extract_extras("requests")
    assert name == "requests"
    assert extras == set()


def test_extract_extras_single() -> None:
    name, extras = extract_extras("markitdown[pptx]")
    assert name == "markitdown"
    assert extras == {"pptx"}


def test_extract_extras_multi() -> None:
    name, extras = extract_extras("markitdown[pptx,docx,xls]")
    assert name == "markitdown"
    assert extras == {"pptx", "docx", "xls"}


def test_extract_extras_strips_whitespace() -> None:
    name, extras = extract_extras("pkg[a, b , c]")
    assert name == "pkg"
    assert extras == {"a", "b", "c"}


# ---------- prerelease 过滤行为 ----------


class _FakeResp:
    def __init__(self, payload: dict[str, Any]) -> None:
        self._payload = payload

    def raise_for_status(self) -> None:  # noqa: D401
        pass

    def json(self) -> dict[str, Any]:
        return self._payload


class _FakeSession:
    def __init__(self, payload: dict[str, Any]) -> None:
        self._payload = payload

    def get(self, url: str, timeout: int = 30) -> _FakeResp:  # noqa: ARG002
        return _FakeResp(self._payload)


def _make_session_with(versions: list[str]) -> _FakeSession:
    return _FakeSession({"releases": {v: [] for v in versions}})


def test_get_all_versions_drops_prereleases_by_default() -> None:
    """默认 allow_prerelease=False: rc / a / b / dev 全部被过滤."""
    session = _make_session_with(["1.0.0", "2.0.0rc1", "3.0.0a1", "4.0.0.dev1", "1.5.0"])

    versions = _get_all_versions(session, "pkg", "https://pypi.org")  # type: ignore[arg-type]

    assert versions == ["1.5.0", "1.0.0"]


def test_get_all_versions_keeps_post_release() -> None:
    """post release 不算 prerelease, 必须保留."""
    session = _make_session_with(["1.0.0", "1.0.0.post1", "2.0.0rc1"])

    versions = _get_all_versions(session, "pkg", "https://pypi.org")  # type: ignore[arg-type]

    assert "1.0.0.post1" in versions
    assert "2.0.0rc1" not in versions


def test_get_all_versions_allow_prerelease_keeps_all() -> None:
    """allow_prerelease=True: 全部保留."""
    session = _make_session_with(["1.0.0", "2.0.0rc1", "3.0.0a1"])

    versions = _get_all_versions(
        session, "pkg", "https://pypi.org", allow_prerelease=True,  # type: ignore[arg-type]
    )

    assert set(versions) == {"1.0.0", "2.0.0rc1", "3.0.0a1"}


def test_get_all_versions_fallback_when_only_prereleases(
    caplog: pytest.LogCaptureFixture,
) -> None:
    """fallback: 仅有预发行版时回退保留全部, 并打 WARNING 日志(不静默)."""
    session = _make_session_with(["1.0.0a1", "2.0.0rc1", "3.0.0.dev1"])

    with caplog.at_level("WARNING", logger="pip-mirror"):
        versions = _get_all_versions(session, "only-pre", "https://pypi.org")  # type: ignore[arg-type]

    assert set(versions) == {"1.0.0a1", "2.0.0rc1", "3.0.0.dev1"}
    assert any(
        "only-pre" in rec.message and "仅有预发行版" in rec.message and "回退" in rec.message
        for rec in caplog.records
    ), f"应有 fallback warning 日志, got: {[r.message for r in caplog.records]}"


# ---------- _filter_versions 对矛盾 spec 返回空 ----------


def test_filter_versions_conflicting_spec() -> None:
    """矛盾约束(如 >=2.0 且 <1.0)导致过滤后版本为空."""
    versions = ["2.0.0", "1.26.0", "1.21.0", "0.9.0"]
    result = _filter_versions(versions, ">=2.0,<1.0")
    assert result == []


def test_filter_versions_compatible_release() -> None:
    """==1.* 是 compatible release,应匹配 1.x 系列版本."""
    versions = ["2.0.0", "1.1.0", "1.0.0", "0.9.0"]
    result = _filter_versions(versions, "==1.*")
    assert result == ["1.1.0", "1.0.0"]


# ---------- _parse_requires_dist 按 target 环境过滤 ----------


def test_parse_requires_dist_filters_by_target() -> None:
    """传入 target 时,只保留匹配该 target 的约束."""
    reqs = [
        'dep>=1.0; python_version >= "3.11"',
        'dep<1.0; python_version < "3.11"',
        'other; extra == "dev"',
        'windep; sys_platform == "win32"',
    ]
    target = TargetEnv("3.12", "3.12.0", "linux", "x86_64")
    deps = _parse_requires_dist(reqs, extras=set(), target=target)
    specs = {d.specifier for d in deps}
    assert ">=1.0" in specs
    assert "<1.0" not in specs
    # win32 约束在 linux target 下被过滤
    assert not any(d.name == "windep" for d in deps)

    target_win = TargetEnv("3.9", "3.9.0", "win32", "AMD64")
    deps_win = _parse_requires_dist(reqs, extras=set(), target=target_win)
    specs_win = {d.specifier for d in deps_win}
    assert "<1.0" in specs_win
    assert ">=1.0" not in specs_win
    assert any(d.name == "windep" for d in deps_win)


def test_parse_requires_dist_no_target_keeps_all() -> None:
    """不传 target 时保留所有约束(旧行为)."""
    reqs = [
        'dep>=1.0; python_version >= "3.11"',
        'dep<1.0; python_version < "3.11"',
    ]
    deps = _parse_requires_dist(reqs, extras=set())
    specs = {d.specifier for d in deps}
    assert specs == {">=1.0", "<1.0"}


# ---------- _compute_version_windows ----------


def test_compute_version_windows_basic() -> None:
    """两个 target 不同解,窗口并集正确."""
    target_solutions = [
        {"dep": "2.0.0"},
        {"dep": "1.0.0"},
    ]
    all_versions = {"dep": ["2.0.0", "1.5.0", "1.0.0", "0.5.0"]}
    result = _compute_version_windows(target_solutions, all_versions, max_versions=3)
    # target1(2.0.0) 窗口 = [2.0.0, 1.5.0, 1.0.0]
    # target2(1.0.0) 窗口 = [1.5.0, 1.0.0, 0.5.0]
    # 并集 = [2.0.0, 1.5.0, 1.0.0, 0.5.0]
    assert result["dep"] == ["2.0.0", "1.5.0", "1.0.0", "0.5.0"]


def test_compute_version_windows_overlap() -> None:
    """两个 target 解相同,窗口重叠,结果不应重复."""
    target_solutions = [
        {"dep": "1.5.0"},
        {"dep": "1.5.0"},
    ]
    all_versions = {"dep": ["2.0.0", "1.5.0", "1.0.0", "0.5.0"]}
    result = _compute_version_windows(target_solutions, all_versions, max_versions=3)
    # 窗口 = [2.0.0, 1.5.0, 1.0.0]
    assert result["dep"] == ["2.0.0", "1.5.0", "1.0.0"]


def test_compute_version_windows_not_in_all_versions() -> None:
    """solution version 不在 all_versions 中,只保留该版本本身."""
    target_solutions = [{"dep": "9.9.9"}]
    all_versions = {"dep": ["2.0.0", "1.0.0"]}
    result = _compute_version_windows(target_solutions, all_versions, max_versions=3)
    assert result["dep"] == ["9.9.9"]


# ---------- resolve_dependencies 集成 ----------


def test_resolve_dependencies_returns_dict_of_lists(
    caplog: pytest.LogCaptureFixture,
) -> None:
    """新 resolver 返回 dict[str, list[str]],不与旧 ResolvedDep 耦合."""
    with caplog.at_level("WARNING", logger="pip-mirror"), \
            patch("pip_mirror.dependency_resolver._resolve_one_target_sat") as mock_sat, \
            patch("pip_mirror.dependency_resolver._get_all_versions") as mock_versions, \
            patch("pip_mirror.dependency_resolver.make_session") as mock_session:

        mock_sat.return_value = {"dep": "1.0.0"}
        mock_versions.return_value = ["1.0.0"]
        mock_cm = MagicMock()
        mock_cm.__enter__ = MagicMock(return_value=MagicMock())
        mock_cm.__exit__ = MagicMock(return_value=False)
        mock_session.return_value = mock_cm

        result = resolve_dependencies(
            top_packages=["pkg"],
            top_versions={"pkg": ["1.0.0"]},
            pypi_url="https://pypi.org",
            workers=1,
            max_versions=3,
        )

        assert isinstance(result, dict)
        assert "dep" in result
        assert isinstance(result["dep"], list)
        assert result["dep"] == ["1.0.0"]
