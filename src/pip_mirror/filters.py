"""Wheel 文件平台过滤逻辑."""

# 接受的平台 tag
# 所有 x86_64 manylinux 标准都接受 —— 老的 manylinux 标准声明
# 更宽的 glibc 兼容范围（向后兼容），同样能在现代 Linux 上运行。
# PEP 600 复合 tag（如 manylinux1_x86_64.manylinux_2_28_x86_64）是 OR 语义，
# 不能因为出现“老”标准就拒绝整个 wheel。
_ACCEPTED_PLATFORMS = {
    # Windows x86 (32-bit)
    "win32",
    # Windows x64 (64-bit)
    "win_amd64",
    # Linux x86_64 - 全系列 manylinux 标准（向后兼容）
    "manylinux1_x86_64",          # PEP 513, glibc 2.5
    "manylinux2010_x86_64",       # PEP 571, glibc 2.12
    "manylinux2014_x86_64",       # PEP 599, glibc 2.17
    "manylinux_2_5_x86_64",       # PEP 600 ≡ manylinux1
    "manylinux_2_12_x86_64",      # PEP 600 ≡ manylinux2010
    "manylinux_2_17_x86_64",      # PEP 600 ≡ manylinux2014
    "manylinux_2_24_x86_64",
    "manylinux_2_28_x86_64",
    "manylinux_2_31_x86_64",
    "manylinux_2_34_x86_64",
    "manylinux_2_35_x86_64",
    "manylinux_2_39_x86_64",
    "linux_x86_64",
    # 通用平台
    "any",
}

# 拒绝的平台 tag（包含这些子串的直接排除）
# 仅排除真正不兼容的架构 / libc，不要在此列出“老旧但 x86_64 兼容”的 manylinux 标准。
_REJECTED_SUBSTRINGS = (
    # ARM 架构
    "aarch64",
    "arm64",
    "armv",
    "arm_",
    "armhf",
    "armel",
    "arm32",
    # musl (排除所有 musllinux)
    "musllinux",
    # macOS
    "macosx",
    # 其他架构
    "s390x",
    "ppc64le",
    "ppc64",
    "riscv64",
    "wasm32",
)


def is_accepted_wheel(filename: str) -> bool:
    """判断一个 wheel 文件名是否匹配接受的平台.

    支持复合 platform tag（如 manylinux_2_17_x86_64.manylinux2014_x86_64），
    只要其中任意一个子 tag 被接受且没有子 tag 被拒绝，就接受.

    Args:
        filename: wheel 文件名

    Returns:
        True 如果平台被接受
    """
    if not filename.endswith(".whl"):
        return False

    parts = filename[:-4].split("-")
    if len(parts) < 5:
        return False

    platform_tag = parts[-1]
    sub_tags = platform_tag.split(".")

    # 先检查所有子 tag 是否包含被拒绝的
    for sub in sub_tags:
        for rejected in _REJECTED_SUBSTRINGS:
            if rejected in sub:
                return False

    # 再检查是否有任意子 tag 被接受
    for sub in sub_tags:
        if sub in _ACCEPTED_PLATFORMS:
            return True

    return False


def is_pure_python_wheel(filename: str) -> bool:
    """判断是否为纯 Python wheel（platform tag 为 any）."""
    if not filename.endswith(".whl"):
        return False

    parts = filename[:-4].split("-")
    if len(parts) < 5:
        return False

    return parts[-1] == "any"


def normalize_package_name(name: str) -> str:
    """PEP 503 包名规范化: 小写, 将 _ . 替换为 -."""
    return name.lower().replace("_", "-").replace(".", "-")


def is_source_distribution(filename: str) -> bool:
    """判断是否为源码分发包 (sdist)."""
    return filename.endswith((".tar.gz", ".zip", ".tar.bz2", ".tar.xz"))
