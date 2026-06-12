#!/usr/bin/env python3
"""把 packages/simple 下已存在但 .store.db 里没记录的文件补录进库。

用法：
    python3 scripts/backfill_store.py [repository_dir]

默认 repository_dir 为 ./packages。
"""

import hashlib
import sqlite3
import sys
import tarfile
import zipfile
from pathlib import Path

DEFAULT_REPO = Path("./packages")


def normalize_name(name: str) -> str:
    return name.lower().replace("_", "-").replace(".", "-")


def parse_metadata(body: bytes) -> tuple[str, str] | None:
    name = version = None
    for line in body.decode("utf-8", errors="ignore").splitlines():
        if line.startswith("Name:"):
            name = line.split(":", 1)[1].strip()
        elif line.startswith("Version:"):
            version = line.split(":", 1)[1].strip()
        if name and version:
            break
    if name and version:
        return normalize_name(name), version
    return None


def parse_wheel(path: Path) -> tuple[str, str] | None:
    with zipfile.ZipFile(path) as zf:
        for n in zf.namelist():
            if n.endswith(".dist-info/METADATA"):
                return parse_metadata(zf.read(n))
    return None


def parse_sdist(path: Path) -> tuple[str, str] | None:
    if path.name.endswith(".tar.gz"):
        with tarfile.open(path, "r:gz") as tf:
            for m in tf.getmembers():
                if m.name.endswith("/PKG-INFO") or m.name == "PKG-INFO":
                    f = tf.extractfile(m)
                    if f:
                        return parse_metadata(f.read())
    elif path.name.endswith(".zip"):
        with zipfile.ZipFile(path) as zf:
            for n in zf.namelist():
                if n.endswith("/PKG-INFO") or n == "PKG-INFO":
                    return parse_metadata(zf.read(n))
    return None


def hash_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while True:
            chunk = f.read(65536)
            if not chunk:
                break
            h.update(chunk)
    return h.hexdigest()


def ensure_table(cur: sqlite3.Cursor) -> None:
    cur.execute(
        """
        CREATE TABLE IF NOT EXISTS downloaded_files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            filename TEXT NOT NULL,
            package_name TEXT NOT NULL,
            version TEXT NOT NULL DEFAULT '',
            sha256 TEXT NOT NULL,
            size INTEGER,
            metadata_sha256 TEXT,
            yanked TEXT,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(package_name, filename)
        )
        """
    )


def ensure_columns(cur: sqlite3.Cursor) -> None:
    cur.execute("PRAGMA table_info(downloaded_files)")
    columns = {row[1] for row in cur.fetchall()}
    if "yanked" not in columns:
        cur.execute("ALTER TABLE downloaded_files ADD COLUMN yanked TEXT")
    if "metadata_sha256" not in columns:
        cur.execute("ALTER TABLE downloaded_files ADD COLUMN metadata_sha256 TEXT")


def main() -> None:
    repo = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_REPO
    db_path = repo / ".store.db"
    db_path.parent.mkdir(parents=True, exist_ok=True)

    conn = sqlite3.connect(str(db_path))
    cur = conn.cursor()
    ensure_table(cur)
    ensure_columns(cur)
    conn.commit()

    simple = repo / "simple"
    if not simple.exists():
        print(f"目录不存在: {simple}")
        return

    inserted = 0
    skipped = 0
    failed = 0

    for pkg_dir in sorted(simple.iterdir()):
        if not pkg_dir.is_dir():
            continue
        for f in sorted(pkg_dir.iterdir()):
            if not f.is_file():
                continue
            name = f.name
            try:
                if name.endswith(".whl"):
                    meta = parse_wheel(f)
                elif name.endswith((".tar.gz", ".zip", ".tar.bz2", ".tar.xz")):
                    meta = parse_sdist(f)
                else:
                    continue

                if not meta:
                    print(f"无法解析元数据: {f}")
                    failed += 1
                    continue

                pkg, ver = meta
                sha256 = hash_file(f)
                size = f.stat().st_size

                cur.execute(
                    """
                    INSERT OR IGNORE INTO downloaded_files
                        (filename, package_name, version, sha256, size, yanked)
                    VALUES (?, ?, ?, ?, ?, ?)
                    """,
                    (name, pkg, ver, sha256, size, None),
                )
                if cur.rowcount > 0:
                    inserted += 1
                else:
                    skipped += 1
            except Exception as e:
                print(f"处理失败 {f}: {e}")
                failed += 1

    conn.commit()
    print(f"补录完成: 新增 {inserted} 条, 已存在 {skipped} 条, 失败 {failed} 条")


if __name__ == "__main__":
    main()
