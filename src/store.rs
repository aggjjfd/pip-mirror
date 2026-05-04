use std::path::Path;
use std::sync::Mutex;

use dashmap::DashMap;
use rusqlite::Connection;
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
}

impl DownloadStore {
    pub fn open(db_path: &Path) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS downloaded_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                filename TEXT NOT NULL UNIQUE,
                package_name TEXT NOT NULL,
                version TEXT NOT NULL DEFAULT '',
                sha256 TEXT NOT NULL,
                size INTEGER,
                metadata_sha256 TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS idx_dl_package ON downloaded_files(package_name);
            CREATE INDEX IF NOT EXISTS idx_dl_filename ON downloaded_files(filename);",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn add_file(
        &self,
        rec: &FileRecord<'_>,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO downloaded_files (filename, package_name, version, sha256, size)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![rec.filename, rec.package_name, rec.version, rec.sha256, rec.size],
        )?;
        Ok(())
    }

    pub fn set_metadata_sha256(
        &self,
        filename: &str,
        meta_sha256: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloaded_files SET metadata_sha256 = ?1 WHERE filename = ?2",
            rusqlite::params![meta_sha256, filename],
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

    pub fn hash_file(path: &Path) -> Result<String, std::io::Error> {
        let mut hasher = Sha256::new();
        let mut file = std::fs::File::open(path)?;
        std::io::copy(&mut file, &mut hasher)?;
        Ok(format!("{:x}", hasher.finalize()))
    }
}
