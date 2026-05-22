# CLAUDE.md

Project context for Claude Code sessions. Read this first.

## What this is

`localharness` is a Rust-native agent SDK for Google's Gemini API.
Single crate, zero external binaries — `cargo add` and you have an
agent loop with streaming text, tool calling, hooks, policies,
triggers, MCP integration, and context compaction.

- Published on [crates.io/crates/localharness](https://crates.io/crates/localharness)
- Repo at [github.com/compusophy/localharness](https://github.com/compusophy/localharness)
- Native target: stable Rust 1.85+, tokio-driven
- wasm32 target: same crate compiles to the browser; live demo at
  [antig-compusophys-projects.vercel.app](https://antig-compusophys-projects.vercel.app/)

## Repo layout

```
src/                       library crate
├── lib.rs                 re-exports + module roots
├── agent.rs               Agent facade (Layer 1)
├── conversation.rs        Conversation + ChatResponse (Layer 2)
├── connections/           Connection / ConnectionStrategy traits (Layer 3)
├── content.rs             Content, Media, Part (user-facing message types)
├── tools.rs               Tool trait + ToolRunner + ClosureTool
├── hooks.rs               6 hook traits + HookRunner
├── policy.rs              Predicate / Policy / Decision + workspace_only
├── triggers.rs            Trigger trait + TriggerRunner + every()
├── runtime.rs             cfg-gated spawn helper + MaybeSendSync marker
├── filesystem/            Filesystem trait + Native + OPFS impls (M3)
├── types.rs               wire-adjacent enums (BuiltinTool, Step, etc.)
├── error.rs               Error + Result
└── backends/
    ├── gemini/
    │   ├── api.rs         GeminiClient + SSE decoder (CRLF + LF tolerant)
    │   ├── wire.rs        REST request/response types
    │   ├── loop.rs        run_turn — the inner agent loop
    │   ├── compaction.rs  history summarisation
    │   ├── tools/         11 built-in tools
    │   └── mod.rs         GeminiConnectionStrategy + GeminiConnection
    └── mcp/               stdio MCP client (native-only)

localharness-web/          wasm cdylib (publish=false)
└── src/lib.rs             wasm-bindgen wrapper exposing chat() to JS

web/                       static site for Vercel
├── index.html             chat UI
└── pkg/                   wasm-pack output (committed for deploy)

scripts/
├── release.{ps1,sh}       atomic release tool (see RELEASING.md)
├── build-web.{ps1,sh}     wasm-pack build → web/pkg/
└── probe-gemini.ps1       isolate request-shape vs. response-parse bugs

DESIGN.md                  phase plan + per-module spec
RELEASING.md               step-by-step + recovery table
CHANGELOG.md               per-version changes (Keep-a-Changelog)
UPSTREAM.md                history with the Python repo
vercel.json                static-deploy config (no build step)
.vercelignore              keep target/ out of the upload (28k+ files)
```

## Build / test / run

```sh
cargo build                                                   # native (default features)
cargo test                                                    # full test suite
cargo check --no-default-features --target wasm32-unknown-unknown  # wasm guardrail
./scripts/build-web.sh                                        # rebuild wasm bundle
vercel deploy --prod --yes                                    # deploy web/
```

## Cargo features

- `native` (default): enables `tokio` multi-thread + process + fs +
  io-util, plus the `walkdir` and `tempfile` deps. Required for the 6
  filesystem builtins, `run_command`, and the MCP stdio bridge.
- (wasm targets) automatically drop `walkdir`/`tempfile` and add
  `wasm-bindgen-futures`, `uuid/js`, `getrandom/js` via target-cfg.

wasm callers should depend with `default-features = false`.

## The wasm story (M2.5)

The crate compiles to `wasm32-unknown-unknown` because:

- `src/runtime.rs::spawn` cfg-gates `tokio::spawn` (native) vs.
  `wasm_bindgen_futures::spawn_local` (wasm).
- `src/runtime.rs::MaybeSendSync` is `Send + Sync` on native and
  empty on wasm. Every trait that used to require `: Send + Sync`
  now requires `: MaybeSendSync`.
- Every `#[async_trait]` is `cfg_attr`'d to use `?Send` on wasm so
  browser-fetch futures (which aren't `Send`) can satisfy the trait
  method signatures.
- `Connection::subscribe_steps` returns a `StepStream` type alias
  that maps to `BoxStream` (native) or `LocalBoxStream` (wasm).
- `JoinHandle` storage and abort logic is cfg-gated; on wasm we
  fire-and-forget via `spawn_local`.
- Tools that need OS primitives are gated behind `feature = "native"`:
  6 fs builtins, `run_command`, MCP. The 4 portable ones
  (`ask_question`, `finish`, `generate_image`, `start_subagent`) work
  on both targets.

When adding new traits or `tokio::spawn` calls, mirror these patterns
or wasm will break silently (the gated modules don't trip in a default
`cargo check`).

## Common gotchas

- **PowerShell 5.1 stderr trap.** `release.ps1` wraps native commands
  in `Invoke-Native` because PS5 turns every cargo stderr line into a
  terminating error. Don't call `cargo`/`git`/`gh` directly inside
  the script.
- **Gemini 3.x `thought: false` parts.** The wire `Part` enum is
  untagged; `Part::Thought { thought: bool, .. }` is declared
  *before* `Part::Text { text }`. Gemini 3.x stamps every part with a
  `thought` field, so a normal text part deserializes into
  `Part::Thought { thought: false, text: Some(...), .. }`. Consumers
  must handle that variant explicitly. See
  `localharness-web/src/lib.rs` for the working match.
- **SSE on wasm uses CRLF.** Browser fetch surfaces Gemini's SSE
  with `\r\n\r\n` frame separators. `GeminiSseStream::take_frame`
  now matches both `\n\n` and `\r\n\r\n`. Don't regress to LF-only.
- **`max-age=immutable` on `/pkg/*` was a footgun.** `vercel.json`
  uses `max-age=0, must-revalidate` so redeploys actually take effect
  without forcing a hard-reload. Add a version query string before
  re-enabling long caching.
- **The release script only commits `Cargo.toml` + `Cargo.lock` +
  `CHANGELOG.md`.** Anything else that needs to ship in a release
  must be committed *before* invoking the script. See RELEASING.md.

## Release process

```sh
# 1. Land all the feature work as normal commits.
# 2. Edit CHANGELOG.md - add `## [X.Y.Z]` heading (no date - script adds).
# 3. Run the atomic release script.
./scripts/release.sh X.Y.Z          # bash / git-bash
pwsh scripts/release.ps1 -Version X.Y.Z   # PowerShell on Windows
```

The script does pre-flight checks → version bump → cargo verify →
commit → tag → push → cargo publish → GH release in one shot. If it
fails mid-way, consult the recovery table in `RELEASING.md`; don't
hand-fix.

## What's planned

- **Inline tool-call rendering in the web demo.** Today the OPFS panel
  shows the *result* of fs builtin calls (files appear after a turn),
  but the chat transcript doesn't surface "the model called
  `create_file(notes.md)`" mid-stream. Add collapsible tool-call blocks
  to the transcript using the existing `chat()` callback shape (will
  need a second callback channel or a typed event stream).
- **Provider-agnostic Filesystem usage.** The trait sits below
  `Connection` so any future backend (OpenAI, Anthropic, local model)
  can reuse the same `OpfsFilesystem` + `NativeFilesystem` without
  duplication. Today only `GeminiBackendConfig::with_filesystem` exists;
  the seam is ready when a second backend lands.
- **Web IDE expansion.** Beyond the file browser: inline editing of
  OPFS files, persistent agent state across reloads (history in OPFS),
  a "scratch" area pre-populated with starter files. Treat the demo as
  a tab-resident IDE the agent shares with the user.

## Filesystem trait (M3)

The 6 fs-shaped builtins (`list_directory`, `view_file`, `find_file`,
`search_directory`, `create_file`, `edit_file`) call into
`crate::filesystem::Filesystem` instead of `tokio::fs` directly. The
trait surface is small:

- `read`, `write_atomic`, `metadata`, `read_dir`, `walk`

Two implementations ship:

- **`NativeFilesystem`** (gated on `feature = "native"`): `tokio::fs` +
  `walkdir` + `tempfile`; atomicity via tempfile + rename.
- **`OpfsFilesystem`** (wasm32 only): Origin Private File System via
  `web-sys`; atomicity via `FileSystemWritableFileStream.close()` swap.

`GeminiConnectionStrategy::connect` honors a caller-supplied
`Filesystem` via `with_filesystem`, otherwise auto-installs
`NativeFilesystem` on native (or `None` on wasm, where the caller is
expected to supply OPFS — `localharness-web` does so). Plug-in impls
(mocks for tests, custom backends) implement the trait and hand a
`SharedFilesystem = Arc<dyn Filesystem>` via the builder.
