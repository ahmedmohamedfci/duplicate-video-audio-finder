import express from "express";
import path from "node:path";
import {
  DATA_DIR,
  DB_PATH,
  HOST,
  PUBLIC_DIR,
  REQUESTED_PORT,
  ROOT_DIR,
  resolvePort,
} from "./config.js";
import "./db.js";
import routes from "./routes.js";

const app = express();
app.use(express.json({ limit: "2mb" }));

app.get("/api/ping", (_req, res) => {
  res.json({
    ok: true,
    ts: Date.now(),
    dataDir: DATA_DIR,
    dbPath: DB_PATH,
  });
});

app.get("/api/instance", (_req, res) => {
  res.json({
    dataDir: DATA_DIR,
    dbPath: DB_PATH,
    rootDir: ROOT_DIR,
    host: HOST,
  });
});

app.use("/api", routes);
app.use(express.static(PUBLIC_DIR));

app.get("*", (req, res, next) => {
  if (req.path.startsWith("/api")) return next();
  res.sendFile(path.join(PUBLIC_DIR, "index.html"));
});

const port = await resolvePort(REQUESTED_PORT);

app.listen(port, HOST, () => {
  console.log(`Duplicate Finder`);
  console.log(`  URL:      http://${HOST}:${port}`);
  console.log(`  Data:     ${DATA_DIR}`);
  console.log(`  Database: ${DB_PATH}`);
  if (port !== REQUESTED_PORT && REQUESTED_PORT !== 0) {
    console.log(`  Note:     port ${REQUESTED_PORT} was busy — using ${port}`);
  }
});
