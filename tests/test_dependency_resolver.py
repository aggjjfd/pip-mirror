"""F8 回归: extract_extras 是 dependency_resolver 的公开 API."""

from __future__ import annotations

from pip_mirror.dependency_resolver import extract_extras


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
