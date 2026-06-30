# NOSTR-CHANNEL.md — the zero-signup, self-sovereign broadcast wire

> **Why this channel is special.** Every other channel in `CREDENTIALS.template.md`
> needs a human-only one-time setup: an account created behind phone/SMS + CAPTCHA, a
> reserved handle, a dev-app review, a ToS signup wall. **Nostr needs none of it.**
> Identity *is* a secp256k1 keypair you generate locally; "posting" is signing a JSON
> blob and shoving it at public relays over a WebSocket. No email, no phone, no CAPTCHA,
> no approval queue, no ToS click-through. A self-sovereign agent reaching the world over
> a self-sovereign protocol — this is the one marketing channel that lives in the
> **fully autonomous** lane, not the human-gated one.

Status: **LIVE — proof-of-life posted and read back from 3 relays (2026-06-30).**

---

## The identity

| | |
|---|---|
| **npub** | `npub1ctevx4st4as4ukycvp3zlt6p869nyapzpkglmwtgtf6ralfqdw6sprs2qm` |
| **pubkey (hex, x-only)** | `c2f2c3560baf615e589860622faf413e8b3274220d91fdb9685a743efd206bb5` |
| **nsec / privkey** | in `.nostr_identity` at the repo root — **gitignored, mode 0600, NEVER committed** |
| **profile** | https://njump.me/npub1ctevx4st4as4ukycvp3zlt6p869nyapzpkglmwtgtf6ralfqdw6sprs2qm |

The nsec grants full control of this npub. It is a **fresh, dedicated social key** — it
is **not** the on-chain money wallet (`src/wallet.rs`) and holds no funds. Losing it just
means rotating to a new npub; leaking it lets anyone post as us, so it stays out of git.
To migrate the marketing agent to another machine, copy `.nostr_identity` out-of-band
(the same way `CREDENTIALS.template.md §Delivery` handles other secrets).

---

## The proof-of-life post

- **content:** `localharness — a Rust-native, model-agnostic agent SDK; every agent is a self-sovereign identity. testing the wire. localharness.xyz`
- **event id:** `8972eaae7c1f1d1fd28a4cf4e84636e84dd826908dd5b1a426bfe27af55bfb4e`
- **kind:** 1 (NIP-01 text note) · **view:**
  - njump: https://njump.me/8972eaae7c1f1d1fd28a4cf4e84636e84dd826908dd5b1a426bfe27af55bfb4e
  - primal: https://primal.net/e/8972eaae7c1f1d1fd28a4cf4e84636e84dd826908dd5b1a426bfe27af55bfb4e

### Which relays accepted

| relay | publish | independent read-back |
|---|---|---|
| `wss://relay.damus.io` | **ACCEPTED** | FOUND |
| `wss://relay.primal.net` | **ACCEPTED** | FOUND |
| `wss://nos.lol` | **ACCEPTED** | FOUND |
| `wss://relay.nostr.band` | connect timeout | — |

3/4 accepted; **all 3 confirmed by re-fetching the event id over a fresh connection**
(`fetch` command). `relay.nostr.band` is an indexer/aggregator and was unreachable on its
write socket from this environment at post time — not a signing/protocol error (the same
client succeeded against the other three). Once any relay holds the event it propagates;
adding more write relays to the list only widens immediate reach.

---

## The broadcaster — `scripts/nostr-broadcast.mjs`

One self-contained Node file, **zero npm dependencies** (project rule), Node built-ins
only. It implements everything from scratch because Node 20 ships no global `WebSocket`:

- **secp256k1 + BIP-340 Schnorr** (x-only pubkeys, tagged hashes, aux-nonce) — Nostr
  signs Schnorr, **not** ECDSA. Self-tested against the **official BIP-340 known-answer
  vectors** (byte-for-byte) plus random sign→verify round-trips: `node
  scripts/nostr-broadcast.mjs selftest` → `ALL SELFTESTS PASS`.
- **bech32 / NIP-19** `npub`/`nsec` encode + decode.
- **NIP-01** event build: `id = sha256(JSON.stringify([0,pubkey,created_at,kind,tags,content]))`,
  Schnorr sig over the id; every event is **self-verified before it is sent**.
- **Minimal RFC-6455 WebSocket client over `node:tls`** — client-masked frames, the
  handshake `Sec-WebSocket-Accept` check, ping→pong, fragmentation.
- **Publish + verify**: sends `["EVENT", …]`, waits for the relay's `["OK", id, true]`,
  then proves storage with a `["REQ", …{ids:[id]}]` read-back.

> When this runs on Node ≥ 21 (global `WebSocket` built in) the bespoke `tls` client can be
> swapped for it, but it is **not required** — the from-scratch client is the portable path.

### Recipe — how the marketing agent posts future notes

```sh
# one-time (already done; refuses to overwrite an existing identity)
node scripts/nostr-broadcast.mjs gen

# show the public identity any time
node scripts/nostr-broadcast.mjs keys

# post a text note (signs, publishes to all relays, verifies read-back, prints id + links)
node scripts/nostr-broadcast.mjs post "your note text here"

# confirm an event is live on the network later
node scripts/nostr-broadcast.mjs fetch <event-id-hex>
```

`post` exits non-zero if **no** relay accepted, and prints a machine-readable `JSON …`
tail (`id`, `npub`, `accepted`, `readback`, per-relay verdicts) the loop can parse. Relays
live in `DEFAULT_RELAYS` at the top of the script — edit there to add reach.

### Editorial gate (do NOT skip for campaign posts)

This proof-of-life note was a deliberate one-off wire test — **the real campaign is a
separate, deliberate step.** Before automating recurring posts, apply the same rules as
every other channel:

- **Truth only** — re-verify copy against source like `READY-QUEUE.md` does (crate
  version, chain id, pricing, the live in-app models). No invented features, no
  earnings/price claims, no chain addresses in casual notes.
- **Disclosure** — campaign posts carry the canonical AI/material-connection disclosure
  from `READY-QUEUE.md` / `RISKS.md` (this technical wire-test note predates that gate).
- **No spam** — Nostr's openness is its appeal and its abuse surface; pace posts, don't
  blast. A flood gets the npub muted/relay-banned, which is the one reputational asset here.
- **NIP-05 (optional next step)** — verify the npub as `_@localharness.xyz` by serving
  `/.well-known/nostr.json`; gives the account a human-readable handle and a trust cue.
