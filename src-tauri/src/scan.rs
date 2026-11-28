use std::path::{Path, PathBuf};

use rusqlite::params;
use serde_json::json;
use walkdir::WalkDir;

use crate::db::{now_iso, prune_missing_files, CatalogDb};
use crate::jobs::JobHub;

pub fn run_scan(hub: &JobHub, job_id: &str) -> Result<serde_json::Value, String> {
    let db = CatalogDb::open()?;
    let mut removed = prune_missing_files(&db)?;
    let roots_raw = db.get_setting("roots")?.unwrap_or_else(|| "[]".into());
    let exts_raw = db
        .get_setting("extensions")?
        .unwrap_or_else(|| "[]".into());
    let roots: Vec<String> = serde_json::from_str(&roots_raw).unwrap_or_default();
    let extensions: Vec<String> = serde_json::from_str(&exts_raw).unwrap_or_default();
    let ext_set: std::collections::HashSet<String> = extensions
        .into_iter()
        .map(|e| {
            let e = e.trim().to_ascii_lowercase();
            if e.starts_with('.') {
                e
            } else {
                format!(".{e}")
            }
        })
        .collect();

    if roots.is_empty() {
        return Err("Add at least one root folder first".into());
    }
    if ext_set.is_empty() {
        return Err("Add at least one extension".into());
    }

    hub.update(job_id, json!({"status":"running","message":"walking folders…"}))?;

    let mut seen = std::collections::HashSet::<String>::new();
    let mut scanned = 0i64;
    let mut matched = 0i64;
    let updated_at = now_iso();

    for root in &roots {
        if hub.is_cancelled(job_id) {
            break;
        }
        let root_path = PathBuf::from(root);
        if !root_path.exists() {
            return Err(format!("Root not found: {root}"));
        }
        for entry in WalkDir::new(&root_path).into_iter().filter_map(|e| e.ok()) {
            if hub.is_cancelled(job_id) {
                break;
            }
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            scanned += 1;
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| format!(".{}", e.to_ascii_lowercase()))
                .unwrap_or_default();
            if !ext_set.contains(&ext) {
                continue;
            }
            let meta = match std::fs::metadata(path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let full = path_to_string(path);
            matched += 1;
            seen.insert(full.clone());
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            db.conn
                .execute(
                    "INSERT INTO files (path, root, ext, size, mtime_ms, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(path) DO UPDATE SET
                       root=excluded.root,
                       ext=excluded.ext,
                       size=excluded.size,
                       mtime_ms=excluded.mtime_ms,
                       updated_at=excluded.updated_at,
                       fingerprint=CASE WHEN files.size=excluded.size AND files.mtime_ms=excluded.mtime_ms THEN files.fingerprint ELSE NULL END,
                       fingerprint_len=CASE WHEN files.size=excluded.size AND files.mtime_ms=excluded.mtime_ms THEN files.fingerprint_len ELSE NULL END,
                       fingerprint_at=CASE WHEN files.size=excluded.size AND files.mtime_ms=excluded.mtime_ms THEN files.fingerprint_at ELSE NULL END,
                       fp_method=CASE WHEN files.size=excluded.size AND files.mtime_ms=excluded.mtime_ms THEN files.fp_method ELSE NULL END,
                       duration_sec=CASE WHEN files.size=excluded.size AND files.mtime_ms=excluded.mtime_ms THEN files.duration_sec ELSE NULL END,
                       width=CASE WHEN files.size=excluded.size AND files.mtime_ms=excluded.mtime_ms THEN files.width ELSE NULL END,
                       height=CASE WHEN files.size=excluded.size AND files.mtime_ms=excluded.mtime_ms THEN files.height ELSE NULL END,
                       error=CASE WHEN files.size=excluded.size AND files.mtime_ms=excluded.mtime_ms THEN files.error ELSE NULL END",
                    params![full, root, ext, meta.len() as i64, mtime_ms, updated_at],
                )
                .map_err(|e| e.to_string())?;

            if matched % 200 == 0 {
                hub.set_progress(
                    job_id,
                    matched,
                    matched.max(scanned),
                    &format!("indexed {matched} media files (scanned {scanned} entries)"),
                )?;
            }
        }
    }

    // remove vanished (not seen under scanned roots)
    let mut stmt = db
        .conn
        .prepare("SELECT id, path, root FROM files")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(i64, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);
    for (id, path, root) in rows {
        if roots.iter().any(|r| r == &root) && !seen.contains(&path) {
            db.conn
                .execute("DELETE FROM files WHERE id = ?1", params![id])
                .map_err(|e| e.to_string())?;
            removed += 1;
        }
    }

    let total: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    hub.set_progress(
        job_id,
        matched,
        matched,
        &format!("scan complete · {total} files in catalog"),
    )?;
    Ok(json!({ "scanned": scanned, "matched": matched, "removed": removed, "total": total }))
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\")
}
