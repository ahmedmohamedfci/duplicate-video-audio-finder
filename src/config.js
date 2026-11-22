import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const ROOT_DIR = path.resolve(__dirname, "..");
export const PUBLIC_DIR = path.join(ROOT_DIR, "public");

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
  node src/server.js [options]
  dupfinder.cmd [options]

Options:
  --data, -d <dir>   Data folder (DB, settings, trash). Default: <app>/data
  --port, -p <n>     Port (default 3847). Use 0 to pick a free port.
  --host <addr>      Listen address (default 127.0.0.1)
  --help, -h         Show help

Examples:
  dupfinder.cmd --data D:\\dup-runs\\batch-a
  dupfinder.cmd --data D:\\dup-runs\\batch-b --port 0
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

/** Resolve a free TCP port. 0 / negative → OS ephemeral; else start from preferred. */
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
