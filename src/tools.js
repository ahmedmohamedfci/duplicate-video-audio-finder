import fs from "node:fs";
import path from "node:path";
import { BUNDLE_ROOT, ROOT_DIR } from "./config.js";

/**
 * Optional local tools next to the app (dev checkout).
 * Packaged .exe does NOT ship ffmpeg — use PATH.
 */
export const TOOLS_BIN = path.join(BUNDLE_ROOT, "tools", "bin");
const EXE_SIDE_BIN = path.join(ROOT_DIR, "tools", "bin");

const NAMES = {
  ffmpeg: "ffmpeg.exe",
  ffprobe: "ffprobe.exe",
  fpcalc: "fpcalc.exe",
};

export function resolveTool(name) {
  const file = NAMES[name] || `${name}.exe`;
  for (const dir of [TOOLS_BIN, EXE_SIDE_BIN]) {
    const local = path.join(dir, file);
    if (fs.existsSync(local)) return local;
  }
  return name; // PATH
}

export function toolPaths() {
  return {
    ffmpeg: resolveTool("ffmpeg"),
    ffprobe: resolveTool("ffprobe"),
    fpcalc: resolveTool("fpcalc"),
    binDir: TOOLS_BIN,
  };
}
