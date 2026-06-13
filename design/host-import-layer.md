# Broader cartridge host-import layer (#40)

## Problem

A published rustlite cartridge is untrusted wasm run OFF the main thread in `web/cartridge-worker.js` (the brick fix: a hung `frame()` can only stall its worker, never the app). Today its entire view of the platform is six host modules, resolved by `resolve_host_fn` in `src/rustlite/typecheck.rs` and implemented twice — once as Rust import closures in `src/app/display.rs::build_host_display` (+ `net`/`audio` submodules) for the in-thread path, and once as JS objects in `cartridge-worker.js` (`host_display`, `host_net`, `host_audio`, `host_log`, `host_time`, `host_abort`, `host_agent`, `host_compose`).

What a cartridge **can** do: draw (display), poll pointer, 64 i32 state slots, WebSocket I/O (wss-only SSRF gate), audio, compose child cartridges, and a narrow `host_agent` bridge (notify the local viewer, viewer_is_owner/has_identity, subscribe/broadcast/request_identity over SubscribeFacet).

What it **can't** do, but should, to be a real platform app:
1. **Read on-chain state** — a leaderboard cartridge can't read its own `$LH` balance, a bounty list, an x402 price, a name's owner/persona, or arbitrary `metadata(id,key)`. Everything interesting on the diamond is invisible.
2. **Persist beyond 64 i32 slots** — `state[64]` is zeroed every load. A save-game, a high-score table, or any structured blob has nowhere to live. The platform already has per-origin OPFS (`OpfsFilesystem`) it can't touch.
3. **Dispatch to an agent** — there is `call_agent`/x402 `ask_agent` in the chat layer, but a cartridge can't ask "what does claude.localharness.xyz say to this prompt?" and render the answer. This is the single highest-leverage gap (an LLM-backed cartridge).
4. **Read richer viewer/wallet context** — only owner/has-identity booleans exist; no viewer address, no balance, no current subdomain name/id as strings.

## Approach

Keep the exact existing shape — DO NOT widen the trust boundary. Three invariants hold:

- **The worker never touches the chain, a key, or the proxy.** It already can't (no wallet, no RPC). Every privileged op is a `postMessage` to the main thread, which holds the signer/sponsor and performs the op, then posts results back. This is precisely the `compose_spawn`/`compose_bytes` and `agent_subscribe`/`agent_context` pattern already in place — we generalize it.
- **Two ABI surfaces, one source of truth.** Every new host fn needs (a) a signature in `resolve_host_fn`, (b) a JS impl in `cartridge-worker.js`, and (c) the in-thread Rust impl in `display.rs`. The worker-host-parity test (`scripts/test-worker-host-parity.mjs`) and `cargo test raster`/`compose` guard drift; new modules need test coverage or they silently fork.
- **Poll-model only, integer/length-prefixed-string ABI.** Async on-chain reads and agent calls CANNOT block a synchronous wasm frame. They follow the established asynchronous-request / cached-read split (like `subscribe()` → fire-and-forget write + `is_subscribed()` cached read): the cartridge *requests* a read by key, gets a request handle back immediately, and *polls* a result slot on a later frame. Strings cross via the same 4-byte-LE-length + UTF-8 layout as `host_net`/`host_agent` (`readString`/`writeString` already exist in the worker; `read_string`/`write_string` in display.rs).

### New host modules (v1 scope)

- **`host::chain`** — read-only on-chain access, request/poll model. Whitelisted read kinds ONLY (no arbitrary RPC, no raw calldata):
  - `lh_balance(addr_ptr) -> req`  / `metadata(id, key_ptr) -> req` / `id_of_name(name_ptr) -> req` / `owner_of_name(name_ptr) -> req` / `x402_price(id) -> req` / `subscriber_count(id) -> req`.
  - `poll(req, out_ptr, max) -> i32` (>=0 result length written, 0 pending, -1 error/unknown req). Numeric results are decimal UTF-8 (reuse the $LH decimal-wei convention).
  - Caps: max 8 inflight requests per cartridge, results expire after N frames, an LRU result ring. The main thread services each via existing `registry::*` read fns (`names::id_of_name`, `subscribe::subscriber_count`, `x402::x402_price_of`, generic `metadata_bytes_of`). All reads are PUBLIC data already exposed in the UI — no new exposure.
- **`host::store`** — durable per-cartridge key/value over OPFS, namespaced under the running subdomain so a cartridge can only read/write its OWN island (no cross-subdomain reads). Request/poll for reads; fire-and-forget for writes.
  - `put(key_ptr, val_ptr) -> i32` / `get(key_ptr) -> req` / `poll(req, out_ptr, max)` / `delete(key_ptr)`.
  - Backed by `OpfsFilesystem` under a fixed prefix e.g. `.lh_cart_store/<value-of-tenant::current_name>/<sanitized-key>`. Hard caps: max key length, max value bytes (e.g. 16 KB), max total bytes per namespace (e.g. 256 KB), max keys. Owner-only WRITE gate is optional for v1 (a visitor's writes stay in their own OPFS — it's their device — so it's naturally sandboxed; document that store is device-local, not synced). Synced/shared store is explicitly v2 (would ride teams_sync / shared_fs).
- **`host::ai`** — single-shot agent dispatch, the LLM-backed-cartridge unlock. Request/poll.
  - `ask(target_name_ptr, prompt_ptr) -> req` / `poll(req, out_ptr, max)`.
  - Main thread routes through the SAME path the chat `call_agent` tool / x402 `ask_agent` uses (`app/remote_call.rs`), so it is metered/paid in `$LH` from the VIEWER's wallet. **This spends money**, so it is gated: `viewer_has_identity` required; a hard per-frame and per-session call cap; and a one-time per-session consent (the main thread shows a swap-in confirm panel — never `window.confirm`, per the no-JS-alerts rule — the first time a cartridge calls `ask`, à la the broadcast composer). Default target = the cartridge's own subdomain persona; an explicit target must be a registered name. Rate-limited like `broadcast` (>=3s).
- **`host::context` (additive to `host::agent`)** — richer viewer/identity reads, all cached at load + refresh, no new privilege: `viewer_address(out_ptr,max)`, `self_name(out_ptr,max)`, `self_id() -> i32`. These let a cartridge label itself and the viewer without a chain round-trip.

## On-chain / contract changes

**None.** Every read targets existing facets/views (RegistryFacet `metadata`, CreditsFacet/`$LH` `balanceOf`, SubscribeFacet, X402Facet price). `host::ai` reuses the live x402 `ask_agent` settlement. No new selector, no `diamondCut`. This is purely a host-layer + ABI expansion.

## File-by-file plan

- `src/rustlite/typecheck.rs` — `resolve_host_fn`: add `chain::*`, `store::*`, `ai::*`, and the new `agent::viewer_address/self_name/self_id` keys with signatures (all `I32`/`String` per the union-free Gemini-schema constraint, though that constraint is for tool schemas not host fns — still keep it integer-clean). Extend `host_fn_tests` with resolve assertions for every new fn (mirrors the existing `host_agent_signatures_resolve`/`host_compose_signatures_resolve` tests).
- `web/cartridge-worker.js` — add `host_chain`, `host_store`, `host_ai` objects + the `viewer_address/self_name/self_id` getters on `host_agent`. Each request fn allocates a request id, posts a typed message (`chain_read`/`store_get`/`store_put`/`ai_ask`), and `poll` reads from a per-module result map filled by new `*_result` inbound messages handled in `applyAgentContext`-style appliers. Wire the new inbound types in `self.onmessage`. Add the new objects to `buildImports()` AND to `buildChildImports()` as INERT stubs (a composited child must not reach chain/store/ai from inside a panel — same rule as `child_net`/`child_agent` today). Export new surfaces in the Node test block.
- `src/app/display.rs` — `build_host_display`: build `host_chain`/`host_store`/`host_ai` import objects for the in-thread path (closures that request + cached-poll), and add the new context getters. Add the main-thread message handlers in the worker `onmessage` switch (alongside `agent_subscribe`/`compose_spawn`): `chain_read` → spawn_local a registry read → post `chain_result`; `store_get`/`store_put` → OPFS via `OpfsFilesystem`; `ai_ask` → consent-gate then route via `remote_call`/`ask_agent` → post `ai_result`. Add a `do_chain_read`/`do_store_op`/`do_ai_ask` async fn family next to `do_feed_subscribe`/`do_compose_spawn`. Reuse `feed_token_id`, `credit_signer`, `sponsor::signer`, `tenant::current_name`.
- `src/app/templates.rs` — an `ai_consent` swap-in panel (model the broadcast_composer) with [allow]/[deny] `data-action` buttons.
- `src/app/events/*` + `mod.rs` — dispatch the consent panel's actions (set a per-session "ai allowed" flag the worker is told via `agent_context`).
- `scripts/test-worker-host-parity.mjs` (or a sibling) — assert the new host objects exist and the inert child stubs return the documented rejection values; a small headless test that a `chain::poll` returns pending(0) then a result after a simulated `*_result` message.
- `web/llms.txt` + the rustlite host-fn docs section + `CLAUDE.md` (cartridge host imports line) + `design/host-import-layer.md` — the five-surface doc SOP. Add example cartridges (`examples/` or a `.rl` sample) showing a balance readout, a saved high score, and an AI-answer panel.

## Risks

- **ABI drift between the JS worker host and the Rust in-thread host** — the standing hazard. Mitigate by landing the parity test FIRST and by keeping the request/poll bookkeeping logic identical. Consider whether the in-thread `build_host_display` path is still exercised at all; if the worker is the only live path for published cartridges, the Rust closures for the NEW modules can be thin/stubbed and the test burden shrinks (confirm during Phase 0).
- **`host::ai` spends viewer `$LH` silently** — the money risk. Consent panel + per-session cap + per-frame cap + identity gate are mandatory, not optional. Never auto-fire on load; require a user-gesture frame (pointer_down) for the first `ask`.
- **OPFS quota / poison** — `host::store` caps (per-value, per-namespace, key count) and key sanitization (no path traversal in the key) are required; a cartridge must not be able to fill the disk or escape its namespace into `.lh_wallet` etc.
- **Read amplification on the public RPC** — `host::chain` polls could hammer the rate-limited RPC. Cache results, coalesce identical inflight requests, cap inflight count, and add a min interval per key (the same instinct that made `refresh_feed_context` skip anonymous visitors).
- **Compose children must stay inert** — a grandchild reaching chain/store/ai would break the panel sandbox; ensure `buildChildImports` stubs ALL new modules (regression-test it).

## Phased build order

- **Phase 0 — foundation + parity (no new caps yet).** Add the request/poll plumbing scaffolding to the worker + display.rs for ONE trivial read (`host::context::self_name`, pure cached string, no chain). Land the parity test. Confirm both host paths and the inert-child path. This de-risks the ABI mechanics before any privileged op.
- **Phase 1 — `host::chain` (read-only).** Wire the whitelisted reads through existing `registry::*` views. Highest value, lowest risk (public data, no spend, no writes). Ship example: live `$LH` balance + subscriber count readout.
- **Phase 2 — `host::store` (durable, device-local).** OPFS-backed namespaced KV with caps. Ship example: persistent high-score table.
- **Phase 3 — `host::ai` (metered dispatch).** Consent panel + caps + routing through `ask_agent`. Ship example: an LLM-answer cartridge against the subdomain's own persona. Hardest + most dangerous; do it last when the request/poll layer is proven.
- **Phase 4 (deferred/v2).** Synced/shared store over teams_sync; write-gated cross-subdomain reads; cartridge-initiated bounties/x402 settlement (value-MOVING, would need the typed-confirm convention extended into the canvas).