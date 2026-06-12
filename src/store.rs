use std::path::Path;
use std::sync::Mutex;

use dashmap::DashMap;
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};

pub struct DownloadStore {
    conn: Mutex<Connection>,
}

pub struct FileRecord<'a> {
    pub filename: &'a str,
    pub package_name: &'a str,
    pub version: &'a str,
    pub sha256: &'a str,
    pub size: Option<u64>,
    pub yanked: Option<&'a str>,
}

fn push_if_missing(
    acc: &mut Vec<crate::downloader::FileInfo>,
    fi: &crate::downloader::FileInfo,
    has: bool,
) {
    if !has {
        acc.push(fi.clone());
    }
}

impl DownloadStore {
    pub fn open(db_path: &Path) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(db_path)?;
        Self::migrate(&conn)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS downloaded_files (
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
            );
            CREATE INDEX IF NOT EXISTS idx_dl_package ON downloaded_files(package_name);
            CREATE INDEX IF NOT EXISTS idx_dl_filename ON downloaded_files(filename);",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn table_columns(
        conn: &Connection,
    ) -> Result<std::collections::HashSet<String>, rusqlite::Error> {
        let mut stmt = conn.prepare("PRAGMA table_info(downloaded_files)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        rows.collect()
    }

    fn add_missing_columns(conn: &Connection) -> Result<(), rusqlite::Error> {
        let table_exists = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='downloaded_files'",
                [],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !table_exists {
            // 表尚不存在，后续 CREATE TABLE IF NOT EXISTS 会一次性创建完整结构，
            // 无需也不应执行 ALTER TABLE。
            return Ok(());
        }
        let columns = Self::table_columns(conn)?;
        let missing = [("yanked", "TEXT"), ("metadata_sha256", "TEXT")]
            .into_iter()
            .filter(|(col, _)| !columns.contains(*col));
        for (col, typ) in missing {
            conn.execute(
                &format!("ALTER TABLE downloaded_files ADD COLUMN {col} {typ}"),
                [],
            )?;
        }
        Ok(())
    }

    fn migrate(conn: &Connection) -> Result<(), rusqlite::Error> {
        let old_sql: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='downloaded_files'",
                [],
                |row| row.get(0),
            )
            .ok();
        let is_old = old_sql
            .as_ref()
            .map(|s| s.contains("UNIQUE(filename)"))
            .unwrap_or(false);
        if is_old {
            conn.execute(
                "ALTER TABLE downloaded_files RENAME TO downloaded_files_old",
                [],
            )?;
            conn.execute_batch(
                "CREATE TABLE downloaded_files (
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
                );
                INSERT INTO downloaded_files (filename, package_name, version, sha256, size, metadata_sha256, created_at)
                SELECT filename, package_name, version, sha256, size, metadata_sha256, created_at FROM downloaded_files_old;
                DROP TABLE downloaded_files_old;",
            )?;
        }
        Self::add_missing_columns(conn)?;
        Ok(())
    }

    pub fn add_file(
        &self,
        rec: &FileRecord<'_>,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO downloaded_files (filename, package_name, version, sha256, size, yanked)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![rec.filename, rec.package_name, rec.version, rec.sha256, rec.size, rec.yanked],
        )?;
        Ok(())
    }

    pub fn add_files_batch(
        &self,
        records: &[FileRecord<'_>],
    ) -> Result<(), rusqlite::Error> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        for rec in records {
            tx.execute(
                "INSERT OR IGNORE INTO downloaded_files (filename, package_name, version, sha256, size, yanked)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![rec.filename, rec.package_name, rec.version, rec.sha256, rec.size, rec.yanked],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_all_yanked(&self) -> DashMap<String, String> {
        let conn = self.conn.lock().unwrap();
        Self::collect_hash_map(
            &conn,
            "SELECT filename, yanked FROM downloaded_files WHERE yanked IS NOT NULL AND yanked != ''",
        )
    }

    pub fn set_metadata_sha256(
        &self,
        package_name: &str,
        filename: &str,
        meta_sha256: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloaded_files SET metadata_sha256 = ?1 WHERE package_name = ?2 AND filename = ?3",
            rusqlite::params![meta_sha256, package_name, filename],
        )?;
        Ok(())
    }

    fn collect_hash_map(
        conn: &Connection,
        query: &str,
    ) -> DashMap<String, String> {
        let map = DashMap::new();
        let Ok(mut stmt) = conn.prepare(query) else {
            return map;
        };
        let Ok(rows) = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) else {
            return map;
        };
        for row in rows.flatten() {
            map.insert(row.0, row.1);
        }
        map
    }

    pub fn get_all_hashes(&self) -> DashMap<String, String> {
        let conn = self.conn.lock().unwrap();
        Self::collect_hash_map(
            &conn,
            "SELECT filename, sha256 FROM downloaded_files",
        )
    }

    pub fn get_all_metadata_hashes(&self) -> DashMap<String, String> {
        let conn = self.conn.lock().unwrap();
        Self::collect_hash_map(
            &conn,
            "SELECT filename, metadata_sha256 FROM downloaded_files WHERE metadata_sha256 IS NOT NULL",
        )
    }

    pub fn has_file(
        &self,
        package_name: &str,
        filename: &str,
    ) -> Result<bool, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM downloaded_files WHERE package_name = ?1 AND filename = ?2",
            rusqlite::params![package_name, filename],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    }

    /// Return files that are not yet in the store.
    pub fn filter_missing_files(
        &self,
        files: &[crate::downloader::FileInfo],
    ) -> Result<Vec<crate::downloader::FileInfo>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut result = Vec::new();
        for fi in files {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM downloaded_files WHERE package_name = ?1 AND filename = ?2",
                rusqlite::params![&fi.package_name, &fi.filename],
                |r| r.get(0),
            )?;
            push_if_missing(&mut result, fi, count > 0);
        }
        Ok(result)
    }

    pub fn hash_file(path: &Path) -> Result<String, std::io::Error> {
        let mut hasher = Sha256::new();
        let mut file = std::fs::File::open(path)?;
        std::io::copy(&mut file, &mut hasher)?;
        Ok(format!("{:x}", hasher.finalize()))
    }

    pub(crate) fn handle_hash_result(
        hr: Result<Result<String, std::io::Error>, tokio::task::JoinError>,
    ) -> String {
        match hr {
            Ok(Ok(hash)) => hash,
            Ok(Err(e)) => {
                tracing::warn!("计算文件 hash 失败: {e}");
                String::new()
            }
            Err(e) => {
                tracing::warn!("hash 计算任务 panic: {e}");
                String::new()
            }
        }
    }

    pub async fn record_download(
        &self,
        fi: &crate::downloader::FileInfo,
        dest: &std::path::Path,
    ) {
        let sha256 = if let Some(h) = fi.sha256.clone() {
            h
        } else {
            let dest = dest.to_path_buf();
            let hr =
                tokio::task::spawn_blocking(move || Self::hash_file(&dest))
                    .await;
            Self::handle_hash_result(hr)
        };
        let size = tokio::fs::metadata(dest).await.ok().map(|m| m.len());
        let rec = FileRecord {
            filename: &fi.filename,
            package_name: &fi.package_name,
            version: &fi.version,
            sha256: &sha256,
            size,
            yanked: fi.yanked.as_deref(),
        };
        if let Err(e) = self.add_file(&rec) {
            tracing::warn!("写入 .store.db 失败: {e}");
        }
    }
}
