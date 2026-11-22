import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here =
  typeof __dirname !== "undefined"
    ? __dirname
    : path.dirname(fileURLToPath(import.meta.url));

/** Set by @yao-pkg/pkg at runtime inside the exe. */
const isPkg = typeof process.pkg !== "undefined";

/**
 * UI assets:
 * - packaged: dist/public next to the bundled entry in the pkg snapshot
 * - source:   <repo>/public
 */
function resolveStaticDir() {
  const candidates = isPkg
    ? [path.join(here, "public"), path.join(here, "dist", "public")]
    : [path.join(here, "..", "public")];
  for (const c of candidates) {
    if (fs.existsSync(path.join(c, "index.html"))) return c;
  }
  return candidates[0];
}

export const STATIC_DIR = resolveStaticDir();
export const BUNDLE_ROOT = isPkg ? here : path.resolve(here, "..");

/** Default data folder: beside the .exe when packaged, else repo root. */
export const ROOT_DIR = isPkg
  ? path.dirname(process.execPath)
  : path.resolve(here, "..");

function parseArgs(argv) {
  const out = {
    data: null,
    port: null,
    host: "127.0.0.1",
    help: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--help" || a === "-h") out.help = true;
    else if (a === "--data" || a === "-d") out.data = argv[++i];
    else if (a.startsWith("--data=")) out.data = a.slice("--data=".length);
    else if (a === "--port" || a === "-p") out.port = argv[++i];
    else if (a.startsWith("--port=")) out.port = a.slice("--port=".length);
    else if (a === "--host") out.host = argv[++i];
    else if (a.startsWith("--host=")) out.host = a.slice("--host=".length);
  }
  return out;
}

export const CLI = parseArgs(process.argv.slice(2));

if (CLI.help) {
  console.log(`Duplicate Finder

Usage:
  dupfinder.exe [options]
  node src/server.js [options]

Options:
  --data, -d <dir>   Data folder (DB, settings, trash). Default: <exe-or-app>/data
  --port, -p <n>     Port (default 3847). Use 0 to pick a free port.
  --host <addr>      Listen address (default 127.0.0.1)
  --help, -h         Show help

Examples:
  dupfinder.exe --data D:\\dup-runs\\batch-a
  dupfinder.exe --data D:\\dup-runs\\batch-b --port 0

Requires ffmpeg, ffprobe, and fpcalc on PATH (not bundled).
`);
  process.exit(0);
}

export const DATA_DIR = path.resolve(
  CLI.data || process.env.DUPFINDER_DATA || path.join(ROOT_DIR, "data")
);
export const DB_PATH = path.join(DATA_DIR, "catalog.sqlite");

const requestedPort = Number(
  CLI.port ?? process.env.PORT ?? process.env.DUPFINDER_PORT ?? 3847
);
export const REQUESTED_PORT = Number.isFinite(requestedPort) ? requestedPort : 3847;
export const HOST = CLI.host || process.env.DUPFINDER_HOST || "127.0.0.1";

fs.mkdirSync(DATA_DIR, { recursive: true });

export function resolvePort(preferred = REQUESTED_PORT) {
  return new Promise((resolve, reject) => {
    const tryListen = (port) => {
      const server = net.createServer();
      server.unref();
      server.on("error", (err) => {
        if (err.code === "EADDRINUSE" && port !== 0) {
          tryListen(port + 1);
          return;
        }
        reject(err);
      });
      server.listen(port, HOST, () => {
        const chosen = server.address().port;
        server.close(() => resolve(chosen));
      });
    };
    tryListen(preferred === 0 ? 0 : preferred);
  });
}
