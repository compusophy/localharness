// Scripted fake Gemini: each POST pops the NEXT scripted turn and streams it
// as properly TERMINATED SSE (unlike fake-gemini.mjs, which stalls forever —
// that one tests Stop; this one tests turn CLASSIFICATION flows). Captures
// every request body so a harness can assert what the client actually sent
// (the whole point of the stall-recovery E2E: proving the hidden nudge turn
// went to the model). CORS-open, loopback only.
import { createServer } from "node:http";

/// `script` = array of turns; each turn = array of Gemini response chunks
/// (already in wire shape: {candidates:[{content:{...}, finishReason?}]}).
/// Returns { server, requests } — `requests` accumulates parsed POST bodies.
export function startScriptedGemini(port, script) {
  const requests = [];
  let turn = 0;
  const server = createServer((req, res) => {
    const cors = {
      "access-control-allow-origin": "*",
      "access-control-allow-methods": "POST, GET, OPTIONS",
      "access-control-allow-headers": "*",
    };
    if (req.method === "OPTIONS") { res.writeHead(204, cors); res.end(); return; }
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try { requests.push(JSON.parse(body)); } catch { requests.push({ raw: body }); }
      const chunks = script[Math.min(turn, script.length - 1)];
      turn += 1;
      res.writeHead(200, { ...cors, "content-type": "text/event-stream", "cache-control": "no-store" });
      for (const c of chunks) res.write("data: " + JSON.stringify(c) + "\r\n\r\n");
      res.end();
    });
  });
  return new Promise((resolve) =>
    server.listen(port, "127.0.0.1", () => resolve({ server, requests }))
  );
}

/// One terminated text turn (the shape a FinalAnswer classifies from).
export function textTurn(text) {
  return [
    { candidates: [{ content: { role: "model", parts: [{ text }] } }] },
    { candidates: [{ content: { role: "model", parts: [] }, finishReason: "STOP" }],
      usageMetadata: { promptTokenCount: 100, candidatesTokenCount: 20 } },
  ];
}

/// A turn that calls ONE function then stops (the client executes the tool,
/// then fires the NEXT request with the result appended).
export function functionCallTurn(name, args) {
  return [
    { candidates: [{ content: { role: "model", parts: [{ functionCall: { name, args } }] } }] },
    { candidates: [{ content: { role: "model", parts: [] }, finishReason: "STOP" }],
      usageMetadata: { promptTokenCount: 120, candidatesTokenCount: 15 } },
  ];
}
