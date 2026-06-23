# src/connections — L3 transport seam subsystem spec

> Module-owned context (auto-loaded when an agent works in `src/connections/`). The
> L3 abstraction: `Connection` = a live backend session, `ConnectionStrategy` = the
> factory that opens one. Agent/Conversation depend ONLY on these traits — never on
> transport details. Backend impls live in `src/backends/` (see its CLAUDE.md).

## ⛔ wasm cfg-gating is the silent-break trap — mirror it on EVERY new impl
The native build and the wasm build diverge HERE, and a default `cargo check`
(native) won't catch a wasm break:
- `StepStream = BoxStream` (native, Send-bound for `tokio::spawn`) vs
  `LocalBoxStream` (wasm32 — browser fetch streams aren't Send). Use the alias, not a
  hardcoded stream type.
- Both traits are `: MaybeSendSync` (= `Send + Sync` native / empty on wasm) and every
  `async fn` is `#[cfg_attr(not(wasm), async_trait)]` / `#[cfg_attr(wasm,
  async_trait(?Send))]`. A new `Connection`/`ConnectionStrategy` impl MUST repeat this
  pattern or wasm breaks — and it breaks SILENTLY (gated modules don't trip a native
  check). After touching this seam, run
  `cargo check --no-default-features --features browser-app --target wasm32-unknown-unknown`.

## Trait contract notes
- `subscribe_steps()` returns an INDEPENDENT cursor each call — the source is a
  broadcast channel, so late subscribers still see steps that arrive after they
  subscribe. Don't assume a single consumer.
- `send` switches the turn boundary; `send_trigger` pushes an out-of-band event that
  does NOT; `send_tool_results` is a no-op for backends that dispatch tools inline
  (Gemini). Implement accordingly.
- `cancel_turn` defaults to a no-op — override it for cooperative cancellation
  (stop at the next safe boundary, emit a terminal step so the turn ends cleanly).
  Idempotent + safe-when-idle is required.
- Every method takes `&self` (impls are `Arc`-shared) so tools/triggers can call back
  into the connection without exclusive access.
