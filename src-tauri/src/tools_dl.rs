use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use walkdir::WalkDir;

use crate::jobs::JobHub;
use crate::paths;

#[cfg(windows)]
const FFMPEG_URL: &str =
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";
#[cfg(windows)]
const FPCALC_URL: &str =
    "https://github.com/acoustid/chromaprint/releases/download/v1.5.1/chromaprint-fpcalc-1.5.1-windows-x86_64.zip";

#[cfg(not(windows))]
const FFMPEG_URL: &str =
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz";
#[cfg(not(windows))]
const FPCALC_URL: &str =
    "https://github.com/acoustid/chromaprint/releases/download/v1.5.1/chromaprint-fpcalc-1.5.1-linux-x86_64.tar.gz";

pub fn run_download_tools(hub: &JobHub, job_id: &str) -> Result<serde_json::Value, String> {
    let bin = paths::bin_dir();
    fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    let tmp = paths::data_dir().join("_tools_tmp");
    if tmp.exists() {
        let _ = fs::remove_dir_all(&tmp);
    }
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;

    hub.update(
        job_id,
        json!({"status":"running","progress":0.05,"total":3,"done":0,"message":"downloading ffmpeg…"}),
    )?;
    if hub.is_cancelled(job_id) {
        cleanup(&tmp);
        return Ok(json!({"cancelled": true}));
    }

    let ffmpeg_archive = tmp.join("ffmpeg.zip");
    download(FFMPEG_URL, &ffmpeg_archive)?;
    hub.update(
        job_id,
        json!({"progress":0.35,"done":1,"message":"extracting ffmpeg / ffprobe…"}),
    )?;

    let ffmpeg_extract = tmp.join("ffmpeg");
    fs::create_dir_all(&ffmpeg_extract).map_err(|e| e.to_string())?;
    extract_archive(&ffmpeg_archive, &ffmpeg_extract)?;

    let ffmpeg = find_named(&ffmpeg_extract, &exe_name("ffmpeg"))
        .ok_or_else(|| "ffmpeg.exe not found in archive".to_string())?;
    let ffprobe = find_named(&ffmpeg_extract, &exe_name("ffprobe"))
        .ok_or_else(|| "ffprobe.exe not found in archive".to_string())?;
    copy_file(&ffmpeg, &bin.join(exe_name("ffmpeg")))?;
    copy_file(&ffprobe, &bin.join(exe_name("ffprobe")))?;

    if hub.is_cancelled(job_id) {
        cleanup(&tmp);
        return Ok(json!({"cancelled": true}));
    }

    hub.update(
        job_id,
        json!({"progress":0.55,"done":2,"message":"downloading fpcalc…"}),
    )?;
    let fpcalc_archive = tmp.join("fpcalc.zip");
    download(FPCALC_URL, &fpcalc_archive)?;
    hub.update(
        job_id,
        json!({"progress":0.8,"message":"extracting fpcalc…"}),
    )?;

    let fpcalc_extract = tmp.join("fpcalc");
    fs::create_dir_all(&fpcalc_extract).map_err(|e| e.to_string())?;
    extract_archive(&fpcalc_archive, &fpcalc_extract)?;
    let fpcalc = find_named(&fpcalc_extract, &exe_name("fpcalc"))
        .ok_or_else(|| "fpcalc.exe not found in archive".to_string())?;
    copy_file(&fpcalc, &bin.join(exe_name("fpcalc")))?;

    cleanup(&tmp);

    hub.update(
        job_id,
        json!({"progress":1.0,"done":3,"message":"tools ready in data/bin"}),
    )?;

    Ok(json!({
        "binDir": paths::path_to_string(&bin),
        "files": [
            exe_name("ffmpeg"),
            exe_name("ffprobe"),
            exe_name("fpcalc"),
        ]
    }))
}

fn exe_name(tool: &str) -> String {
    if cfg!(windows) {
        format!("{tool}.exe")
    } else {
        tool.to_string()
    }
}

fn download(url: &str, dest: &Path) -> Result<(), String> {
    let status = {
        let mut c = Command::new("curl");
        c.args([
            "-L",
            "--fail",
            "--retry",
            "3",
            "--retry-delay",
            "2",
            "-o",
        ]);
        c.arg(dest);
        c.arg(url);
        hide_window(&mut c);
        c.status()
            .map_err(|e| format!("curl not available ({e}). Install curl or put tools on PATH."))?
    };
    if !status.success() {
        return Err(format!("download failed for {url}"));
    }
    if !dest.is_file() || dest.metadata().map(|m| m.len()).unwrap_or(0) < 1000 {
        return Err(format!("download incomplete: {}", dest.display()));
    }
    Ok(())
}

fn extract_archive(archive: &Path, dest: &Path) -> Result<(), String> {
    let status = {
        let mut c = Command::new("tar");
        c.args(["-xf"]);
        c.arg(archive);
        c.arg("-C");
        c.arg(dest);
        hide_window(&mut c);
        c.status()
            .map_err(|e| format!("tar not available ({e})"))?
    };
    if !status.success() {
        return Err(format!("extract failed: {}", archive.display()));
    }
    Ok(())
}

fn find_named(root: &Path, name: &str) -> Option<PathBuf> {
    let name_l = name.to_ascii_lowercase();
    WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .find(|e| {
            e.file_type().is_file()
                && e.file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&name_l)
        })
        .map(|e| e.path().to_path_buf())
}

fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::copy(from, to).map_err(|e| format!("copy {} → {}: {e}", from.display(), to.display()))?;
    Ok(())
}

fn cleanup(tmp: &Path) {
    let _ = fs::remove_dir_all(tmp);
}

#[cfg(windows)]
fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_window(_cmd: &mut Command) {}
