# Duplicate Video / Audio Finder

Find near-duplicate media by comparing **middle-segment audio fingerprints** (20%→80% of duration). Intros/outros at the ends are ignored. Files shorter than **3 minutes** are skipped for near-dup matching.

## Requirements

- Node.js **22.5+** (uses built-in `node:sqlite`)
- Bundled tools in `tools/bin/` (ffmpeg, ffprobe, fpcalc) — downloaded for you

Re-download tools anytime:

```powershell
powershell -File scripts/download-tools.ps1
```

The app prefers `tools/bin/*.exe`. Later you can switch to system PATH by removing those files (it falls back automatically).

## Run

```bash
npm install
npm start
```

Open http://127.0.0.1:3847

### Multiple instances

Each instance needs its **own data folder** (separate DB / settings / trash):

```bat
dupfinder.cmd --data D:\dup-runs\a
dupfinder.cmd --data D:\dup-runs\b --port 0
```

| Flag | Meaning |
|---|---|
| `--data`, `-d` | Data directory |
| `--port`, `-p` | Port (default `3847`; `0` = any free port). If busy, next free port is used |
| `--host` | Listen address (default `127.0.0.1`) |

### Portable package

```bat
npm run package
```

Creates `release/dupfinder/` (app + deps + `tools/bin`). Copy that folder anywhere; run `dupfinder.cmd --data ...` (Node 22.5+ still required on PATH).

GitHub Actions (`.github/workflows/package.yml`) builds the same Windows zip on push/PR and attaches it to version tags (`v*`) / releases.

## Non-blocking UI

Scan, fingerprint, match, and delete all start as **background jobs**. The HTTP API returns immediately (`202`) and progress streams over **SSE**. The page stays usable while work runs.

## Flow

1. **Settings** — add root folders, extensions
2. **Save & scan** — catalog files into SQLite
3. **Run fingerprint** — middle 60% audio only (worker pool)
4. **Find duplicates** — similarity groups
5. **Groups** — pick Keep, A/B preview, queue deletes
6. **Delete queued** — soft-move into `data/trash`

## Defaults

| Setting | Value |
|---|---|
| Min duration | 180 sec |
| Middle window | 20% → 80% |
| Workers | 4 |
| Min confidence | 85% |
