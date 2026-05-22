# `localharness` — design document

This document plans the **0.2.x** line: replacing the Go `localharness`
binary with a Rust-native agent runtime that talks to the Gemini API
directly. After 0.2.x ships, the crate has zero Go dependencies and
zero external binaries — `cargo add localharness` is enough.

Treat this document as the spec. Each section names concrete files,
types, and acceptance criteria. Future sessions execute against it
without re-deriving the architecture.

## Contents

1. [Why](#why) — the case for replacing the Go binary
2. [What stays](#what-stays) — the public surface we preserve
3. [What changes](#what-changes) — the runtime we rebuild
4. [Gemini API surface](#gemini-api-surface) — exactly what we call
5. [Built-in tool catalog](#built-in-tool-catalog) — every tool the Go
   harness exposes, ported one by one
6. [Phase plan](#phase-plan) — milestone-sized chunks
7. [Module-by-module spec — Phase 1](#module-by-module-spec--phase-1)
8. [Deprecation timeline](#deprecation-timeline) — when
   `LocalConnectionStrategy` goes away
9. [Open questions](#open-questions)

---

## Why

The Go binary is a black box. It bundles Google's release infrastructure,
its own update channel, and a protocol that's only loosely versioned.
Users pay for it in:

- **Install friction** — `pip install google-antigravity` to get a Go
  binary, then point an env var at a wheel-private path. Awful for a
  Rust ecosystem.
- **Opaque behavior** — when the harness misbehaves, we read Go stack
  traces over stderr and guess.
- **Platform fragility** — the binary is platform-specific; non-x64
  Linux is hit-or-miss.
- **No introspection** — we can't reason about retries, timeouts, or
  rate-limit behavior because we don't own the loop.

Rust-native solves all of the above. The Gemini REST API is stable,
public, and well-documented. The agent loop is ~500 lines of Rust.

## What stays

The public surface from 0.1.x is the contract. Existing 0.1.x code
should compile against 0.2.x with **only a backend-selection change**
(`Agent::start_local` → `Agent::start_gemini`).

Preserved verbatim:

- `Agent`, `AgentConfig` (builder pattern)
- `Conversation`, `ChatResponse`, `ChatCursor`
- `Connection` trait (the abstraction boundary)
- `ConnectionStrategy` trait
- Every hook trait (`PreTurnHook`, `PreToolCallDecideHook`, …)
- `HookContext`, `SessionContext`, `TurnContext`, `OperationContext`
- `Policy`, `evaluate`, `enforce`, `workspace_only`, `allow_all`,
  `deny_all`
- `Tool` trait, `ClosureTool`, `ToolRunner`, `ToolContext`
- `Trigger`, `TriggerRunner`, `every`
- All `types::*` (Step, ToolCall, ToolResult, UsageMetadata,
  StreamChunk, etc.)
- All `content::*` (Content, Part, Media, MediaKind)

Removed:

- `connections::local::LocalConnection` and `LocalConnectionStrategy`
  (kept deprecated through 0.2.x, removed in 0.3.0)
- All `proto::*` types tied to the Go binary's WebSocket shape
- `LocalAgentConfig` (replaced by `GeminiAgentConfig`; alias kept
  through 0.2.x for source compat)

## What changes

New modules:

```
src/
├── backends/
│   ├── mod.rs            – backend selection
│   └── gemini/
│       ├── mod.rs        – GeminiConnectionStrategy + GeminiConnection
│       ├── api.rs        – Gemini REST client (reqwest)
│       ├── wire.rs       – Gemini request/response types
│       ├── loop.rs       – the agent loop (the heart of the runtime)
│       └── tools/        – built-in tool implementations
│           ├── mod.rs
│           ├── list_directory.rs
│           ├── view_file.rs
│           ├── search_directory.rs
│           ├── find_file.rs
│           ├── create_file.rs
│           ├── edit_file.rs
│           ├── run_command.rs
│           ├── ask_question.rs
│           ├── generate_image.rs
│           └── finish.rs
```

`connections/local.rs` becomes a deprecation stub that re-exports
nothing useful in 0.3.0.

New deps:

| crate | why |
|-------|-----|
| `reqwest` (rustls, json, stream) | HTTPS to the Gemini API |
| `eventsource-stream` *(maybe)* | parse SSE responses; alt: hand-rolled |
| `uuid` | stable IDs for steps and tool calls |
| `walkdir` | `find_file` / `search_directory` recursion |
| `globset` | glob patterns in `find_file` |
| `grep-searcher` or `regex` | content search in `search_directory` |
| `tempfile` | safe write-then-rename in `create_file` / `edit_file` |
| `which` | resolve commands in `run_command` (sandbox-aware) |

Removed deps (eventually):
`tokio-tungstenite`, `prost`, `prost-types`, `dunce`, `path-clean`.

## Gemini API surface

We only need a tiny slice of the Gemini API. All requests:

```
POST https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent?alt=sse
x-goog-api-key: <key>
content-type: application/json
```

Request body (`wire::GenerateContentRequest`):

```jsonc
{
  "systemInstruction": { "parts": [ { "text": "..." } ] },
  "contents": [
    { "role": "user",  "parts": [ { "text": "..." } ] },
    { "role": "model", "parts": [ { "functionCall": { "name": "...", "args": {...} } } ] },
    { "role": "user",  "parts": [ { "functionResponse": { "name": "...", "response": {...} } } ] }
  ],
  "tools": [
    { "functionDeclarations": [
        { "name": "...", "description": "...", "parameters": { /* JSON Schema */ } }
    ] }
  ],
  "toolConfig": { "functionCallingConfig": { "mode": "AUTO" } },
  "generationConfig": {
    "thinkingConfig": { "thinkingBudget": 0 },        // or omit
    "responseMimeType": "application/json",            // when structured output
    "responseSchema": { /* JSON Schema */ }
  }
}
```

Response (streaming, `text/event-stream`, one JSON object per `data:`):

```jsonc
{
  "candidates": [{
    "content": {
      "role": "model",
      "parts": [
        { "text": "incremental text chunk" },
        { "thought": true, "text": "incremental thought" },
        { "functionCall": { "name": "view_file", "args": { "path": "..." } } }
      ]
    },
    "finishReason": "STOP"   // or "TOOL_USE", or absent for in-flight
  }],
  "usageMetadata": {
    "promptTokenCount": 12,
    "candidatesTokenCount": 34,
    "thoughtsTokenCount": 5,
    "totalTokenCount": 51
  }
}
```

That's the whole protocol. The agent loop wraps it.

### Image generation

Separate endpoint, separate model. Same auth header.

```
POST https://generativelanguage.googleapis.com/v1beta/models/{image_model}:generateContent
```

Request supplies a prompt; response carries base64 PNG bytes in
`candidates[0].content.parts[].inlineData`.

## Built-in tool catalog

Every tool the Go harness exposes, in target-port order. Each row
documents the schema we declare to the model and the Rust function
signature that implements it.

| Tool | Read/Write | Args | Returns | Phase |
|------|:----------:|------|---------|:-----:|
| `list_directory` | R | `path: string` | `{ entries: [{ name, kind, size }] }` | 2 |
| `view_file` | R | `path: string, start_line?: int, end_line?: int` | `{ content: string, total_lines: int }` | 2 |
| `find_file` | R | `path: string, pattern: string` | `{ matches: [string] }` | 2 |
| `search_directory` | R | `path: string, pattern: string, file_glob?: string` | `{ matches: [{ path, line, text }] }` | 2 |
| `create_file` | W | `path: string, content: string` | `{ ok: true }` | 3 |
| `edit_file` | W | `path: string, old_string: string, new_string: string` | `{ ok: true, replacements: int }` | 3 |
| `run_command` | W | `command: string, working_dir?: string, timeout_sec?: int` | `{ stdout, stderr, exit_code }` | 3 |
| `ask_question` | I/O | `AskQuestionInteractionSpec` | `QuestionHookResult` | 4 |
| `generate_image` | W | `prompt: string` | `{ media: Media }` | 4 |
| `finish` | term | `structured_output?: any` | (terminates turn) | 4 |
| `start_subagent` | spawn | `system_instructions, initial_prompt` | `{ steps, final_response }` | 5 |

Notes:

- **Read-only set** (`BuiltinTool::READ_ONLY`) stays `list_directory`,
  `search_directory`, `find_file`, `view_file`, `finish`.
- **Write tools** must be gated by a policy (the existing
  write-tool-requires-policy check in `Agent::start_*` enforces this).
- **`canonical_path`** on `ToolCall` is populated by the runtime
  (after `dunce::canonicalize`) so `workspace_only` keeps working.
- **`run_command`** uses `tokio::process::Command`. Default
  `timeout_sec` is 30; the runtime kills the child on timeout.
- **`edit_file`** uses write-then-rename via `tempfile::NamedTempFile`
  in the target directory to keep edits atomic.

## Phase plan

| Phase | Version | Deliverable | Estimated effort |
|:-----:|---------|-------------|------------------|
| **1** | 0.2.0-alpha.1 | Gemini backend skeleton: text-only chat, streaming text deltas, usage metadata. No tools, no thoughts, no structured output. Tests against the real API. | 1 session |
| **2** | 0.2.0-alpha.2 | Tool calling end-to-end. Read-only built-ins (`list_directory`, `view_file`, `find_file`, `search_directory`, `finish`). Custom tools too. Hooks + policies dispatched correctly. | 1 session |
| **3** | 0.2.0-alpha.3 | Write tools (`create_file`, `edit_file`, `run_command`). Workspace-sandbox check baked in. Full policy integration. | 1 session |
| **4** | 0.2.0-beta.1 | Streaming thoughts (`thinkingConfig`), `ask_question` round-trip, structured output (`finish` + `responseSchema`), `generate_image`. | 1 session |
| **5** | 0.2.0 | GA. `LocalConnectionStrategy` is `#[deprecated]`. Docs front-page Gemini backend. Smoke tests pass against live API. | 1 session |
| **6** | 0.3.0 | Delete `LocalConnectionStrategy` and the proto layer. Add subagents. | future |
| **7** | 0.4.0 | Compaction (sliding-window context management), MCP client integration. | future |
| **8** | 0.5.0 | `wasm32-unknown-unknown` target + live browser demo. Native code stays gated behind a `native` cargo feature; the Agent loop runs in a tab via a `localharness-web` cdylib. | shipped |
| **9** | 0.6.0 | `Filesystem` trait + `NativeFilesystem` + `OpfsFilesystem`. The 6 fs-shaped builtins now run in a browser tab against the Origin Private File System. New `with_filesystem` builder lets callers plug in any impl. Web demo gains an OPFS file browser. | shipped |

Per phase: tests, doctests, smoke example, CHANGELOG entry, release via
`scripts/release.{sh,ps1}`.

### Phase 8 notes — wasm32 + web demo (0.5.0)

Phase 8 isn't in the original phase plan; it was triggered by a
"kitchen-sink demo" request after 0.4.0 GA. Scope:

- **`native` cargo feature.** Default-on. Gates `tokio` multi-thread
  + `process` + `fs` features, `walkdir`, `tempfile`, the 6 fs
  builtins, `run_command`, and the MCP stdio bridge. wasm callers opt
  out via `default-features = false`.
- **`src/runtime.rs`.** New module — defines `spawn` (cfg-gated
  `tokio::spawn` vs `wasm_bindgen_futures::spawn_local`) and a
  `MaybeSendSync` marker (`Send + Sync` on native, empty on wasm).
- **Trait Send-relaxation.** Every `#[async_trait]` is `cfg_attr`'d
  to `async_trait(?Send)` on wasm so browser-fetch futures (which
  aren't `Send`) can satisfy the method signatures. Trait supertraits
  changed from `: Send + Sync` to `: MaybeSendSync`.
- **`Connection::subscribe_steps`.** Returns a new `StepStream` type
  alias that maps to `BoxStream` (native) or `LocalBoxStream` (wasm).
- **`localharness-web/` cdylib.** Out-of-tree (not a workspace
  member). Depends on `localharness = { path = "..", default-features
  = false }`. Stores one `Agent` per browser tab in a
  `thread_local<RefCell<Option<Rc<Agent>>>>`. Exposes
  `start_session`, `chat(prompt, on_chunk)`, `reset_session` to JS.
- **`web/` static site.** `index.html` (chat UI, marked.js for
  assistant-side markdown) + `web/pkg/` (wasm-pack output, committed
  for static deploy). Vercel serves `web/` verbatim with
  `max-age=0, must-revalidate` on `/pkg/*` so redeploys don't get
  stuck behind aggressive caching.
- **SSE parser fix.** `GeminiSseStream::take_frame` now matches both
  `\n\n` and `\r\n\r\n` frame separators — browser fetch surfaces
  Gemini's SSE with CRLF. A regression test covers the CRLF case.

What didn't ship in Phase 8 (largely closed by Phase 9 / 0.6.0):

- ~~The 6 fs builtins on wasm.~~ Shipped in 0.6.0 via `OpfsFilesystem`.
- Tool-call rendering in the web demo. Today only assistant text streams;
  tool calls are dispatched in-Rust but the UI surfaces only their net
  effect (e.g. the OPFS file list refresh after a turn). Inline tool-call
  rendering is still pending.
- `policy.rs::dunce::canonicalize` compiles on wasm but degrades to
  identity (Err-on-canonicalize), so `workspace_only` policies match
  trivially. Not a runtime panic — but the workspace check isn't
  meaningful on wasm until canonicalisation lives in the `Filesystem`
  layer.

### Phase 9 notes — Filesystem trait + OPFS (0.6.0)

Triggered by the M3 plan in CLAUDE.md. Scope:

- **`Filesystem` trait** (`src/filesystem/mod.rs`). Five async methods —
  `read`, `write_atomic`, `metadata`, `read_dir`, `walk` — plus
  `DirEntry` / `WalkEntry` / `Metadata` / `EntryKind`. The
  `write_atomic` docstring spells out the atomicity contract.
- **`NativeFilesystem`** (gated on `feature = "native"`). Wraps
  `tokio::fs` + `walkdir` + `tempfile`; atomicity via tempfile + rename.
- **`OpfsFilesystem`** (wasm32 only). Backs the trait against
  `navigator.storage.getDirectory()`. Atomicity via
  `FileSystemWritableFileStream.close()` swap. Recursive walk + async
  iteration over `FileSystemDirectoryHandle.entries()` via the JS
  iterator protocol (web-sys's typed wrappers don't expose async
  iterators directly, so we use `Reflect::get` + `JsFuture`).
- **6 fs tools refactored.** Each holds an `Arc<dyn Filesystem>`,
  lost its per-file `cfg(feature = "native")` gate. Constructors
  changed from unit structs to `Tool::new(fs)` — source-compat break
  for downstream code that built tools directly; `register_builtins`
  unchanged.
- **`with_filesystem` builder** on both `GeminiBackendConfig` and
  `GeminiAgentConfig`. Without it, native installs `NativeFilesystem`
  automatically and wasm skips fs-builtin registration.
- **`localharness-web` wires OPFS.** Plugs `OpfsFilesystem` via
  `with_filesystem`, enables `CapabilitiesConfig::unrestricted()` so
  all 10 portable builtins register.
- **Web demo file browser.** Vanilla JS panel reads
  `navigator.storage.getDirectory()` directly (same OPFS root the
  Rust side uses), with breadcrumb navigation, file preview, and a
  wipe button. Auto-refreshes after each chat turn.

## Module-by-module spec — Phase 1

### `src/backends/gemini/api.rs`

Thin async HTTPS client over `reqwest::Client`. One public type:

```rust
pub struct GeminiClient {
    http: reqwest::Client,
    api_key: SecretString,        // zeroize on drop (use `secrecy` or hand-roll)
    base_url: Url,                // default https://generativelanguage.googleapis.com
}

impl GeminiClient {
    pub fn new(api_key: impl Into<String>) -> Self { ... }
    pub fn with_base_url(self, url: Url) -> Self { ... }

    /// Streaming generate. Caller gets an `impl Stream<Item = Result<GenerateChunk>>`.
    pub async fn stream_generate(
        &self,
        model: &str,
        req: wire::GenerateContentRequest,
    ) -> Result<impl Stream<Item = Result<wire::GenerateChunk>> + Send>;
}
```

Internally: builds the URL `models/{model}:streamGenerateContent?alt=sse`,
sets headers, posts JSON, parses SSE frames into `GenerateChunk`s.

### `src/backends/gemini/wire.rs`

`serde` types matching the Gemini REST contract. Keep field naming
verbatim — use `#[serde(rename_all = "camelCase")]` at type level.
Types needed for Phase 1:

- `GenerateContentRequest`
- `Content { role: ContentRole, parts: Vec<Part> }`
- `Part` enum: `Text { text }`, `Thought { text, signature? }`,
  `FunctionCall { name, args }`, `FunctionResponse { name, response }`,
  `InlineData { mime_type, data }`
- `ContentRole` (`User`, `Model`)
- `FunctionDeclaration { name, description, parameters: serde_json::Value }`
- `Tool { function_declarations: Vec<FunctionDeclaration> }`
- `ToolConfig { function_calling_config: FunctionCallingConfig }`
- `GenerationConfig { thinking_config?, response_mime_type?, response_schema? }`
- `ThinkingConfig { thinking_budget?: u32 }`
- `GenerateChunk { candidates: Vec<Candidate>, usage_metadata?: WireUsage }`
- `Candidate { content: Content, finish_reason?: FinishReason }`
- `WireUsage` (camelCase mirror of our `UsageMetadata`)
- `FinishReason` (Stop, MaxTokens, Safety, Recitation, ToolUse, Other)

### `src/backends/gemini/loop.rs`

The agent loop. One public function:

```rust
pub(crate) async fn run_turn(
    client: &GeminiClient,
    config: &GeminiBackendConfig,
    tool_runner: &ToolRunner,
    hook_runner: &HookRunner,
    history: &mut Vec<wire::Content>,
    user_message: wire::Content,
    step_tx: &broadcast::Sender<Step>,
    idle: &AtomicBool,
) -> Result<TurnOutcome>;
```

Pseudocode:

```text
push user_message onto history
loop:
    build request from { system_instruction, history, tools }
    stream response chunks:
        accumulate text -> emit Step { Text, content_delta }
        accumulate thought -> emit Step { Thought, thinking_delta }
        observe function_call -> queue for dispatch
        on finish_reason:
            STOP -> emit terminal text step, set idle, return
            TOOL_USE -> break inner loop
    for each queued function_call:
        route through hook_runner.dispatch_pre_tool_call
        if denied: build functionResponse with error
        else: tool_runner.execute(name, args) -> functionResponse
    append functionCall + functionResponse parts to history
    continue outer loop
```

### `src/backends/gemini/mod.rs`

Public surface:

```rust
pub struct GeminiBackendConfig {
    pub api_key: String,
    pub model: String,                       // default: DEFAULT_MODEL
    pub image_model: String,
    pub system_instructions: Option<SystemInstructions>,
    pub thinking: Option<ThinkingLevel>,
    pub response_schema: Option<String>,
    pub base_url: Option<Url>,               // override for tests/proxies
    pub conversation_id: Option<String>,     // for resume
}

pub struct GeminiConnectionStrategy { /* … */ }
impl GeminiConnectionStrategy {
    pub fn new(config: GeminiBackendConfig) -> Self { ... }
}

#[async_trait]
impl ConnectionStrategy for GeminiConnectionStrategy {
    async fn connect(&self) -> Result<Arc<dyn Connection>> { ... }
}

pub struct GeminiConnection {
    /* the AtomicBool idle flag, broadcast::Sender<Step>, history Mutex,
       client, config, hook_runner, tool_runner. */
}

#[async_trait]
impl Connection for GeminiConnection {
    fn is_idle(&self) -> bool { /* atomic */ }
    fn conversation_id(&self) -> &str { /* uuid */ }
    async fn send(&self, content: Content) -> Result<()> { /* spawn run_turn */ }
    fn subscribe_steps(&self) -> BoxStream<'static, Result<Step>> { /* broadcast */ }
    async fn send_tool_results(&self, _: Vec<ToolResult>) -> Result<()> {
        // No-op: the Gemini backend dispatches tools inline inside run_turn.
        Ok(())
    }
    async fn send_trigger(&self, content: String) -> Result<()> { /* same as send */ }
    async fn wait_for_idle(&self) -> Result<()> { ... }
    async fn shutdown(&self) -> Result<()> { ... }
}
```

### `src/agent.rs` additions

```rust
impl Agent {
    pub async fn start_gemini(config: GeminiAgentConfig) -> Result<Self> { ... }
}

pub struct GeminiAgentConfig {
    pub agent: AgentConfig,
    pub gemini: GeminiBackendConfig,
}
```

The existing `start_local` keeps working through 0.2.x, marked
`#[deprecated(since = "0.2.0", note = "use start_gemini")]`.

### Acceptance criteria — Phase 1

1. `cargo build` + `cargo test` + `cargo clippy --all-targets -D warnings` green.
2. `examples/test_agent.rs` updated to demonstrate the Gemini backend
   against a live `GEMINI_API_KEY`.
3. A new `examples/text_chat.rs` shows the 10-line quick start using
   `start_gemini`.
4. No new clippy lints. No `unwrap()` on hot paths. No mutex held
   across `.await`.
5. `cargo doc --no-deps` builds clean.
6. CHANGELOG entry: "`### Added` — Gemini direct backend (Phase 1).
   `### Deprecated` — `Agent::start_local` and `LocalConnectionStrategy`."

## Deprecation timeline

| Version | `LocalConnectionStrategy` | `GeminiConnectionStrategy` |
|---------|---------------------------|---------------------------|
| 0.1.x | sole backend | n/a |
| 0.2.0-alpha.1 | works, undocumented | text-only |
| 0.2.0-alpha.2 | works, undocumented | + tool calling |
| 0.2.0-alpha.3 | works, undocumented | + write tools |
| 0.2.0-beta.1 | `#[deprecated]` | + streaming/thoughts/finish/image |
| 0.2.0 | `#[deprecated]`, hidden from docs | default, documented |
| 0.3.0 | **removed** | sole backend |

## Open questions

1. **Secret handling**. Use `secrecy::SecretString` for the API key or
   hand-rolled `Zeroizing<String>`? `secrecy` is one more dep but
   battle-tested.
2. **Compaction strategy**. Gemini context windows are big but not
   infinite. Sliding-window with summarization is the obvious answer;
   defer to phase 7. For phase 1-5 we just send the full history.
3. **Rate limit / retry policy**. Out of scope for phase 1; users wrap
   `start_gemini` with `tower` or similar. We may add `RetryConfig` in
   a later phase.
4. **MCP support**. Phase 7. The Python SDK's MCP bridge gives this for
   free once we have a generic "external tool source" abstraction; the
   `Tool` trait already supports it.
5. **Subagent isolation**. Each subagent gets its own
   `GeminiConnection` with a fresh history but shared `ToolRunner`?
   Or shared `ToolRunner` with restricted policy? Defer to phase 6.

## Out of scope for 0.2.x

- Vertex AI auth (Service Account flow). The API key flow covers the
  same models for almost all users; Vertex parity is a 0.4.x consideration.
- Non-Gemini providers. The `Connection` abstraction permits an
  `OpenAiConnectionStrategy` later, but that's a separate crate or
  feature gate.
- Local model backends (llama.cpp etc.). Same: future work.
