import db, { getSetting } from "../db.js";
import { yieldEventLoop } from "../jobs.js";
import {
  ensureTools,
  fingerprintMiddle,
  packFingerprint,
  probeFile,
} from "../media.js";

function poolMap(items, concurrency, worker) {
  let idx = 0;
  let active = 0;
  let rejected = false;

  return new Promise((resolve, reject) => {
    const results = new Array(items.length);

    const pump = () => {
      if (rejected) return;
      if (idx >= items.length && active === 0) {
        resolve(results);
        return;
      }
      while (active < concurrency && idx < items.length) {
        const i = idx++;
        active++;
        Promise.resolve()
          .then(() => worker(items[i], i))
          .then((value) => {
            results[i] = value;
            active--;
            pump();
          })
          .catch((err) => {
            rejected = true;
            reject(err);
          });
      }
    };

    pump();
  });
}

export async function runFingerprint(ctx) {
  const tools = await ensureTools();
  if (!tools.ok) {
    throw new Error(
      `Missing tools on PATH: ${tools.missing.join(", ")}. Install ffmpeg and chromaprint (fpcalc).`
    );
  }

  const minDuration = Number(getSetting("min_duration_sec") || 180);
  const workers = Math.max(1, Math.min(8, Number(getSetting("workers") || 4)));
  const startRatio = Number(getSetting("middle_start") || 0.2);
  const endRatio = Number(getSetting("middle_end") || 0.8);

  const rows = db
    .prepare(
      `SELECT id, path, size, mtime_ms, duration_sec, fingerprint_len
       FROM files
       ORDER BY id`
    )
    .all();

  const updateProbe = db.prepare(`
    UPDATE files
    SET duration_sec = @duration_sec,
        width = @width,
        height = @height,
        error = NULL,
        updated_at = @updated_at
    WHERE id = @id
  `);

  const updateFp = db.prepare(`
    UPDATE files
    SET fingerprint = @fingerprint,
        fingerprint_len = @fingerprint_len,
        fingerprint_at = @fingerprint_at,
        error = NULL,
        updated_at = @updated_at
    WHERE id = @id
  `);

  const updateErr = db.prepare(`
    UPDATE files SET error = @error, updated_at = @updated_at WHERE id = @id
  `);

  const todo = [];
  for (const row of rows) {
    if (row.fingerprint_len && row.duration_sec != null) continue;
    todo.push(row);
  }

  ctx.setProgress(0, todo.length, `probing/fingerprinting ${todo.length} files…`);

  let done = 0;
  let skippedShort = 0;
  let failed = 0;
  let fingerprinted = 0;

  await poolMap(todo, workers, async (row) => {
    if (ctx.isCancelled()) return;

    const ts = new Date().toISOString();
    try {
      let duration = row.duration_sec;
      let width = null;
      let height = null;

      if (duration == null) {
        const probe = await probeFile(row.path);
        duration = probe.durationSec;
        width = probe.width;
        height = probe.height;
        updateProbe.run({
          id: row.id,
          duration_sec: duration,
          width,
          height,
          updated_at: ts,
        });
      }

      if (duration == null || duration < minDuration) {
        skippedShort++;
      } else {
        const values = await fingerprintMiddle(
          row.path,
          duration,
          startRatio,
          endRatio
        );
        const blob = packFingerprint(values);
        updateFp.run({
          id: row.id,
          fingerprint: blob,
          fingerprint_len: values.length,
          fingerprint_at: ts,
          updated_at: ts,
        });
        fingerprinted++;
      }
    } catch (err) {
      failed++;
      updateErr.run({
        id: row.id,
        error: err?.message || String(err),
        updated_at: ts,
      });
    } finally {
      done++;
      if (done % 2 === 0 || done === todo.length) {
        ctx.setProgress(
          done,
          todo.length,
          `fingerprinted ${fingerprinted} · skipped short ${skippedShort} · failed ${failed}`
        );
        await yieldEventLoop();
      }
    }
  });

  return {
    processed: done,
    fingerprinted,
    skippedShort,
    failed,
    minDuration,
  };
}
