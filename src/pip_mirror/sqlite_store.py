"""SQLite 存储下载记录."""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class StoredFile:
    """已存储的文件记录."""

    filename: str
    package_name: str
    version: str
    sha256: str
    size: int | None
    downloaded_at: str


class DownloadStore:
    """SQLite 下载记录存储."""

    def __init__(self, db_path: Path):
        self.db_path = db_path
        self._ensure_table()

    def _ensure_table(self) -> None:
        """确保表存在."""
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        with sqlite3.connect(str(self.db_path)) as conn:
            conn.execute("""
                CREATE TABLE IF NOT EXISTS downloaded_files (
                    filename TEXT PRIMARY KEY,
                    package_name TEXT NOT NULL,
                    version TEXT NOT NULL,
                    sha256 TEXT NOT NULL,
                    size INTEGER,
                    downloaded_at TEXT NOT NULL
                )
            """)
            conn.execute("""
                CREATE INDEX IF NOT EXISTS idx_pkg_ver
                ON downloaded_files(package_name, version)
            """)
            conn.execute("""
                CREATE TABLE IF NOT EXISTS file_metadata (
                    filename TEXT PRIMARY KEY,
                    metadata_sha256 TEXT NOT NULL
                )
            """)
            conn.commit()

    def get_sha256(self, filename: str) -> str | None:
        """获取文件的 sha256."""
        with sqlite3.connect(str(self.db_path)) as conn:
            row = conn.execute(
                "SELECT sha256 FROM downloaded_files WHERE filename = ?",
                (filename,),
            ).fetchone()
            return row[0] if row else None

    def get_all_hashes(self) -> dict[str, str]:
        """获取所有文件的 sha256 映射."""
        with sqlite3.connect(str(self.db_path)) as conn:
            rows = conn.execute(
                "SELECT filename, sha256 FROM downloaded_files"
            ).fetchall()
            return {filename: sha256 for filename, sha256 in rows}

    def get_files_by_version(self, package_name: str, version: str) -> list[StoredFile]:
        """获取某个版本的所有文件记录."""
        with sqlite3.connect(str(self.db_path)) as conn:
            rows = conn.execute(
                "SELECT filename, package_name, version, sha256, size, downloaded_at "
                "FROM downloaded_files WHERE package_name = ? AND version = ?",
                (package_name, version),
            ).fetchall()
            return [
                StoredFile(
                    filename=r[0],
                    package_name=r[1],
                    version=r[2],
                    sha256=r[3],
                    size=r[4],
                    downloaded_at=r[5],
                )
                for r in rows
            ]

    def add_file(
        self,
        filename: str,
        package_name: str,
        version: str,
        sha256: str,
        size: int | None,
    ) -> None:
        """添加文件记录."""
        from datetime import datetime, timezone

        downloaded_at = datetime.now(timezone.utc).isoformat()
        with sqlite3.connect(str(self.db_path)) as conn:
            conn.execute(
                "INSERT OR REPLACE INTO downloaded_files "
                "(filename, package_name, version, sha256, size, downloaded_at) "
                "VALUES (?, ?, ?, ?, ?, ?)",
                (filename, package_name, version, sha256, size, downloaded_at),
            )
            conn.commit()

    def remove_file(self, filename: str) -> None:
        """删除文件记录."""
        with sqlite3.connect(str(self.db_path)) as conn:
            conn.execute(
                "DELETE FROM downloaded_files WHERE filename = ?",
                (filename,),
            )
            conn.execute(
                "DELETE FROM file_metadata WHERE filename = ?",
                (filename,),
            )
            conn.commit()

    # --- metadata (PEP 658) ---

    def get_metadata_sha256(self, filename: str) -> str | None:
        """获取文件的 metadata sha256."""
        with sqlite3.connect(str(self.db_path)) as conn:
            row = conn.execute(
                "SELECT metadata_sha256 FROM file_metadata WHERE filename = ?",
                (filename,),
            ).fetchone()
            return row[0] if row else None

    def get_all_metadata_hashes(self) -> dict[str, str]:
        """获取所有文件的 metadata sha256 映射."""
        with sqlite3.connect(str(self.db_path)) as conn:
            rows = conn.execute(
                "SELECT filename, metadata_sha256 FROM file_metadata"
            ).fetchall()
            return {filename: sha256 for filename, sha256 in rows}

    def set_metadata_sha256(self, filename: str, metadata_sha256: str) -> None:
        """设置文件的 metadata sha256."""
        with sqlite3.connect(str(self.db_path)) as conn:
            conn.execute(
                "INSERT OR REPLACE INTO file_metadata (filename, metadata_sha256) "
                "VALUES (?, ?)",
                (filename, metadata_sha256),
            )
            conn.commit()
