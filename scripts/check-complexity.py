#!/usr/bin/env python3
"""检查 Rust 源码复杂度指标: 圈复杂度 ≤10, 复杂度评分 ≤67, 文件 NLOC ≤350."""

import json
import subprocess
import sys

MAX_CCN = 10
MAX_SCORE = 67
MAX_FILE_NLOC = 350


def run_lynx(*args: str) -> list[dict]:
    result = subprocess.run(
        ["lynx_eye", "src/", "-r", "--format", "json", *args],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        print(f"ERROR: lynx_eye failed: {result.stderr}", file=sys.stderr)
        sys.exit(1)
    return json.loads(result.stdout)


def main() -> int:
    errors = 0

    # 1. 圈复杂度 > MAX_CCN
    high_ccn = run_lynx("--min-ccn", str(MAX_CCN + 1))
    if high_ccn:
        print(f"ERROR: 圈复杂度超过 {MAX_CCN} 的函数:", file=sys.stderr)
        for item in high_ccn:
            print(
                f"  {item['file']}:{item['start_line']} {item['name']}() "
                f"CCN={item['ccn']} Score={item['complexity_score']:.1f}",
                file=sys.stderr,
            )
        errors += 1

    # 2. 复杂度评分 > MAX_SCORE
    high_score = run_lynx("--min-score", str(MAX_SCORE + 1))
    if high_score:
        print(f"ERROR: 复杂度评分超过 {MAX_SCORE} 的函数:", file=sys.stderr)
        for item in high_score:
            print(
                f"  {item['file']}:{item['start_line']} {item['name']}() "
                f"Score={item['complexity_score']:.1f} CCN={item['ccn']}",
                file=sys.stderr,
            )
        errors += 1

    # 3. 文件 NLOC > MAX_FILE_NLOC
    all_data = run_lynx()
    file_nloc: dict[str, int] = {}
    for item in all_data:
        file_nloc[item["file"]] = file_nloc.get(item["file"], 0) + item["nloc"]

    for fname, nloc in sorted(file_nloc.items()):
        if nloc > MAX_FILE_NLOC:
            print(
                f"ERROR: src/{fname} 非注释代码行数 {nloc} (max {MAX_FILE_NLOC})",
                file=sys.stderr,
            )
            errors += 1

    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
