# Big-picture refactor roadmap

> Provenance: 7-lens principal-architect review + adversarial skeptic verification
> (workflow `wf_8f0d577a-2bb`, 2026-07-02: 29 agents, 35 findings, 22 high-value
> proposals → **7 survived** the skeptics). The company loop's ARCHITECTURE dept
> executes this top-down, ONE item at a time to completion, updating Status as
> items land. The KILLED list at the bottom is binding: each died for a grounded
> reason — do not re-propose without new evidence.
>
> Overall verdict (consensus across lenses): the bones are good — the L1/L2/L3
> layering is real, the wasm cfg discipline is consistent, and the repo already
> knows how to hoist a generic seam (compaction.rs) and a pure core (turn_flow.rs).
> The debt is growth-by-accretion: three hand-mirrored turn loops, per-backend
> hand-cloned surface, two god-modules, and two docs/scripts drift traps.

Status: ☐ open · ◐ in progress · ☑ shipped (note commit)

---

## R1 ☑ CLAUDE.md stale load-bearing facts + staleness guard — `2ac8b4d` (2026-07-02)

Source: quality-infrastructure v4. CLAUDE.md is loaded into EVERY agent session and
is 9 minors stale: line ~19 pins "**0.51.x**" (crate is 0.60.20+); the "Canonical
addresses (post-reset)" table is the MODERATO TESTNET config mislabeled canonical
(mainnet diamond `0x8ab4f3a5…f3a77` per chain.rs:65 is primary); the Documentation
SOP still claims "README = derived copy of skill.md / guard `readme_skill_in_sync`"
while tests/readme_skill_in_sync.rs documents the exact opposite (#56 reversed;
guard is now `readme_is_substantive_but_guarded`).

Do: (1) drop the crate-version literal (point at Cargo.toml); (2) replace the
address table with a pointer to `src/registry/chain.rs` (MAINNET/MODERATO) +
contracts/README.md, keeping chain ids/RPCs; (3) rewrite the README-SOP paragraph;
(4) add `claude_md_facts_not_stale` test beside `no_doc_drift`: (a) no
`(**X.Y.x**)`-shaped version literal in CLAUDE.md, (b) every 40-hex `0x…` address
in CLAUDE.md appears verbatim in chain.rs or src/app/sponsor.rs. Mirror edits into
AGENTS.md (byte-sync test enforces). Risk ~zero.

## R2 ☑ release.ps1 → delegating shim over release.sh — `e160999` (2026-07-02)

Source: quality-infrastructure v4. release.ps1 is a 224-line hand-port — the exact
drift class build-web.ps1's own header documents as having silently shipped a
TESTNET bundle. Already visibly drifted both ways (release.sh has a
`cargo package --list` sanity step the port lacks; the port added a `node`
pre-flight release.sh lacks).

Do: replace the body with the proven verify.ps1/build-web.ps1 shim pattern
(resolve git-bash beside git.exe, delegate `release.sh $Version`, propagate exit
code); delete Invoke-Native + PS-side tag/CHANGELOG logic; port the `command -v
node` pre-flight back into release.sh; update scripts/CLAUDE.md (keep the ATOMIC
invariant text). Validate by running the shim so it exercises delegation + env,
expecting the "already released" pre-flight abort (there is NO dry-run mode — never
validate with an unreleased version). Risk low; fallback is release.sh directly.

## R3 ☑ Turn-loop phase A: loop_util migration + the shipped #29 drift bug — `35339a5` (2026-07-02)

Source: crate-architecture v4 + backends-connections v5 (phase 1 of R7; zero-risk,
land first). loop_util.rs's own doc says it's "currently consumed only by the
OpenAI loop". Byte-identical private copies: `extract_canonical_path`
(gemini/loop.rs:579, anthropic/loop.rs:747), `resolve_tool_args`
(anthropic/loop.rs:642, differs only in a log string); `emit_error` free fns vs the
hoisted state.rs:68 method (gated feature="openai" — widen per its comment).
⭐ PROVEN DRIFT BUG to fix here: the #29 stream-open retry exists in gemini:218 +
anthropic:244 but is MISSING in openai:207 — hoist the retry-wrapped open into
backends/retry.rs so it cannot drift again (behavior change for openai: changelog
it). Dedupe the copied resolve_tool_args test suites. Gate: existing per-backend
loop tests unmodified.

## R4 ☑ chat_toolset(): one tool-assembly for all backends — `877c1d8` (2026-07-02)

Source: app-browser-ide v5. src/app/chat/session.rs registers the identical ~70-tool
list twice (Anthropic branch 341-411, Gemini branch 480-550 — 146 `.with_tool()`
calls for 72 constructors) plus duplicated hook/capability/auth wiring; the branches
have already drifted once (start_subagent/generate_image gating, session.rs:74-80).
Tool metadata additionally lives in chat/prompt.rs prose, docs_manifest AGENT_TOOLS,
and llms.txt (that N-places problem is the docs SOP, out of scope here).

Do (narrowed, no SDK change): extract `fn chat_toolset(set_persona_allowed,
found_company_allowed, key, base_url) -> Vec<Arc<dyn Tool>>` + a shared assembly
step for capabilities/policies/confirm-dedup hooks/filesystem/system_instructions;
backend branches keep only genuine specifics (key/model, max-tokens naming,
thinking/temperature, auth, history_loads gate). Verify: tool-name count identical
on both backends; add a name-list constant test so they can never drift again.

## R5 ☐ Split display.rs (2,933 lines, 11 thread_locals) into display/ (2–3 days)

Source: app-browser-ide v4. One module fuses: worker lifecycle/watchdog (~1,425
lines), pointer/touch state, feed/notify bridge, compose bridge, http fetch bridge,
the ~600-line host::mp WebRTC star+mesh bridge, host::chat poll bridge (~2140-2455),
host::audio engine (mod audio 2700-2933), overlay/embed UI chrome, AND a pure
HTML→framebuffer rasterizer (2474-2699, zero web-sys, zero tests).

Do: display/{worker.rs, surface.rs, bridge/{feed,compose,http,mp,chat,audio}.rs},
thread_local state module-private per bridge; hoist the pure rasterizer to crate
root (src/html_fb.rs beside raster.rs) with native unit tests. Mechanical moves;
risk concentrates in the worker onmessage dispatch split — re-verify
test-compose-wiring.mjs (it guards cartridge-worker.js↔compose.rs vectors; neither
is touched, but confirm). Update src/app/CLAUDE.md map.

## R6 ☐ Make the Connection seam real: session surface + public start_with_strategy (2–4 days, pre-1.0 breaking batch)

Source: sdk-consumer v5. `Agent::start_with_factory` (agent.rs:1313) is PRIVATE — a
downstream `ConnectionStrategy` impl can never produce an Agent, so the crate's
"model-agnostic behind the Connection seam" pitch is documentation fiction. Agent
instead holds four typed `Option<Arc<XxxConnection>>` fields + six 4-arm if-let
methods; every backend carries `with_typed_capture` slot machinery; capabilities
are double-copied at agent.rs:987/1066/1099/1131.

Do: (1) session surface on the L3 seam (default-method or `SessionConnection`
subtrait): history_bytes / set_history_bytes (required — initial_history rides it)
/ compact / clear_history / transcript / set_thinking_override / set_model_override
— all four backends already implement exactly this (move-only); delete the typed
fields, if-let chains, and capture slots. Every new trait method repeats the
cfg_attr(?Send)/MaybeSendSync pattern + wasm guard build; regression-check browser
session save/restore (history.rs + session replay rides history_bytes). (2) `pub
async fn Agent::start_with_strategy(config, strategy)` — thin wrapper; unblocks
road-to-v1.0 item 1 (custom-Connection example, impossible today). (3) DOWNSCOPED:
do NOT collapse the five *AgentConfig builders into a generic AgentBuilder (they
encode real divergence); macro-generate only the ~10 truly-identical forwarders and
fix the capabilities double-copy at ONE bootstrap point. Breaking → lands in the
1.0.0 batch with the Phase-1 freeze (5901e29).

## R7 ☐ Generic TurnEngine: one streaming turn loop, three thin providers (1–2 weeks)

Source: backends-connections v5 + crate-architecture v4 (the big one; do LAST,
after R3/R6 have thinned the surface). anthropic/loop.rs opens with "Mirrors
backends/gemini/loop.rs 1:1 in control flow" — gemini ~605 / anthropic ~770 /
openai ~583 non-test lines re-implement the identical scaffold (idle/cancel
atomics, gate_pre_turn, retry-on-open, stall arm, MAX_TOOL_ROUNDS, finish-tool +
finish_summary, merge_round, turn_complete, dispatch_post_turn, compaction trigger).

Do: `backends::turn_engine` generic over a static-dispatch TurnProvider trait
(zero-sized marker + associated Message type — the proven CompactionModel pattern;
no async-trait gymnastics, wasm-safe by construction). Provider-owned surface (from
the real 3-way diff): build_request; stream-event fold (gemini thought-parts +
thoughtSignature, anthropic index-keyed thinking/signature/input_json deltas,
openai index-keyed tool_call fragments); resolve_pending_calls →
Vec<ResolvedCall{id,name,args,parse_error}>; assemble_assistant_message
(thinking-signed-first is anthropic-owned); tool-results → Vec<Message> (openai =
one message per call, gemini/anthropic = batched); finish-tool response shape;
map_finish_reason. Exactly two control-flow hooks with defaults: on_stream_end
(anthropic pause_turn resume, MAX_PAUSE_RESUMES) and on_cancel_with_pending_calls
(anthropic #82 tool_result balancing). Migration order: openai (already on
loop_util) → anthropic (proves both hooks) → gemini (always-on path, last). The
loop pinning tests (pre_turn_deny, inline_tool_call_step_is_done, usage folding,
thinking-block ordering, #82) must pass UNMODIFIED against the engine. OUT of
scope: mock/, mcp/, local/ (local's partial 4th copy = follow-up note, don't
block). Expected: ~1,900 duplicated hot-path lines → one engine + three
~150-250-line providers; every future loop fix lands once.

---

## Killed by skeptics — do NOT re-propose without new evidence

- **Crate-root junk drawer / semver** (x2 lenses): root modules are already handled
  by the Phase-1 freeze decisions; "silently inside the 1.0 promise" was wrong.
- **Compile Rust cores to wasm for proxy/worker**: proxy is EDGE runtime (no wasm
  file loading); the hand-port parity-test regime is the working answer.
- **Move on-chain ops out of events/**: run_sponsored_tempo_call is already UI-free;
  payoff collapses.
- **Widen Connection seam via dyn/enum dispatch**: re-litigates documented state.rs
  static-dispatch decision (R6 does the narrow, correct version).
- **schemars/typed-args tool schemas** (6th+ flagging): proposal fails its own
  safety claims; the ~90 hand schemas stay. PERMANENTLY dead absent a new design.
- **Generate the on-chain interface from one manifest**: past selector incidents
  wouldn't have been prevented by it; gen cost > drift cost at current facet churn.
- **Proxy is a "second agent platform"**: the two TS chain stacks are deliberate +
  complementary; scheduler/mcp runtimes are intentionally minimal.
- **Relay allowlist as data table**: "mostly exceptions" was factually wrong (~35
  of ~50 selectors still gate on LH_RELAY_FUNDED).
- **Un-compile the platform from SDK consumers / cut speculative subsystems**:
  stale evidence (road-to-v1 item 6 already executed; keeper has a shipped CLI
  consumer). Business-value calls, not refactors.
- **wasm-test harness for src/app**: real gap, but migration cost undercounted;
  the hoisted-pure-core pattern remains the answer (see R5's rasterizer hoist).
- **CI/gate topology rework**: the real gaps are one-line additive fixes, not a
  restructure.

## Lower-value backlog (v2–v3; pick up opportunistically, never over R-items)

L1 backend-onboarding builder ritual (partially absorbed by R6) · gate compilers/
platform cores behind features (business call — surface to user first) · app
#[test]s never run (mitigate via R5-style hoists) · events/mod.rs inline flow
bodies + 28 thread_locals (partially absorbed by R5) · mock as parallel impl ·
dead gemini/tools shim + render_system near-variant · one submission context for
79 *_sponsored wrappers · facet dead weight + persona duplication · stringly Http/
Other error variants (consider before 1.0 alongside R6) · loop_util stall
(superseded by R3/R7) · money-path E2E proofs run only by hand + stale wrong-chain
scripts · templates.rs growth hazard.
