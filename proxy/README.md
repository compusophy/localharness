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
vercel env add PROXY_METER_KEY production       # meter EOA priv key (per-request mode)
vercel env add COST_PER_REQUEST_WEI production  # optional; default 0.01 LH (1e16)
```

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
| 500    | upstream / config error                  |

## Auth model

1. **Freshness** — reject if `|now - timestamp| > 24h`. The on-chain session is
   the real gate, so the token only proves the caller signed recently (re-signed
   per session). Bounded-replayable within the window — see the known limit.
2. **Signature** — recover the EOA from the personal-sign and require it matches
   the token's `address`.
3. **On-chain gate** — `eth_call` the diamond at
   `0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930` on Tempo Moderato
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
