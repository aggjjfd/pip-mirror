#!/usr/bin/env python3
"""解析 Cargo --timings HTML 报告，输出关键瓶颈。"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

from bs4 import BeautifulSoup


def parse_summary(table: BeautifulSoup) -> dict[str, str]:
    """解析 summary-table 里的基础信息。"""
    result: dict[str, str] = {}
    for row in table.find_all("tr"):
        cells = row.find_all("td")
        if len(cells) == 2:
            key = cells[0].get_text(strip=True).rstrip(":")
            result[key] = cells[1].get_text(separator=" ", strip=True)
    return result


def parse_units(table: BeautifulSoup) -> list[dict[str, str]]:
    """解析顶部耗时单元表格。"""
    rows: list[dict[str, str]] = []
    for tr in table.find("tbody").find_all("tr"):
        cells = tr.find_all("td")
        if len(cells) >= 3:
            rows.append({
                "rank": cells[0].get_text(strip=True),
                "unit": cells[1].get_text(separator=" ", strip=True),
                "duration": cells[2].get_text(strip=True),
                "features": cells[5].get_text(strip=True) if len(cells) > 5 else "",
            })
    return rows


def parse_unit_data(script_text: str) -> list[dict]:
    """从 <script> 中提取 UNIT_DATA JSON。"""
    match = re.search(r"const UNIT_DATA = (\[.*?\]);", script_text, re.DOTALL)
    if not match:
        return []
    return json.loads(match.group(1))


def format_seconds(value: str) -> float:
    """把 '46.6s' 或 '112.4s (1m 52.4s)' 转成秒数。"""
    text = value.strip()
    # 优先取最前面的 xxs 部分
    match = re.search(r"(\d+(?:\.\d+)?)\s*s", text)
    if match:
        return float(match.group(1))
    return 0.0


def main() -> int:
    html_path = Path("target/cargo-timings/cargo-timing.html")
    if not html_path.exists():
        print(f"未找到报告文件: {html_path}", file=sys.stderr)
        return 1

    soup = BeautifulSoup(html_path.read_text(encoding="utf-8"), "html.parser")
    tables = soup.find_all("table", class_="my-table")

    summary = parse_summary(tables[0])
    units = parse_units(tables[1])

    script = soup.find("script", string=re.compile(r"const UNIT_DATA"))
    unit_data = parse_unit_data(script.string) if script else []

    print("=" * 60)
    print("Cargo Build Timings 分析报告")
    print("=" * 60)
    for key, value in summary.items():
        print(f"{key:20s}: {value}")
    print()

    # 计算 build.rs / C 编译 / codegen / 链接的占比
    total = format_seconds(summary.get("Total time", "0s"))

    build_script_total = 0.0
    c_compile_total = 0.0
    pip_mirror_bin = 0.0
    pip_mirror_lib = 0.0

    for item in unit_data:
        name = item.get("name", "")
        target = item.get("target", "")
        duration = item.get("duration", 0.0)

        if name == "pip-mirror":
            if target.strip() == 'pip-mirror "bin"':
                pip_mirror_bin = duration
            else:
                pip_mirror_lib = duration
        elif "build-script (run)" in str(item):
            build_script_total += duration
            # 典型的 C 依赖 build script
            if name in ("libsqlite3-sys", "aws-lc-sys", "zstd-sys", "ring"):
                c_compile_total += duration

    # 用表格数据重新计算更准确的 build script 耗时（unit_data 可能不含 sections）
    build_script_sum = sum(
        format_seconds(u["duration"])
        for u in units
        if "build-script (run)" in u["unit"] or "build-script" in u["unit"]
    )

    print("耗时分布（基于 Top 表格）")
    print("-" * 60)
    print(f"{'总耗时':20s}: {total:.1f}s")
    print(f"{'pip-mirror bin':20s}: {pip_mirror_bin:.1f}s ({pip_mirror_bin / total * 100:.1f}%)")
    print(f"{'pip-mirror lib':20s}: {pip_mirror_lib:.1f}s ({pip_mirror_lib / total * 100:.1f}%)")
    print(f"{'build-script 合计':20s}: {build_script_sum:.1f}s ({build_script_sum / total * 100:.1f}%)")
    print()
    print("分析：")
    print(f"  - 整个构建共 {total:.1f}s，其中 {pip_mirror_bin:.1f}s 花在最后的 pip-mirror bin 上，")
    print("    这是 release 模式下全程序 LTO + codegen-units=1 的链接开销。")
    print(f"  - build-script 合计 {build_script_sum:.1f}s，主要是 C/C++ 依赖的现场编译。")
    print(f"  - pip-mirror lib 本身只占 {pip_mirror_lib / total * 100:.1f}%，说明业务代码编译不是瓶颈。")
    print()

    print("Top 20 编译单元")
    print("-" * 60)
    for u in units[:20]:
        pct = format_seconds(u["duration"]) / total * 100
        print(f"{u['rank']:>3s}. {u['unit']:<45s} {u['duration']:>8s} ({pct:5.1f}%)")
    print()
    print("分析：")
    print("  - 第 1 名是 pip-mirror bin（46.6s），是单个体最大的时间块，出现在所有依赖编译完成后。")
    print("  - 第 2~4 名全是 build-script(run)：libsqlite3-sys 40.5s、aws-lc-sys 28.5s、zstd-sys 22.5s。")
    print("    这三个 crate 都在 build.rs 里用 cc crate 编译 C/C++ 源码（SQLite、aws-lc、zstd）。")
    print("  - ring 的 build-script(run) 也花了 4.0s，同样在编译 C/汇编代码。")
    print("  - 第 5 名 pip-mirror lib 14.3s，是项目自身业务代码的编译，占比仅 12.7%。")
    print()

    # 按类别分组
    c_deps = [u for u in units if u["unit"].startswith(("libsqlite3-sys", "aws-lc-sys", "zstd-sys", "ring"))]
    rustls_tls = [u for u in units if any(x in u["unit"] for x in ("rustls", "aws-lc", "ring", "reqwest"))]
    web_framework = [u for u in units if any(x in u["unit"] for x in ("axum", "tower", "hyper", "tokio"))]

    print("重点类别汇总")
    print("-" * 60)
    c_total = sum(format_seconds(u['duration']) for u in c_deps)
    tls_total = sum(format_seconds(u['duration']) for u in rustls_tls)
    web_total = sum(format_seconds(u['duration']) for u in web_framework)
    print(f"C 依赖编译 (SQLite/aws-lc/zstd/ring): {c_total:.1f}s")
    print(f"TLS/网络栈 (rustls/aws-lc/ring/reqwest): {tls_total:.1f}s")
    print(f"Web/异步运行时 (axum/tower/hyper/tokio): {web_total:.1f}s")
    print()
    print("分析：")
    print(f"  - C 依赖编译合计 {c_total:.1f}s，占总时间 {c_total / total * 100:.1f}%，是第一大类开销。")
    print("    其中 libsqlite3-sys（rusqlite bundled）占 36.0%，aws-lc-sys（rustls 的默认密码学后端）占 25.4%。")
    print(f"  - TLS/网络栈合计 {tls_total:.1f}s，占总时间 {tls_total / total * 100:.1f}%。")
    print("    reqwest 默认走了 rustls + aws-lc-rs，而不是系统 native-tls。")
    print(f"  - Web/异步运行时合计 {web_total:.1f}s，占总时间 {web_total / total * 100:.1f}%，")
    print("    tokio full features 拉入了大量模块，但比 C 编译和 LTO 链接轻得多。")
    print()

    return 0


if __name__ == "__main__":
    sys.exit(main())
