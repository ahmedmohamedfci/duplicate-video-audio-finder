import db, { getSetting, withTransaction } from "../db.js";
import { yieldEventLoop } from "../jobs.js";
import { fingerprintSimilarity, unpackFingerprint } from "../media.js";

/**
 * Build an inverted index of fingerprint subhashes for candidate generation,
 * then verify with similarity. Keeps memory bounded by streaming from SQLite.
 */
export async function runMatch(ctx) {
  const minDuration = Number(getSetting("min_duration_sec") || 180);
  const minConfidence = Number(getSetting("min_confidence") || 85) / 100;

  const rows = db
    .prepare(
      `SELECT id, path, size, duration_sec, fingerprint, fingerprint_len
       FROM files
       WHERE fingerprint IS NOT NULL
         AND duration_sec IS NOT NULL
         AND duration_sec >= ?
       ORDER BY id`
    )
    .all(minDuration);

  ctx.setMessage(`indexing ${rows.length} fingerprints…`);

  /** @type {Map<string, Array<{id:number, pos:number}>>} */
  const index = new Map();
  const fps = new Map();

  for (let i = 0; i < rows.length; i++) {
    const row = rows[i];
    const fp = unpackFingerprint(row.fingerprint);
    fps.set(row.id, { fp, size: row.size, duration: row.duration_sec });

    // Index every 4th window of 2 consecutive frames
    for (let p = 0; p + 1 < fp.length; p += 4) {
      const key = `${fp[p]}:${fp[p + 1]}`;
      let bucket = index.get(key);
      if (!bucket) {
        bucket = [];
        index.set(key, bucket);
      }
      if (bucket.length < 64) bucket.push({ id: row.id, pos: p });
    }

    if (i % 100 === 0) {
      ctx.setProgress(i, rows.length, `indexed ${i}/${rows.length}`);
      await yieldEventLoop();
    }
  }

  ctx.setMessage("finding candidate pairs…");

  /** @type {Map<string, number>} vote counts pairKey -> votes */
  const votes = new Map();

  let processed = 0;
  for (const row of rows) {
    if (ctx.isCancelled()) break;
    const a = fps.get(row.id);
    if (!a) continue;

    const candidateScores = new Map();
    for (let p = 0; p + 1 < a.fp.length; p += 4) {
      const key = `${a.fp[p]}:${a.fp[p + 1]}`;
      const bucket = index.get(key);
      if (!bucket) continue;
      for (const hit of bucket) {
        if (hit.id <= row.id) continue; // unordered pairs, avoid dup work
        candidateScores.set(
          hit.id,
          (candidateScores.get(hit.id) || 0) + 1
        );
      }
    }

    for (const [otherId, score] of candidateScores) {
      if (score < 3) continue;
      const pairKey = `${row.id}:${otherId}`;
      votes.set(pairKey, score);
    }

    processed++;
    if (processed % 50 === 0) {
      ctx.setProgress(processed, rows.length, `candidates from ${processed} files`);
      await yieldEventLoop();
    }
  }

  const pairs = [...votes.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, 50_000);

  ctx.setMessage(`verifying ${pairs.length} candidate pairs…`);

  /** Union-find for grouping */
  const parent = new Map();
  const find = (x) => {
    let p = parent.get(x) ?? x;
    while (p !== (parent.get(p) ?? p)) p = parent.get(p);
    parent.set(x, p);
    return p;
  };
  const union = (a, b) => {
    const ra = find(a);
    const rb = find(b);
    if (ra !== rb) parent.set(ra, rb);
  };

  /** @type {Map<string, number>} */
  const pairConfidence = new Map();
  let verified = 0;

  for (const [pairKey] of pairs) {
    if (ctx.isCancelled()) break;
    const [aId, bId] = pairKey.split(":").map(Number);
    const a = fps.get(aId);
    const b = fps.get(bId);
    if (!a || !b) continue;

    // Duration gate: allow ±35% difference (intros/outros + trims)
    const dRatio =
      Math.min(a.duration, b.duration) / Math.max(a.duration, b.duration);
    if (dRatio < 0.65) continue;

    const sim = fingerprintSimilarity(a.fp, b.fp);
    if (sim >= minConfidence) {
      union(aId, bId);
      pairConfidence.set(pairKey, sim);
    }

    verified++;
    if (verified % 25 === 0) {
      ctx.setProgress(verified, pairs.length, `verified ${verified}/${pairs.length}`);
      await yieldEventLoop();
    }
  }

  // Materialize groups
  const clusters = new Map();
  for (const row of rows) {
    if (!parent.has(row.id) && ![...pairConfidence.keys()].some((k) => k.includes(String(row.id)))) {
      continue;
    }
    if (!parent.has(row.id)) continue;
    const root = find(row.id);
    if (!clusters.has(root)) clusters.set(root, new Set());
    clusters.get(root).add(row.id);
  }

  // Also add partners from pairConfidence into clusters
  for (const pairKey of pairConfidence.keys()) {
    const [aId, bId] = pairKey.split(":").map(Number);
    const root = find(aId);
    if (!clusters.has(root)) clusters.set(root, new Set());
    clusters.get(root).add(aId);
    clusters.get(root).add(bId);
  }

  const clearMembers = db.prepare("DELETE FROM group_members");
  const clearGroups = db.prepare("DELETE FROM groups");
  const insertGroup = db.prepare(`
    INSERT INTO groups (confidence, member_count, reclaimable_bytes, reviewed, created_at)
    VALUES (?, ?, ?, 0, ?)
  `);
  const insertMember = db.prepare(`
    INSERT INTO group_members (group_id, file_id, offset_sec, is_keep, queued_delete)
    VALUES (?, ?, 0, ?, ?)
  `);
  const getFile = db.prepare(
    `SELECT id, size, duration_sec, width, height FROM files WHERE id = ?`
  );

  const clusterList = [...clusters.values()];
  const groupCount = withTransaction(() => {
    clearMembers.run();
    clearGroups.run();
    let count = 0;

    for (const members of clusterList) {
      if (members.size < 2) continue;

      const files = [...members].map((id) => getFile.get(id)).filter(Boolean);
      files.sort((a, b) => {
        const resA = (a.width || 0) * (a.height || 0);
        const resB = (b.width || 0) * (b.height || 0);
        if (resB !== resA) return resB - resA;
        if ((b.duration_sec || 0) !== (a.duration_sec || 0)) {
          return (b.duration_sec || 0) - (a.duration_sec || 0);
        }
        return (b.size || 0) - (a.size || 0);
      });

      let conf = 0;
      let confN = 0;
      for (let i = 0; i < files.length; i++) {
        for (let j = i + 1; j < files.length; j++) {
          const key =
            files[i].id < files[j].id
              ? `${files[i].id}:${files[j].id}`
              : `${files[j].id}:${files[i].id}`;
          if (pairConfidence.has(key)) {
            conf += pairConfidence.get(key);
            confN++;
          }
        }
      }
      const confidence = confN ? conf / confN : minConfidence;
      const keepId = files[0].id;
      const reclaimable = files
        .filter((f) => f.id !== keepId)
        .reduce((s, f) => s + (f.size || 0), 0);

      const info = insertGroup.run(
        confidence,
        files.length,
        reclaimable,
        new Date().toISOString()
      );
      const groupId = Number(info.lastInsertRowid);
      for (const f of files) {
        // Never auto-queue deletes — user adds each file manually in the UI
        insertMember.run(groupId, f.id, f.id === keepId ? 1 : 0, 0);
      }
      count++;
    }
    return count;
  });

  ctx.setProgress(1, 1, `found ${groupCount} duplicate groups`);
  return { files: rows.length, pairsChecked: verified, groups: groupCount };
}
