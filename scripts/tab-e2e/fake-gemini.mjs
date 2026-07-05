// Fake Gemini SSE endpoint: streams ONE text chunk then goes SILENT (holds
// the connection open) — a faithful "model paused mid-stream" state for the
// Stop-button cancel test. CORS-open. Nothing ever leaves the machine.
// Importable (`startFakeGemini(port)`) or standalone: node fake-gemini.mjs [port]
import { createServer } from "node:http";
import { pathToFileURL } from "node:url";

const CHUNK = JSON.stringify({
  candidates: [{ content: { role: "model", parts: [{ text: "Once upon a time, in a land of test fixtures" }] } }],
});

export function startFakeGemini(port) {
  const srv = createServer((req, res) => {
    const cors = {
      "access-control-allow-origin": "*",
      "access-control-allow-methods": "POST, GET, OPTIONS",
      "access-control-allow-headers": "*",
    };
    if (req.method === "OPTIONS") { res.writeHead(204, cors); res.end(); return; }
    res.writeHead(200, { ...cors, "content-type": "text/event-stream", "cache-control": "no-store" });
    res.write("data: " + CHUNK + "\r\n\r\n");
    // ... and now: total silence. Never end the response.
    req.on("close", () => res.destroy());
  });
  return new Promise((resolve) => srv.listen(port, "127.0.0.1", () => {
    console.log("fake gemini on http://127.0.0.1:" + port);
    resolve(srv);
  }));
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await startFakeGemini(Number(process.argv[2] || 8793));
}
