// Minimal static server for web/ with correct wasm MIME (no deps).
// Importable (`serve(root, port)`) or standalone: node serve.mjs <root> [port]
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { pathToFileURL } from "node:url";

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".wasm": "application/wasm",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".webmanifest": "application/manifest+json",
  ".txt": "text/plain; charset=utf-8",
  ".md": "text/plain; charset=utf-8",
};

/// Start the server; resolves once listening. READ-ONLY over `root`.
export function serve(root, port) {
  const srv = createServer(async (req, res) => {
    try {
      let p = new URL(req.url, "http://x").pathname;
      if (p === "/") p = "/index.html";
      const file = normalize(join(root, p));
      if (!file.startsWith(normalize(root))) throw new Error("traversal");
      const body = await readFile(file);
      res.writeHead(200, {
        "content-type": MIME[extname(file).toLowerCase()] || "application/octet-stream",
        "cache-control": "no-store",
      });
      res.end(body);
    } catch {
      res.writeHead(404);
      res.end("not found");
    }
  });
  return new Promise((resolve) => srv.listen(port, "127.0.0.1", () => {
    console.log(`serving ${root} on http://127.0.0.1:${port}`);
    resolve(srv);
  }));
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await serve(process.argv[2], Number(process.argv[3] || 8792));
}
