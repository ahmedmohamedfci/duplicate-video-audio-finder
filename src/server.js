import express from "express";
import path from "node:path";
import { PORT, PUBLIC_DIR } from "./config.js";
import "./db.js";
import routes from "./routes.js";

const app = express();
app.use(express.json({ limit: "2mb" }));

// Never let long handlers starve — all heavy work is job-based.
app.get("/api/ping", (_req, res) => {
  res.json({ ok: true, ts: Date.now() });
});

app.use("/api", routes);
app.use(express.static(PUBLIC_DIR));

app.get("*", (req, res, next) => {
  if (req.path.startsWith("/api")) return next();
  res.sendFile(path.join(PUBLIC_DIR, "index.html"));
});

app.listen(PORT, "127.0.0.1", () => {
  console.log(`Duplicate Finder running at http://127.0.0.1:${PORT}`);
  console.log("UI stays responsive — scan/fingerprint/match run as background jobs.");
});
