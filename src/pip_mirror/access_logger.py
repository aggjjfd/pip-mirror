"""HTTP 访问日志：记录 IP、时间、下载内容到 SQLite."""

from __future__ import annotations

import logging
import sqlite3
import threading
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

logger = logging.getLogger("pip-mirror")

_INIT_SQL = """
CREATE TABLE IF NOT EXISTS access_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp   TEXT    NOT NULL,
    client_ip   TEXT    NOT NULL,
    method      TEXT    NOT NULL,
    path        TEXT    NOT NULL,
    status_code INTEGER NOT NULL,
    user_agent  TEXT,
    bytes_sent  INTEGER,
    referer     TEXT
);

CREATE INDEX IF NOT EXISTS idx_access_log_timestamp
    ON access_log(timestamp);

CREATE INDEX IF NOT EXISTS idx_access_log_client_ip
    ON access_log(client_ip);

CREATE INDEX IF NOT EXISTS idx_access_log_path
    ON access_log(path);
"""


@dataclass
class AccessRecord:
    """单次访问记录."""

    timestamp: str
    client_ip: str
    method: str
    path: str
    status_code: int
    user_agent: str | None = None
    bytes_sent: int | None = None
    referer: str | None = None


class AccessLogger:
    """SQLite 访问日志记录器（线程安全）."""

    def __init__(self, db_path: Path) -> None:
        self._db_path = db_path
        self._local = threading.local()
        self._init_db()

    def _get_conn(self) -> sqlite3.Connection:
        """获取线程本地连接."""
        if not hasattr(self._local, "conn") or self._local.conn is None:
            self._local.conn = sqlite3.connect(str(self._db_path), check_same_thread=False)
        return self._local.conn

    def _init_db(self) -> None:
        """初始化数据库表."""
        self._db_path.parent.mkdir(parents=True, exist_ok=True)
        with sqlite3.connect(str(self._db_path)) as conn:
            conn.executescript(_INIT_SQL)
            conn.commit()
        logger.info(f"访问日志数据库: {self._db_path}")

    def log(self, record: AccessRecord) -> None:
        """记录一次访问."""
        try:
            conn = self._get_conn()
            conn.execute(
                """
                INSERT INTO access_log
                (timestamp, client_ip, method, path, status_code, user_agent, bytes_sent, referer)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    record.timestamp,
                    record.client_ip,
                    record.method,
                    record.path,
                    record.status_code,
                    record.user_agent,
                    record.bytes_sent,
                    record.referer,
                ),
            )
            conn.commit()
        except Exception as e:
            logger.warning(f"访问日志写入失败: {e}")

    def get_stats(self, limit: int = 20) -> list[dict[str, Any]]:
        """获取最近访问统计."""
        with sqlite3.connect(str(self._db_path)) as conn:
            conn.row_factory = sqlite3.Row
            rows = conn.execute(
                """
                SELECT timestamp, client_ip, method, path, status_code, bytes_sent
                FROM access_log
                ORDER BY id DESC
                LIMIT ?
                """,
                (limit,),
            ).fetchall()
            return [dict(row) for row in rows]

    def get_top_ips(self, limit: int = 10) -> list[tuple[str, int]]:
        """获取下载量最多的 IP."""
        with sqlite3.connect(str(self._db_path)) as conn:
            rows = conn.execute(
                """
                SELECT client_ip, COUNT(*) as count
                FROM access_log
                WHERE status_code = 200
                GROUP BY client_ip
                ORDER BY count DESC
                LIMIT ?
                """,
                (limit,),
            ).fetchall()
            return rows

    def get_top_paths(self, limit: int = 20) -> list[tuple[str, int]]:
        """获取下载量最多的路径."""
        with sqlite3.connect(str(self._db_path)) as conn:
            rows = conn.execute(
                """
                SELECT path, COUNT(*) as count
                FROM access_log
                WHERE status_code = 200
                GROUP BY path
                ORDER BY count DESC
                LIMIT ?
                """,
                (limit,),
            ).fetchall()
            return rows

    def get_summary(self) -> dict[str, Any]:
        """获取访问汇总."""
        with sqlite3.connect(str(self._db_path)) as conn:
            total = conn.execute(
                "SELECT COUNT(*) FROM access_log"
            ).fetchone()[0]
            total_200 = conn.execute(
                "SELECT COUNT(*) FROM access_log WHERE status_code = 200"
            ).fetchone()[0]
            unique_ips = conn.execute(
                "SELECT COUNT(DISTINCT client_ip) FROM access_log"
            ).fetchone()[0]
            return {
                "total_requests": total,
                "successful_requests": total_200,
                "unique_ips": unique_ips,
            }
