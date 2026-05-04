"""测试 wheel 平台过滤逻辑."""

from __future__ import annotations

import pytest

from pip_mirror.filters import (
    _ACCEPTED_PLATFORMS,
    is_accepted_wheel,
    is_pure_python_wheel,
    is_source_distribution,
    normalize_package_name,
)


class TestIsAcceptedWheel:
    """测试 is_accepted_wheel 对复合/单一 platform tag 的判定."""

    # ---------- 被接受的 Linux x86_64 wheel ----------
    @pytest.mark.parametrize(
        "filename",
        [
            # 原始 bug: tornado 6.5.5 复合 manylinux tag 包含“老”标准
            # PEP 600 OR 语义，不能因为出现 manylinux1/manylinux_2_5 就拒绝
            (
                "tornado-6.5.5-cp39-abi3-"
                "manylinux1_x86_64.manylinux_2_28_x86_64.manylinux_2_5_x86_64.whl"
            ),
            # 单一标准
            "foo-1.0-py3-none-manylinux1_x86_64.whl",
            "foo-1.0-py3-none-manylinux2010_x86_64.whl",
            "foo-1.0-py3-none-manylinux2014_x86_64.whl",
            "foo-1.0-py3-none-manylinux_2_5_x86_64.whl",
            "foo-1.0-py3-none-manylinux_2_12_x86_64.whl",
            "foo-1.0-py3-none-manylinux_2_17_x86_64.whl",
            "foo-1.0-py3-none-manylinux_2_24_x86_64.whl",
            "foo-1.0-py3-none-manylinux_2_28_x86_64.whl",
            "foo-1.0-py3-none-manylinux_2_39_x86_64.whl",
            "foo-1.0-py3-none-linux_x86_64.whl",
            # Windows
            "foo-1.0-py3-none-win_amd64.whl",
            "foo-1.0-py3-none-win32.whl",
            # 复合接受 tag（两个都是接受的 manylinux 标准）
            "foo-1.0-py3-none-manylinux_2_17_x86_64.manylinux2014_x86_64.whl",
        ],
    )
    def test_accepted(self, filename: str) -> None:
        assert is_accepted_wheel(filename) is True

    # ---------- 被拒绝的 wheel ----------
    @pytest.mark.parametrize(
        "filename",
        [
            # musl
            "foo-1.0-py3-none-musllinux_1_2_x86_64.whl",
            # macOS
            "foo-1.0-py3-none-macosx_10_9_x86_64.whl",
            "foo-1.0-py3-none-macosx_10_9_universal2.whl",
            # ARM
            "foo-1.0-py3-none-manylinux_2_28_aarch64.whl",
            "foo-1.0-py3-none-macosx_11_0_arm64.whl",
            "foo-1.0-py3-none-linux_armv7l.whl",
            "foo-1.0-py3-none-win_arm64.whl",
            # 其他架构
            "foo-1.0-py3-none-manylinux_2_28_s390x.whl",
            "foo-1.0-py3-none-manylinux_2_28_ppc64le.whl",
            "foo-1.0-py3-none-manylinux_2_28_riscv64.whl",
            "foo-1.0-py3-none-wasm32.whl",
            # 复合 tag 中只要包含被拒绝子串，整体拒绝
            (
                "foo-1.0-py3-none-"
                "musllinux_1_2_x86_64.manylinux_2_28_x86_64.whl"
            ),
            # i686 不在接受列表中，也不是纯拒绝子串，最终因无接受 tag 被拒
            "foo-1.0-py3-none-manylinux1_i686.whl",
            "foo-1.0-py3-none-manylinux2014_i686.whl",
        ],
    )
    def test_rejected(self, filename: str) -> None:
        assert is_accepted_wheel(filename) is False

    # ---------- 边界:非 wheel / 格式错误 ----------
    @pytest.mark.parametrize(
        "filename",
        [
            "foo-1.0.tar.gz",
            "foo-1.0.zip",
            "not-a-wheel.txt",
            "short.whl",
            "a-b-c.whl",
        ],
    )
    def test_invalid_filenames(self, filename: str) -> None:
        assert is_accepted_wheel(filename) is False


class TestPurePythonAndSource:
    """测试 is_pure_python_wheel 与 is_source_distribution."""

    def test_pure_python_any(self) -> None:
        assert is_pure_python_wheel("foo-1.0-py3-none-any.whl") is True
        assert is_pure_python_wheel("foo-1.0-py3-none-win_amd64.whl") is False

    def test_is_source_distribution(self) -> None:
        assert is_source_distribution("foo-1.0.tar.gz") is True
        assert is_source_distribution("foo-1.0.zip") is True
        assert is_source_distribution("foo-1.0.tar.bz2") is True
        assert is_source_distribution("foo-1.0.tar.xz") is True
        assert is_source_distribution("foo-1.0-py3-none-any.whl") is False


class TestNormalize:
    """测试包名规范化."""

    @pytest.mark.parametrize(
        "raw,expected",
        [
            ("SomePackage", "somepackage"),
            ("some.package", "some-package"),
            ("some_package", "some-package"),
            ("Some.Package_Name", "some-package-name"),
        ],
    )
    def test_normalize(self, raw: str, expected: str) -> None:
        assert normalize_package_name(raw) == expected


# 额外确保 _ACCEPTED_PLATFORMS 没有拼写错误或空串
def test_accepted_platforms_no_empty() -> None:
    assert "" not in _ACCEPTED_PLATFORMS
    assert all(isinstance(p, str) and p for p in _ACCEPTED_PLATFORMS)
