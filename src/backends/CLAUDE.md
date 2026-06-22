# src/backends — model-backend plumbing subsystem spec

> Module-owned context (auto-loaded when an agent works in `src/backends/`).
> These are the `Connection`/`ConnectionStrategy` impls behind the L3 seam. The
> wire quirks below are subtle and high-blast-radius — a single wrong field 400s
> the provider and bricks ALL chat. Read before touching a backend.

## Fix plumbing in the SHARED core, not per-backend
The shared files own the cross-backend behavior — change them ONCE, not in each
provider: `sse.rs` (SSE frame decoder), `dispatch.rs` (hook-gated tool pipeline),
`runners.rs`, `compaction.rs` (ONE generic fold engine; per-backend
`compaction.rs` are THIN adapters), `stream_timeout.rs`, `state.rs`. Per-backend
dirs (`gemini/ anthropic/ openai/ mock/ mcp/ local/`) hold only the wire-specific
client + loop. If a fix would be copy-pasted into two backends, it belongs in the
shared core.

## Gemini (the default path — most quirks)
- **Model IDs FLIP — verify against the LIVE API, never trust memory.**
  `DEFAULT_MODEL = gemini-3.5-flash`; `gemini-2.5-flash` now 400s. `curl` the live
  `:generateContent` before changing/defending a model constant. If the user says a
  model is wrong, TEST THEIRS FIRST.
- **Union-type tool schemas 400 → bricks ALL chat.** `input_schema` must use a
  SINGLE `type` (NOT `["string","null"]`) and no `additionalProperties`/`$schema`/
  `$ref`/`oneOf`/`anyOf`/`allOf`. Nested objects/arrays + `minimum`/`maximum` are
  fine. Guard: `cargo test builtin_tool_schemas_have_no_union_types`.
- **3.x `thought` parts + `thoughtSignature` echo.** Wire `Part` is untagged;
  `Part::Thought` comes BEFORE `Part::Text`, and 3.x stamps EVERY part with
  `thought`, so normal text deserializes into `Part::Thought{thought:false,text:..}`
  — handle explicitly (`wire.rs`). ALSO 3.x stamps each `functionCall` with
  `thoughtSignature` and 400s replayed history MISSING it (bricked multi-round tool
  turns until 0.31.x) — capture + echo it VERBATIM (`wire.rs`/`loop.rs`). Proof:
  `examples/thought_signature_live.rs`. Don't strip `thoughtsTokenCount` into the
  user's billable count (leak fixed 036b47d).

## SSE is CRLF on wasm
Browser fetch surfaces Gemini SSE with `\r\n\r\n`. `GeminiSseStream::take_frame`
(and `sse.rs`) match BOTH `\n\n` and `\r\n\r\n`. Don't regress to LF-only.

## OpenAI / Anthropic (additive backends, no new deps)
- OpenAI: streamed `tool_calls` are INDEX-KEYED FRAGMENTS to concat as they arrive
  (`openai/loop.rs`) — not whole calls per delta. Chat Completions shape.
- Anthropic: Claude Messages API. Both are BYOK or platform-`$LH`-via-proxy.

## Mock / MCP / local
- `mock/`: deterministic offline backend (`Agent::start_mock`), wasm-clean — use it
  for native tests of the agent loop without a network.
- `mcp/`: stdio MCP client — `feature=native` only (no wasm).
- `local/`: in-browser Gemma 3 270M via Burn wgpu — `feature=local`, HEAVY (~570MB),
  OFF the default bundle. getrandom-0.4 needs the wasm_js backend; burn-store DIRECT
  (memmap2 wasm-broken); GPU read-back MUST `into_data_async().await`.

## Error classification is OWNED by `crate::error_codes::classify`, not here
A backend surfaces the RAW provider error; `classify` maps it to `LH3xxx`. A 429 /
quota / spend-cap is `BACKEND_RATE_LIMIT`, NOT out-of-credits (`BACKEND_CREDITS`) —
don't re-conflate them in a backend. The chat surface + telemetry read the code.
External Gemini spend-cap 429s are suppressed from telemetry in `app/chat`.

## wasm: every `#[async_trait]` is `cfg_attr`'d `?Send`; `StepStream` is Box vs
LocalBox per target (`runtime.rs`). Mirror these when adding a backend or it breaks
SILENTLY on wasm (gated modules don't trip a default `cargo check`).
