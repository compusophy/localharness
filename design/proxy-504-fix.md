# Proxy 504 on slow claude requests — root cause + staged fix

**Status: DIAGNOSED, fix STAGED, NOT deployed.** Hold the deploy for an attended
smoke — this is the live inference path for every user and the fix's correctness
depends on the Vercel plan (below). This note is the greenlight-ready hand-off.

## Evidence from a real frontier run (GH Actions run 30610629205, 2026-07-31)
Dispatched `terminal-bench-2.1` with `model=claude-sonnet-5 --n-tasks 1`. Task =
`terminal-bench/write-compressor`. Harbor result: **Trials 1, Exceptions 0, reward
0.0 → 0/1**. Zero exceptions hides the real story — the agent transcript
(`/logs/agent/localharness.txt`) is **5 lines**: the two round-1 recon tool calls
(`view_file /app/decomp.c`, `list_directory /app`) and then **nothing** — no round
2, no final answer, and no `work failed:` error line. `artifacts/logs/artifacts` is
**empty**; `data.comp` was never created. Interpretation: round 1 (small context)
came back fast; round 2 (context now includes decomp.c) hit claude's slow first
token, the edge function 504'd, `retry.rs` re-tried the deterministic timeout until
harbor's per-agent timeout **SIGKILL'd** the process (hence a truncated log with no
clean error). The 0/1 is a **proxy artifact, not a capability measure** — claude
never got to attempt the task. This is why frontier numbers are worthless until the
504 is fixed, and why the install/key/routing path is otherwise PROVEN (recon ran).

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
a failed request" invariant). Vercel **edge** functions enforce a ~25s wall-clock cap. Two sub-mechanisms, and
the fix covers both so we don't need to disambiguate before deploying: (a) if
claude is slow to FIRST token (`await fetch` sends `accept: text/event-stream`, so
it resolves on upstream HEADERS — a slow first token delays even the headers),
the function is killed before any Response exists; (b) if headers come back
promptly but the full stream runs long, it's a mid-stream TOTAL-duration kill.
The real run below (round 1 fast, round 2 dead once context grew) fits either. The
recommended Node fix (`maxDuration: 300`, no ~25s edge cap) resolves BOTH; the
preview smoke (step 3) tells us which it was, only to decide the Hobby-plan
fallback. A keepalive on the response body can't help path (a) — there is no body
until the upstream headers arrive.

## Fix (recommended): move this ONE function to the Node runtime
The whole handler uses only standard Web APIs (`fetch`, `Response`,
`TransformStream`/`ReadableStream` in `meteredBody`, `upstream.body`) plus viem +
@noble/curves — all Node 18+ compatible. Nothing edge-specific (no `EdgeRuntime`,
no edge KV, no `waitUntil`). The billing FLOW is unchanged (same statements, same
order); Node serverless functions have **no ~25s first-byte cap** — the limit
becomes total `maxDuration`.

```diff
- export const config = { runtime: 'edge' };
+ export const config = { maxDuration: 300 };
```

⚠️ **Syntax matters — verified against the repo.** `nodejsNN.x` is the
`vercel.json functions[].runtime` value, NOT an inline `config.runtime` value
(grep: 27 files set `runtime: 'edge'`, zero set `nodejs`). REMOVING
`runtime: 'edge'` reverts the function to Vercel's DEFAULT Node.js runtime — do
not add a `nodejs20.x` string inline (it's silently ignored / rejected, leaving
the fn on edge and the 504 unfixed). Set only `maxDuration`. If a specific Node
major is required, pin it via `package.json` `engines`, not inline.

Two things are Node-runtime BEHAVIORS to VERIFY on a preview deploy, not assume:
(a) that Vercel's Node runtime accepts this function's web `Request→Response`
handler shape (it does — GA — but confirm the deploy builds), and (b) that it
streams the `ReadableStream` INCREMENTALLY rather than buffering the whole body
(a Node-streaming gotcha). Smoke step 2 below is the hard pass/fail gate for (b).

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

⚠️ Two consequences of committing an HTTP 200 before the upstream status is known,
to handle if this fallback is ever adopted: (1) **status inversion** — an upstream
4xx/5xx can no longer be surfaced as an HTTP status, so the client's transient-code
retry (`src/backends/retry.rs` retries only on 5xx/504 at stream-OPEN) never fires;
a genuine upstream outage becomes an in-band SSE error the client must be taught to
treat as retryable. (2) **uncharged-serve** — once 200 is sent, the primary path's
"502 and don't serve" on a `meterDebit` broadcast failure is impossible, so an
in-`start()` debit failure serves the call for free (revenue leak). Both are
acceptable ONLY because this path ships only if the Node plan ceiling forces it;
spell out an explicit policy (abort with a client-retryable SSE error frame) before
adopting it.

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
