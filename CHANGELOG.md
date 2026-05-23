# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.7.0] - 2026-05-23

M4 — the browser-resident IDE moves into the crate as `src/app/`,
gated on `feature = "browser-app"`. The previous `localharness-web`
JS-binding crate and the ~700 lines of inline JS in `web/index.html`
are gone; the UI is now pure Rust + maud HTML templates + HTMX-style
fragment swaps.

### Added

- **`feature = "browser-app"`** (default off). Compiles `src/app/`
  into the crate as a wasm cdylib. Pulls in `maud` for HTML templating
  and `console_error_panic_hook`. Has no effect on a native build.
- **`src/app/`** — the in-tab IDE. Modules: `mod` (mount + state),
  `templates` (maud), `dom` (web-sys helpers), `events` (delegated
  click + keydown), `chat` (turn flow), `opfs` (file browser).
  Architectural rule: no imperative DOM manipulation — all updates are
  `swap_inner` / `swap_outer` / `insert_adjacent_html` targeted at
  fixed element ids.
- **Inline tool-call rendering.** Each `ToolCall` from the
  `StreamChunk` stream renders a collapsible `<details>` block in
  the assistant turn; the matching `ToolResult` swaps the block's
  status pill (`⋯ running` → `✓ done` / `✗ error`) and fills the
  args + result panes.
- **Rust-driven OPFS panel.** The file browser now reads through the
  `Filesystem` trait (was: hand-rolled JS over `navigator.storage`).
  Navigate via `data-action="opfs-nav"` + `data-arg=path`; open files
  via `data-action="opfs-open"`. Refreshes after every chat turn.

### Changed

- **`web/index.html`** shrunk from ~700 lines of JS application code +
  ~250 lines of HTML/CSS to a ~15-line bootstrap (style + `<div id="root">`
  + a one-line `import init` script). All chrome is rendered by Rust
  templates.
- **`scripts/build-web.{sh,ps1}`** now invokes `wasm-pack build .
  --features browser-app --no-default-features` against the root crate
  (was: `wasm-pack build ./localharness-web`). Output bundle name
  changed from `localharness_web*` to `localharness*`.
- **`[lib] crate-type = ["lib", "cdylib"]`** added so native consumers
  still get an rlib and wasm-pack gets a cdylib from the same package.
- `[package.metadata.wasm-pack.profile.release].wasm-opt = false` —
  modern rustc emits post-MVP wasm ops (bulk-memory,
  nontrapping-fptoint) that the wasm-pack-bundled wasm-opt rejects.
  Costs ~10-20% binary size; gains a build that doesn't depend on a
  moving toolchain target.

- **Markdown rendering for assistant text** via `pulldown-cmark`
  (optional dep, pulled in by `browser-app`). Renders at end-of-turn
  per text segment; tool-call blocks remain interleaved between
  rendered segments.
- **`Filesystem::delete(path)`** trait method. Implemented on
  `NativeFilesystem` (recursive `remove_dir_all` / `remove_file`) and
  `OpfsFilesystem` (`removeEntry` with `recursive: true`). Required
  `FileSystemRemoveOptions` web-sys feature. Source-compat break for
  external `Filesystem` impls — they must implement the new method.
- **OPFS wipe button** now actually wipes. Confirms via `window.confirm`,
  walks the OPFS root, deletes every top-level entry, refreshes the panel.
- **Per-turn timing pills** in the status line —
  `done · ttft N ms · total M ms · K turns`.
- **Conversation history persistence.** `GeminiConnection::history_bytes()`
  / `set_history_bytes()` serialize/restore the Gemini wire history as
  opaque bytes. `GeminiAgentConfig::with_history_bytes()` seeds a new
  agent on startup. `Agent::history_bytes()` exposes the typed accessor
  for non-trait Gemini APIs (typed handle stashed during
  `start_gemini` via a new `GeminiConnectionStrategy::with_typed_capture`).
  The browser app writes `.lh_history.json` to OPFS after every turn
  and restores on mount; the "new conversation" button also deletes
  the marker file so a reload starts fresh.
- **Inline OPFS file editing.** The file viewer gains an `edit` button
  that swaps it into an editor (textarea + save/cancel). Save calls
  `Filesystem::write_atomic` and re-renders the viewer with the new
  contents.
- **Public transcript view** for repainting a UI on session resume.
  New types `TranscriptEntry { role, text }` + `TranscriptRole`; new
  methods `GeminiConnection::transcript()` and `Agent::transcript()`;
  new free function `decode_transcript_bytes(&[u8])` for the
  no-instance case (the browser app uses this on mount before any
  agent exists). Tool-call activity is intentionally dropped from the
  projection — this is the human-readable view.

### Removed

- **`localharness-web/`** crate. The published SDK never re-exported it
  (it was `publish = false`), and no external consumer existed. All
  its functionality (`start_session`, `chat`) moved into `src/app/`
  as internal-only code.

## [0.6.0] - 2026-05-22

M3 — fs builtins on a portable `Filesystem` trait with native + OPFS
implementations. The same 6 fs-shaped tools the CLI uses now run in a
browser tab against the Origin Private File System.

### Added

- **`Filesystem` trait** (`src/filesystem/`). Five-method async surface
  (`read`, `write_atomic`, `metadata`, `read_dir`, `walk`) plus
  `DirEntry` / `WalkEntry` / `Metadata` / `EntryKind` value types. The
  `write_atomic` docstring spells out the atomicity contract every impl
  must satisfy.
- **`NativeFilesystem`** (gated on `feature = "native"`). Wraps
  `tokio::fs` + `walkdir` + `tempfile`. Atomicity via tempfile + rename.
- **`OpfsFilesystem`** (wasm32 only). Backs the trait against the
  browser's Origin Private File System via `web-sys`. Atomicity via
  `FileSystemWritableFileStream.close()` swap. Recursive walk + async
  iteration over `FileSystemDirectoryHandle.entries()`.
- **`GeminiBackendConfig::with_filesystem(fs)`** and the delegating
  **`GeminiAgentConfig::with_filesystem(fs)`**. Plug in any
  `Filesystem` impl; `Arc<ConcreteFs>` unsize-coerces to
  `Arc<dyn Filesystem>` automatically.
- **Browser demo gains the 6 fs builtins.** `localharness-web` now
  ships an `OpfsFilesystem` to the agent and enables the full
  capabilities set, so the model in the live demo can `list_directory`,
  `view_file`, `find_file`, `search_directory`, `create_file`, and
  `edit_file` against per-origin OPFS storage.

### Changed

- The 6 fs built-ins (`list_directory`, `view_file`, `find_file`,
  `search_directory`, `create_file`, `edit_file`) no longer call
  `tokio::fs` / `walkdir` / `tempfile` directly — they hold an
  `Arc<dyn Filesystem>` and dispatch through the trait. Their
  constructors changed from unit structs to `Tool::new(fs)`. Source
  compat for downstream code that built tools directly is broken; the
  `register_builtins` path is unchanged.
- The 6 fs built-ins lost their per-file `#[cfg(feature = "native")]`
  gates. They now compile on all targets; registration is gated by
  whether `BuiltinDeps.fs` is `Some(_)`. On native, `connect`
  auto-installs `NativeFilesystem`; on wasm, callers supply an OPFS
  (or other) impl via `with_filesystem`.
- `GeminiConnectionStrategy::connect` honors a caller-supplied
  filesystem before falling back to the platform default.

## [0.5.0] - 2026-05-22

Phase 8 — the SDK now compiles to `wasm32-unknown-unknown`. The same
`Agent` loop the CLI uses runs inside a browser tab; a live demo is
hosted at [antig-compusophys-projects.vercel.app](https://antig-compusophys-projects.vercel.app/).

### Added

- **`wasm32-unknown-unknown` target.** `cargo check
  --no-default-features --target wasm32-unknown-unknown` succeeds.
  The full `Agent → Conversation → Connection → ToolRunner` chain
  is available in the browser; 4 portable built-in tools
  (`ask_question`, `finish`, `generate_image`, `start_subagent`)
  register automatically.
- **`native` cargo feature** (default-on). Gates the parts of the
  SDK that need OS primitives: subprocess spawning, multi-threaded
  tokio, the 6 filesystem builtins (`list_directory`, `view_file`,
  `find_file`, `search_directory`, `create_file`, `edit_file`),
  `run_command`, and the MCP stdio bridge. wasm callers depend with
  `default-features = false`.
- **`src/runtime.rs`** — new module. `runtime::spawn` cfg-gates
  between `tokio::spawn` (native) and
  `wasm_bindgen_futures::spawn_local` (wasm).
  `runtime::MaybeSendSync` is a marker trait that's `Send + Sync` on
  native and empty on wasm — every trait supertraits it instead of
  `Send + Sync` directly.
- **`Connection::subscribe_steps`** now returns a `StepStream` type
  alias that maps to `BoxStream` on native (Send-bound, for
  `tokio::spawn` compatibility) and `LocalBoxStream` on wasm (where
  browser fetch streams aren't `Send`).
- **`localharness-web/` cdylib** (not published). wasm-bindgen
  reference wrapper exposing `start_session(api_key)`,
  `chat(prompt, on_chunk)`, `reset_session()` to JavaScript. Stores
  one `Agent` per tab in a `thread_local<RefCell<Option<Rc<Agent>>>>`.
- **`web/` static site** with `index.html` (streaming chat UI,
  markdown rendering, multi-turn conversation, key cached in
  sessionStorage) and `web/pkg/` (committed wasm-pack output).
- **`vercel.json` + `.vercelignore`** for static-deploy config.
- **`scripts/build-web.{ps1,sh}`** to rebuild the wasm bundle.
- **`scripts/probe-gemini.ps1`** — isolates request-shape vs
  response-parse bugs by hitting the live Gemini API with curl-style
  diagnostics.
- **`CLAUDE.md`** at the repo root — project orientation for future
  Claude Code sessions.
- **`DESIGN.md` Phase 8 addendum** documenting the wasm scope and
  what's deferred.

### Changed

- Every `#[async_trait]` site is now `cfg_attr`'d to use
  `async_trait(?Send)` on wasm so reqwest's browser-fetch futures
  (which aren't `Send`) can satisfy the trait method signatures.
- Trait supertraits — `Tool`, `Connection`, `ConnectionStrategy`,
  the 6 hook traits, `Trigger` — changed from `: Send + Sync` to
  `: MaybeSendSync`.
- `JoinHandle` storage in `Agent` / `Conversation` /
  `TriggerRunner` is cfg-gated; on wasm we fire-and-forget through
  `spawn_local` (no abort handle).
- README adds a "Run in the browser" section and the status line
  now mentions wasm32.

### Fixed

- **`GeminiSseStream::take_frame`** now accepts `\r\n\r\n` frame
  separators in addition to `\n\n`. Browser fetch surfaces Gemini's
  SSE with CRLF — the old parser silently dropped every frame on
  wasm (0 chunks emitted). Regression test covers the CRLF case.

### Compatibility

- 0.x → 0.x: the trait supertrait change (`Send + Sync` →
  `MaybeSendSync`) is source-compatible for downstream impls
  because `MaybeSendSync` is blanket-implemented for any
  `T: Send + Sync` on native. On wasm the bound is relaxed.
- `wasm-bindgen-futures` is a new wasm-target-only dependency.
  Native consumers don't pull it in.

## [0.4.0] - 2026-05-21

GA of Phase 7 — context-window compaction + MCP stdio bridge. The
crate now covers every roadmap item from the original `DESIGN.md`.

### Added

- README expanded with feature-tour entries for MCP-bridged tools
  and automatic compaction.

### Changed

- Built-in tool table marks `start_subagent` as shipping (was
  "not yet implemented" in 0.2.0).

This release contains no code changes vs `0.4.0-alpha.2` other than
the bump and the doc edits. The two alphas covered the implementation.

## [0.4.0-alpha.2] - 2026-05-21

### Added

- **MCP stdio client** under `backends::mcp`. The agent can now expose
  tools served by an external [MCP][mcp] server. Configure via
  `with_mcp_server(McpServerConfig::Stdio { command, args })`; the
  bridge spawns the server, performs the JSON-RPC `initialize`
  handshake, fetches `tools/list`, and registers each remote tool into
  the `ToolRunner` as an `McpTool` adapter. Tool calls are forwarded
  to the server with a 60 s per-call timeout; the response is
  flattened into `{ text, images, is_error }`.

  Scope (alpha.2):
  - Stdio transport only. `Sse` / `Http` variants on
    `McpServerConfig` are accepted at the type level but
    `connect()` returns `Error::Config`. SSE / HTTP land in a later
    alpha.
  - Tools surface only — prompts, resources, sampling, and
    subscriptions are out of scope.
  - Eager registration. Tools are fetched once at connect; server-side
    tool changes are not re-discovered.
  - Custom or built-in tools already registered under the same name
    **win** (MCP doesn't overwrite).

- `AgentConfig::with_mcp_server` and `GeminiAgentConfig::with_mcp_server`
  builder methods.
- Re-exports: `McpBridge`, `McpClient`, `McpToolDecl` from the crate
  root.
- The agent shutdown sequence tears down every MCP subprocess after
  the connection closes.

[mcp]: https://modelcontextprotocol.io

## [0.4.0-alpha.1] - 2026-05-21

### Added

- **Context-window compaction** under
  `backends::gemini::compaction`. When the last turn's
  `prompt_token_count` exceeds
  `CapabilitiesConfig::compaction_threshold`, the loop summarizes the
  oldest history entries via a separate Gemini call and replaces them
  with one synthetic user-role turn tagged `[compacted prior context]`.

  Algorithm:
  - Always preserve the system instruction and the **last 6 user/model
    pairs** verbatim.
  - Honor function-call / function-response pairing — never split a
    `Model { functionCall }` from its `User { functionResponse }`.
  - If summarization fails (network, missing client), fall back to a
    drop-oldest strategy with a tag so the model knows context was
    dropped.
  - A turn never errors out because of a compaction failure; the loop
    logs at WARN and continues.

- 4 new unit tests covering `pick_split` boundary behavior and the
  `should_compact` threshold check. Total: 24 passing.

### Notes

- Threshold is opt-in via `CapabilitiesConfig::compaction_threshold`
  (existing field — previously unused). Set to `None` (default) to
  disable. Typical values: 60-80% of your model's max context window.
- Compaction is intentionally conservative: a small history isn't
  compacted at all (`MIN_HISTORY_TO_COMPACT = 8`).

## [0.3.0] - 2026-05-20

### Removed (BREAKING)

- `Agent::start_local`, `LocalAgentConfig`, `LocalConfig`,
  `connections::local::LocalConnection`,
  `connections::local::LocalConnectionStrategy`, and the entire `proto`
  module are gone. The Go-binary backend they implemented was
  `#[deprecated]` since `0.2.0-alpha.1`; migrate to `start_gemini` /
  `GeminiAgentConfig`.
- Dependencies dropped: `tokio-tungstenite`, `prost`, `prost-types`,
  `path-clean`. The `signal` tokio feature is no longer enabled.
- `Error::ProtoEncode`, `Error::ProtoDecode`, `Error::WebSocket`,
  `Error::BinaryNotFound` removed (no callers). `Error::Http` added in
  case a future backend wants it.

### Added

- **`start_subagent` built-in tool** — completes the 11/11 `BuiltinTool`
  matrix. Spawns a one-shot subagent against the parent's Gemini client:
  takes `{ system_instructions, prompt }`, runs a single text-only turn,
  returns `{ final_response, finish_reason }`. No tool delegation in v1
  (subagent tool dispatch is 0.4.x work).

### Changed

- Crate description updated for the post-Go-binary world.

## [0.2.0] - 2026-05-20

GA of the Rust-native runtime. The crate is now fully self-contained —
no Go binary, no Python install, no localhost daemon.

### Added

- README rewritten for the Gemini backend as the documented default.
  Built-in tool catalog table, structured-output and workspace
  examples, updated architecture diagram showing the inline tool
  dispatch loop.

### Changed

- The `start_gemini` API surface is now considered stable for 0.2.x.
  Breaking changes will require a minor (or major) bump.

### Deprecated

- `Agent::start_local`, `LocalAgentConfig`, `LocalConfig`,
  `LocalConnection`, `LocalConnectionStrategy` remain marked
  `#[deprecated(since = "0.2.0-alpha.1")]`. Removal scheduled for 0.3.0.

## [0.2.0-beta.1] - 2026-05-20

### Added

- **`generate_image` built-in tool** — calls the Gemini image-generation
  model (default `gemini-3.1-flash-image-preview`) via a new
  `GeminiClient::generate` non-streaming method. Returns
  `{ mime_type, data_base64, bytes_len }`; the agent's `image_model`
  config and shared `GeminiClient` are injected at strategy time.
- **`ask_question` built-in tool** (default no-op). Returns
  `{ skipped: true, responses: [] }`. Designed to be overridden — a
  user-registered `ask_question` tool wins (the strategy never
  overwrites). Lets the model attempt interactive flows on hosts that
  don't yet wire interactive UI.
- `BuiltinDeps` struct passed to `register_builtins` so future built-ins
  can pick up additional construction context (image client today).

### Status

All 11 `BuiltinTool` variants except `start_subagent` are now
implemented. Subagents land in 0.3.x.

## [0.2.0-alpha.3] - 2026-05-20

### Added

- **Three write tools** under `backends::gemini::tools`:
  - `create_file(path, content)` — atomic write via `NamedTempFile` +
    rename. Refuses to overwrite. Auto-creates parent directories.
  - `edit_file(path, old_string, new_string, replace_all?)` — exact-once
    substring replacement (or `replace_all: true` to replace every
    occurrence). Atomic write.
  - `run_command(command, working_dir?, timeout_sec?)` — shell exec
    (`cmd /C` on Windows, `sh -c` elsewhere). Per-stream 256 KiB output
    cap, default 30s / max 600s timeout, `kill_on_drop`, surfaces
    `{stdout, stderr, exit_code, timed_out}`.
- All three are auto-registered when `CapabilitiesConfig` enables them
  (the unrestricted default). Workspace-only safety: pair with
  `with_workspace(...)` to gate file writes inside specified directories.

### Changed

- `extract_canonical_path` now resolves the parent directory when the
  target file does not yet exist (necessary for `create_file` to be
  guarded by `workspace_only`).
- 8 new unit tests covering create/edit/run_command happy + error
  paths. Total: 20 tests passing.

### Dependencies

- `tempfile = "3"` (atomic file writes).

## [0.2.0-alpha.2] - 2026-05-20

### Added

- **Tool calling end-to-end** through the Gemini backend. The agent
  loop now drives a model ↔ tool dispatch loop: streams the response,
  collects `functionCall` parts, routes each through hooks → policies →
  `ToolRunner`, appends `functionResponse` parts to history, and
  continues until the model produces no more function calls (or hits
  the 16-round safety cap).
- **Five read-only built-in tools** under `backends::gemini::tools`:
  - `list_directory(path)` — sorted children with name/kind/size.
  - `view_file(path, start_line?, end_line?)` — 1-indexed inclusive
    range, 256 KiB truncation cap, UTF-8 lossy.
  - `find_file(path, pattern, max_depth?)` — glob-matched recursive
    file search, 1000-match cap.
  - `search_directory(path, pattern, file_glob?, case_sensitive?)` —
    regex content search, 500-match / 4 MiB-per-file cap.
  - `finish(output?)` — terminates the turn; captures structured
    output when the agent is configured with a response schema.
- `tools::ToolRunner::iter_tools()` — snapshot every registered tool
  for `FunctionDeclaration` construction.
- `GeminiBackendConfig::with_capabilities` and `GeminiAgentConfig`
  routes built-in selection through `CapabilitiesConfig::effective_tools`.
- Built-in tools are auto-registered into the `ToolRunner` at connect
  time per the capability list. User-registered tools of the same name
  win (no overwrite).
- Unit tests for `list_directory`, `view_file` against the real
  filesystem.

### Changed

- `Agent::start_local` / `start_gemini` now go through
  `start_with_factory<S, F>` so backends can opt into runner injection.
  The Gemini strategy uses this to dispatch function calls inline.
- The agent loop emits `Step { kind: ToolCall, target: Environment }`
  events when dispatching, so `ChatResponse::tool_calls()` lights up.
- `walkdir`, `globset`, `regex` added as deps (built-ins only).

## [0.2.0-alpha.1] - 2026-05-20

### Added

- **`Agent::start_gemini(GeminiAgentConfig)`** — Rust-native Gemini
  backend. Talks to the Gemini REST API directly via `reqwest`; no Go
  binary, no Python install, no external process. This is Phase 1 of
  the 0.2.x runtime per `DESIGN.md`.
- `backends::gemini::{GeminiBackendConfig, GeminiConnectionStrategy,
  GeminiConnection}` — public API for the new backend.
- `backends::gemini::api::GeminiClient` — async client over `reqwest`
  with API-key redaction in `Debug`. Includes a small SSE decoder
  (`GeminiSseStream`) that handles partial chunks and `[DONE]` terminators.
- `backends::gemini::wire::*` — `serde` types matching the Gemini REST
  contract (camelCase verbatim). Round-trip tests cover text, thought,
  and `functionCall` part shapes.
- `backends::gemini::loop::run_turn` — the agent loop. Streams text and
  thought deltas, accumulates the assistant turn into history, emits a
  terminal `Step`. Phase 1 is text-only; tool calls land in Phase 2.
- `examples/text_chat.rs` — end-to-end example against `GEMINI_API_KEY`:
  streams tokens, prints usage summary.

### Changed

- `ChatResponse::text_stream()`, `thoughts()`, `tool_calls()` now return
  `BoxStream<...>` so callers can iterate with `.next().await` without
  needing to `Box::pin` themselves.
- `Agent::start_local`/`start_gemini` share a single
  `start_with_strategy` bootstrap — every future backend gets the same
  hook/tool/policy wiring for free.

### Deprecated

- `Agent::start_local` and the entire 0.1.x `LocalConnection`
  (Go-binary-backed) backend. Will be removed in 0.3.0.

## [0.1.1] - 2026-05-20

### Changed

- Rewrote `README.md` as a full crate landing page: hero example,
  collapsible feature tour (streaming, dual-cursor, custom tools,
  policies, workspace, triggers, multimodal, resume), ASCII
  architecture diagrams, design-notes section, comparison table
  vs the Python SDK, and FAQ.

### Added

- `RELEASING.md`, `CHANGELOG.md`, and `scripts/release.{sh,ps1}`
  define a one-command atomic release process.

## [0.1.0] - 2026-05-20

### Added

- Initial Rust port of the [`google-antigravity`][upstream] Python SDK,
  pinned to upstream commit
  [`d6be9ca`](https://github.com/google-antigravity/antigravity-sdk-python/commit/d6be9ca).
- **`Agent`** (Layer 1) — builder-style config, write-tool safety check,
  background dispatcher routing custom tool calls through
  hooks → policies → `ToolRunner` → `send_tool_results`.
- **`Conversation` + `ChatResponse`** (Layer 2) — stateful session with
  multi-cursor lazy stream (replay-from-zero per cursor). Filtered
  cursors: `text_stream`, `thoughts`, `tool_calls`. Per-turn usage,
  cumulative usage, structured output extraction.
- **`Connection` + `LocalConnection`** (Layer 3) — transport over the
  `localharness` binary. `AtomicBool` for idle, `tokio::sync::broadcast`
  for step fan-out, bounded `mpsc` inbox (cap 16), single
  `tokio::select!` supervisor, separate process supervisor with
  `kill_on_drop`. 10 s handshake timeout.
- **Hook system** — six trait kinds (session start/end, pre/post turn,
  pre/post tool call) with hierarchical `HookContext`.
- **Policy engine** — Python-matching precedence (specific deny ≻
  specific ask ≻ specific allow ≻ wildcard deny ≻ wildcard ask ≻
  wildcard allow), `enforce()` adapter, `workspace_only()` with
  component-wise path containment (defeats `/foo/bar-evil` vs
  `/foo/bar` prefix tricks).
- **`ToolRunner`** — lock-free context swap via `arc_swap`, `ClosureTool`
  builder for ad-hoc tools.
- **`TriggerRunner`** — `every()` helper, abort-on-drop,
  `TriggerDelivery` semantics.
- **Multimodal input** — `Content` / `Part` / `Media` with `from_path()`
  MIME inference; `Bytes`-backed payloads (refcounted, zero-copy clones).
- **Typed errors** — flat `thiserror` enum; `io::Error`,
  `serde_json::Error`, `prost` errors fold via `#[from]`.
- **Smoke example** (`cargo run --example smoke`) — end-to-end against a
  stubbed `Connection`.
- **Upstream sync infrastructure** — `UPSTREAM.md` pins commit;
  `scripts/sync-upstream.{sh,ps1}` diff against pin without modifying
  the working tree.

[upstream]: https://github.com/google-antigravity/antigravity-sdk-python
[Unreleased]: https://github.com/compusophy/localharness/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/compusophy/localharness/releases/tag/v0.1.0
