use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::paths;

pub struct CatalogDb {
    pub conn: Connection,
}

impl CatalogDb {
    pub fn open() -> Result<Self, String> {
        let path = paths::db_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(&path).map_err(|e| e.to_string())?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA foreign_keys = ON;
            ",
        )
        .map_err(|e| e.to_string())?;
        let db = Self { conn };
        db.migrate()?;
        db.ensure_defaults()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "
            CREATE TABLE IF NOT EXISTS settings (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS files (
              id INTEGER PRIMARY KEY,
              path TEXT NOT NULL UNIQUE,
              root TEXT NOT NULL,
              ext TEXT NOT NULL,
              size INTEGER NOT NULL,
              mtime_ms INTEGER NOT NULL,
              duration_sec REAL,
              width INTEGER,
              height INTEGER,
              fingerprint BLOB,
              fingerprint_len INTEGER,
              fingerprint_at TEXT,
              fp_method TEXT,
              error TEXT,
              updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_files_duration ON files(duration_sec);
            CREATE TABLE IF NOT EXISTS groups (
              id INTEGER PRIMARY KEY,
              confidence REAL NOT NULL,
              member_count INTEGER NOT NULL,
              reclaimable_bytes INTEGER NOT NULL DEFAULT 0,
              reviewed INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS group_members (
              group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
              file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
              offset_sec REAL NOT NULL DEFAULT 0,
              is_keep INTEGER NOT NULL DEFAULT 0,
              queued_delete INTEGER NOT NULL DEFAULT 0,
              PRIMARY KEY (group_id, file_id)
            );
            CREATE TABLE IF NOT EXISTS jobs (
              id TEXT PRIMARY KEY,
              type TEXT NOT NULL,
              status TEXT NOT NULL,
              progress REAL NOT NULL DEFAULT 0,
              total INTEGER NOT NULL DEFAULT 0,
              done INTEGER NOT NULL DEFAULT 0,
              message TEXT NOT NULL DEFAULT '',
              error TEXT,
              result_json TEXT,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            ",
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn ensure_defaults(&self) -> Result<(), String> {
        let defaults = [
            (
                "roots",
                "[]",
            ),
            (
                "extensions",
                r#"[".mp4",".mkv",".avi",".mov",".wmv",".webm",".mp3",".m4a",".aac",".flac",".wav",".ogg",".opus"]"#,
            ),
            ("workers", "4"),
            ("min_duration_sec", "180"),
            ("fp_window_sec", "60"),
            ("min_confidence", "85"),
            (
                "trash_dir",
                &paths::path_to_string(&paths::trash_dir()),
            ),
        ];
        for (k, v) in defaults {
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
                    params![k, v],
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, String> {
        self.conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

pub fn now_iso() -> String {
    // simple UTC-ish local timestamp
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// Delete catalog rows whose paths no longer exist on disk (cascades group_members).
pub fn prune_missing_files(db: &CatalogDb) -> Result<i64, String> {
    let mut stmt = db
        .conn
        .prepare("SELECT id, path FROM files")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);
    let mut removed = 0i64;
    for (id, path) in rows {
        if !Path::new(&path).exists() {
            db.conn
                .execute("DELETE FROM files WHERE id = ?1", params![id])
                .map_err(|e| e.to_string())?;
            removed += 1;
        }
    }
    Ok(removed)
}

