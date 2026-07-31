# Proxy 504 on slow claude requests — root cause + staged fix

**Status: DIAGNOSED, fix STAGED, NOT deployed.** Hold the deploy for an attended
smoke — this is the live inference path for every user and the fix's correctness
depends on the Vercel plan (below). This note is the greenlight-ready hand-off.

## Symptom
`localharness work --model claude-sonnet-5` (the Terminal-Bench 2.1 driver) dies
after ~2 model rounds with `FUNCTION_INVOCATION_TIMEOUT` (HTTP 504). Flash
(`gemini-3.6-flash`) survives ~8 rounds before the same death. Deterministic —
client retry (`src/backends/retry.rs` already retries `BACKEND_SERVER`/504 up to
`MAX_STREAM_ATTEMPTS` on an OPEN stream) can't help because every retry re-times
out identically.

## Root cause
`proxy/api/gemini.ts` is `export const config = { runtime: 'edge' }` (line 45).
The handler does `const upstream = await fetch(<provider>)` and only builds the
`Response` **after** the upstream responds (so the 2xx status can gate billing —
`meterDebit`/`settleX402NoWait` fire only on `upstream.ok`, the "never charge for
a failed request" invariant). Vercel **edge** functions enforce a ~25s
**first-byte** timeout: when claude is slow to first token (extended thinking +
large TB context), that `await fetch` hasn't resolved at 25s, so Vercel kills the
function **before any Response exists**. A keepalive on the response body can't
help — there is no body yet; the clock runs on the `await`, not the stream.

## Fix (recommended): move this ONE function to the Node runtime
The whole handler uses only standard Web APIs (`fetch`, `Response`,
`TransformStream`/`ReadableStream` in `meteredBody`, `upstream.body`) plus viem +
@noble/curves — all Node 18+ compatible. Nothing edge-specific (no `EdgeRuntime`,
no edge KV, no `waitUntil`). So the billing flow is byte-identical under Node;
only the timeout model changes. Node serverless functions have **no ~25s
first-byte cap** — the limit becomes total `maxDuration`.

```diff
- export const config = { runtime: 'edge' };
+ export const config = { runtime: 'nodejs20.x', maxDuration: 300 };
```

The in-memory `reserve`/`release` burst maps stay best-effort/advisory under Node
(they already are per-isolate); nonce serialization is on-chain (the awaited
`meterDebit` broadcast), not in-memory, so billing correctness is unaffected.

## ⚠️ The one open question — Vercel plan ceiling (verify before deploy)
Node `maxDuration` caps by plan: **Hobby = 60s (hard)**, **Pro = up to 300s**,
Enterprise higher. This is the WHOLE function budget, not first-byte:
- If **Pro+**, `maxDuration: 300` covers a full slow-claude round (thinking +
  long output, ~30–120s). Ship it.
- If **Hobby (60s cap)**, a long claude response could exceed 60s *total* and get
  killed **mid-stream** — arguably worse than today (edge at least streams once
  it starts). In that case DON'T use the Node fix; use the edge fallback below.

Confirm the plan first: `vercel projects ls` / dashboard, or just deploy to a
preview and time a claude round.

## Fallback (if the plan caps node maxDuration too low): edge keepalive-first
Stay `runtime: 'edge'` but stop awaiting `fetch` before returning. Return a
`Response(new ReadableStream({ start(c){…} }))` immediately (satisfies the
first-byte clock), and INSIDE `start`: emit an SSE keepalive comment (`: ping\n\n`
— our decoder `src/backends/sse.rs` skips comment/heartbeat frames, verified),
then `await fetch(upstream)`, then re-establish the 2xx-before-charge gate
*inside the stream* (peek `upstream.status`; on non-2xx enqueue the error body
and close WITHOUT charging; on 2xx run the existing `meterDebit`/settle then pipe
`upstream.body`). Bigger surface — it relocates the billing gate into the stream
— so it's the fallback, not the default.

## Smoke plan (attended, in order)
1. Deploy: `cd proxy && vercel --prod`.
2. Cheap first: `localharness work --as tbench --model gemini-3.6-flash "list the
   files here and read one"` — confirm streaming still flows incrementally (Node
   streaming can buffer if misconfigured; watch for a lump-sum vs incremental
   response).
3. The real test: `localharness work --as tbench --model claude-sonnet-5 "<a
   task that made it 504 before>"` — confirm it runs >2 rounds without 504.
4. Then the frontier TB run: dispatch `terminal-bench-2.1` workflow with
   `model=claude-sonnet-5` for the first real frontier scoreline.

## Why this wasn't auto-deployed
Core inference path for all users + a plan-dependent correctness question +
a known Node-streaming-buffering gotcha class = must be watched live, not shipped
during an unattended overnight loop. TB integration itself is DONE and the claude
routing is PROVEN (cost + `-m` echo); this 504 is the sole blocker to a completed
frontier number, and the number can wait for a 5-minute attended smoke.
