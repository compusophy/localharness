# Metering economics — flat-per-request is a money leak

> Deep dive (2026-06-16). Verdict: the flat `$LH`-per-request charge is
> **exploitable AND loses money on honest agentic traffic**. Fix: bill on actual
> token usage (with margin + a hard output cap); flat is at most a *floor*.

## The structural flaw

`proxy/api/_prices.ts::priceOf()` returns a **flat** per-model charge (Gemini
0.01 `$LH`; Claude haiku/sonnet/opus 0.01/0.05/0.20; OpenAI tiers alike),
clamped *down* by `MAX_COST_PER_REQUEST_WEI`. `gemini.ts` debits that flat amount
and **never reads `usageMetadata`** — the charge is fully decoupled from the real
per-token cost we pay the upstream. So:

```
margin = flat_charge − real_token_cost
```

…where `real_token_cost` is set by the *caller* (input length up to ~1M-token
windows; output length up to 66k–128k, billed 3–6× input; × the `run_send` loop,
`MAX_AUTO_CONTINUATIONS=10`, over a transcript that grows each turn). For any
fixed charge an adversary drives `real_token_cost` arbitrarily high → margin is
unbounded-negative. No surveyed production system (OpenRouter, Bedrock,
Helicone/Portkey) meters variable LLM cost with a flat per-request fee.

## Numbers (live-verified Gemini + Anthropic; OpenAI GPT-5 estimated, flagged)

Per 1M tokens in/out: `gemini-3.5-flash` $1.50/$9.00 ($0.15 cached);
`gemini-2.5-flash` $0.30/$2.50; Haiku $1/$5; Sonnet $3/$15; Opus $5/$25.

- **Break-even (1:3 in:out): `gemini-3.5-flash` (the DEFAULT) ≈ 1.4k tokens.** A
  routine 1k-in/3k-out turn already costs $0.0285 = **2.85× the $0.01 charge.**
  Haiku ~2.6k, Sonnet ~4.2k, Opus ~10k. Only `gemini-2.5-flash` (~5k) has real
  headroom on $0.01.
- **Worst case, one request** (1M in + max out): Gemini ~$2.09 vs $0.01 (**209×**);
  Opus ~$8.20 vs $0.20 (**41×**).
- **Worst case, one user message** (10-deep `run_send`, context held high): Gemini
  ~$20.7 net loss; Opus ~$80.
- **Output-asymmetry attack** (tiny prompt, max output): Gemini 66k out = $0.594
  (**59×** the $0.01); Opus 128k out = $3.20 (16×). Invisible to any input-side guard.
- **Honest distribution** (`gemini-3.5-flash` vs $0.01): median ≈ break-even, mean
  (history-grown ~15k-in) ≈ $0.034 (loss $0.024), p95 ≈ $0.19, p99 ≈ $0.46.

**The `MAX_COST_PER_REQUEST_WEI` clamp does NOT protect margin** — code-confirmed
it clamps the *charge* down, never the upstream *pay*. A 1M-token Opus call costs
$5+ to serve while we bill ≤$1: a guaranteed loss even at the ceiling.

## Fix — bill on actual usage (the universal pattern)

### Near-term (proxy-only; no facet/x402 change)
- **A. Flat → FLOOR.** After the upstream 2xx, read `usageMetadata`
  (input/output/cached counts — in every provider response) and debit
  `max(flat_floor, real_cost × margin)` from a per-model rate table (input rate,
  output rate at its real 3–6× weight, cached at ~0.1×). Tiny requests still pay
  the flat $0.01; big/long ones pay their way. Moves margin from
  unbounded-negative to ≥0 on every request.
- **B. Hard caps BEFORE forwarding.** Cap upstream `max_tokens` to a bounded value
  (NOT the model's 66k/128k ceiling) — neutralizes the output-asymmetry attack,
  the highest-leverage exploit — and reject/charge-scale oversized input. Ship
  regardless of pricing model.
- Reclassify the clamp as overcharge protection (not margin). Add a per-identity
  sliding-window spend cap; meter **inside** the `run_send` loop on real usage per
  request, not once per user message.

### Medium-term — x402 "Upto" (sign-max / settle-actual)
Coinbase x402 "Upto" (shipped 2026-04-10) is the near drop-in: the caller signs a
per-request **MAX** `$LH` (sized from `max_tokens` × list rate × margin); the proxy
computes the final cost from the post-response token count and settles **only the
actual**, up to the max — refunding the difference. Maps directly onto the existing
`X402Facet.settle()` (settle_actual ≤ signed_max is compatible). Loss bounded to
zero per request; `$LH` stays the unit. **Post-paid serve-then-bill is NOT the
primary rail** — it creates an unsecured receivable (caller triggers a max-cost
response, vanishes); sign-a-max closes that.

### Long-term — `$LH` streaming vouchers (the `run_send` loop as ONE channel)
Open one `$LH` payment channel (deposit + `maxDeposit` cap), issue off-chain
ecrecover-only vouchers carrying the **cumulative** `$LH` owed (one tick per N
output tokens / SSE event), settle periodically + on close, refund unused. The
whole 10-deep loop becomes one capped session, not 10 charges. **Caveat: Tempo
MPP settles USDC.e, NOT `$LH`** — so this needs OUR OWN cumulative-voucher + settle
mechanism (a thin channel facet, or extend `X402Facet` with a monotonic-claim
nonce), not stock MPP.

## Open business decisions (need the owner)
- **Margin multiple** — 1.3× (thin growth) vs higher (to fund sponsor gas + the
  fiat-ramp Stripe spread). Sets subsidized vs break-even vs profitable.
- **Input-cap policy** — hard reject above N, or charge-scale (let big legit
  RAG/code turns through, bill them fully)?
- **`max_tokens` cap** — global, or derived from the caller's signed budget so it
  auto-scales with willingness-to-pay?
- **Rate-table freshness** — env-driven (like `_prices.ts`) + a scheduled
  re-verify against live provider pricing (model IDs + prices drift; see CLAUDE.md).
- **Verify OpenAI GPT-5 pricing** against the official page before any GPT margin —
  current figures are aggregator estimates; GPT-5.5 Pro at ~$30/$180 would be a
  landmine at a 0.20 flat.

## Shipped so far
- **Env-gated request caps** in `gemini.ts` (Option B, OFF by default →
  byte-identical to today): set `LH_MAX_OUTPUT_TOKENS` to cap upstream max-output
  per request (kills the 59× output attack), and `LH_MAX_CREDITS_BODY_BYTES` to
  bound input. Deployed flag-off; flip the envs to enable. (OpenAI now also
  set-if-absent so the cap binds there too.)
- **Option A FOUNDATION (`proxy/api/_usage.ts`) — built + tested.** Pure, verified
  building blocks: a per-model per-token rate table (live-verified Gemini +
  Anthropic; OpenAI estimated), `usageCostWei(provider, model, usage, marginBps)`,
  `extractUsage(provider, sse)` SSE parsers for all three providers, and
  `meteredAmountWei(...)` = `max(floor, usageCostWei(extractUsage(sse)))` (the
  whole live debit decision, one pure call). Verified by
  `scripts/test-metering-usage.mjs` (14 hand-computed assertions, Node 20).
- **Option A WIRED into `gemini.ts` — FLAG-GATED, default OFF (byte-identical to
  today when off; verified by reading + `tsc`).** When `LH_TOKEN_METERING=1`, a
  METER-path caller (funded `creditOf`, NOT x402 / not session-only-free) gets a
  passthrough `meteredBody` tee: every byte streams to the caller VERBATIM, and on
  stream-END (`TransformStream.flush`, which runs inside the response lifetime — no
  `waitUntil` dependency) the caller is debited `meteredAmountWei(...)`. OpenAI gets
  `stream_options.include_usage` injected so it emits usage. x402 stays flat-exact
  (token-based x402 = "Upto", below); session-only stays free. No-usage SSE / early
  client disconnect → falls back to the flat floor (never free). The
  passthrough-byte-fidelity + flush-amount are unit-proven (test above).

## Go-live for Option A (the SUPERVISED activation — ~2 min, owner-watched)
The CODE is shipped; activating it is intentionally a supervised flip because
**Vercel Edge inlines `process.env` at BUILD time** — flipping the flag needs a
redeploy, not just a dashboard env change:
1. **Pick the margin** (business): `LH_MARGIN_BPS` (default `13000` = 1.3×; 10000 =
   raw cost). Also strongly consider setting `LH_MAX_OUTPUT_TOKENS` (Option B) so a
   max-output request can't outrun the affordability gate (the gate still checks
   `creditOf >= flat floor`, not the actual).
2. **Set the envs** on the `proxy` Vercel project: `LH_TOKEN_METERING=1` (+
   `LH_MARGIN_BPS`, `LH_MAX_OUTPUT_TOKENS`).
3. **Redeploy** the proxy (`cd proxy && vercel --prod`) so the new env is inlined.
4. **LIVE-verify** each provider actually emits usage in its SSE (the one thing
   fixtures can't prove): make one real metered call per provider and confirm the
   debit ≈ the hand-computed token cost (not the flat floor). If a provider returns
   no usage frame, `meteredAmountWei` falls back to the floor — so a wiring miss
   under-charges, never over-charges.

## x402 "Upto" (sign-max / settle-actual) — FACET + PROXY built, staged
The agent-to-agent x402 rail (today flat-exact) gets token-metering via the same
sign-max/settle-actual shape as Coinbase x402 "Upto":
- **`X402Facet.settleUpto(from, to, maxValue, actualValue, …)` — DONE + tested**
  (`contracts/test/X402Upto.t.sol`, 9 tests). The payer signs an authorization
  whose `value` is a MAX; the facilitator measures the actual and this moves
  `min(actual, max)` — never more (`AmountExceedsMax`). Reuses the existing
  `PaymentAuthorization` typehash (the signed digest is over `maxValue`) and SHARES
  the one-shot `(from, nonce)` with `settle`, so a max-auth is consumable exactly
  once by either path. CEI/replay/window/low-s/EIP-1271 identical to `settle`.
- **Proxy side — DONE (staged, flag-gated, byte-identical when off; tsc clean).**
  `_x402.ts`: an X402Auth `scheme` field (`'exact'` default / `'upto'`), the
  overpay-ceiling skipped for `'upto'` (a generous max is intended), and
  `settleUptoNoWait(auth, actualWei)` (caps the actual at the signed max).
  `gemini.ts`: when `LH_TOKEN_METERING` is on and the caller's x402 auth is
  `'upto'`, the settle is DEFERRED to the response tee — `meteredAmountWei(...)` is
  settled via `settleUpto` on stream-end (vs the immediate exact settle). An
  `'upto'` auth with token-metering OFF is rejected (would overcharge as exact).
- **Remaining (supervised):** (1) `diamondCut` ADD the `settleUpto` selector to the
  live X402Facet (a money-critical recut — NOT done autonomously); (2) the
  caller/CLI signs a MAX + sets `scheme:'upto'` in X-PAYMENT (sized from
  `max_tokens × rate × margin`), and the proxy advertises `x402-upto` in its 402
  challenge. Until (1)+(2) the proxy ACCEPTS upto but no client sends it and the
  selector isn't cut — fully inert.

## Still future
- **`$LH` streaming vouchers** — the whole `run_send` loop as one capped channel.
