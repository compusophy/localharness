# Scoping: conversation-scoped ToolContext (Antigravity review follow-up)

## Finding: the primitive already SHIPPED — the gap is adoption + hooks
`ToolContext` (src/tools.rs:52-89) already carries a per-session KV store (`get_state`/`set_state`,
parking_lot RwLock<HashMap<String,Value>>) and is wired once at agent start (src/agent.rs:1111) and
threaded through dispatch (src/backends/dispatch.rs → ToolRunner::execute:154). **Zero adopters**:
every app chat tool binds it as `_ctx` (all ~60 ClosureTool sites in src/app/chat/tools/*).

## Real problems today (state rides tab-scoped thread_locals, not the conversation)
1. confirm_guard.rs:26-40 — GATE / LAST_USER_MSG / AWAITING_CONFIRMATION thread_locals; can't use
   ToolContext because it's a PreToolCallDecideHook and hooks only see TurnContext, not the KV.
2. chat/dedup.rs:40-43 — RUN_HASHES per-session dedup set; manual lifecycle, leaks across resets.
3. display/surface.rs:103-150 — LAST_CARTRIDGE / PENDING_EMBED: the tool→turn-loop wasm handoff
   (run_cartridge stashes, stream_turn launches) is a static side-channel, untestable natively.
4. Re-derivation: `tenant::current_tenant_owner()` recomputed inside ~17 tool bodies per call
   (platform.rs:451,616,934,1060,1211,1386,1471; misc.rs:53,138,279,365,450; room.rs:35).

## Minimal additive shape (no 1.0-frozen-surface break)
- Expose the SAME session KV on `TurnContext`/`OperationContext` (Arc<ToolContext> or just the
  state map) so hooks + the turn loop share it with tools — this is the ONLY missing API.
- Lifecycle: mint a fresh ToolContext per conversation (agent.rs already does; app session.rs must
  too on session reset). Optional sugar: `get_state_as::<T>()` via serde — no trait change.
- ClosureTool/tool_params! need nothing: closures already receive `Option<Arc<ToolContext>>`.
- wasm: already correct — parking_lot RwLock compiles on wasm32; Tool is MaybeSendSync; no new bounds.

## Verdict: small step at 1.0, not a project
Bar check: it deletes ~5 real statics (confirm_guard×3, dedup, pending-embed) and fixes their
reset-leak class; the other ~23 thread_locals are UI/worker/tab state that a conversation context
should NOT absorb. Do: hook-side KV exposure + per-session lifecycle + migrate those 3 call sites
as proof (~150 LOC, additive). Don't: new trait, DI framework, or a tenant-owner cache (a stale
owner across a claim/release is worse than the re-derive). Never: context-passing rewrite of src/app.
