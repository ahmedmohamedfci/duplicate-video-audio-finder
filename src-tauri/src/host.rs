use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, OnceLock};

use rusqlite::params;
use rust_embed::RustEmbed;
use serde_json::json;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use crate::db::CatalogDb;
use crate::delete;
use crate::fingerprint;
use crate::jobs::JobHub;
use crate::match_dups;
use crate::media;
use crate::paths;
use crate::scan;
use crate::tools_dl;

pub const HOST_ADDR: &str = "127.0.0.1";
pub const HOST_PORT_START: u16 = 3847;
const HOST_PORT_ATTEMPTS: u16 = 32;

static BOUND_PORT: OnceLock<u16> = OnceLock::new();

#[derive(RustEmbed)]
#[folder = "../public/"]
struct DistAssets;

pub fn host_url() -> String {
    let port = BOUND_PORT.get().copied().unwrap_or(HOST_PORT_START);
    format!("http://{HOST_ADDR}:{port}/")
}

pub fn bound_port() -> Option<u16> {
    BOUND_PORT.get().copied()
}

pub fn start(hub: Arc<JobHub>) {
    std::thread::spawn(move || {
        if let Err(err) = run_server(hub) {
            eprintln!("host stopped: {err}");
        }
    });
}

fn bind_server() -> Result<(Server, u16), String> {
    let mut last = String::new();
    for offset in 0..HOST_PORT_ATTEMPTS {
        let port = HOST_PORT_START.saturating_add(offset);
        match Server::http(format!("{HOST_ADDR}:{port}")) {
            Ok(server) => {
                let _ = BOUND_PORT.set(port);
                return Ok((server, port));
            }
            Err(e) => last = format!("{port}: {e}"),
        }
    }
    Err(format!(
        "Cannot bind on {HOST_PORT_START}+ ({last})"
    ))
}

fn run_server(hub: Arc<JobHub>) -> Result<(), String> {
    let (server, port) = bind_server()?;
    eprintln!("Duplicate Finder host listening on {} (port {port})", host_url());

    for mut request in server.incoming_requests() {
        let method = request.method().clone();
        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or("/").to_string();
        let query = url.split('?').nth(1).unwrap_or("").to_string();

        // Stream media off the hot path — never buffer whole files in RAM.
        if matches!(method, Method::Get | Method::Head) && path.starts_with("/api/media/") {
            let id = path.trim_start_matches("/api/media/");
            match id.parse::<i64>() {
                Ok(file_id) => respond_media_stream(request, file_id, method == Method::Head),
                Err(_) => {
                    let _ = request.respond(error_json(400, "bad file id"));
                }
            }
            continue;
        }

        let mut body = Vec::new();
        if matches!(method, Method::Post | Method::Put | Method::Patch) {
            let _ = Read::read_to_end(request.as_reader(), &mut body);
        }

        let response = match route(&hub, &method, &path, &query, &body) {
            Ok(r) => r,
            Err(e) => error_json(500, &e),
        };
        let _ = request.respond(response);
    }
    Ok(())
}

fn route(
    hub: &Arc<JobHub>,
    method: &Method,
    path: &str,
    query: &str,
    body: &[u8],
) -> Result<Response<std::io::Cursor<Vec<u8>>>, String> {
    match (method, path) {
        (Method::Get, "/api/ping") | (Method::Get, "/api/health") => {
            let (ok, missing, paths_json) = media::tools_ok();
            Ok(json_ok(json!({
                "ok": true,
                "tools": {
                    "ok": ok,
                    "missing": missing,
                    "paths": paths_json,
                    "binDir": paths::path_to_string(&paths::bin_dir())
                }
            })))
        }
        (Method::Get, "/api/instance") => Ok(json_ok(json!({
            "dataDir": paths::path_to_string(&paths::data_dir()),
            "dbPath": paths::path_to_string(&paths::db_path()),
        }))),
        (Method::Get, "/api/settings") => {
            let db = CatalogDb::open()?;
            let mut map = serde_json::Map::new();
            let mut stmt = db
                .conn
                .prepare("SELECT key, value FROM settings")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (k, v) = row.map_err(|e| e.to_string())?;
                let parsed = serde_json::from_str::<serde_json::Value>(&v)
                    .unwrap_or(serde_json::Value::String(v));
                map.insert(k, parsed);
            }
            Ok(json_ok(serde_json::Value::Object(map)))
        }
        (Method::Put, "/api/settings") => {
            let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
            let db = CatalogDb::open()?;
            if let Some(roots) = v.get("roots") {
                db.set_setting("roots", &roots.to_string())?;
            }
            if let Some(ext) = v.get("extensions") {
                db.set_setting("extensions", &ext.to_string())?;
            }
            for key in [
                "workers",
                "min_duration_sec",
                "fp_window_sec",
                "min_confidence",
                "trash_dir",
            ] {
                if let Some(val) = v.get(key) {
                    let s = match val {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    db.set_setting(key, &s)?;
                }
            }
            // return full settings
            route(hub, &Method::Get, "/api/settings", "", &[])
        }
        (Method::Get, "/api/stats") => {
            let db = CatalogDb::open()?;
            let files: i64 = db
                .conn
                .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
                .unwrap_or(0);
            let fingerprinted: i64 = db
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM files WHERE fingerprint IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let groups: i64 = db
                .conn
                .query_row("SELECT COUNT(*) FROM groups", [], |r| r.get(0))
                .unwrap_or(0);
            let unreviewed: i64 = db
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM groups WHERE reviewed = 0",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let queued = delete::get_queued().unwrap_or_default();
            let queued_bytes: i64 = queued
                .iter()
                .filter_map(|r| r.get("size").and_then(|v| v.as_i64()))
                .sum();
            Ok(json_ok(json!({
                "files": files,
                "fingerprinted": fingerprinted,
                "groups": groups,
                "unreviewed": unreviewed,
                "queuedDeletes": queued.len(),
                "queuedBytes": queued_bytes,
            })))
        }
        (Method::Post, "/api/jobs/scan") => start_job(hub, "scan", "scanning…", scan::run_scan),
        (Method::Post, "/api/jobs/fingerprint") => {
            start_job(hub, "fingerprint", "fingerprinting…", fingerprint::run_fingerprint)
        }
        (Method::Post, "/api/jobs/match") => {
            start_job(hub, "match", "matching…", match_dups::run_match)
        }
        (Method::Post, "/api/jobs/delete") => {
            start_job(hub, "delete", "moving to trash…", delete::run_soft_delete)
        }
        (Method::Post, "/api/jobs/download-tools") => {
            start_job(
                hub,
                "download-tools",
                "downloading ffmpeg…",
                tools_dl::run_download_tools,
            )
        }
        (Method::Get, "/api/jobs") => Ok(json_ok(json!(hub.list(20)?))),
        (Method::Get, p) if p.starts_with("/api/jobs/") && !p.ends_with("/cancel") && !p.ends_with("/events") => {
            let id = p.trim_start_matches("/api/jobs/");
            Ok(json_ok(hub.get(id)?))
        }
        (Method::Post, p) if p.starts_with("/api/jobs/") && p.ends_with("/cancel") => {
            let id = p
                .trim_start_matches("/api/jobs/")
                .trim_end_matches("/cancel")
                .trim_end_matches('/');
            let cancelled = hub.request_cancel(id);
            Ok(json_ok(json!({ "cancelled": cancelled, "job": hub.get(id).ok() })))
        }
        (Method::Get, "/api/groups") => {
            let min_conf: f64 = query_param(query, "minConfidence")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0)
                / 100.0;
            let unreviewed = query_param(query, "unreviewed").unwrap_or("1") != "0";
            let path_filter = query_param(query, "path")
                .unwrap_or_default()
                .to_ascii_lowercase();
            let db = CatalogDb::open()?;
            let sql = if unreviewed {
                "SELECT * FROM groups WHERE confidence >= ?1 AND reviewed = 0
                 ORDER BY reclaimable_bytes DESC, confidence DESC, id"
            } else {
                "SELECT * FROM groups WHERE confidence >= ?1
                 ORDER BY reclaimable_bytes DESC, confidence DESC, id"
            };
            let mut stmt = db.conn.prepare(sql).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![min_conf], |r| {
                    Ok(json!({
                        "id": r.get::<_, i64>("id")?,
                        "confidence": r.get::<_, f64>("confidence")?,
                        "member_count": r.get::<_, i64>("member_count")?,
                        "reclaimable_bytes": r.get::<_, i64>("reclaimable_bytes")?,
                        "reviewed": r.get::<_, i64>("reviewed")?,
                        "created_at": r.get::<_, String>("created_at")?,
                    }))
                })
                .map_err(|e| e.to_string())?;
            let mut groups = Vec::new();
            for row in rows {
                let g = row.map_err(|e| e.to_string())?;
                if path_filter.is_empty() {
                    groups.push(g);
                    continue;
                }
                let gid = g.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                let has = db
                    .conn
                    .prepare(
                        "SELECT f.path FROM group_members gm JOIN files f ON f.id=gm.file_id WHERE gm.group_id=?1",
                    )
                    .ok()
                    .and_then(|mut s| {
                        s.query_map(params![gid], |r| r.get::<_, String>(0))
                            .ok()
                            .map(|it| {
                                it.filter_map(|x| x.ok())
                                    .any(|p| p.to_ascii_lowercase().contains(&path_filter))
                            })
                    })
                    .unwrap_or(false);
                if has {
                    groups.push(g);
                }
            }
            Ok(json_ok(json!(groups)))
        }
        (Method::Get, p) if p.starts_with("/api/groups/") => {
            let id: i64 = p
                .trim_start_matches("/api/groups/")
                .parse()
                .map_err(|_| "bad group id".to_string())?;
            Ok(json_ok(group_detail(id)?))
        }
        (Method::Patch, p) if p.starts_with("/api/groups/") => {
            let id: i64 = p
                .trim_start_matches("/api/groups/")
                .parse()
                .map_err(|_| "bad group id".to_string())?;
            let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
            let db = CatalogDb::open()?;
            if let Some(keep) = v.get("keepFileId").and_then(|x| x.as_i64()) {
                db.conn
                    .execute(
                        "UPDATE group_members SET is_keep=0 WHERE group_id=?1",
                        params![id],
                    )
                    .map_err(|e| e.to_string())?;
                db.conn
                    .execute(
                        "UPDATE group_members SET is_keep=1, queued_delete=0 WHERE group_id=?1 AND file_id=?2",
                        params![id, keep],
                    )
                    .map_err(|e| e.to_string())?;
            }
            if let Some(reviewed) = v.get("reviewed").and_then(|x| x.as_bool()) {
                db.conn
                    .execute(
                        "UPDATE groups SET reviewed=?1 WHERE id=?2",
                        params![if reviewed { 1 } else { 0 }, id],
                    )
                    .map_err(|e| e.to_string())?;
            }
            if let Some(file_id) = v.get("fileId").and_then(|x| x.as_i64()) {
                if let Some(qd) = v.get("queuedDelete").and_then(|x| x.as_bool()) {
                    db.conn
                        .execute(
                            "UPDATE group_members SET queued_delete=?1, is_keep=CASE WHEN ?1=1 THEN 0 ELSE is_keep END
                             WHERE group_id=?2 AND file_id=?3",
                            params![if qd { 1 } else { 0 }, id, file_id],
                        )
                        .map_err(|e| e.to_string())?;
                }
            }
            Ok(json_ok(group_detail(id)?))
        }
        (Method::Get, "/api/queued-deletes") => Ok(json_ok(json!(delete::get_queued()?))),
        (Method::Post, "/api/queued-deletes/clear") => Ok(json_ok(delete::clear_queue()?)),
        (Method::Post, "/api/fingerprints/clear") => Ok(json_ok(delete::clear_fingerprints()?)),
        (Method::Get, "/") | (Method::Get, "/index.html") => serve_asset("index.html"),
        (Method::Get, p) => {
            let name = p.trim_start_matches('/');
            if name.is_empty() {
                serve_asset("index.html")
            } else {
                serve_asset(name)
            }
        }
        _ => Ok(error_json(404, "not found")),
    }
}

fn start_job<F>(
    hub: &Arc<JobHub>,
    job_type: &str,
    message: &str,
    f: F,
) -> Result<Response<std::io::Cursor<Vec<u8>>>, String>
where
    F: Fn(&JobHub, &str) -> Result<serde_json::Value, String> + Send + 'static,
{
    let id = hub.create(job_type, message)?;
    let hub2 = hub.clone();
    let id2 = id.clone();
    let msg = message.to_string();
    std::thread::spawn(move || {
        let _ = hub2.update(&id2, json!({"status":"running","message": msg}));
        match f(&hub2, &id2) {
            Ok(result) => {
                if hub2.is_cancelled(&id2) {
                    let _ = hub2.cancel_done(&id2, result);
                } else {
                    let _ = hub2.finish(&id2, result);
                }
            }
            Err(e) => {
                let _ = hub2.fail(&id2, &e);
            }
        }
    });
    Ok(json_status(202, hub.get(&id)?))
}

fn group_detail(id: i64) -> Result<serde_json::Value, String> {
    let db = CatalogDb::open()?;
    let group = db
        .conn
        .query_row("SELECT * FROM groups WHERE id=?1", params![id], |r| {
            Ok(json!({
                "id": r.get::<_, i64>("id")?,
                "confidence": r.get::<_, f64>("confidence")?,
                "member_count": r.get::<_, i64>("member_count")?,
                "reclaimable_bytes": r.get::<_, i64>("reclaimable_bytes")?,
                "reviewed": r.get::<_, i64>("reviewed")?,
                "created_at": r.get::<_, String>("created_at")?,
            }))
        })
        .map_err(|_| "group not found".to_string())?;
    let mut stmt = db
        .conn
        .prepare(
            "SELECT gm.*, f.path, f.ext, f.size, f.duration_sec, f.width, f.height, f.mtime_ms, f.error
             FROM group_members gm JOIN files f ON f.id = gm.file_id
             WHERE gm.group_id = ?1
             ORDER BY gm.is_keep DESC, f.size DESC",
        )
        .map_err(|e| e.to_string())?;
    let members: Vec<_> = stmt
        .query_map(params![id], |r| {
            Ok(json!({
                "group_id": r.get::<_, i64>("group_id")?,
                "file_id": r.get::<_, i64>("file_id")?,
                "offset_sec": r.get::<_, f64>("offset_sec")?,
                "is_keep": r.get::<_, i64>("is_keep")?,
                "queued_delete": r.get::<_, i64>("queued_delete")?,
                "path": r.get::<_, String>("path")?,
                "ext": r.get::<_, String>("ext")?,
                "size": r.get::<_, i64>("size")?,
                "duration_sec": r.get::<_, Option<f64>>("duration_sec")?,
                "width": r.get::<_, Option<i64>>("width")?,
                "height": r.get::<_, Option<i64>>("height")?,
                "mtime_ms": r.get::<_, Option<i64>>("mtime_ms")?,
                "error": r.get::<_, Option<String>>("error")?,
            }))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    let mut out = group.as_object().cloned().unwrap_or_default();
    out.insert("members".into(), json!(members));
    Ok(serde_json::Value::Object(out))
}

fn serve_asset(name: &str) -> Result<Response<std::io::Cursor<Vec<u8>>>, String> {
    let data = DistAssets::get(name).ok_or_else(|| format!("missing asset {name}"))?;
    let mime = match name.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    };
    let mut res = Response::from_data(data.data.as_ref().to_vec());
    if let Ok(h) = Header::from_bytes("Content-Type", mime) {
        res.add_header(h);
    }
    Ok(res)
}

fn media_lookup(id: i64) -> Result<(String, &'static str), String> {
    let db = CatalogDb::open()?;
    let (path, ext): (String, String) = db
        .conn
        .query_row(
            "SELECT path, ext FROM files WHERE id=?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "file not found".to_string())?;
    let mime = match ext.to_ascii_lowercase().as_str() {
        ".mp4" | ".m4v" => "video/mp4",
        ".webm" => "video/webm",
        ".mkv" => "video/x-matroska",
        ".mov" => "video/quicktime",
        ".mp3" => "audio/mpeg",
        ".m4a" => "audio/mp4",
        ".wav" => "audio/wav",
        ".flac" => "audio/flac",
        ".ogg" | ".oga" => "audio/ogg",
        _ => "application/octet-stream",
    };
    Ok((path, mime))
}

fn parse_bytes_range(header: &str, file_len: u64) -> Option<(u64, u64)> {
    let s = header.strip_prefix("bytes=")?.trim();
    if s.contains(',') {
        return None; // multi-range not supported
    }
    let (start_s, end_s) = s.split_once('-')?;
    if start_s.is_empty() {
        // suffix: bytes=-N
        let n: u64 = end_s.parse().ok()?;
        if n == 0 || file_len == 0 {
            return None;
        }
        let n = n.min(file_len);
        return Some((file_len - n, file_len - 1));
    }
    let start: u64 = start_s.parse().ok()?;
    if start >= file_len {
        return None;
    }
    let end = if end_s.is_empty() {
        file_len - 1
    } else {
        end_s.parse::<u64>().ok()?.min(file_len - 1)
    };
    if end < start {
        return None;
    }
    Some((start, end))
}

fn header(name: &str, value: &str) -> Option<Header> {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).ok()
}

/// Stream file bytes (optional Range). Never loads the whole file into memory.
fn respond_media_stream(request: Request, id: i64, head_only: bool) {
    let (path, mime) = match media_lookup(id) {
        Ok(v) => v,
        Err(e) => {
            let _ = request.respond(error_json(404, &e));
            return;
        }
    };
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            let _ = request.respond(error_json(500, &format!("open media: {e}")));
            return;
        }
    };
    let file_len = match file.metadata() {
        Ok(m) => m.len(),
        Err(e) => {
            let _ = request.respond(error_json(500, &e.to_string()));
            return;
        }
    };

    let range_hdr = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .map(|h| h.value.as_str().to_string());

    if let Some(range_val) = range_hdr.as_deref() {
        let Some((start, end)) = parse_bytes_range(range_val, file_len) else {
            let mut headers = Vec::new();
            if let Some(h) = header("Content-Type", mime) {
                headers.push(h);
            }
            if let Some(h) = header("Accept-Ranges", "bytes") {
                headers.push(h);
            }
            if let Some(h) = header("Content-Range", &format!("bytes */{file_len}")) {
                headers.push(h);
            }
            let empty: &[u8] = &[];
            let _ = request.respond(Response::new(StatusCode(416), headers, empty, Some(0), None));
            return;
        };

        let content_len = end - start + 1;
        let Ok(content_len_usize) = usize::try_from(content_len) else {
            let _ = request.respond(error_json(500, "range too large"));
            return;
        };
        let mut headers = Vec::new();
        if let Some(h) = header("Content-Type", mime) {
            headers.push(h);
        }
        if let Some(h) = header("Accept-Ranges", "bytes") {
            headers.push(h);
        }
        if let Some(h) = header(
            "Content-Range",
            &format!("bytes {start}-{end}/{file_len}"),
        ) {
            headers.push(h);
        }
        if let Some(h) = header("Content-Length", &content_len.to_string()) {
            headers.push(h);
        }

        if head_only {
            let empty: &[u8] = &[];
            let _ = request.respond(Response::new(
                StatusCode(206),
                headers,
                empty,
                Some(content_len_usize),
                None,
            ));
            return;
        }

        let mut file = file;
        if let Err(e) = file.seek(SeekFrom::Start(start)) {
            let _ = request.respond(error_json(500, &format!("seek: {e}")));
            return;
        }
        let limited = file.take(content_len);
        let _ = request.respond(Response::new(
            StatusCode(206),
            headers,
            limited,
            Some(content_len_usize),
            None,
        ));
        return;
    }

    let file_len_usize = usize::try_from(file_len).unwrap_or(usize::MAX);
    let mut headers = Vec::new();
    if let Some(h) = header("Content-Type", mime) {
        headers.push(h);
    }
    if let Some(h) = header("Accept-Ranges", "bytes") {
        headers.push(h);
    }
    if let Some(h) = header("Content-Length", &file_len.to_string()) {
        headers.push(h);
    }

    if head_only {
        let empty: &[u8] = &[];
        let _ = request.respond(Response::new(
            StatusCode(200),
            headers,
            empty,
            Some(file_len_usize),
            None,
        ));
        return;
    }

    let _ = request.respond(Response::new(
        StatusCode(200),
        headers,
        file,
        Some(file_len_usize),
        None,
    ));
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let mut it = pair.splitn(2, '=');
        let k = it.next()?;
        let v = it.next().unwrap_or("");
        if k == key {
            Some(v)
        } else {
            None
        }
    })
}

fn json_ok(v: serde_json::Value) -> Response<std::io::Cursor<Vec<u8>>> {
    json_status(200, v)
}

fn json_status(code: u16, v: serde_json::Value) -> Response<std::io::Cursor<Vec<u8>>> {
    let body = v.to_string().into_bytes();
    let mut res = Response::from_data(body).with_status_code(StatusCode(code));
    if let Ok(h) = Header::from_bytes("Content-Type", "application/json; charset=utf-8") {
        res.add_header(h);
    }
    res
}

fn error_json(code: u16, msg: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    json_status(code, json!({ "error": msg }))
}
