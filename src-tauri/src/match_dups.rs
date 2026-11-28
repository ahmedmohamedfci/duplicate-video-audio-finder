use std::collections::{HashMap, HashSet};

use rusqlite::params;
use serde_json::json;

use crate::db::{now_iso, prune_missing_files, CatalogDb};
use crate::jobs::JobHub;
use crate::media::{fingerprint_similarity, unpack_fingerprint};

pub fn run_match(hub: &JobHub, job_id: &str) -> Result<serde_json::Value, String> {
    let db = CatalogDb::open()?;
    let _pruned = prune_missing_files(&db)?;
    let min_duration: f64 = db
        .get_setting("min_duration_sec")?
        .and_then(|s| s.parse().ok())
        .unwrap_or(180.0);
    let min_confidence: f64 = db
        .get_setting("min_confidence")?
        .and_then(|s| s.parse().ok())
        .unwrap_or(85.0)
        / 100.0;

    let mut stmt = db
        .conn
        .prepare(
            "SELECT id, size, duration_sec, fingerprint FROM files
             WHERE fingerprint IS NOT NULL AND duration_sec IS NOT NULL AND duration_sec >= ?1
             ORDER BY id",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<(i64, i64, f64, Vec<u8>)> = stmt
        .query_map(params![min_duration], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    hub.update(
        job_id,
        json!({"status":"running","message": format!("indexing {} fingerprints…", rows.len())}),
    )?;

    let mut fps: HashMap<i64, (Vec<i32>, i64, f64)> = HashMap::new();
    let mut index: HashMap<String, Vec<(i64, usize)>> = HashMap::new();

    for (i, (id, size, duration, blob)) in rows.iter().enumerate() {
        if hub.is_cancelled(job_id) {
            break;
        }
        let fp = unpack_fingerprint(blob);
        for p in (0..fp.len().saturating_sub(1)).step_by(4) {
            let key = format!("{}:{}", fp[p], fp[p + 1]);
            let bucket = index.entry(key).or_default();
            if bucket.len() < 64 {
                bucket.push((*id, p));
            }
        }
        fps.insert(*id, (fp, *size, *duration));
        if i % 100 == 0 {
            hub.set_progress(job_id, i as i64, rows.len() as i64, &format!("indexed {i}/{}", rows.len()))?;
        }
    }

    hub.update(job_id, json!({"message":"finding candidate pairs…"}))?;
    let mut votes: HashMap<(i64, i64), i64> = HashMap::new();
    let mut processed = 0i64;
    for (id, (fp, _, _)) in &fps {
        if hub.is_cancelled(job_id) {
            break;
        }
        let mut scores: HashMap<i64, i64> = HashMap::new();
        for p in (0..fp.len().saturating_sub(1)).step_by(4) {
            let key = format!("{}:{}", fp[p], fp[p + 1]);
            if let Some(bucket) = index.get(&key) {
                for (other, _) in bucket {
                    if *other <= *id {
                        continue;
                    }
                    *scores.entry(*other).or_default() += 1;
                }
            }
        }
        for (other, score) in scores {
            if score >= 3 {
                votes.insert((*id, other), score);
            }
        }
        processed += 1;
        if processed % 50 == 0 {
            hub.set_progress(
                job_id,
                processed,
                fps.len() as i64,
                &format!("candidates from {processed} files"),
            )?;
        }
    }

    let mut pairs: Vec<_> = votes.into_iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1));
    pairs.truncate(50_000);

    hub.update(
        job_id,
        json!({"message": format!("verifying {} candidate pairs…", pairs.len())}),
    )?;

    let mut parent: HashMap<i64, i64> = HashMap::new();
    let find = |parent: &mut HashMap<i64, i64>, x: i64| -> i64 {
        let mut root = *parent.get(&x).unwrap_or(&x);
        while root != *parent.get(&root).unwrap_or(&root) {
            root = *parent.get(&root).unwrap_or(&root);
        }
        parent.insert(x, root);
        root
    };
    let mut pair_confidence: HashMap<(i64, i64), f64> = HashMap::new();
    let mut verified = 0i64;
    for ((a_id, b_id), _) in &pairs {
        if hub.is_cancelled(job_id) {
            break;
        }
        let Some(a) = fps.get(a_id) else { continue };
        let Some(b) = fps.get(b_id) else { continue };
        let d_ratio = a.2.min(b.2) / a.2.max(b.2);
        if d_ratio < 0.65 {
            continue;
        }
        let sim = fingerprint_similarity(&a.0, &b.0);
        if sim >= min_confidence {
            let ra = find(&mut parent, *a_id);
            let rb = find(&mut parent, *b_id);
            if ra != rb {
                parent.insert(ra, rb);
            }
            pair_confidence.insert((*a_id, *b_id), sim);
        }
        verified += 1;
        if verified % 25 == 0 {
            hub.set_progress(
                job_id,
                verified,
                pairs.len() as i64,
                &format!("verified {verified}/{}", pairs.len()),
            )?;
        }
    }

    let mut clusters: HashMap<i64, HashSet<i64>> = HashMap::new();
    for (a_id, b_id) in pair_confidence.keys() {
        let root = find(&mut parent, *a_id);
        clusters.entry(root).or_default().insert(*a_id);
        clusters.entry(root).or_default().insert(*b_id);
    }

    db.conn
        .execute_batch("DELETE FROM group_members; DELETE FROM groups;")
        .map_err(|e| e.to_string())?;

    let mut group_count = 0i64;
    for members in clusters.values() {
        if members.len() < 2 {
            continue;
        }
        let mut files: Vec<(i64, i64, f64, i64, i64)> = Vec::new();
        for id in members {
            if let Ok(row) = db.conn.query_row(
                "SELECT id, size, duration_sec, COALESCE(width,0), COALESCE(height,0) FROM files WHERE id=?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<f64>>(2)?.unwrap_or(0.0), r.get(3)?, r.get(4)?)),
            ) {
                files.push(row);
            }
        }
        files.sort_by(|a, b| {
            let ra = a.3 * a.4;
            let rb = b.3 * b.4;
            rb.cmp(&ra)
                .then(b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
                .then(b.1.cmp(&a.1))
        });
        let mut conf = 0.0;
        let mut conf_n = 0;
        for i in 0..files.len() {
            for j in (i + 1)..files.len() {
                let key = if files[i].0 < files[j].0 {
                    (files[i].0, files[j].0)
                } else {
                    (files[j].0, files[i].0)
                };
                if let Some(s) = pair_confidence.get(&key) {
                    conf += *s;
                    conf_n += 1;
                }
            }
        }
        let confidence = if conf_n > 0 {
            conf / conf_n as f64
        } else {
            min_confidence
        };
        let keep_id = files[0].0;
        let reclaimable: i64 = files.iter().filter(|f| f.0 != keep_id).map(|f| f.1).sum();
        db.conn
            .execute(
                "INSERT INTO groups (confidence, member_count, reclaimable_bytes, reviewed, created_at)
                 VALUES (?1, ?2, ?3, 0, ?4)",
                params![confidence, files.len() as i64, reclaimable, now_iso()],
            )
            .map_err(|e| e.to_string())?;
        let group_id = db.conn.last_insert_rowid();
        for f in &files {
            db.conn
                .execute(
                    "INSERT INTO group_members (group_id, file_id, offset_sec, is_keep, queued_delete)
                     VALUES (?1, ?2, 0, ?3, 0)",
                    params![group_id, f.0, if f.0 == keep_id { 1 } else { 0 }],
                )
                .map_err(|e| e.to_string())?;
        }
        group_count += 1;
    }

    hub.set_progress(
        job_id,
        1,
        1,
        &format!("found {group_count} duplicate groups"),
    )?;
    Ok(json!({
        "files": fps.len(),
        "pairsChecked": verified,
        "groups": group_count,
    }))
}
