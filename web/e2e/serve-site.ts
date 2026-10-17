// The project's site, docs/, as GitHub Pages serves it: `node
// e2e/serve-site.ts <port>`, started by the e2e runner
// (e2e.site.config.ts, app.command). Files only, read from disk each time.
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const port = Number(process.argv[2]);
if (!Number.isInteger(port)) throw new Error("usage: node e2e/serve-site.ts <port>");

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "docs");
const TYPES: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".png": "image/png",
  ".gif": "image/gif",
  ".mp4": "video/mp4",
  ".woff2": "font/woff2",
  ".md": "text/markdown; charset=utf-8",
};

http
  .createServer((req, res) => {
    let rel: string;
    try {
      rel = decodeURIComponent(new URL(req.url ?? "/", "http://site").pathname).replace(/\/$/, "/index.html");
    } catch {
      // A malformed address (%E0) is the request's fault, not the server's end.
      res.writeHead(400).end("bad address");
      return;
    }
    const file = path.join(root, rel);
    // Nothing outside docs/.
    if (!file.startsWith(root + path.sep) || !fs.existsSync(file) || !fs.statSync(file).isFile()) {
      res.writeHead(404).end("not found");
      return;
    }
    res.writeHead(200, { "content-type": TYPES[path.extname(file)] ?? "application/octet-stream", "cache-control": "no-store" });
    fs.createReadStream(file)
      .on("error", () => res.destroy())
      .pipe(res);
  })
  .listen(port, "127.0.0.1");
