# src/backends — model-backend plumbing subsystem spec

> Module-owned context (auto-loaded when an agent works in `src/backends/`).
> These are the `Connection`/`ConnectionStrategy` impls behind the L3 seam. The
> wire quirks below are subtle and high-blast-radius — a single wrong field 400s
> the provider and bricks ALL chat. Read before touching a backend.

## Fix plumbing in the SHARED core, not per-backend
The shared files own the cross-backend behavior — change them ONCE, not in each
provider: `sse.rs` (SSE frame decoder), `dispatch.rs` (hook-gated tool pipeline),
`runners.rs`, `compaction.rs` (ONE generic fold engine; per-backend
`compaction.rs` are THIN adapters), `stream_timeout.rs`, `state.rs`, and
`turn_engine.rs` (R7 COMPLETE: ONE generic streaming turn loop behind a
static-dispatch `TurnProvider` seam — same pattern as `CompactionModel`; async
edges ride in as closures so it stays wasm-safe. ALL THREE streaming backends —
gemini (the always-on default), anthropic, openai — ride it; the two
control-flow hooks (anthropic pause_turn resume + #82 cancel balancing) default
to no-ops for gemini/openai. A scaffold fix lands ONCE, in the engine).
Per-backend dirs (`gemini/ anthropic/ openai/ mock/ mcp/ local/`) hold only the
wire-specific client + loop/provider. If a fix would be copy-pasted into two
backends, it belongs in the shared core.

## Gemini (the default path — most quirks)
- **Model IDs FLIP — verify against the LIVE API, never trust memory.**
  `DEFAULT_MODEL = gemini-3.7-flash`, `DEFAULT_IMAGE_GENERATION_MODEL =
  gemini-3.1-flash-image`; `gemini-2.5-flash` and `gemini-2.0-flash-exp-image-
  generation` now 404/400. `scripts/gemini-model-drift.sh` diffs both consts against
  the live catalog and names any newer Flash — run it before defending a pin. `curl` the live
  `:generateContent` before changing/defending a model constant. If the user says a
  model is wrong, TEST THEIRS FIRST.
- **3.7's migration doc OVERSTATES the break — we live-probed it (2026-08-13).**
  Google's 3.7 checklist says to strip `temperature`/`top_p`/`top_k` and to replace
  `thinking_budget` with `thinking_level`. On the v1beta `:generateContent` endpoint
  NONE of that is enforced: `temperature` → 200, `thinkingConfig.thinkingBudget` →
  200 (and still scales reasoning), a real 2-round tool loop echoing
  `thoughtSignature` with a `functionResponse` carrying NO `call_id` → 200, and the
  same with PARALLEL calls → 200. So the 3.6→3.7 flip needed no wire change. What IS
  real: `thinkingLevel: "minimal"` 400s on 3.7 (3.6 allows it) — irrelevant while we
  send a budget, and a REASON not to migrate to the enum (it has no 4th rung, so
  `Minimal`/`Low` would collapse and routine turns would cost more). 3.7 also stamps
  a `functionCall.id` we don't model; dropping it on the echo is verified harmless.
  ⛔ Don't "fix" this wire off the migration doc alone — probe first.
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

- **A content-BLOCKED candidate wires as `"content": {}`** — an object with NO
  `role` and no `parts`, beside `finishReason: PROHIBITED_CONTENT` (same for
  SAFETY/BLOCKLIST). So `wire::Content::role` DEFAULTS to `model` (a candidate is
  always the model's turn); without that the whole chunk failed to decode
  ("missing field `role` at line 1 column 30") and the turn died as `gemini sse
  decode: …` — an infra-looking crash instead of the blocked stop
  `map_finish_reason` + `turn_flow::classify_empty` already classify (TB full-set
  2026-08-01, task `dna-assembly`). Fixture: `wire::BLOCKED_FRAME_JSON`, the
  VERBATIM captured frame — wire / SSE / fold tests all assert against it.

- **A blocked PROMPT is a DIFFERENT frame: `promptFeedback.blockReason`, NO
  candidate** — so there is no `finishReason` to map. Unmodelled it decoded fine,
  folded to nothing, and the turn ended with an EMPTY note, which
  `turn_flow::classify_empty` reads as `Blank` → "check your session/balance" for
  what was a content block (mis-blaming the user's credits). Now
  `GenerateChunk.prompt_feedback` → `RoundAccum.prompt_block` → a named "prompt
  blocked by …" note. Only a `blockReason` means blocked (ratings alone don't);
  every field is `#[serde(default)]` + `#[serde(other)]`, so an unknown reason is
  still a BLOCK and an absent `promptFeedback` behaves exactly as before. Fixture:
  `wire::PROMPT_BLOCKED_FRAME_JSON` (synthesized from the documented v1beta shape,
  NOT captured). Same class of bug: `SPII` / `LANGUAGE` sat in the wire enum with
  no `map_finish_reason` arm and fell to the `_` catch-all's empty note. DRIFT
  GUARD `every_content_block_reason_classifies_as_blocked` — every block reason
  (candidate- and prompt-level) must carry a note `classify_empty` reads as
  `Blocked`. Its tripwire is two exhaustive `match`es with no `_` arm, so a new
  `FinishReason`/`BlockReason` variant is a COMPILE error there until you
  declare whether it blocks. ⛔ Still MISSING from `FinishReason`:
  `UNEXPECTED_TOOL_CALL` / `TOO_MANY_TOOL_CALLS` — documented values that decode
  to `Unknown` and report a FAILED turn as a clean `(Done, "")`. Not content
  blocks, so not the mis-blame bug; give them arms when you next touch this.

## SSE is CRLF on wasm
Browser fetch surfaces Gemini SSE with `\r\n\r\n`. `GeminiSseStream::take_frame`
(and `sse.rs`) match BOTH `\n\n` and `\r\n\r\n`. Don't regress to LF-only.

## OpenAI / Anthropic (additive backends, no new deps)
- OpenAI: streamed `tool_calls` are INDEX-KEYED FRAGMENTS to concat as they arrive
  (`openai/loop.rs`) — not whole calls per delta. Chat Completions shape.
- Anthropic: Claude Messages API. Both are BYOK or platform-`$LH`-via-proxy.
- **Refusal text SURFACES in the finish note — never a silent Done.** OpenAI: a
  structured-outputs refusal streams `delta.refusal` (content null) and finishes
  with a plain `"stop"`; the fold accumulates it and `map_finish_reason` ends the
  turn as a refusal-classified Error carrying the text. Anthropic: a `refusal`
  stop's `stop_details.explanation` (fallback category) is appended to the
  "stopped by refusal" note. `turn_flow::classify_empty` matches both by
  substring ("refusal"/"content filter"), so appended detail stays classified.

## Mock / MCP / local
- `mock/`: deterministic offline backend (`Agent::start_mock`), wasm-clean — use it
  for native tests of the agent loop without a network. It RIDES the shared
  `turn_engine` (a `MockProvider` whose "stream" is the scripted step sequence;
  scripted turns split into engine rounds at tool-call boundaries), so mock-driven
  loop tests exercise the SHIPPED turn loop, not a parallel one.
- `mcp/`: stdio MCP client — `feature=native` only (no wasm).
- `local/`: in-browser Gemma 3 270M via Burn wgpu — `feature=local`, HEAVY (~570MB),
  OFF the default bundle. getrandom-0.4 needs the wasm_js backend; burn-store DIRECT
  (memmap2 wasm-broken); GPU read-back MUST `into_data_async().await`. Decode is
  KV-CACHED (`GemmaModel::forward_cached`; `forward` delegates over a throwaway
  cache — ONE attention codepath) and STREAMS text-delta Steps per token
  (`generate_streamed`; StreamEmitter holds back a partial `"\nUser:"` marker and
  goes quiet on non-prefix-stable decodes). SLIDING-WINDOW attention per
  `config.json` `layer_types` (15 sliding layers, `sliding_window=512`; layers
  5/11/17 global): mask = HF `masking_utils.py` semantics (`k <= q && k > q-512`,
  pure predicate `gemma::attn_blocked`); sliding KV caches TRIM to the last 511
  rows (`kept_cache_len`) — keys keep their ABSOLUTE-position RoPE, trimming
  only drops rows. Parity/speed proofs: ignored tests `gemma_kv_parity_and_speed`
  / `gemma_native_stream` / `gemma_sliding_window_parity` (>512-token prompt;
  GEMMA_DIR=weights dir).

## Error classification is OWNED by `crate::error_codes::classify`, not here
A backend surfaces the RAW provider error; `classify` maps it to `LH3xxx`. A 429 /
quota / spend-cap is `BACKEND_RATE_LIMIT`, NOT out-of-credits (`BACKEND_CREDITS`) —
don't re-conflate them in a backend. The chat surface + telemetry read the code.
The stream-OPEN retry (`retry.rs`, #29) keys off these codes — a transport wording
`classify` misses fails a turn HARD (#41 was the bare "error sending request" on
mobile → `BACKEND_SEND`, retried ONCE/500ms; a retry past the response can double-
bill since the proxy floor-debits after upstream 2xx, so it's capped tighter).
A MID-STREAM failure still fails the turn (no retry — bytes already went out),
but the engine now PERSISTS the partial assistant text to history first
(`fail_keeping_partial_answer!`, text only — half-accumulated tool calls must
never land in history). Dropping it left the transcript painted with an answer
the model had no memory of and history ending on a dangling user turn, so the
next message redid the work (telemetry #68/#71).
The engine opens via `open_stream_with_retry_or_cancel`: the OPEN await itself
races the cancel flag (100ms slices) — Stop while the POST is in flight drops
the open future (aborts the request) and NEVER retries; a cancel is a distinct
`OpenOutcome::Cancelled`, not an error the retry loop could swallow.
External Gemini spend-cap 429s are suppressed from telemetry in `app/chat`.

## wasm: every `#[async_trait]` is `cfg_attr`'d `?Send`; `StepStream` is Box vs
LocalBox per target (`runtime.rs`). Mirror these when adding a backend or it breaks
SILENTLY on wasm (gated modules don't trip a default `cargo check`).
