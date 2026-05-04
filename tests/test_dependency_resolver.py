"""F8 回归: extract_extras 是 dependency_resolver 的公开 API."""

from __future__ import annotations

from typing import Any

import pytest

from pip_mirror.dependency_resolver import _get_all_versions, extract_extras


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

