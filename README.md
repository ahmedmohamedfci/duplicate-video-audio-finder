# Duplicate Video / Audio Finder

Find near-duplicate media by comparing **center-window audio fingerprints**. Intros/outros are ignored. Files shorter than **3 minutes** are skipped for near-dup matching.

Built as a **standalone Windows EXE** with [Tauri 2](https://tauri.app/) — same packaging approach as [localFileExplorer2](https://github.com/ahmedmohamedfci/localFileExplorer2).

## Requirements

- **ffmpeg**, **ffprobe**, and **fpcalc** on your system `PATH` (not bundled)
- For local builds: Node.js 22+, Rust stable, Visual Studio C++ tools

## Standalone EXE

### Download from CI

Push to `main` / `master` runs **Build standalone EXE**. Download the `dupfinder-windows` artifact (`dupfinder.exe` only — no zip, no ffmpeg, no source).

### Build locally

```powershell
npm install
.\build-standalone.ps1
```

Output:

```text
src-tauri\target\release\dupfinder.exe
```

Or:

```bash
npm run build
```

### Run

```bat
dupfinder.exe
dupfinder.exe --data D:\dup-runs\batch-a
dupfinder.exe --data-dir D:\dup-runs\batch-b
```

| Flag / env | Meaning |
|---|---|
| `--data`, `--data-dir`, `-d` | Data directory (SQLite + trash). Default: `./data` next to the EXE |
| `DUPFINDER_DATA` | Same as `--data` if no CLI flag |

Each instance should use its **own data folder**.

## Dev

```bash
npm install
npm run dev
```

Opens the desktop window; UI + API are served from an embedded localhost host (port `3847+`).

## Flow

1. **Settings** — add root folders, extensions, fingerprint window
2. **Save & scan** — catalog files into SQLite
3. **Run fingerprint** — center audio window (default 60s, PATH workers)
4. **Find duplicates** — similarity groups
5. **Groups** — Keep / queue deletes, A/B preview
6. **Delete queued** — soft-move into `data/trash`

## Non-blocking UI

Scan, fingerprint, match, and delete run as **background jobs**. Progress is polled from `/api/jobs/:id` so the UI stays interactive.

## Defaults

| Setting | Value |
|---|---|
| Min duration | 180 sec |
| Fingerprint window | center 60s (min 15s) |
| Workers | 4 |
| Min confidence | 85% |
