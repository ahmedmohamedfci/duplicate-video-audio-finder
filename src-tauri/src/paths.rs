use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// `--data` / `--data-dir` / `-d` / `DUPFINDER_DATA`
pub fn init_from_cli_and_env() -> PathBuf {
    if let Some(existing) = DATA_DIR.get() {
        return existing.clone();
    }
    let override_dir = parse_data_arg().or_else(|| {
        std::env::var("DUPFINDER_DATA")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
    });
    let dir = if let Some(custom) = override_dir {
        absolutize(&custom)
    } else {
        let exe = std::env::current_exe().expect("current_exe");
        let exe_dir = exe
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        exe_dir.join("data")
    };
    std::fs::create_dir_all(&dir).ok();
    let _ = DATA_DIR.set(dir.clone());
    dir
}

fn parse_data_arg() -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1usize;
    while i < args.len() {
        let arg = args[i].as_str();
        if let Some(rest) = arg.strip_prefix("--data=") {
            return Some(PathBuf::from(rest));
        }
        if let Some(rest) = arg.strip_prefix("--data-dir=") {
            return Some(PathBuf::from(rest));
        }
        match arg {
            "--data" | "--data-dir" | "-d" => {
                if let Some(v) = args.get(i + 1) {
                    return Some(PathBuf::from(v));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(path)
}

pub fn data_dir() -> PathBuf {
    DATA_DIR
        .get()
        .cloned()
        .unwrap_or_else(init_from_cli_and_env)
}

pub fn db_path() -> PathBuf {
    data_dir().join("catalog.sqlite")
}

pub fn trash_dir() -> PathBuf {
    data_dir().join("trash")
}

/// Local tool binaries (`ffmpeg.exe`, `ffprobe.exe`, `fpcalc.exe`).
pub fn bin_dir() -> PathBuf {
    data_dir().join("bin")
}

pub fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\")
}
