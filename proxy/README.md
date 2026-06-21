# localharness-proxy

A Gemini **credit proxy** for the localharness platform, deployed as a Vercel
**Edge Function** (TypeScript).

The browser app (Rust/wasm), in **platform-credits** mode, points its
`GeminiClient` at this proxy via `with_base_url` and sends the **same path and
body** it would send to Google. The proxy holds the real Gemini API key (in its
Vercel env, **never** shipped to the browser), authenticates the caller, checks
the caller has a valid on-chain credit session, and — if so — forwards the
request to Gemini (real key injected as a header, never in the URL) and streams
the SSE response back. It is a transparent reverse-proxy: `vercel.json` rewrites
`/v1beta/*` to the function, so the proxy origin is a drop-in Gemini base URL.

This is the **metering point** for `$LH` credits and the future home of
x402 / MPP per-request payment hooks.

## Deploy

This is its **own Vercel project**, separate from the static site in `web/`.

```sh
cd proxy
vercel deploy --prod
```

Set the env in **this project** (the real key lives ONLY here — never bundled
into the wasm):

```sh
vercel env add GEMINI_API_KEY production        # the platform Gemini key
vercel env add ANTHROPIC_API_KEY production     # optional; enables claude-* via /v1/messages
vercel env add OPENAI_API_KEY production        # optional; enables gpt-* via /v1/chat/completions
vercel env add PROXY_METER_KEY production       # meter EOA priv key (per-request mode)
vercel env add COST_PER_REQUEST_WEI production  # optional; default 0.01 LH (1e16)
vercel env add MAX_COST_PER_REQUEST_WEI production  # optional; per-call debit ceiling, default 1 LH (1e18)
```

## MPP on-ramp (`/mpp/onramp`) — USDC.e → $LH for autonomous agents

The crypto-native sibling of the Stripe fiat on-ramp: an agent pays **USDC.e on
Tempo mainnet** and gets `$LH` minted at **web parity (1 USDC.e = 100 $LH**, the
same `$1 = 100 $LH` issuance rate as the card path). No human, no card — the
fully self-onboarding rail (`design/cli-mainnet-onboarding.md` C-2). Minting goes
through the **same MintGateFacet / `mintFromFiat` / `ISSUER_ROLE`** valve as the
Stripe webhook (`api/_stripe.ts` helpers); the only new surface is the on-chain
USDC.e settlement verify in `api/_mpp.ts`.

The flow is the MPP **charge intent** (`docs.stripe.com/payments/machine/mpp`,
equivalent to x402 "exact"):

1. The agent `POST /mpp/onramp` (auth token in `x-goog-api-key`, identical to the
   other routes) **without** a payment credential → **402** + a
   `WWW-Authenticate: Payment` challenge (`method="tempo"`, `intent="charge"`, a
   base64url `request` payload quoting the USDC.e price + the treasury recipient).
2. The agent pays the quoted USDC.e to the treasury on Tempo, then **retries**
   with `Authorization: Payment payload="<base64url {settlementTx, payTo}>"`.
3. The proxy **verifies the on-chain USDC.e transfer itself** (recipient == the
   treasury, amount, confirmed; replay-protected by a MintGateFacet one-shot
   receipt keyed on the settlement tx hash — never on client input) and
   GROSS-mints `$LH` into `payTo`'s meter → **200** + a `Payment-Receipt` header.

> ROBUSTNESS: we deliberately do **not** depend on Stripe's preview `mppx` verify
> library. We emit the MPP-shaped 402 (so the endpoint is MPP-compatible) and
> verify settlement ourselves (hardened like the x402 settle verify), so the full
> `mppx` facilitator verify can be swapped in later behind the same interface.

Env (mainnet, in addition to the on-ramp's shared `ONRAMP_*` / `FIAT_ISSUER_KEY`
from the Stripe section):

```sh
vercel env add ONRAMP_TREASURY production   # REQUIRED — the USDC.e recipient address; NO default (unset → endpoint closed)
vercel env add ONRAMP_USDCE production       # optional; default 0x20c0…b9537d11c60e8b50 (Tempo mainnet USDC.e)
vercel env add ONRAMP_MIN_CONFIRMATIONS production  # optional; default 1
vercel env add MPP_MIN_LH production         # optional; default 100 (whole $LH per charge floor)
vercel env add MPP_MAX_LH production         # optional; default 50000 (whole $LH per charge ceiling)
```

`FIAT_ISSUER_KEY` (signs the EIP-712 `FiatMint`) and `ONRAMP_SUBMITTER_KEY` (pays
gas, falls back to `PROXY_METER_KEY`) are shared with the Stripe path. Tests:
`./test/run.sh` (peg parity, the 402 challenge, settlement verify, idempotent mint).

For **per-request mode**, the `PROXY_METER_KEY` EOA must (1) be funded with
native Tempo gas and (2) be registered as the diamond's meter:
`cast send $DIAMOND "setMeter(address)" 0x<meterAddr> ...`. Time-session mode
needs neither — it only reads `sessionExpiryOf`.

No build step is required: Vercel auto-detects `api/gemini.ts` as a function,
`export const config = { runtime: 'edge' }` selects the Edge runtime, and
`vercel.json` rewrites `/v1beta/*` onto it.

## Request contract

The client calls the proxy exactly as it would call Gemini:

```
POST https://<proxy-origin>/v1beta/models/<model>:streamGenerateContent?alt=sse
x-goog-api-key: <address>:<timestamp>:<signature>
content-type: application/json

<the normal Gemini generateContent body>
```

Auth rides in the **`x-goog-api-key`** header as a localharness token
`<address>:<timestamp>:<signature>` (a real Gemini key has no colons, so the two
are unambiguous). The `signature` is an Ethereum **personal_sign** over the
exact ASCII message:

```
localharness-proxy:<address.toLowerCase()>:<timestamp>
```

`<model>` and the method (`generateContent` | `streamGenerateContent`) are
extracted from the path and allowlisted (`[A-Za-z0-9.\-]+`) so nothing the
caller controls can reshape the key-bearing upstream request.

### Response

The upstream Gemini response is passed straight through: status, `content-type`
(`text/event-stream` for streaming), and the streaming body. CORS is restricted
to `*.localharness.xyz` (and `localhost` for dev).

### Error codes

| Status | Meaning                                  |
|--------|------------------------------------------|
| 204    | CORS preflight (`OPTIONS`)               |
| 400    | unsupported path / bad model             |
| 401    | missing/stale/bad/mismatched auth token  |
| 402    | no active on-chain credit session        |
| 405    | non-POST method                          |
| 413    | request body too large (declared OR streamed past the cap) |
| 500    | upstream / config error                  |
| 502    | metering submission failed (fail-closed) |

## Auth model

1. **Freshness** — reject if `|now - timestamp| > 5 min` (`FRESHNESS_WINDOW_SECS`).
   The check is two-sided (`Math.abs`), so a future-dated stamp is rejected too,
   and the timestamp must be a non-negative integer. The on-chain session is the
   real gate; the token only proves the caller signed recently (re-signed per
   request). Bounded-replayable within the 5-minute window — see the known limit.
2. **Signature** — recover the EOA from the personal-sign and require it matches
   the token's `address`.
3. **On-chain gate** — `eth_call` the diamond at
   `0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c` on Tempo Moderato
   (chainId 42431, RPC `https://rpc.moderato.tempo.xyz`). Serve if EITHER a
   TIME session (`sessionExpiryOf(address) > now`) OR a funded PER-REQUEST
   balance (`creditOf(address) >= COST_PER_REQUEST_WEI`). When served on
   credit (no session), debit `meter(address, cost)` via the meter key (a
   standard EIP-1559 tx through viem) before forwarding — fail closed (502)
   if the debit can't be submitted, so a request is never served free.

## Known limit (accepted for the invited testnet beta)

A session is **time-bounded and all-you-can-use** within its window, and the
auth token is replayable within the freshness window. Abuse is bounded by the
on-chain session + Gemini's own rate limits — acceptable for an invited testnet
beta with a free-tier key (same risk class as the embedded sponsor key). The
public / mainnet-safe fix is **per-request x402 metering** (pay-per-call), not
shipped here.

### Bill-shock / rate limit (known gap — documented, not half-built)

A compromised or leaked auth key (or session) can flood requests and drain an
identity's whole `$LH` per-request balance before the owner notices. There is
**no per-identity request-rate limit**, and there cannot be one without a
*stateful* store — which this stack deliberately does NOT have (Edge Functions
are stateless per invocation; the project's substrate is Tempo + the browser, no
databases/daemons). What bounds the damage today:

- **The on-chain `creditOf` balance is the spend cap.** Per-request mode only
  drains what the user chose to deposit; the contract reverts once it's gone (a
  user never goes negative). Keep the deposited balance small to bound exposure.
- **`MAX_COST_PER_REQUEST_WEI`** clamps the per-call debit so no single request
  (or a price-env typo) can charge an absurd amount — a hard ceiling per call.
- Time-session mode is all-you-can-use within the window but spends no
  per-request balance; its blast radius is Gemini's own upstream rate limits.

**Fix path (when the no-off-chain-infra rule is relaxed for the proxy, or for
mainnet):** a short-TTL counter keyed by `address` in a KV store (Vercel KV /
Upstash) — increment per request, reject past N/min — would add a true sliding-
window limiter. It is intentionally **not** implemented here: it would introduce
a new stateful off-chain component, and a fake in-memory limiter on a stateless
Edge function (per-instance, reset on every cold start) would be security
theater. The clean cap is per-request x402 metering (above).
