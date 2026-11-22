use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::paths;

#[cfg(windows)]
fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_window(_cmd: &mut Command) {}

fn exe_name(tool: &str) -> String {
    if cfg!(windows) {
        format!("{tool}.exe")
    } else {
        tool.to_string()
    }
}

/// Prefer `data/bin/<tool>.exe`, then PATH.
pub fn resolve_tool(tool: &str) -> PathBuf {
    let local = paths::bin_dir().join(exe_name(tool));
    if local.is_file() {
        return local;
    }
    PathBuf::from(exe_name(tool))
}

fn tool_exists(tool: &str) -> bool {
    let path = resolve_tool(tool);
    let mut c = Command::new(&path);
    c.arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_window(&mut c);
    match c.output() {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => false,
    }
}

pub fn tools_ok() -> (bool, Vec<String>, serde_json::Value) {
    let mut missing = Vec::new();
    let mut resolved = serde_json::Map::new();
    for tool in ["ffprobe", "ffmpeg", "fpcalc"] {
        let path = resolve_tool(tool);
        resolved.insert(
            tool.to_string(),
            serde_json::Value::String(path.to_string_lossy().into_owned()),
        );
        if !tool_exists(tool) {
            missing.push(tool.to_string());
        }
    }
    (
        missing.is_empty(),
        missing,
        serde_json::Value::Object(resolved),
    )
}

pub struct Probe {
    pub duration_sec: Option<f64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
}

pub fn probe_file(path: &Path) -> Result<Probe, String> {
    let path_str = path.to_str().ok_or("invalid path")?;
    let mut c = Command::new(resolve_tool("ffprobe"));
    c.args([
        "-v",
        "error",
        "-show_entries",
        "format=duration:stream=width,height,codec_type",
        "-of",
        "json",
        path_str,
    ])
    .stdin(Stdio::null());
    hide_window(&mut c);
    let out = c.output().map_err(|e| format!("ffprobe: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let duration = v
        .pointer("/format/duration")
        .and_then(|x| x.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| v.pointer("/format/duration").and_then(|x| x.as_f64()));
    let mut width = None;
    let mut height = None;
    if let Some(arr) = v.get("streams").and_then(|s| s.as_array()) {
        for s in arr {
            if s.get("codec_type").and_then(|t| t.as_str()) == Some("video") {
                width = s.get("width").and_then(|x| x.as_i64());
                height = s.get("height").and_then(|x| x.as_i64());
                break;
            }
        }
    }
    Ok(Probe {
        duration_sec: duration,
        width,
        height,
    })
}

/// Fingerprint a fixed center window via ffmpeg → fpcalc pipe.
pub fn fingerprint_center(
    path: &Path,
    duration_sec: f64,
    window_sec: f64,
) -> Result<Vec<i32>, String> {
    let length = window_sec.max(5.0).min(duration_sec.max(5.0));
    let start = (duration_sec / 2.0 - length / 2.0).max(0.0);
    let path_str = path.to_str().ok_or("invalid path")?;

    let mut ff = Command::new(resolve_tool("ffmpeg"));
    ff.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-ss",
        &format!("{start}"),
        "-t",
        &format!("{length}"),
        "-i",
        path_str,
        "-map",
        "0:a:0?",
        "-vn",
        "-ac",
        "1",
        "-ar",
        "11025",
        "-c:a",
        "pcm_s16le",
        "-f",
        "wav",
        "pipe:1",
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    hide_window(&mut ff);

    let mut fp = Command::new(resolve_tool("fpcalc"));
    fp.args([
        "-raw",
        "-length",
        &format!("{}", length.ceil() as i64 + 5),
        "-",
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    hide_window(&mut fp);

    let mut ff_child = ff.spawn().map_err(|e| format!("ffmpeg: {e}"))?;
    let mut fp_child = fp.spawn().map_err(|e| format!("fpcalc: {e}"))?;

    let mut ff_stdout = ff_child.stdout.take().ok_or("ffmpeg stdout")?;
    let mut fp_stdin = fp_child.stdin.take().ok_or("fpcalc stdin")?;
    std::io::copy(&mut ff_stdout, &mut fp_stdin).map_err(|e| e.to_string())?;
    drop(fp_stdin);

    let ff_status = ff_child.wait().map_err(|e| e.to_string())?;
    let fp_out = fp_child.wait_with_output().map_err(|e| e.to_string())?;
    if !fp_out.status.success() {
        return Err(format!(
            "fpcalc failed (ffmpeg ok={}): {}",
            ff_status.success(),
            String::from_utf8_lossy(&fp_out.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&fp_out.stdout);
    let line = text
        .lines()
        .find(|l| l.starts_with("FINGERPRINT="))
        .ok_or_else(|| format!("no fingerprint in output: {text}"))?;
    let values = line
        .trim_start_matches("FINGERPRINT=")
        .split(',')
        .filter_map(|s| s.trim().parse::<i32>().ok())
        .collect::<Vec<_>>();
    if values.is_empty() {
        return Err("empty fingerprint".into());
    }
    Ok(values)
}

pub fn pack_fingerprint(values: &[i32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for &v in values {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out
}

pub fn unpack_fingerprint(bytes: &[u8]) -> Vec<i32> {
    bytes
        .chunks_exact(4)
        .map(|c| i32::from_be_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

pub fn fingerprint_similarity(a: &[i32], b: &[i32]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let max_off: i32 = 8;
    let mut best: f64 = 0.0;
    for off in -max_off..=max_off {
        let mut matched = 0usize;
        let mut compared = 0usize;
        for (i, &sv) in short.iter().enumerate() {
            let j = i as i32 + off;
            if j < 0 || j as usize >= long.len() {
                continue;
            }
            compared += 1;
            let xor = (sv as u32) ^ (long[j as usize] as u32);
            if bit_count(xor) <= 8 {
                matched += 1;
            }
        }
        if compared > 0 {
            best = best.max(matched as f64 / compared as f64);
        }
    }
    best
}

fn bit_count(mut n: u32) -> u32 {
    n = n - ((n >> 1) & 0x5555_5555);
    n = (n & 0x3333_3333) + ((n >> 2) & 0x3333_3333);
    (((n + (n >> 4)) & 0x0f0f_0f0f).wrapping_mul(0x0101_0101)) >> 24
}
