import { randomUUID } from "node:crypto";
import db from "./db.js";

const running = new Map();
const listeners = new Map();

function now() {
  return new Date().toISOString();
}

export function createJob(type, message = "queued") {
  const id = randomUUID();
  const ts = now();
  db.prepare(
    `INSERT INTO jobs (id, type, status, progress, total, done, message, created_at, updated_at)
     VALUES (?, ?, 'queued', 0, 0, 0, ?, ?, ?)`
  ).run(id, type, message, ts, ts);
  return getJob(id);
}

export function getJob(id) {
  const row = db.prepare("SELECT * FROM jobs WHERE id = ?").get(id);
  if (!row) return null;
  return {
    ...row,
    result: row.result_json ? JSON.parse(row.result_json) : null,
  };
}

export function listJobs(limit = 20) {
  return db
    .prepare(
      `SELECT id, type, status, progress, total, done, message, error, created_at, updated_at
       FROM jobs ORDER BY created_at DESC LIMIT ?`
    )
    .all(limit);
}

export function updateJob(id, patch) {
  const job = getJob(id);
  if (!job) return null;
  const next = {
    status: patch.status ?? job.status,
    progress: patch.progress ?? job.progress,
    total: patch.total ?? job.total,
    done: patch.done ?? job.done,
    message: patch.message ?? job.message,
    error: patch.error === undefined ? job.error : patch.error,
    result_json:
      patch.result === undefined
        ? job.result_json
        : JSON.stringify(patch.result),
  };
  db.prepare(
    `UPDATE jobs
     SET status = ?, progress = ?, total = ?, done = ?, message = ?, error = ?, result_json = ?, updated_at = ?
     WHERE id = ?`
  ).run(
    next.status,
    next.progress,
    next.total,
    next.done,
    next.message,
    next.error,
    next.result_json,
    now(),
    id
  );
  const updated = getJob(id);
  emit(id, updated);
  return updated;
}

export function subscribe(jobId, fn) {
  if (!listeners.has(jobId)) listeners.set(jobId, new Set());
  listeners.get(jobId).add(fn);
  return () => listeners.get(jobId)?.delete(fn);
}

function emit(jobId, payload) {
  const set = listeners.get(jobId);
  if (!set) return;
  for (const fn of set) {
    try {
      fn(payload);
    } catch {
      // ignore listener errors
    }
  }
}

export function isTypeRunning(type) {
  for (const job of running.values()) {
    if (job.type === type) return true;
  }
  return false;
}

/**
 * Start a background job without blocking the HTTP response.
 * Work runs after the current event-loop turn; progress is polled / SSE'd.
 */
export function startJob(type, runner, message = "starting") {
  if (isTypeRunning(type)) {
    const err = new Error(`${type} is already running`);
    err.code = "JOB_BUSY";
    throw err;
  }

  const job = createJob(type, message);
  const handle = { type, cancel: false };
  running.set(job.id, handle);

  const ctx = {
    jobId: job.id,
    isCancelled: () => handle.cancel,
    setProgress: (done, total, msg) => {
      const progress = total > 0 ? Math.min(1, done / total) : 0;
      updateJob(job.id, {
        status: "running",
        done,
        total,
        progress,
        message: msg ?? getJob(job.id)?.message ?? "",
      });
    },
    setMessage: (msg) => updateJob(job.id, { message: msg, status: "running" }),
  };

  setImmediate(async () => {
    updateJob(job.id, { status: "running", message });
    try {
      const result = await runner(ctx);
      if (handle.cancel) {
        updateJob(job.id, {
          status: "cancelled",
          message: "cancelled",
          result,
        });
      } else {
        updateJob(job.id, {
          status: "done",
          progress: 1,
          message: "done",
          result,
        });
      }
    } catch (err) {
      updateJob(job.id, {
        status: "error",
        error: err?.message || String(err),
        message: "failed",
      });
    } finally {
      running.delete(job.id);
    }
  });

  return getJob(job.id);
}

export function cancelJob(id) {
  const handle = running.get(id);
  if (handle) {
    handle.cancel = true;
    updateJob(id, { message: "cancelling…" });
    return true;
  }
  return false;
}

/** Yield to the event loop so HTTP/SSE stay responsive during long loops. */
export function yieldEventLoop() {
  return new Promise((resolve) => setImmediate(resolve));
}
