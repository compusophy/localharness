// Scripted fake Gemini: each POST pops the NEXT scripted turn and streams it
// as properly TERMINATED SSE (unlike fake-gemini.mjs, which stalls forever —
// that one tests Stop; this one tests turn CLASSIFICATION flows). Captures
// every request body so a harness can assert what the client actually sent
// (the whole point of the stall-recovery E2E: proving the hidden nudge turn
// went to the model). CORS-open, loopback only.
import { createServer } from "node:http";

/// `script` = array of turns; each turn is EITHER an array of Gemini response
/// chunks (wire shape: {candidates:[{content:{...}, finishReason?}]}) streamed
/// as terminated SSE, OR an `httpErrorTurn(status, body)` record, which answers
/// the POST with that HTTP status + body instead of a stream (the upstream-
/// rejection path: the client never opens a stream at all).
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
      const entry = script[Math.min(turn, script.length - 1)];
      turn += 1;
      if (!Array.isArray(entry) && entry && entry.httpStatus) {
        res.writeHead(entry.httpStatus, { ...cors, "content-type": "application/json", "cache-control": "no-store" });
        res.end(entry.body);
        return;
      }
      res.writeHead(200, { ...cors, "content-type": "text/event-stream", "cache-control": "no-store" });
      for (const c of entry) res.write("data: " + JSON.stringify(c) + "\r\n\r\n");
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

/// A PROMPT-level block: Google refused the INPUT, so the frame carries
/// `promptFeedback.blockReason` and NOT ONE candidate — there is no
/// `finishReason` to map, which is exactly why an unmodelled decode read as a
/// blank turn ("check your session/balance") instead of a named block.
///
/// The shape MIRRORS the Rust fixture `wire::PROMPT_BLOCKED_FRAME_JSON`
/// (src/backends/gemini/wire.rs) field for field — keep the two in sync; that
/// fixture is synthesized from the documented v1beta shape, NOT captured live.
export function promptBlockedTurn(blockReason = "SAFETY") {
  return [
    {
      promptFeedback: {
        blockReason,
        safetyRatings: [
          { category: "HARM_CATEGORY_SEXUALLY_EXPLICIT", probability: "NEGLIGIBLE" },
          { category: "HARM_CATEGORY_HATE_SPEECH", probability: "HIGH" },
          { category: "HARM_CATEGORY_HARASSMENT", probability: "NEGLIGIBLE" },
          { category: "HARM_CATEGORY_DANGEROUS_CONTENT", probability: "NEGLIGIBLE" },
        ],
      },
      usageMetadata: { promptTokenCount: 274, totalTokenCount: 274 },
      modelVersion: "gemini-3.7-flash",
      responseId: "hRZuaqS7BfKe_uMP66G90AQ",
    },
  ];
}

/// A turn the upstream REJECTS: the POST answers `status` with `body` and no
/// stream ever opens (`api.rs` turns it into `Error::http_status`). Use a
/// Google-shaped error body — that string is all `error_codes::classify` has.
export function httpErrorTurn(status, body) {
  return { httpStatus: status, body };
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
