# Browser E2E smoke — 2026-06-26 audit (behavioral browser-app fixes)

> **For a Claude Code session WITH browser-use.** The 2026-06-26 audit shipped
> behavioral `src/app/` + `web/cartridge-worker.js` changes (commit `73d1c49`,
> deployed to the `antig` prod site). They compile clean across both wasm configs
> but were NOT browser-E2E'd at author time. Drive each flow below on the LIVE site
> (or a `?view=…` preview) and confirm the expected outcome. Report any regression
> — these touch identity, the cross-origin signer trust boundary, and multiplayer.

Full finding detail: `design/security-audit-2026-06-26.md` (IDs referenced below).

## 1. Seed device-adoption (H1) — `events/devices.rs`, `app/mod.rs`, `wallet.rs`
- Owner agent → admin → **add a device**: a QR + an **8-char** code appear (was 6).
- On a second device/profile, open the QR URL (`localharness.xyz/?adopt=1#s=<ct>`).
  - ✅ **After load, the address bar / history must NOT retain `#s=<ct>`** (it is
    stripped via `history.replaceState` — the core H1 leak fix). Check the back
    button / history too.
  - Enter the 8-char code → seed imports → the device adopts the SAME identity and
    owns its subdomains. Derive is now 200k iterated keccak (~tens of ms — should be
    imperceptible). ✅ Linking still works end-to-end.
- The `localharness link` CLI path must still decrypt a browser-generated payload
  (shared `wallet::adopt_code_key`).

## 2. Cross-origin signer trust boundary (M5 / L15 / L16) — `app/signer.rs`
The signer is now **default-deny** per selector. Confirm legit flows still sign:
- As an **owner**: `register`, `setMetadata` (publish app/persona), `submitFeedback`,
  and `send_lh` all still sign + submit. ✅ (the diamond selector allowlist mirrors
  the relay's `DIAMOND_WRITE_SIGS`, so nothing legit should regress).
- As a **visitor** on a priced agent subdomain: the per-turn `$LH` payment (a
  `transfer` to the agent's TBA) **must still succeed** — this was wrongly rejected
  by the #81 owner-check before (L16). ✅ Send a paid message to a priced agent.
- Negative: a `$LH` `transferFrom` is rejected; `transferWithMemo` is treated like
  `transfer` (owner-checked), `transferFromWithMemo` rejected (L15 default-deny).

## 3. Multiplayer peer trust (L18) — `app/display.rs`
- Open a multiplayer cartridge (e.g. `slither` / `pong`) in **2+ tabs**; join the
  shared arena. ✅ Gameplay syncs, no desync. The host now attributes each frame to
  the connection index and ignores a forged `"p"` tag from Host/Mesh roles (only a
  Joiner's frames over the trusted host link honor `"p"`). Smoke = "still works".

## 4. `?rpc=1` cross-agent calls (L17) — `app/agent_rpc.rs`
- Your OWN agents still answer local `?rpc=1` calls. ✅
- A foreign `*.localharness.xyz` page can no longer drive your loaded agent for free
  (now owner-gated / consent-required). 

## 5. Cartridge SSRF gate (L19) — `web/cartridge-worker.js`
- A cartridge calling `host_net.open("wss://…")` must be **rejected** for loopback /
  IP-literal hosts: `wss://localhost`, `wss://127.0.0.1`, `wss://2130706433`
  (decimal 127.0.0.1), `wss://0x7f.0.0.1` (hex), `wss://localhost.` (trailing dot).
  ✅ A normal `wss://realhost.example.com` is allowed.

## 6. App data/io (L1, L33, L34) — `app/mod.rs`, `notifications.rs`, `feedback.rs`
- L1: re-importing a seed in the SAME tab shouldn't silently orphan at-rest files —
  prefer a full reload after import (verify history/lessons still readable).
- L34: submitting the same feedback text twice in one session — the receipt should
  not falsely show "✓ sent" for a silently-deduped second submit.
- L33: the on-chain inbox bell shouldn't drop a notification on a transient RPC blip.

## How to run
Use browser-use to load the prod site (or a Vercel preview), authenticate as a test
identity, and walk each flow. The cheap dogfood path is your own
`claude.localharness.xyz` + a throwaway second identity for the adopt/visitor cases.
