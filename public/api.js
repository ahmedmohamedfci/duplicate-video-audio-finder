export async function api(path, options = {}) {
  const res = await fetch(`/api${path}`, {
    headers: { "Content-Type": "application/json", ...(options.headers || {}) },
    ...options,
  });
  const text = await res.text();
  let data = null;
  try {
    data = text ? JSON.parse(text) : null;
  } catch {
    data = { raw: text };
  }
  if (!res.ok) {
    const err = new Error(data?.error || res.statusText || "request failed");
    err.status = res.status;
    err.data = data;
    throw err;
  }
  return data;
}

/**
 * Poll job progress (works with the Tauri localhost host; no SSE needed).
 */
export function watchJob(jobId, { onUpdate, onDone, onError } = {}) {
  let stopped = false;
  let timer = null;

  const tick = async () => {
    if (stopped) return;
    try {
      const job = await api(`/jobs/${jobId}`);
      onUpdate?.(job);
      if (job.status === "done" || job.status === "cancelled") {
        stopped = true;
        onDone?.(job);
        return;
      }
      if (job.status === "error") {
        stopped = true;
        onError?.(job);
        return;
      }
    } catch {
      // keep polling briefly while host starts
    }
    if (!stopped) timer = setTimeout(tick, 500);
  };

  tick();
  return () => {
    stopped = true;
    if (timer) clearTimeout(timer);
  };
}

export function formatBytes(n) {
  if (n == null || !Number.isFinite(n)) return "—";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 && i > 0 ? v.toFixed(2) : v < 100 && i > 0 ? v.toFixed(1) : Math.round(v)} ${u[i]}`;
}

export function formatDuration(sec) {
  if (sec == null || !Number.isFinite(sec)) return "—";
  const s = Math.round(sec);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = s % 60;
  if (h) return `${h}:${String(m).padStart(2, "0")}:${String(r).padStart(2, "0")}`;
  return `${m}:${String(r).padStart(2, "0")}`;
}

export function pct(progress) {
  return `${Math.round((progress || 0) * 100)}%`;
}
