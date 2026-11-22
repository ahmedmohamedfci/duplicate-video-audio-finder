import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const ROOT_DIR = path.resolve(__dirname, "..");
export const DATA_DIR = path.join(ROOT_DIR, "data");
export const DB_PATH = path.join(DATA_DIR, "catalog.sqlite");
export const PUBLIC_DIR = path.join(ROOT_DIR, "public");
export const PORT = Number(process.env.PORT || 3847);
