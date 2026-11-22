import express from "express";
import fs from "node:fs";
import db, {
  getAllSettings,
  setSetting,
  setSettingJson,
  withTransaction,
} from "./db.js";
import {
  cancelJob,
  getJob,
  listJobs,
  startJob,
  subscribe,
} from "./jobs.js";
import { ensureTools } from "./media.js";
import { runScan } from "./services/scan.js";
import { runFingerprint } from "./services/fingerprint.js";
import { runMatch } from "./services/match.js";
import {
  clearDeleteQueue,
  getQueuedDeletes,
  runSoftDelete,
} from "./services/delete.js";

const router = express.Router();

router.get("/health", async (_req, res) => {
  const tools = await ensureTools();
  res.json({ ok: true, tools });
});

router.get("/settings", (_req, res) => {
  res.json(getAllSettings());
});

router.put("/settings", (req, res) => {
  const body = req.body || {};
  if (Array.isArray(body.roots)) setSettingJson("roots", body.roots);
  if (Array.isArray(body.extensions)) {
    setSettingJson(
      "extensions",
      body.extensions.map((e) => {
        const x = String(e).trim().toLowerCase();
        return x.startsWith(".") ? x : `.${x}`;
      })
    );
  }
  for (const key of [
    "workers",
    "min_duration_sec",
    "middle_start",
    "middle_end",
    "min_confidence",
    "trash_dir",
  ]) {
    if (body[key] !== undefined && body[key] !== null) {
      setSetting(key, body[key]);
    }
  }
  res.json(getAllSettings());
});

router.get("/stats", (_req, res) => {
  const files = db.prepare("SELECT COUNT(*) AS c FROM files").get().c;
  const fingerprinted = db
    .prepare(
      "SELECT COUNT(*) AS c FROM files WHERE fingerprint IS NOT NULL"
    )
    .get().c;
  const groups = db.prepare("SELECT COUNT(*) AS c FROM groups").get().c;
  const unreviewed = db
    .prepare("SELECT COUNT(*) AS c FROM groups WHERE reviewed = 0")
    .get().c;
  const queued = getQueuedDeletes();
  const queuedBytes = queued.reduce((s, r) => s + (r.size || 0), 0);
  res.json({
    files,
    fingerprinted,
    groups,
    unreviewed,
    queuedDeletes: queued.length,
    queuedBytes,
  });
});

function launch(type, runner, message) {
  try {
    return startJob(type, runner, message);
  } catch (err) {
    if (err.code === "JOB_BUSY") {
      const e = new Error(err.message);
      e.status = 409;
      throw e;
    }
    throw err;
  }
}

router.post("/jobs/scan", (_req, res, next) => {
  try {
    const job = launch("scan", runScan, "scanning…");
    res.status(202).json(job);
  } catch (err) {
    next(err);
  }
});

router.post("/jobs/fingerprint", (_req, res, next) => {
  try {
    const job = launch("fingerprint", runFingerprint, "fingerprinting…");
    res.status(202).json(job);
  } catch (err) {
    next(err);
  }
});

router.post("/jobs/match", (_req, res, next) => {
  try {
    const job = launch("match", runMatch, "matching…");
    res.status(202).json(job);
  } catch (err) {
    next(err);
  }
});

router.post("/jobs/delete", (_req, res, next) => {
  try {
    const job = launch("delete", runSoftDelete, "moving to trash…");
    res.status(202).json(job);
  } catch (err) {
    next(err);
  }
});

router.get("/jobs", (_req, res) => {
  res.json(listJobs());
});

router.get("/jobs/:id", (req, res) => {
  const job = getJob(req.params.id);
  if (!job) return res.status(404).json({ error: "job not found" });
  res.json(job);
});

router.post("/jobs/:id/cancel", (req, res) => {
  const ok = cancelJob(req.params.id);
  res.json({ cancelled: ok, job: getJob(req.params.id) });
});

/** Server-Sent Events for live job progress — UI stays interactive. */
router.get("/jobs/:id/events", (req, res) => {
  const job = getJob(req.params.id);
  if (!job) return res.status(404).json({ error: "job not found" });

  res.setHeader("Content-Type", "text/event-stream");
  res.setHeader("Cache-Control", "no-cache");
  res.setHeader("Connection", "keep-alive");
  res.flushHeaders?.();

  const send = (payload) => {
    res.write(`data: ${JSON.stringify(payload)}\n\n`);
  };

  send(job);
  const unsub = subscribe(req.params.id, send);

  const heartbeat = setInterval(() => {
    res.write(": ping\n\n");
  }, 15000);

  req.on("close", () => {
    clearInterval(heartbeat);
    unsub();
  });
});

router.get("/groups", (req, res) => {
  const minConf = Number(req.query.minConfidence || 0) / 100;
  const unreviewedOnly = String(req.query.unreviewed || "1") !== "0";
  const pathFilter = String(req.query.path || "").trim().toLowerCase();

  let sql = `
    SELECT g.*
    FROM groups g
    WHERE g.confidence >= ?
  `;
  const params = [minConf];
  if (unreviewedOnly) sql += " AND g.reviewed = 0";
  sql += " ORDER BY g.reclaimable_bytes DESC, g.confidence DESC, g.id";

  let groups = db.prepare(sql).all(...params);

  if (pathFilter) {
    const members = db.prepare(`
      SELECT gm.group_id, f.path
      FROM group_members gm
      JOIN files f ON f.id = gm.file_id
    `);
    const byGroup = new Map();
    for (const row of members.all()) {
      if (!byGroup.has(row.group_id)) byGroup.set(row.group_id, []);
      byGroup.get(row.group_id).push(row.path.toLowerCase());
    }
    groups = groups.filter((g) =>
      (byGroup.get(g.id) || []).some((p) => p.includes(pathFilter))
    );
  }

  res.json(groups);
});

router.get("/groups/:id", (req, res) => {
  const group = db
    .prepare("SELECT * FROM groups WHERE id = ?")
    .get(req.params.id);
  if (!group) return res.status(404).json({ error: "group not found" });

  const members = db
    .prepare(
      `SELECT gm.*, f.path, f.ext, f.size, f.duration_sec, f.width, f.height, f.mtime_ms, f.error
       FROM group_members gm
       JOIN files f ON f.id = gm.file_id
       WHERE gm.group_id = ?
       ORDER BY gm.is_keep DESC, f.size DESC`
    )
    .all(group.id);

  res.json({ ...group, members });
});

router.patch("/groups/:id", (req, res) => {
  const groupId = Number(req.params.id);
  const group = db.prepare("SELECT id FROM groups WHERE id = ?").get(groupId);
  if (!group) return res.status(404).json({ error: "group not found" });

  const { keepFileId, reviewed, queueDeletes, fileId, queuedDelete } =
    req.body || {};

  withTransaction(() => {
    if (keepFileId != null) {
      db.prepare(`UPDATE group_members SET is_keep = 0 WHERE group_id = ?`).run(
        groupId
      );
      db.prepare(
        `UPDATE group_members SET is_keep = 1, queued_delete = 0
         WHERE group_id = ? AND file_id = ?`
      ).run(groupId, keepFileId);
    }
    if (typeof reviewed === "boolean") {
      db.prepare(`UPDATE groups SET reviewed = ? WHERE id = ?`).run(
        reviewed ? 1 : 0,
        groupId
      );
    }
    if (queueDeletes === true) {
      db.prepare(
        `UPDATE group_members SET queued_delete = 1
         WHERE group_id = ? AND is_keep = 0`
      ).run(groupId);
    }
    if (queueDeletes === false) {
      db.prepare(
        `UPDATE group_members SET queued_delete = 0 WHERE group_id = ?`
      ).run(groupId);
    }
    if (fileId != null && typeof queuedDelete === "boolean") {
      db.prepare(
        `UPDATE group_members
         SET queued_delete = ?, is_keep = CASE WHEN ? = 1 THEN 0 ELSE is_keep END
         WHERE group_id = ? AND file_id = ?`
      ).run(queuedDelete ? 1 : 0, queuedDelete ? 1 : 0, groupId, fileId);
    }
  });

  const updated = db.prepare("SELECT * FROM groups WHERE id = ?").get(groupId);
  const members = db
    .prepare(
      `SELECT gm.*, f.path, f.ext, f.size, f.duration_sec, f.width, f.height
       FROM group_members gm JOIN files f ON f.id = gm.file_id
       WHERE gm.group_id = ?`
    )
    .all(groupId);
  res.json({ ...updated, members });
});

router.get("/queued-deletes", (_req, res) => {
  res.json(getQueuedDeletes());
});

router.post("/queued-deletes/clear", (_req, res) => {
  res.json(clearDeleteQueue());
});

/** Stream a local media file for A/B preview (range-aware). */
router.get("/media/:fileId", (req, res) => {
  const row = db
    .prepare("SELECT path, ext FROM files WHERE id = ?")
    .get(req.params.fileId);
  if (!row || !fs.existsSync(row.path)) {
    return res.status(404).json({ error: "file not found" });
  }

  const stat = fs.statSync(row.path);
  const ext = row.ext.toLowerCase();
  const type =
    {
      ".mp4": "video/mp4",
      ".webm": "video/webm",
      ".mkv": "video/x-matroska",
      ".mp3": "audio/mpeg",
      ".m4a": "audio/mp4",
      ".wav": "audio/wav",
      ".ogg": "audio/ogg",
      ".flac": "audio/flac",
      ".aac": "audio/aac",
    }[ext] || "application/octet-stream";

  const range = req.headers.range;
  if (range) {
    const m = /bytes=(\d+)-(\d*)/.exec(range);
    const start = Number(m[1]);
    const end = m[2] ? Number(m[2]) : Math.min(start + 1024 * 1024 - 1, stat.size - 1);
    res.status(206);
    res.setHeader("Content-Range", `bytes ${start}-${end}/${stat.size}`);
    res.setHeader("Accept-Ranges", "bytes");
    res.setHeader("Content-Length", end - start + 1);
    res.setHeader("Content-Type", type);
    fs.createReadStream(row.path, { start, end }).pipe(res);
    return;
  }

  res.setHeader("Content-Length", stat.size);
  res.setHeader("Content-Type", type);
  res.setHeader("Accept-Ranges", "bytes");
  fs.createReadStream(row.path).pipe(res);
});

router.use((err, _req, res, _next) => {
  const status = err.status || 500;
  res.status(status).json({ error: err.message || String(err) });
});

export default router;
