import fs from "node:fs";
import path from "node:path";
import db, { getSetting } from "../db.js";
import { yieldEventLoop } from "../jobs.js";

export function getQueuedDeletes() {
  return db
    .prepare(
      `SELECT gm.group_id, gm.file_id, f.path, f.size
       FROM group_members gm
       JOIN files f ON f.id = gm.file_id
       WHERE gm.queued_delete = 1
       ORDER BY gm.group_id, f.path`
    )
    .all();
}

export function clearDeleteQueue() {
  const info = db
    .prepare(`UPDATE group_members SET queued_delete = 0 WHERE queued_delete = 1`)
    .run();
  return { cleared: Number(info.changes) || 0 };
}

export async function runSoftDelete(ctx) {
  const trashRoot = getSetting("trash_dir");
  if (!trashRoot) throw new Error("trash_dir not configured");
  fs.mkdirSync(trashRoot, { recursive: true });

  const queued = getQueuedDeletes();
  ctx.setProgress(0, queued.length, `moving ${queued.length} files to trash…`);

  const clearFlag = db.prepare(
    `UPDATE group_members SET queued_delete = 0 WHERE group_id = ? AND file_id = ?`
  );
  const markReviewed = db.prepare(
    `UPDATE groups SET reviewed = 1 WHERE id = ?`
  );

  let moved = 0;
  let failed = 0;
  const errors = [];

  for (const row of queued) {
    if (ctx.isCancelled()) break;
    try {
      const base = path.basename(row.path);
      const stamp = Date.now();
      const destDir = path.join(trashRoot, `group-${row.group_id}`);
      fs.mkdirSync(destDir, { recursive: true });
      let dest = path.join(destDir, base);
      if (fs.existsSync(dest)) {
        dest = path.join(destDir, `${stamp}-${base}`);
      }
      fs.renameSync(row.path, dest);
      clearFlag.run(row.group_id, row.file_id);
      markReviewed.run(row.group_id);
      // Keep catalog row but note path moved? For now delete catalog entry
      db.prepare("DELETE FROM files WHERE id = ?").run(row.file_id);
      moved++;
    } catch (err) {
      failed++;
      errors.push({ path: row.path, error: err.message });
    }
    ctx.setProgress(moved + failed, queued.length, `moved ${moved} · failed ${failed}`);
    await yieldEventLoop();
  }

  return { moved, failed, errors, trashRoot };
}
