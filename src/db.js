import fs from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { DATA_DIR, DB_PATH } from "./config.js";

fs.mkdirSync(DATA_DIR, { recursive: true });

const db = new DatabaseSync(DB_PATH);
db.exec("PRAGMA journal_mode = WAL");
db.exec("PRAGMA synchronous = NORMAL");
db.exec("PRAGMA foreign_keys = ON");

db.exec(`
  CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
  );

  CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    root TEXT NOT NULL,
    ext TEXT NOT NULL,
    size INTEGER NOT NULL,
    mtime_ms INTEGER NOT NULL,
    duration_sec REAL,
    width INTEGER,
    height INTEGER,
    xxhash TEXT,
    fingerprint BLOB,
    fingerprint_len INTEGER,
    fingerprint_at TEXT,
    error TEXT,
    updated_at TEXT NOT NULL
  );

  CREATE INDEX IF NOT EXISTS idx_files_duration ON files(duration_sec);
  CREATE INDEX IF NOT EXISTS idx_files_fp ON files(fingerprint_len);

  CREATE TABLE IF NOT EXISTS groups (
    id INTEGER PRIMARY KEY,
    confidence REAL NOT NULL,
    member_count INTEGER NOT NULL,
    reclaimable_bytes INTEGER NOT NULL DEFAULT 0,
    reviewed INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
  );

  CREATE TABLE IF NOT EXISTS group_members (
    group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    offset_sec REAL NOT NULL DEFAULT 0,
    is_keep INTEGER NOT NULL DEFAULT 0,
    queued_delete INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (group_id, file_id)
  );

  CREATE INDEX IF NOT EXISTS idx_members_file ON group_members(file_id);

  CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY,
    type TEXT NOT NULL,
    status TEXT NOT NULL,
    progress REAL NOT NULL DEFAULT 0,
    total INTEGER NOT NULL DEFAULT 0,
    done INTEGER NOT NULL DEFAULT 0,
    message TEXT NOT NULL DEFAULT '',
    error TEXT,
    result_json TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
  );
`);

const DEFAULTS = {
  roots: "[]",
  extensions: JSON.stringify([
    ".mp4",
    ".mkv",
    ".avi",
    ".mov",
    ".wmv",
    ".webm",
    ".mp3",
    ".m4a",
    ".aac",
    ".flac",
    ".wav",
    ".ogg",
    ".opus",
  ]),
  workers: "4",
  min_duration_sec: "180",
  middle_start: "0.2",
  middle_end: "0.8",
  min_confidence: "85",
  trash_dir: path.join(DATA_DIR, "trash"),
};

const insertSetting = db.prepare(
  "INSERT OR IGNORE INTO settings (key, value) VALUES (?, ?)"
);
for (const [key, value] of Object.entries(DEFAULTS)) {
  insertSetting.run(key, value);
}

export function getSetting(key) {
  const row = db.prepare("SELECT value FROM settings WHERE key = ?").get(key);
  return row ? row.value : null;
}

export function getSettingJson(key, fallback) {
  const raw = getSetting(key);
  if (raw == null) return fallback;
  try {
    return JSON.parse(raw);
  } catch {
    return fallback;
  }
}

export function setSetting(key, value) {
  db.prepare(
    `INSERT INTO settings (key, value) VALUES (?, ?)
     ON CONFLICT(key) DO UPDATE SET value = excluded.value`
  ).run(key, String(value));
}

export function setSettingJson(key, value) {
  setSetting(key, JSON.stringify(value));
}

export function getAllSettings() {
  const rows = db.prepare("SELECT key, value FROM settings").all();
  const out = {};
  for (const row of rows) {
    try {
      out[row.key] = JSON.parse(row.value);
    } catch {
      const n = Number(row.value);
      out[row.key] =
        Number.isFinite(n) && String(n) === row.value ? n : row.value;
    }
  }
  return out;
}

/** node:sqlite has no better-sqlite3-style .transaction(); wrap manually. */
export function withTransaction(fn) {
  db.exec("BEGIN IMMEDIATE");
  try {
    const result = fn();
    db.exec("COMMIT");
    return result;
  } catch (err) {
    try {
      db.exec("ROLLBACK");
    } catch {
      // ignore
    }
    throw err;
  }
}

export default db;
