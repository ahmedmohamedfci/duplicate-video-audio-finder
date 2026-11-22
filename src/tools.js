import fs from "node:fs";
import path from "node:path";
import { ROOT_DIR } from "./config.js";

/** Prefer bundled binaries in tools/bin; fall back to PATH later if missing. */
export const TOOLS_BIN = path.join(ROOT_DIR, "tools", "bin");

const NAMES = {
  ffmpeg: "ffmpeg.exe",
  ffprobe: "ffprobe.exe",
  fpcalc: "fpcalc.exe",
};

export function resolveTool(name) {
  const local = path.join(TOOLS_BIN, NAMES[name] || `${name}.exe`);
  if (fs.existsSync(local)) return local;
  // Fallback: bare name on PATH (for when you switch later)
  return name;
}

export function toolPaths() {
  return {
    ffmpeg: resolveTool("ffmpeg"),
    ffprobe: resolveTool("ffprobe"),
    fpcalc: resolveTool("fpcalc"),
    binDir: TOOLS_BIN,
  };
}
