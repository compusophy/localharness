# Model-agnostic evolution + the multi-model arc

## 1. The shift

localharness began as a Rust *port* of Google's "antigravity" Python SDK and is still described, in places, as "an agent SDK for Gemini." That framing is now wrong twice over: the crate is already architecturally decoupled — `src/connections/mod.rs` defines backend-neutral `Connection` / `ConnectionStrategy` traits, and `Agent`/`Conversation`/`Step`/`ToolCall`/`Content` depend only on those traits, never on Gemini wire — and the long-arc product is a model-agnostic, eventually self-hosting agent OS, not a Google client. What is actually hardwired is narrow: one concrete backend (`src/backends/gemini/`), Gemini model-id defaults sitting in the neutral `types.rs`, a `start_gemini`-only facade entry, and a Gemini-only credit proxy. The doc framing ("mirrors `google.antigravity.types`", `antig::mcp`, "SDK for Gemini") is legacy skin over an already-general skeleton.

**Principle:** localharness is one Rust crate with a *pluggable* LLM backend behind a single `ConnectionStrategy` seam — Gemini today, Anthropic next, local WebGPU and an own coding model on the arc. Backends are wire adapters; everything above Layer 3 is provider-neutral. The seam is real, not aspirational — and the first second-backend proves it by construction.

---

## 2. Phase A — Shed the port language

Doc-only changes (no behavior). Land these alongside the Anthropic backend so the headline matches reality. Each is `file:line → action`. **Real / safe to do now** unless flagged.

**Rewrite now (pure legacy antigravity-port language):**
- `src/content.rs:3-6` — replace `//! Mirrors google.antigravity.types.{Image,Document,...}` + "same harness build the Python SDK targets" with: self-standing multimodal input primitives; MIME lists enumerate types accepted by the supported backends.
- `src/types.rs:1-6` — replace `//! These mirror the Pydantic models in google/antigravity/types.py ... The Rust port uses owned data...` with: provider-neutral wire-adjacent boundary types every backend maps onto; owned data at the boundary, `Bytes` on hot paths.
- `src/backends/mcp/transport.rs:151` — `debug!(target: "antig::mcp", ...)` → `target: "localharness::mcp"`.

**Reframe headline identity (Gemini → "pluggable backends, Gemini today"):**
- `src/lib.rs:1` — `# localharness — Rust-native agent SDK for Gemini` → `Rust-native, model-agnostic agent SDK (Gemini today; pluggable backends)`.
- `Cargo.toml:6` — description → "A Rust-native agent SDK with pluggable LLM backends (Gemini today)...". Keep `keywords` (`gemini` is accurate + discoverable; crates.io caps at 5); revisit only when a 2nd backend ships.
- `README.md:5-6` — banner → "...agent SDK with pluggable LLM backends (Gemini today) — and a self-sovereign, browser-resident agent platform built on it."
- `README.md:19` — "A complete **Gemini** agent loop" → "A complete agent loop" (the loop is backend-agnostic; Gemini is the shipping backend).
- `web/llms.txt:155` — "Backend is Gemini API..." → "The default/shipping backend is the Gemini API..., behind a Connection/ConnectionStrategy trait layer additional backends plug into."
- `CLAUDE.md:7-8` — "...agent SDK for Google's Gemini API **and**..." → "...agent SDK with pluggable LLM backends (Gemini is the one shipping backend today) **and**...".

**Reframe the `DEFAULT_MODEL` comment (keep the value):**
- `src/types.rs:12-16` — keep `DEFAULT_MODEL = "gemini-3.5-flash"`; change the comment to "Default model **for the Gemini backend**. Verify ids against the live API before changing. Other backends carry their own default (e.g. `claude-haiku-4-5-20251001` for Anthropic)." (The deeper fix — moving the const *into* the backend module — is Phase B.)

**Leave as-is (flagged, not bugs):**
- `CHANGELOG.md:2760-2796` ("Initial Rust port of `google-antigravity`...") and `:1156` ("Antigravity-style icon toggles") — real history / a UI design reference. Do not rewrite deep changelog history.
- `contracts/.../LocalharnessRegistryFacet.sol:14` ("port of the flat `LocalharnessRegistry`") — "port" means the contract's own predecessor, not antigravity. False positive.
- `README.md:70`, `:145`, `web/llms.txt:79-81`, `lib.rs:46-52` — already model-agnostic OR accurately describe today's single Gemini BYOK/credit path. Update **when** Anthropic-via-credits actually ships, not before.

Honest note: shedding the language is cheap and partly cosmetic. The framing becomes *false-by-construction* — and thus genuinely shed — only when a second backend exists. Phase A is best landed in the same release as Phase B.

---

## 3. Phase B — The Anthropic backend (buildable)

A second backend is **one `ConnectionStrategy` + one `Connection` + a `wire`/`api`/`loop` trio + one shared-tool refactor**, with zero changes to Layers 1–3. New module mirroring `backends/gemini/`:

```
src/backends/anthropic/
├── mod.rs   AnthropicBackendConfig + AnthropicConnectionStrategy + AnthropicConnection
├── api.rs   AnthropicClient + MessagesSseStream (event:-block decoder)
├── wire.rs  Messages API types + From/Into to neutral Content/Part/Step/ToolCall/UsageMetadata
├── loop.rs  run_turn (Gemini control flow, Anthropic data shapes)
└── compaction.rs (reuse the summarize-prefix strategy over a /v1/messages one-shot)
```
Gated on a new `anthropic` Cargo feature; `backends/mod.rs` gains `#[cfg(feature="anthropic")] pub mod anthropic;`. The wasm `cfg_attr(..., async_trait(?Send))` mirror on both impls is **mandatory** (silent-wasm-break gotcha).

**Model IDs live in the backend, not `types.rs`** (verified live, June 2026):
```rust
pub const DEFAULT_MODEL: &str = "claude-haiku-4-5-20251001"; // cheapest, subsidized default
pub const SONNET_MODEL:  &str = "claude-sonnet-4-6";
pub const OPUS_MODEL:    &str = "claude-opus-4-8";           // 1M context, the Rust-coding tier
```
No `image_model` — Anthropic has no image-generation endpoint.

**The load-bearing wire differences (all absorbed below Layer 3):**

| Concern | Gemini | Anthropic | Absorbed by |
|---|---|---|---|
| Endpoint / auth | `:streamGenerateContent?alt=sse`, `x-goog-api-key` | `POST /v1/messages`, `x-api-key` + `anthropic-version: 2023-06-01` | `api.rs` |
| System prompt | `systemInstruction` Content | top-level `system: String` | `build_request` |
| Roles | `user` / **`model`** | `user` / **`assistant`** | `wire::Role` |
| Content shape | untagged `Part` (first-key) | `type`-tagged `Block` (`text`/`thinking`/`tool_use`/`tool_result`/`image`) | `wire::Block` enum |
| Tool call ↔ result | by **name** | by **`id`** (`tool_use_id`) | the **already-present** neutral `ToolCall.id`/`ToolResult.id` (Gemini leaves `None`) |
| Tool args delivery | whole `Value` in one chunk | streamed `input_json_delta.partial_json` fragments | loop's per-index `BTreeMap<u32, BlockAccum>` accumulator |
| SSE framing | `data:` JSON per candidate, `[DONE]` | named `event:`/`data:` deltas, ends on `message_stop` | `MessagesSseStream` state machine |
| Usage | one `usageMetadata`/chunk | split: `message_start.input_tokens` + `message_delta.output_tokens` | accumulate → neutral `UsageMetadata` (`cache_read_input_tokens` → existing `cached_content_token_count`, free prompt-caching surfacing) |
| Thinking | `thinkingConfig.thinking_budget` (0=off) | `thinking.budget_tokens` (≥1024, omit=off; **needs `max_tokens > budget`**) | `thinking_level_to_config` + clamp |
| `max_tokens` | optional | **required** (default 8192) | `MessagesRequest.max_tokens` |
| Stop reasons | `STOP/SAFETY/BLOCKLIST/...` | `end_turn/tool_use/max_tokens/refusal/pause_turn` | terminal-status match (`pause_turn` → re-request to resume) |

**What's reused verbatim:** `register_builtins` and every fs/portable builtin (`list_directory…edit_file`, `finish`, `ask_question`, `call_agent`, `compile_rustlite`, `render_html`, `run_cartridge`) — they consume neutral `Tool::input_schema()` JSON, which Anthropic's `tools[].input_schema` takes raw. The SSE frame-buffering skeleton (CRLF-tolerant `take_frame`, partial-chunk buffering) is wire-agnostic and should be *lifted*, not reimplemented. The loop's *structure* (accumulate text/thoughts/calls → hook→policy→`ToolRunner` dispatch → append results → re-loop → terminal `Step`) is copy-equivalent; only request-build and block-matching are rewritten.

**The one real shared-code refactor the second backend forces:** `start_subagent` and `generate_image` currently embed a Gemini `SharedClient` + `GenerateContentRequest`. Introduce a neutral trait the `BuiltinDeps` carries instead:
```rust
trait OneShot: MaybeSendSync {
    async fn complete(&self, system: Option<&str>, prompt: &str) -> Result<String>;
    async fn generate_image(&self, prompt: &str) -> Result<Media>; // Anthropic: returns config error
}
```
Anthropic's impl wraps a non-streaming `/v1/messages`; `generate_image` is simply not registered (or returns a clean tool error) on that backend.

**Facade (additive, non-breaking):** add `Agent::start_anthropic(AnthropicAgentConfig)` paralleling `start_gemini`. The internal `gemini_connection: Option<Arc<GeminiConnection>>` field (`agent.rs:305`) becomes a backend-neutral session-ops handle (small `trait SessionOps { history_bytes/compact/transcript }` the `Connection` exposes) so those methods work for either backend. The audit's bigger ask — a generic `Agent::start_with(strategy)` and dropping `gemini`/`with_api_key` off the neutral `AgentConfig` — is the clean long-term shape but is a **breaking** change; defer to the next major. *(UPDATE 2026-06-10: the `gemini`/`with_api_key` half is DONE — commit 47a4b2f removed them as write-only dead API; they were never read, so nothing broke. A public generic `start_with(strategy)` remains open — only the private `start_with_factory` exists.)*

**Build order (smallest shippable increments):** (1) `wire.rs` + deserialize unit tests; (2) `api.rs` `MessagesSseStream` + canned-frame tests (split-across-chunks + CRLF); (3) `loop.rs::run_turn` (+ the `OneShot` refactor lands here); (4) `mod.rs` strategy/connection + schema-parity guard test; (5) `Agent::start_anthropic` + the session-ops generalization; (6) proxy generalization (Phase C — gates *credits*; BYOK works without it); (7) Phase A doc fixes.

Honest scope: BYOK-Anthropic is fully shippable from steps 1–5 alone (`AnthropicBackendConfig::new(key)` → direct to `api.anthropic.com`). The *credit* path needs Phase C.

---

## 4. Phase C — Multi-model credit proxy, per-model pricing, payments

Today's `proxy/api/gemini.ts` (the one accepted server) is Gemini-only on two axes: it validates only the `/v1beta/models/<model>:<method>` path and charges one flat `COST_PER_REQUEST_WEI`. The auth token (`address:timestamp:signature`, Ethereum personal-sign) is **already provider-neutral**. Generalize minimally — a route table and a price table, **no new state, DB, queue, or second deployment.**

**(a) Provider routing.** Rename → `proxy/api/llm.ts` (keep old path as a `vercel.json` rewrite alias so `CREDIT_PROXY_URL` stays stable). Classify by path: `/v1beta/models/<model>:<method>` → Gemini; `/v1/messages` → Anthropic (model is in the body — parse once, price on it, forward the **exact bytes**). The proxy stays a dumb router: the *client's chosen backend already speaks the right dialect*, so no provider-shape translation happens server-side.

**(b) Both platform keys.** Add `ANTHROPIC_API_KEY` env beside `GEMINI_API_KEY`; an `UPSTREAM` table selects base + auth headers (`x-api-key` + `anthropic-version` for Anthropic). Neither key ever reaches the browser. Add `x-api-key`/`anthropic-version` to CORS allow-headers.

**(c) Per-model price table** in `$LH` wei, env-overridable, **unknown model → mid-price, never free** (an attacker can't request `model:"free-x"` to dodge the meter):
```
gemini:gemini-3.5-flash             0.01 LH
anthropic:claude-haiku-4-5-20251001 0.01 LH
anthropic:claude-sonnet-4-6         0.05 LH
anthropic:claude-opus-4-8           0.20 LH
unknown → DEFAULT_PRICE (mid); optional STRICT_MODELS=1 → 402 instead
```
Expose read-only `GET /api/prices` so the browser Usage panel + CLI render per-model cost and pre-flight "can I afford one Opus call" without hardcoding.

**Gating diff:** compute `cost = priceOf(provider, model)` *before* the credit check; gate `credit >= cost` (not the flat const); `meterDebit(address, cost)`; provider-select base + auth. Session mode keeps all-you-can-use semantics for beta (no per-call debit).

**The three+one access modes:**
- **BYOK, either provider** — `base_url = None`, raw key in the provider's native header, **skips the proxy entirely**. (Audit's "BYOK accepts Gemini OR Anthropic key.")
- **Platform credits, either provider** — `base_url = CREDIT_PROXY_URL`, the localharness token rides in whichever header the chosen backend uses; proxy recovers the address, gates on session/meter, prices per `provider:model`, debits, injects the platform key, forwards. User holds zero provider keys.
- **Subsidized-no-key** — identical proxy path; the credit balance was *bootstrapped by an invite* (`?invite=CODE` → `RedeemFacet.redeem` mints `$LH`). The proxy never knows how credits were minted — it only sees an active session / funded meter. **This is the user's stated near-term win: one redeemed balance calls `gemini-3.5-flash` (0.01) AND `claude-haiku-4-5` (0.01) AND `claude-sonnet-4-6` (0.05), each priced by the table, no key from either provider.**
- (fourth, Phase D: **local** — `base_url = None`, no proxy, price `0`.)

**Client (`chat.rs`/CLI):** add a persisted `lh_model = "provider:model"` selector beside `lh_model_access`; branch to construct the right `ConnectionStrategy`; route credits-mode through the same proxy. `resolve_credit_access`, `proxy_auth_token`, `credit_signer` are **reused unchanged** (already provider-neutral). Usage panel reads `/api/prices`.

**The x402 / MPP payment future.** The meter has a structural ceiling for mainnet: a *time session* can't price per model (a 1-hr session = unbounded free Opus), and the *meter* debit trusts `PROXY_METER_KEY` to debit honestly, fronts gas per call, and races on nonce. The fix turns each metered call into a **caller-signed payment event**, using the **already-shipped `X402Facet.settle` + `x402_hook`**:
1. Client looks up `cost` from `/api/prices`, **signs an x402 authorization** (`registry::sign_x402`) for `value = cost`, fresh nonce, short `validBefore`.
2. Sends it as an `X-PAYMENT` header (Coinbase x402 convention: 402 → attach → retry) alongside the identity token.
3. Proxy verifies locally (signer == address, value ≥ price, window valid, nonce unused via `authorizationState`), **sponsored-settles** (`settle_x402_sponsored` — AlphaUSD gas fronted by the sponsor, not the user), forwards. Nonce burned on-chain → replay impossible, caller-chosen nonce → race gone.

This is per-model, per-call, caller-authorized, mainnet-safe — and it's the *same* X402Facet used for `call_agent`, so **agent-to-agent inference payment is identical plumbing** (an agent's TBA pays per inference out of its own `$LH`). **MPP / Tempo stablecoin rails are the on-ramp** (card/fiat/BTC/stablecoin → AlphaUSD → `$LH`); x402 is the *spend*; the price table is the *meter* between them. Each layer stays on-chain except the one proxy. Migration: ship per-model pricing on the meter path now (zero client churn beyond the selector) → add `X-PAYMENT` as a third gate (opt-in) → mainnet makes x402 the only gate for premium models; sessions retire or become a prepaid bulk-discount wrapper.

Real vs aspirational: provider routing + per-model pricing + BYOK-either + subsidized-no-key are **all buildable now** off shipped facets. The x402-per-call gate is **also real** (the facet + signer ship today) but is the *next* phase, not the immediate one.

---

## 5. Phase D — Local embedded models

**Verdict: real today for the right size class — shippable as a feature in weeks, genuinely useful for narrow tasks, NOT a drop-in for the main agent loop.** A `WebLlmConnectionStrategy` (wasm32 + `browser-app` only) wraps web-llm (MLC) or Transformers.js v4 via wasm-bindgen, implements `Connection` exactly like Gemini except `send` drives an in-process WebGPU decode loop and `subscribe_steps` emits from the token callback. No proxy, no key, no session — BYOK taken to its limit: bring-your-own-*compute*, price `0` in the table.

What's actually true (June 2026): the in-browser sweet spot is **0.5B–3B at 4-bit** (~300MB–2GB, <10s load, **40–180 tok/s**); Qwen 3.5 0.8B / Phi-3.5-mini / Gemma 2 2B / Llama 3.2 3B all run. **Chrome caps ~4GB VRAM/tab**, so "~4GB Gemma/Qwen/Kimi" is the *ceiling*, not the target — aim 2–3GB. WebGPU keeps ~85% of native decode perf; Transformers.js v4 is 3–10× v3.

The real engineering gap is **tool calling**: small local models emit tool calls as templated *text*, not structured `tool_use` blocks, so the backend needs a per-model parser mapping output → the neutral `Step::ToolCall` (same adapter pattern as Anthropic, one layer deeper). OPFS caches the weights (`OpfsFilesystem` exists); gate behind an explicit `Action::DownloadLocalModel` — **never auto-pull on a marketing visit** (same gate-discipline as wallet creation).

Honest limits: 40–180 tok/s is fine for a 200-token answer, painful for a 10-step `run_send` loop; a 2B model won't drive `create_and_publish_app` end-to-end. Position it as a **router fast-path** — classify intent / draft rustlite / summarize / run offline, then *escalate to Gemini/Claude for the hard turn*. Sequencing: **weeks** = WebGPU-detect + one 0.8B model, text-only, no tools (offline-chat proof); **months** = tool-call template parser → local model drives fs builtins → difficulty router (local ↔ Gemini ↔ Claude per turn).

---

## 6. Phase E — The own coding model + the Opus flywheel

**Verdict: the data-generation + verification flywheel is real, and this repo is *unusually* well-equipped because it already owns the verifier. Fine-tuning is cheap; the moat is the verified dataset, not the model.**

Correct the base-model premise: Kimi K2.6 is 1T params (32B active MoE) — too big to *be* the device model and overkill to retune for one language. The honest split:
- **Teacher (problem generator):** **Opus 4.8** (`claude-opus-4-8`) — the stronger Rust coder, reached **via the Phase B Anthropic backend**. ✓ exactly the user's plan.
- **Student (the model you own):** a small base you can LoRA cheaply and *eventually run in Phase D's browser backend* — **Qwen2.5-Coder-7B** is the proven RLVR target (+25% HumanEval+ in **48 H100-hrs**), or a 1.5–3B coder for the browser ceiling. "Kimi-retuned" is the aspiration; the shippable student is 1.5–7B. Name that honestly in release notes.

**The flywheel — reusing what already exists.** `scripts/verify.sh` is *already* a 6-stage proof-of-spec gate whose stages 3–6 instantiate real wasm (rustlite → compile → run → render → compose). That **is** the RLVR verifier:
1. **Generate** — Opus emits `{problem, reference solution .rl, tests}` triples.
2. **Verify** — run `verify.sh` on the reference; **reject any triple whose own solution fails the gate** → every kept problem is machine-checkable.
3. **Grade student** — student attempts; run the *same* gate on its output; pass/fail = verifiable reward, no human label.
4. **Train** — SFT on Opus references, then RLVR on pass/fail (LoRA; 200 → thousands; Opus generates a tier-1…tier-N difficulty curriculum). Student improves → harder problems → repeat.

Why this is defensible *for localharness specifically*: the verifier is the unsolved bottleneck of RLVR in every other domain — and **you already built it** (for release safety, an unrelated reason). It's **rustlite-specific**: a general Rust coder exists; a model fluent in *your* rustlite subset + the `host::display`/`host::compose`/`host::net` cartridge ABI does not, and Opus-generated-and-`verify.sh`-verified problems in that ABI are a dataset no one else can make. 200 problems is the calibrated starting order of magnitude.

Sequencing: **weeks (no GPU)** = `scripts/gen-problems.sh` calling Opus (via the new backend) → keep only gate-passing triples → ship a 200-problem verified dataset in-repo (*a publishable artifact needing no training run*); **months** = ~50–100 H100-hrs (~hundreds of $) to LoRA-SFT a Qwen-Coder then RLVR with `verify.sh` as reward; **long arc** = quantize to 4-bit → load as a Phase D WebGPU backend → *localharness ships its own rustlite-native model, in-browser, zero API cost*. That is where Phases D and E converge.

---

## 7. Phase F — Decentralized compute/learning cluster + browser-OS-VM

**Verdict: where vaporware lives unless ruthless about the smallest real step. Honest baseline: browser-tab *training* of LLM-scale models is NOT real this decade; geo-distributed *GPU* training IS (Nous DisTrO 15B, Prime Intellect INTELLECT-1/-2 10–32B) — but those are GPU nodes, not browser tabs. What browser tabs CAN do today is distributed *inference* and distributed *data generation*.**

**The smallest real first step reuses five shipped facets — a verified-data-contribution market, not training.** Phase E's flywheel is embarrassingly parallel (every problem is independent):
1. A participant's browser runs the **generator** (own key or platform credits) + the **verifier** (`verify.sh`'s wasm stages, already in-browser via the cartridge harness).
2. They submit verified triples **on-chain** using the **`FeedbackFacet` append-only-log shape** (a `DatasetFacet` storing content-hash + contributor; triple bytes to OPFS/IPFS, proof-of-verification + hash on-chain).
3. They're **paid in `$LH`** via the **already-shipped `X402Facet`** — the same settlement as agent-to-agent, now human-to-platform for verified data. Every component exists; you're pointing shipped facets at a new payload.

**Distributed-inference variant (also real, slightly later):** once Phase D ships, a tab with a loaded local model *is* an inference node. A requesting agent calls a peer via `call_agent`, the peer runs local inference, settles in `$LH` over x402 — a decentralized inference market with the credit proxy *not even in the path*. The honest gap (did the peer actually run the model? latency?) is exactly what the **ERC-8004 reputation/validation facet** (already on the roadmap) addresses: validators **re-run `verify.sh` to slash liars** — the verifier composes here too.

**Long-arc, named as vision not roadmap:** actual collaborative *training* (DisTrO-style gradient sharing) needs GPU nodes + a gradient-aggregation protocol you don't have — genuinely multi-year. The "learning cluster" as co-training only becomes real *after* the data + inference markets exist and create demand. Don't lead with it.

---

## Sequenced plan

| Horizon | Ship | Status / cost | Unlocks |
|---|---|---|---|
| **Weeks** | **Phase B + C keystone: `AnthropicConnectionStrategy` + multi-provider proxy + Phase A doc fixes** — subsidized no-key user calls Gemini *and* Claude, per-model `$LH` pricing | **Real today.** Days of wire-adapter work | Model-agnostic proven by construction; antigravity framing dead; **Opus teacher online** |
| **Weeks** | **`gen-problems.sh`** — Opus emits triples → `verify.sh` keeps only gate-passing → 200-problem verified dataset in-repo | **Real, no GPU.** Uses the new backend + the verifier you own | The flywheel's data layer; a publishable artifact |
| **Weeks → Months** | **WebGPU local backend v0** — one 0.8B model, OPFS-cached, text-only, explicit download gate | **Real today** (web-llm / Transformers.js v4); tools deferred | Zero-cost offline path; inference-node substrate |
| **Weeks → Months** | **x402-per-call gate** (`X-PAYMENT` header → `settle_x402_sponsored`) as a third proxy gate | **Real** (facet + signer shipped); opt-in | Mainnet-safe per-model payment; agent-to-agent inference payment |
| **Months** | **Tool-call template parser** + **difficulty router** (local ↔ Gemini ↔ Claude) | Engineering, not research | Genuine zero-cost agent for narrow turns |
| **Months** | **LoRA-SFT + RLVR** a Qwen-Coder, `verify.sh` as reward | ~50–100 H100-hrs (~hundreds of $) | localharness's *own* coding model |
| **Months** | **Verified-data market** — `DatasetFacet` (Feedback-shaped) + x402 payout | **Real** — reuses 5 shipped facets | Decentralized data production, paid in `$LH` |
| **Long arc** | Quantize own model → WebGPU backend (D + E converge); **distributed inference market** (peer local-inference over x402 + ERC-8004 validation) | Real once the above land | Self-hosting, self-improving agent OS |
| **Multi-year** | **Collaborative *training*** (DisTrO-style) across GPU nodes coordinated on-chain | Frontier; needs GPU nodes + gradient protocol you don't have | **Vision, not roadmap** — the "learning cluster" end state |

**The throughline:** Anthropic backend → is the teacher that → generates the dataset that → trains the own-model that → runs in the local backend that → becomes an inference node in the cluster that → `verify.sh` polices throughout. The verifier built for release safety turns out to be the load-bearing primitive for the entire arc — which is why **the dataset, not the model and not the cluster, is the moat.**

## The single highest-leverage first move

**Ship the Anthropic backend as a second `ConnectionStrategy` and make the credit proxy multi-provider in the same stroke.** It is the only step shippable in *days*, it delivers the user's stated near-term win (invite-code user calls both Gemini and Claude, zero keys), and it is the architectural proof that "model-agnostic" is real: every later phase (local WebGPU, the own coding model) plugs into the *same* seam, and the day `AnthropicConnectionStrategy` ships, "mirrors `google.antigravity.types`" becomes false-by-construction. Concretely, the first commit is `src/backends/anthropic/wire.rs` + its deserialize tests — the smallest increment that starts the whole arc.

*Bias check, stated honestly:* the user's memory records repeated declines of "a second LLM backend for its own sake." This is not that — it's greenlit verbatim as the *enabling substrate* for local models, the own coding model, and per-model credit pricing. If the user still prefers not to, the fallback keystone is the WebGPU local backend first (also a `ConnectionStrategy`, also proves the seam) — but it's weeks not days and ships zero immediate user value. Anthropic-first is the recommendation.