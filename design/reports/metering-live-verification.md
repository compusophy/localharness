# Token-metering LIVE SSE verification (the supervised-flip prerequisite)

> 2026-06-18. Ran the HUMAN-REVIEW CHECKLIST in `proxy/api/gemini.ts`
> (`TOKEN_METERING` flag) against the LIVE mainnet proxy
> (`https://proxy-tau-ten-15.vercel.app`, chain 4217). Verdict below.
> Companion to `design/metering.md` (the economics).

## What was tested

Real credit-path calls through the live proxy with a funded mainnet identity
(`claude` = `0x63140875…8561`; meter pre-funded 10 $LH via `topup`, wallet→meter
`depositCredits`). Auth = `address:timestamp:signature` personal-sign over
`localharness-proxy:<addr>:<ts>` (the same token the browser/CLI send). Each raw
SSE was captured and run through a faithful port of `_usage.ts::extractUsage`.

Metering is still OFF in prod (`LH_TOKEN_METERING` unset), so these calls billed
the FLAT floor — confirmed: the meter dropped 1.00 $LH per successful call (5
calls → 10 → 5 $LH). The verification is about whether the usage FRAMES are
present + parseable, which is independent of the flag.

## Per-provider results

### Gemini (`gemini-3.5-flash`, the default) — usage frame PRESENT, parses ✅ (one caveat)
Live `usageMetadata` (last/cumulative chunk, small call):
```
{"promptTokenCount":20,"candidatesTokenCount":30,"totalTokenCount":343,
 "thoughtsTokenCount":293,"promptTokensDetails":[...],"serviceTier":"standard"}
```
`extractUsage('gemini', sse)` → `{inputTokens:20, outputTokens:30, cachedInputTokens:0}` ✅.

CAVEAT (margin, not correctness): Gemini 3.x emits a **`thoughtsTokenCount`** the
parser does NOT read. Google bills reasoning tokens at the OUTPUT rate
(`totalTokenCount = prompt + candidates + thoughts`), so `extractUsage` UNDER-counts
output by exactly `thoughtsTokenCount`. Live big call: 35 in / 2224 candidates /
**1481 thoughts** → parser bills 2224 out, Google charges us 3705 out — a 66%
under-count of the metered figure. This only ever UNDER-charges (never over), and at
today's 1 $LH floor it is moot (see below), but if floors drop or thinking budgets
grow it is a real leak. Fix when it matters: add `thoughtsTokenCount` to
`outputTokens` (or use `totalTokenCount - promptTokenCount`).

### Anthropic (`claude-haiku-4-5-20251001`) — usage frame PRESENT, parses ✅
Live `message_start.message.usage` → `input_tokens:27, cache_read_input_tokens:0`
(+ placeholder `output_tokens:8`, ignored); final `message_delta.usage` →
`output_tokens:44` (cumulative). `extractUsage('anthropic', sse)` →
`{inputTokens:27, outputTokens:44, cachedInputTokens:0}` ✅ — exactly the documented
shape. A 3615-input call parsed correctly too (input read from BOTH frames; delta
also re-carries `input_tokens`/`cache_read_input_tokens`). A forced-cache attempt did
NOT engage caching (proxy forwards the body verbatim; the system block didn't trip
the cache), so the `cache_read_input_tokens:0` FALLBACK path was exercised live and
is correct — the cached-discount math is unit-proven in `test-metering-usage.mjs`
but a live cache HIT was not observed.

### OpenAI — NOT a live path (no key)
The proxy returns `500 proxy missing OPENAI_API_KEY` BEFORE any charge. OpenAI is
parked; no call can be served, metered or not. Its usage frame (the late `usage`
chunk, only with `stream_options.include_usage`, which the handler injects when
metering is on) is therefore UNVERIFIABLE live and IRRELEVANT to the flip. If
OpenAI is ever enabled, re-run this check first.

## Billing delta (why the flip is SAFE today)

`meteredAmountWei = max(floor, usageCostWei × margin)`. With the live floors
(Gemini 1 $LH, Haiku 1, Sonnet 5, Opus 20) the metered figure is FAR below the floor
for normal traffic, so almost everything still bills the floor:

| call (live)                     | extractUsage cost @1.3x | floor | billed (metered ON) |
|---------------------------------|-------------------------|-------|---------------------|
| Gemini small (20/30)            | 0.00039 $LH             | 1 $LH | **1 $LH (floor)**   |
| Gemini 800-word essay (35/2224) | 0.026 $LH (0.043 w/thoughts) | 1 $LH | **1 $LH (floor)** |
| Haiku small (27/44)             | 0.00032 $LH             | 1 $LH | **1 $LH (floor)**   |
| Haiku 3615-in (3615/4)          | 0.0047 $LH              | 1 $LH | **1 $LH (floor)**   |

Break-even on the 1 $LH Gemini floor @1.3x ≈ ~85k output tokens — only a genuinely
huge response exceeds it. So flipping `LH_TOKEN_METERING=1` with today's floors is
nearly a no-op on billing: it can only RAISE the charge on outlier mega-requests
(the exact margin leak metering exists to close), never lower it. The
`thoughtsTokenCount` under-count is fully masked by the floor at these prices.

## GO / NO-GO

**GO** — safe to flip with the current floors. Both live providers (Gemini,
Anthropic) emit a usage frame that `extractUsage` parses to non-zero input/output;
OpenAI is inert (no key, no charge). The math is floor-clamped so the flip only
bites on outlier requests, where it correctly bills above the flat floor instead of
eating an unbounded loss. No-usage / disconnect falls back to the floor (never free).
`RATE_USD` matches the live provider pricing in `metering.md` (Gemini 3.5 flash
$1.5/$9, Haiku $1/$5, Sonnet $3/$15, Opus $5/$25). `scripts/test-metering-usage.mjs`
green (22 assertions); `tsc --noEmit` clean.

Two NON-blocking follow-ups to track (file before lowering any floor):
1. Add Gemini `thoughtsTokenCount` to `outputTokens` (66% output under-count on
   thinking-heavy calls; masked by the floor today).
2. No live Anthropic cache HIT observed — the `cache_read_input_tokens` discount is
   unit-tested only. Re-verify with a real cache hit before relying on the discount.

## The exact flip (HUMAN runs — NOT done here)

On the `proxy` Vercel project ONLY (Edge inlines `process.env` at build → needs a
redeploy, not just a dashboard edit):
```sh
vercel env add LH_TOKEN_METERING   # value: 1
vercel env add LH_MARGIN_BPS       # OPTIONAL — default 13000 (= 1.3×); 10000 = raw cost
cd proxy && vercel --prod          # redeploy so the new env inlines
```
Then watch the first metered debits. `LH_MAX_OUTPUT_TOKENS` is already defaulted to
8192 (caps the output-asymmetry blast radius). To roll back: remove
`LH_TOKEN_METERING` (or set ≠ 1) and redeploy — the flat-floor path is byte-identical.
