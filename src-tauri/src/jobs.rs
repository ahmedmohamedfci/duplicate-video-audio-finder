use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::params;
use serde_json::json;
use uuid::Uuid;

use crate::db::{now_iso, CatalogDb};

#[derive(Clone)]
pub struct JobHub {
    cancel: Arc<Mutex<HashMap<String, bool>>>,
    running_types: Arc<Mutex<HashMap<String, String>>>,
}

impl Default for JobHub {
    fn default() -> Self {
        Self {
            cancel: Arc::new(Mutex::new(HashMap::new())),
            running_types: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl JobHub {
    pub fn is_busy(&self, job_type: &str) -> bool {
        self.running_types
            .lock()
            .values()
            .any(|t| t == job_type)
    }

    pub fn create(&self, job_type: &str, message: &str) -> Result<String, String> {
        if self.is_busy(job_type) {
            return Err(format!("{job_type} is already running"));
        }
        let id = Uuid::new_v4().to_string();
        let db = CatalogDb::open()?;
        let ts = now_iso();
        db.conn
            .execute(
                "INSERT INTO jobs (id, type, status, progress, total, done, message, created_at, updated_at)
                 VALUES (?1, ?2, 'queued', 0, 0, 0, ?3, ?4, ?4)",
                params![id, job_type, message, ts],
            )
            .map_err(|e| e.to_string())?;
        self.running_types.lock().insert(id.clone(), job_type.to_string());
        self.cancel.lock().insert(id.clone(), false);
        Ok(id)
    }

    pub fn is_cancelled(&self, id: &str) -> bool {
        *self.cancel.lock().get(id).unwrap_or(&false)
    }

    pub fn request_cancel(&self, id: &str) -> bool {
        if let Some(v) = self.cancel.lock().get_mut(id) {
            *v = true;
            let _ = self.update(id, json!({"message": "cancelling…"}));
            true
        } else {
            false
        }
    }

    pub fn update(&self, id: &str, patch: serde_json::Value) -> Result<(), String> {
        let db = CatalogDb::open()?;
        let row: (String, f64, i64, i64, String, Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT status, progress, total, done, message, error, result_json FROM jobs WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            )
            .map_err(|e| e.to_string())?;

        let status = patch
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or(&row.0);
        let progress = patch
            .get("progress")
            .and_then(|v| v.as_f64())
            .unwrap_or(row.1);
        let total = patch
            .get("total")
            .and_then(|v| v.as_i64())
            .unwrap_or(row.2);
        let done = patch.get("done").and_then(|v| v.as_i64()).unwrap_or(row.3);
        let message = patch
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or(&row.4);
        let error = if patch.get("error").is_some() {
            patch.get("error").and_then(|v| v.as_str()).map(|s| s.to_string())
        } else {
            row.5
        };
        let result_json = if let Some(r) = patch.get("result") {
            Some(r.to_string())
        } else {
            row.6
        };
        let ts = now_iso();
        db.conn
            .execute(
                "UPDATE jobs SET status=?1, progress=?2, total=?3, done=?4, message=?5, error=?6, result_json=?7, updated_at=?8 WHERE id=?9",
                params![status, progress, total, done, message, error, result_json, ts, id],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn set_progress(&self, id: &str, done: i64, total: i64, message: &str) -> Result<(), String> {
        let progress = if total > 0 {
            (done as f64 / total as f64).min(1.0)
        } else {
            0.0
        };
        self.update(
            id,
            json!({
                "status": "running",
                "done": done,
                "total": total,
                "progress": progress,
                "message": message,
            }),
        )
    }

    fn clear_running(&self, id: &str) {
        self.running_types.lock().remove(id);
        self.cancel.lock().remove(id);
    }

    pub fn finish(&self, id: &str, result: serde_json::Value) -> Result<(), String> {
        self.update(
            id,
            json!({
                "status": "done",
                "progress": 1.0,
                "message": "done",
                "result": result,
            }),
        )?;
        self.clear_running(id);
        Ok(())
    }

    pub fn cancel_done(&self, id: &str, result: serde_json::Value) -> Result<(), String> {
        self.update(
            id,
            json!({
                "status": "cancelled",
                "message": "cancelled",
                "result": result,
            }),
        )?;
        self.clear_running(id);
        Ok(())
    }

    pub fn fail(&self, id: &str, err: &str) -> Result<(), String> {
        self.update(
            id,
            json!({
                "status": "error",
                "message": "failed",
                "error": err,
            }),
        )?;
        self.clear_running(id);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<serde_json::Value, String> {
        let db = CatalogDb::open()?;
        db.conn
            .query_row("SELECT * FROM jobs WHERE id = ?1", params![id], |r| {
                let result_json: Option<String> = r.get("result_json")?;
                Ok(json!({
                    "id": r.get::<_, String>("id")?,
                    "type": r.get::<_, String>("type")?,
                    "status": r.get::<_, String>("status")?,
                    "progress": r.get::<_, f64>("progress")?,
                    "total": r.get::<_, i64>("total")?,
                    "done": r.get::<_, i64>("done")?,
                    "message": r.get::<_, String>("message")?,
                    "error": r.get::<_, Option<String>>("error")?,
                    "result": result_json.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
                    "created_at": r.get::<_, String>("created_at")?,
                    "updated_at": r.get::<_, String>("updated_at")?,
                }))
            })
            .map_err(|e| e.to_string())
    }

    pub fn list(&self, limit: i64) -> Result<Vec<serde_json::Value>, String> {
        let db = CatalogDb::open()?;
        let mut stmt = db
            .conn
            .prepare(
                "SELECT id, type, status, progress, total, done, message, error, created_at, updated_at
                 FROM jobs ORDER BY created_at DESC LIMIT ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "type": r.get::<_, String>(1)?,
                    "status": r.get::<_, String>(2)?,
                    "progress": r.get::<_, f64>(3)?,
                    "total": r.get::<_, i64>(4)?,
                    "done": r.get::<_, i64>(5)?,
                    "message": r.get::<_, String>(6)?,
                    "error": r.get::<_, Option<String>>(7)?,
                    "created_at": r.get::<_, String>(8)?,
                    "updated_at": r.get::<_, String>(9)?,
                }))
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }
}
