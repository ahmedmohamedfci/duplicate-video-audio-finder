use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use rusqlite::params;
use serde_json::json;

use crate::db::{now_iso, CatalogDb};
use crate::jobs::JobHub;
use crate::media::{fingerprint_center, pack_fingerprint, probe_file, tools_ok};

pub fn run_fingerprint(hub: &JobHub, job_id: &str) -> Result<serde_json::Value, String> {
    let (ok, missing, _) = tools_ok();
    if !ok {
        return Err(format!(
            "Missing tools on PATH: {}. Install ffmpeg and chromaprint (fpcalc).",
            missing.join(", ")
        ));
    }

    let db = CatalogDb::open()?;
    let min_duration: f64 = db
        .get_setting("min_duration_sec")?
        .and_then(|s| s.parse().ok())
        .unwrap_or(180.0);
    let workers: usize = db
        .get_setting("workers")?
        .and_then(|s| s.parse().ok())
        .unwrap_or(4)
        .clamp(1, 8);
    let window_sec: f64 = db
        .get_setting("fp_window_sec")?
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(60.0)
        .max(15.0);
    let method = format!("center:{}", window_sec.round() as i64);

    let mut stmt = db
        .conn
        .prepare(
            "SELECT id, path, duration_sec, fingerprint_len, fp_method FROM files ORDER BY id",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<(i64, String, Option<f64>, Option<i64>, Option<String>)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    let todo: Vec<_> = rows
        .into_iter()
        .filter(|(_, _, dur, len, meth)| {
            !(len.unwrap_or(0) > 0 && dur.is_some() && meth.as_deref() == Some(method.as_str()))
        })
        .collect();

    let total = todo.len();
    hub.set_progress(
        job_id,
        0,
        total as i64,
        &format!("center {window_sec}s · {total} files to fingerprint…"),
    )?;

    let done = Arc::new(AtomicUsize::new(0));
    let fingerprinted = Arc::new(AtomicUsize::new(0));
    let skipped = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));

    let chunk = (todo.len() / workers).max(1);
    let mut handles = Vec::new();
    for chunk_rows in todo.chunks(chunk.max(1)) {
        let chunk_rows = chunk_rows.to_vec();
        let hub = hub.clone();
        let job_id = job_id.to_string();
        let method = method.clone();
        let done = done.clone();
        let fingerprinted = fingerprinted.clone();
        let skipped = skipped.clone();
        let failed = failed.clone();
        handles.push(thread::spawn(move || {
            for (id, path, duration_sec, _, _) in chunk_rows {
                if hub.is_cancelled(&job_id) {
                    break;
                }
                let ts = now_iso();
                let result = (|| -> Result<(), String> {
                    let db = CatalogDb::open()?;
                    let mut duration = duration_sec;
                    if duration.is_none() {
                        let probe = probe_file(&PathBuf::from(&path))?;
                        duration = probe.duration_sec;
                        db.conn
                            .execute(
                                "UPDATE files SET duration_sec=?1, width=?2, height=?3, error=NULL, updated_at=?4 WHERE id=?5",
                                params![duration, probe.width, probe.height, ts, id],
                            )
                            .map_err(|e| e.to_string())?;
                    }
                    let Some(dur) = duration else {
                        skipped.fetch_add(1, Ordering::SeqCst);
                        return Ok(());
                    };
                    if dur < min_duration {
                        skipped.fetch_add(1, Ordering::SeqCst);
                        return Ok(());
                    }
                    let values = fingerprint_center(&PathBuf::from(&path), dur, window_sec)?;
                    let blob = pack_fingerprint(&values);
                    db.conn
                        .execute(
                            "UPDATE files SET fingerprint=?1, fingerprint_len=?2, fingerprint_at=?3, fp_method=?4, error=NULL, updated_at=?5 WHERE id=?6",
                            params![blob, values.len() as i64, ts, method, ts, id],
                        )
                        .map_err(|e| e.to_string())?;
                    fingerprinted.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })();
                if let Err(err) = result {
                    failed.fetch_add(1, Ordering::SeqCst);
                    if let Ok(db) = CatalogDb::open() {
                        let _ = db.conn.execute(
                            "UPDATE files SET error=?1, updated_at=?2 WHERE id=?3",
                            params![err, now_iso(), id],
                        );
                    }
                }
                let d = done.fetch_add(1, Ordering::SeqCst) + 1;
                if d % 2 == 0 || d == total {
                    let _ = hub.set_progress(
                        &job_id,
                        d as i64,
                        total as i64,
                        &format!(
                            "fingerprinted {} · skipped short {} · failed {}",
                            fingerprinted.load(Ordering::SeqCst),
                            skipped.load(Ordering::SeqCst),
                            failed.load(Ordering::SeqCst)
                        ),
                    );
                }
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }

    Ok(json!({
        "processed": done.load(Ordering::SeqCst),
        "fingerprinted": fingerprinted.load(Ordering::SeqCst),
        "skippedShort": skipped.load(Ordering::SeqCst),
        "failed": failed.load(Ordering::SeqCst),
        "windowSec": window_sec,
        "method": method,
    }))
}
