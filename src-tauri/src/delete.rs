use std::path::PathBuf;

use rusqlite::params;
use serde_json::json;

use crate::db::CatalogDb;
use crate::jobs::JobHub;
use crate::paths;

pub fn get_queued() -> Result<Vec<serde_json::Value>, String> {
    let db = CatalogDb::open()?;
    let mut stmt = db
        .conn
        .prepare(
            "SELECT gm.group_id, gm.file_id, f.path, f.size
             FROM group_members gm
             JOIN files f ON f.id = gm.file_id
             WHERE gm.queued_delete = 1
             ORDER BY gm.group_id, f.path",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "group_id": r.get::<_, i64>(0)?,
                "file_id": r.get::<_, i64>(1)?,
                "path": r.get::<_, String>(2)?,
                "size": r.get::<_, i64>(3)?,
            }))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub fn clear_queue() -> Result<serde_json::Value, String> {
    let db = CatalogDb::open()?;
    let n = db
        .conn
        .execute(
            "UPDATE group_members SET queued_delete = 0 WHERE queued_delete = 1",
            [],
        )
        .map_err(|e| e.to_string())?;
    Ok(json!({ "cleared": n }))
}

pub fn clear_fingerprints() -> Result<serde_json::Value, String> {
    let db = CatalogDb::open()?;
    let n = db
        .conn
        .execute(
            "UPDATE files SET fingerprint=NULL, fingerprint_len=NULL, fingerprint_at=NULL, fp_method=NULL",
            [],
        )
        .map_err(|e| e.to_string())?;
    Ok(json!({ "cleared": n }))
}

pub fn run_soft_delete(hub: &JobHub, job_id: &str) -> Result<serde_json::Value, String> {
    let db = CatalogDb::open()?;
    let trash = db
        .get_setting("trash_dir")?
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(paths::trash_dir);
    std::fs::create_dir_all(&trash).map_err(|e| e.to_string())?;

    let queued = get_queued()?;
    hub.set_progress(
        job_id,
        0,
        queued.len() as i64,
        &format!("moving {} files to trash…", queued.len()),
    )?;

    let mut moved = 0i64;
    let mut failed = 0i64;
    let mut errors = Vec::new();

    for (i, row) in queued.iter().enumerate() {
        if hub.is_cancelled(job_id) {
            break;
        }
        let group_id = row.get("group_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let file_id = row.get("file_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let path = row.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let src = PathBuf::from(path);
        let base = src
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("file-{file_id}"));
        let dest_dir = trash.join(format!("group-{group_id}"));
        let _ = std::fs::create_dir_all(&dest_dir);
        let mut dest = dest_dir.join(&base);
        if dest.exists() {
            dest = dest_dir.join(format!("{}-{base}", chrono_timestamp()));
        }
        match std::fs::rename(&src, &dest) {
            Ok(()) => {
                let db = CatalogDb::open()?;
                let _ = db.conn.execute(
                    "UPDATE group_members SET queued_delete=0 WHERE group_id=?1 AND file_id=?2",
                    params![group_id, file_id],
                );
                let _ = db
                    .conn
                    .execute("UPDATE groups SET reviewed=1 WHERE id=?1", params![group_id]);
                let _ = db
                    .conn
                    .execute("DELETE FROM files WHERE id=?1", params![file_id]);
                moved += 1;
            }
            Err(e) => {
                failed += 1;
                errors.push(json!({ "path": path, "error": e.to_string() }));
            }
        }
        hub.set_progress(
            job_id,
            (i + 1) as i64,
            queued.len() as i64,
            &format!("moved {moved} · failed {failed}"),
        )?;
    }

    Ok(json!({
        "moved": moved,
        "failed": failed,
        "errors": errors,
        "trashRoot": paths::path_to_string(&trash),
    }))
}

fn chrono_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
