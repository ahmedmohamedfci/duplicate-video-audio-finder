import { spawn } from "node:child_process";
import { resolveTool, toolPaths } from "./tools.js";

function run(cmd, args, { timeoutMs = 120_000, maxBuffer = 8 * 1024 * 1024 } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(cmd, args, {
      windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = Buffer.alloc(0);
    let stderr = "";
    let settled = false;

    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      fail(new Error(`${cmd} timed out after ${timeoutMs}ms`));
    }, timeoutMs);

    const fail = (err) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      reject(err);
    };

    const ok = (value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(value);
    };

    child.stdout.on("data", (chunk) => {
      stdout = Buffer.concat([stdout, chunk]);
      if (stdout.length > maxBuffer) {
        child.kill("SIGKILL");
        fail(new Error(`${cmd} stdout exceeded buffer`));
      }
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString("utf8");
      if (stderr.length > maxBuffer) stderr = stderr.slice(-maxBuffer);
    });
    child.on("error", (err) => fail(err));
    child.on("close", (code) => {
      if (code === 0) ok({ stdout, stderr });
      else fail(new Error(`${cmd} exited ${code}: ${stderr.trim() || "no stderr"}`));
    });
  });
}

function commandExists(cmd) {
  return new Promise((resolve) => {
    const child = spawn(cmd, ["-version"], {
      windowsHide: true,
      stdio: "ignore",
    });
    child.on("error", () => resolve(false));
    child.on("close", () => resolve(true));
  });
}

export async function ensureTools() {
  const paths = toolPaths();
  const checks = [
    ["ffmpeg", paths.ffmpeg],
    ["ffprobe", paths.ffprobe],
    ["fpcalc", paths.fpcalc],
  ];
  const missing = [];
  const resolved = {};
  for (const [name, cmd] of checks) {
    const ok = await commandExists(cmd);
    resolved[name] = cmd;
    if (!ok) missing.push(name);
  }
  return {
    ok: missing.length === 0,
    missing,
    paths: resolved,
    binDir: paths.binDir,
  };
}

export async function probeFile(filePath) {
  const { stdout } = await run(
    resolveTool("ffprobe"),
    [
      "-v",
      "error",
      "-show_entries",
      "format=duration:stream=width,height,codec_type",
      "-of",
      "json",
      filePath,
    ],
    { timeoutMs: 60_000 }
  );
  const data = JSON.parse(stdout.toString("utf8"));
  const duration = Number(data.format?.duration);
  let width = null;
  let height = null;
  for (const stream of data.streams || []) {
    if (stream.codec_type === "video" && stream.width && stream.height) {
      width = stream.width;
      height = stream.height;
      break;
    }
  }
  return {
    durationSec: Number.isFinite(duration) ? duration : null,
    width,
    height,
  };
}

/**
 * Fingerprint middle segment [startRatio, endRatio) of the audio stream.
 * Streams wav through ffmpeg → fpcalc so we never write a temp decode file.
 */
export async function fingerprintMiddle(
  filePath,
  durationSec,
  startRatio = 0.2,
  endRatio = 0.8
) {
  const start = Math.max(0, durationSec * startRatio);
  const length = Math.max(1, durationSec * (endRatio - startRatio));

  const wavArgs = [
    "-hide_banner",
    "-loglevel",
    "error",
    "-ss",
    String(start),
    "-t",
    String(length),
    "-i",
    filePath,
    "-vn",
    "-ac",
    "1",
    "-ar",
    "11025",
    "-c:a",
    "pcm_s16le",
    "-f",
    "wav",
    "pipe:1",
  ];

  return pipeFfmpegToFpcalc(wavArgs, Math.ceil(length) + 5);
}

function pipeFfmpegToFpcalc(ffmpegArgs, fpLengthSec) {
  const ffmpeg = resolveTool("ffmpeg");
  const fpcalc = resolveTool("fpcalc");

  return new Promise((resolve, reject) => {
    const ff = spawn(ffmpeg, ffmpegArgs, {
      windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const fp = spawn(
      fpcalc,
      ["-raw", "-length", String(fpLengthSec), "-"],
      {
        windowsHide: true,
        stdio: ["pipe", "pipe", "pipe"],
      }
    );

    let fpOut = "";
    let fpErr = "";
    let ffErr = "";
    let settled = false;

    const fail = (err) => {
      if (settled) return;
      settled = true;
      ff.kill("SIGKILL");
      fp.kill("SIGKILL");
      reject(err);
    };

    ff.stderr.on("data", (c) => {
      ffErr += c.toString("utf8");
    });
    fp.stdout.on("data", (c) => {
      fpOut += c.toString("utf8");
    });
    fp.stderr.on("data", (c) => {
      fpErr += c.toString("utf8");
    });

    ff.stdout.pipe(fp.stdin);
    ff.on("error", fail);
    fp.on("error", fail);

    ff.on("close", () => {});

    fp.on("close", (code) => {
      if (settled) return;
      settled = true;
      if (code !== 0) {
        reject(
          new Error(
            `fpcalc failed (${code}): ${fpErr.trim() || ffErr.trim() || "unknown"}`
          )
        );
        return;
      }
      const match = fpOut.match(/FINGERPRINT=(.+)/);
      if (!match) {
        reject(new Error(`fpcalc produced no fingerprint: ${fpOut || fpErr}`));
        return;
      }
      const values = match[1]
        .split(",")
        .map((x) => Number(x.trim()))
        .filter((n) => Number.isFinite(n));
      if (!values.length) {
        reject(new Error("empty fingerprint"));
        return;
      }
      resolve(values);
    });
  });
}

export function packFingerprint(values) {
  const buf = Buffer.allocUnsafe(values.length * 4);
  for (let i = 0; i < values.length; i++) {
    buf.writeInt32BE(values[i] | 0, i * 4);
  }
  return buf;
}

export function unpackFingerprint(buf) {
  if (!buf || !buf.length) return [];
  // node:sqlite returns BLOBs as Uint8Array, not Buffer
  const bytes = Buffer.isBuffer(buf) ? buf : Buffer.from(buf);
  const out = new Array(Math.floor(bytes.length / 4));
  for (let i = 0; i < out.length; i++) {
    out[i] = bytes.readInt32BE(i * 4);
  }
  return out;
}

/** Compare two chromaprint int arrays; returns 0..1 similarity. */
export function fingerprintSimilarity(a, b) {
  if (!a?.length || !b?.length) return 0;
  const short = a.length <= b.length ? a : b;
  const long = a.length <= b.length ? b : a;
  const maxOff = 8;
  let best = 0;
  for (let off = -maxOff; off <= maxOff; off++) {
    let matched = 0;
    let compared = 0;
    for (let i = 0; i < short.length; i++) {
      const j = i + off;
      if (j < 0 || j >= long.length) continue;
      compared++;
      const xor = (short[i] ^ long[j]) >>> 0;
      const diffBits = bitCount(xor);
      if (diffBits <= 8) matched++;
    }
    if (compared > 0) best = Math.max(best, matched / compared);
  }
  return best;
}

function bitCount(n) {
  n = n - ((n >>> 1) & 0x55555555);
  n = (n & 0x33333333) + ((n >>> 2) & 0x33333333);
  return (((n + (n >>> 4)) & 0x0f0f0f0f) * 0x01010101) >>> 24;
}
