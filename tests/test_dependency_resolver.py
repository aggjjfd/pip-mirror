"""F8 回归: extract_extras 是 dependency_resolver 的公开 API."""

from __future__ import annotations

from typing import Any
from unittest.mock import MagicMock, patch

import pytest

from pip_mirror.dependency_resolver import (
    ResolvedDep,
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


# ---------- _parse_requires_dist 按 python_version marker 过滤 ----------


def test_parse_requires_dist_filters_by_python_version() -> None:
    """传入 python_version 时,只保留匹配该版本的约束."""
    reqs = [
        'dep>=1.0; python_version >= "3.11"',
        'dep<1.0; python_version < "3.11"',
        'other; extra == "dev"',
    ]
    # python 3.12 应只保留 >=1.0
    deps = _parse_requires_dist(reqs, extras=set(), python_version="3.12")
    specs = [d.specifier for d in deps]
    assert ">=1.0" in specs
    assert "<1.0" not in specs

    # python 3.9 应只保留 <1.0
    deps = _parse_requires_dist(reqs, extras=set(), python_version="3.9")
    specs = [d.specifier for d in deps]
    assert "<1.0" in specs
    assert ">=1.0" not in specs


def test_parse_requires_dist_no_python_version_keeps_all() -> None:
    """不传 python_version 时保留所有约束(旧行为)."""
    reqs = [
        'dep>=1.0; python_version >= "3.11"',
        'dep<1.0; python_version < "3.11"',
    ]
    deps = _parse_requires_dist(reqs, extras=set())
    specs = {d.specifier for d in deps}
    assert specs == {">=1.0", "<1.0"}


# ---------- resolve_dependencies 按 Python 版本分组,避免跨版本污染 ----------


def test_resolve_per_python_version_isolates_markers(
    caplog: pytest.LogCaptureFixture,
) -> None:
    """python_version marker 在不同版本间互斥,不应 AND 合并成矛盾."""
    with caplog.at_level("WARNING", logger="pip-mirror"), \
            patch("pip_mirror.dependency_resolver._resolve_one_layer") as mock_layer, \
            patch("pip_mirror.dependency_resolver._get_all_versions") as mock_versions, \
            patch("pip_mirror.dependency_resolver.make_session") as mock_session:
        # 模拟 magika 的 requires_dist 结构:
        # py3.12: onnxruntime>=1.21.0
        # py3.14: onnxruntime>=1.24.1
        # 如果不分组 AND 合并,会得到 >=1.21.0,>=1.24.1(不矛盾);
        # 但真实矛盾场景是: py3.9 <1.20.0 vs py3.14 >=1.24.1
        def side_effect(
            packages, pkg_versions, pkg_extras, pypi_url, workers, session,
            python_version=None,
        ):
            if python_version == "3.9":
                return {"onnxruntime": ["<1.20.0", ">=1.17.0"]}
            if python_version == "3.14":
                return {"onnxruntime": [">=1.24.1"]}
            return {}

        mock_layer.side_effect = side_effect
        mock_versions.return_value = ["1.24.1", "1.21.0", "1.17.0"]
        mock_cm = MagicMock()
        mock_cm.__enter__ = MagicMock(return_value=MagicMock())
        mock_cm.__exit__ = MagicMock(return_value=False)
        mock_session.return_value = mock_cm

        result = resolve_dependencies(
            top_packages=["magika"],
            top_versions={"magika": ["1.0.0"]},
            pypi_url="https://pypi.org",
            workers=1,
        )

        assert "onnxruntime" in result
        dep = result["onnxruntime"]
        assert isinstance(dep, ResolvedDep)
        # 3.9 约束 <1.20.0,>=1.17.0 → 过滤后只剩 1.17.0
        # 3.14 约束 >=1.24.1 → 过滤后只剩 1.24.1
        # 并集 = [1.24.1, 1.17.0]
        assert set(dep.versions) == {"1.24.1", "1.17.0"}
        # 没有 "约束矛盾" / "回退" 日志
        assert not any(
            "约束矛盾" in rec.message or "回退" in rec.message
            for rec in caplog.records
        ), f"不应有 fallback warning, got: {[r.message for r in caplog.records]}"

