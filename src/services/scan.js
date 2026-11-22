import fs from "node:fs";
import path from "node:path";
import db, { getSettingJson } from "../db.js";
import { yieldEventLoop } from "../jobs.js";

function normalizeExt(ext) {
  const e = ext.trim().toLowerCase();
  if (!e) return null;
  return e.startsWith(".") ? e : `.${e}`;
}

export async function runScan(ctx) {
  const roots = getSettingJson("roots", []);
  const extensions = new Set(
    getSettingJson("extensions", []).map(normalizeExt).filter(Boolean)
  );

  if (!roots.length) {
    throw new Error("Add at least one root folder first");
  }
  if (!extensions.size) {
    throw new Error("Add at least one extension");
  }

  const upsert = db.prepare(`
    INSERT INTO files (path, root, ext, size, mtime_ms, updated_at)
    VALUES (@path, @root, @ext, @size, @mtime_ms, @updated_at)
    ON CONFLICT(path) DO UPDATE SET
      root = excluded.root,
      ext = excluded.ext,
      size = excluded.size,
      mtime_ms = excluded.mtime_ms,
      updated_at = excluded.updated_at,
      fingerprint = CASE
        WHEN files.size = excluded.size AND files.mtime_ms = excluded.mtime_ms
        THEN files.fingerprint ELSE NULL END,
      fingerprint_len = CASE
        WHEN files.size = excluded.size AND files.mtime_ms = excluded.mtime_ms
        THEN files.fingerprint_len ELSE NULL END,
      fingerprint_at = CASE
        WHEN files.size = excluded.size AND files.mtime_ms = excluded.mtime_ms
        THEN files.fingerprint_at ELSE NULL END,
      duration_sec = CASE
        WHEN files.size = excluded.size AND files.mtime_ms = excluded.mtime_ms
        THEN files.duration_sec ELSE NULL END,
      width = CASE
        WHEN files.size = excluded.size AND files.mtime_ms = excluded.mtime_ms
        THEN files.width ELSE NULL END,
      height = CASE
        WHEN files.size = excluded.size AND files.mtime_ms = excluded.mtime_ms
        THEN files.height ELSE NULL END,
      error = CASE
        WHEN files.size = excluded.size AND files.mtime_ms = excluded.mtime_ms
        THEN files.error ELSE NULL END
  `);

  const seen = new Set();
  let scanned = 0;
  let matched = 0;
  const updatedAt = new Date().toISOString();

  ctx.setMessage("walking folders…");

  for (const root of roots) {
    if (ctx.isCancelled()) break;
    if (!fs.existsSync(root)) {
      throw new Error(`Root not found: ${root}`);
    }

    const stack = [root];
    while (stack.length) {
      if (ctx.isCancelled()) break;
      const dir = stack.pop();
      let entries;
      try {
        entries = fs.readdirSync(dir, { withFileTypes: true });
      } catch {
        continue;
      }

      for (const entry of entries) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) {
          stack.push(full);
          continue;
        }
        if (!entry.isFile()) continue;

        scanned++;
        const ext = path.extname(entry.name).toLowerCase();
        if (!extensions.has(ext)) continue;

        let st;
        try {
          st = fs.statSync(full);
        } catch {
          continue;
        }

        matched++;
        seen.add(full);
        upsert.run({
          path: full,
          root,
          ext,
          size: st.size,
          mtime_ms: Math.trunc(st.mtimeMs),
          updated_at: updatedAt,
        });

        if (matched % 200 === 0) {
          ctx.setProgress(matched, Math.max(matched, scanned), [
            `indexed ${matched} media files`,
            `(scanned ${scanned} entries)`,
          ].join(" "));
          await yieldEventLoop();
        }
      }
    }
  }

  // Remove DB rows that vanished from disk (for configured roots only)
  const existing = db.prepare("SELECT id, path, root FROM files").all();
  const rootSet = new Set(roots.map((r) => path.normalize(r)));
  const del = db.prepare("DELETE FROM files WHERE id = ?");
  let removed = 0;
  for (const row of existing) {
    if (!rootSet.has(path.normalize(row.root))) continue;
    if (!seen.has(row.path)) {
      del.run(row.id);
      removed++;
    }
  }

  const total = db.prepare("SELECT COUNT(*) AS c FROM files").get().c;
  ctx.setProgress(matched, matched, `scan complete · ${total} files in catalog`);
  return { scanned, matched, removed, total };
}
