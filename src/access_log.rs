use std::sync::Mutex;

use rusqlite::Connection;
use std::path::Path;

pub struct AccessLogger {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone)]
pub struct AccessRecord {
    pub timestamp: String,
    pub client_ip: String,
    pub method: String,
    pub path: String,
    pub status_code: u16,
    pub user_agent: Option<String>,
    pub bytes_sent: Option<u64>,
    pub referer: Option<String>,
}

impl AccessLogger {
    pub fn open(db_path: &Path) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS access_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                client_ip TEXT NOT NULL,
                method TEXT NOT NULL,
                path TEXT NOT NULL,
                status_code INTEGER NOT NULL,
                user_agent TEXT,
                bytes_sent INTEGER,
                referer TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_access_ts ON access_log(timestamp);",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn log(&self, record: &AccessRecord) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO access_log (timestamp, client_ip, method, path, status_code, user_agent, bytes_sent, referer)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                record.timestamp,
                record.client_ip,
                record.method,
                record.path,
                record.status_code,
                record.user_agent,
                record.bytes_sent,
                record.referer,
            ],
        )?;
        Ok(())
    }

    pub fn get_summary(&self) -> Summary {
        let conn = self.conn.lock().unwrap();
        Summary {
            total_requests: conn
                .query_row("SELECT COUNT(*) FROM access_log", [], |r| r.get(0))
                .unwrap_or(0),
            successful_requests: conn
                .query_row(
                    "SELECT COUNT(*) FROM access_log WHERE status_code < 400",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0),
            unique_ips: conn
                .query_row(
                    "SELECT COUNT(DISTINCT client_ip) FROM access_log",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0),
        }
    }

    pub fn get_top_ips(&self, limit: usize) -> Vec<(String, u64)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT client_ip, COUNT(*) as cnt FROM access_log
                 GROUP BY client_ip ORDER BY cnt DESC LIMIT ?1",
            )
            .unwrap();
        stmt.query_map([limit as i64], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .flatten()
            .collect()
    }

    pub fn get_top_paths(&self, limit: usize) -> Vec<(String, u64)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT path, COUNT(*) as cnt FROM access_log
                 GROUP BY path ORDER BY cnt DESC LIMIT ?1",
            )
            .unwrap();
        stmt.query_map([limit as i64], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .flatten()
            .collect()
    }

    fn record_from_row(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<AccessRecord> {
        Ok(AccessRecord {
            timestamp: row.get("timestamp")?,
            client_ip: row.get("client_ip")?,
            method: row.get("method")?,
            path: row.get("path")?,
            status_code: row.get("status_code")?,
            user_agent: row.get("user_agent")?,
            bytes_sent: row.get("bytes_sent")?,
            referer: row.get("referer")?,
        })
    }

    pub fn get_recent(&self, limit: usize) -> Vec<AccessRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT timestamp, client_ip, method, path, status_code, user_agent, bytes_sent, referer FROM access_log ORDER BY id DESC LIMIT ?1").unwrap();
        stmt.query_map([limit as i64], Self::record_from_row)
            .unwrap()
            .flatten()
            .collect()
    }
}

#[derive(Debug, Default)]
pub struct Summary {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub unique_ips: u64,
}
