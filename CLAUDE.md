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
├── app/                   browser-resident IDE (M4) — gated on the
│   ├── mod.rs             `browser-app` feature + wasm32 target. See
│   ├── templates.rs       below for module-by-module notes.
│   ├── dom.rs
│   ├── events.rs
│   ├── chat.rs
│   └── opfs.rs
└── backends/
    ├── gemini/
    │   ├── api.rs         GeminiClient + SSE decoder (CRLF + LF tolerant)
    │   ├── wire.rs        REST request/response types
    │   ├── loop.rs        run_turn — the inner agent loop
    │   ├── compaction.rs  history summarisation
    │   ├── tools/         11 built-in tools
    │   └── mod.rs         GeminiConnectionStrategy + GeminiConnection
    └── mcp/               stdio MCP client (native-only)

web/                       static site for Vercel
├── index.html             bootstrap shell (CSS + #root + init())
└── pkg/                   wasm-pack output (gitignored; built locally
                           and uploaded by `vercel deploy`):
                           localharness.js + localharness_bg.wasm

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
  io-util, plus the `walkdir` and `tempfile` deps. Required for
  `run_command` and the MCP stdio bridge, and is what lets the 6 fs
  builtins register a `NativeFilesystem` by default.
- `browser-app` (off by default): compiles the `src/app/` module into
  the crate as a wasm cdylib — the browser IDE. Pulls in `maud` for
  HTML templating and `console_error_panic_hook`. Has no effect on a
  native build. Built by `scripts/build-web.{sh,ps1}` via
  `wasm-pack build --no-default-features --features browser-app`.
- (wasm targets) automatically drop `walkdir`/`tempfile` and add
  `wasm-bindgen-futures`, `uuid/js`, `getrandom/js` via target-cfg.

Library callers on wasm32 who only want the SDK (not the browser app)
depend with `default-features = false` and skip `browser-app`.

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
  must handle that variant explicitly.
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

## The browser app (M4)

Compiled into the crate as `src/app/`, gated on `feature = "browser-app"`
plus `target_arch = "wasm32"`. The previous `localharness-web` JS-binding
crate and the ~700 lines of inline JS in `web/index.html` are gone; the
browser UI is now pure Rust.

Design rule: **no imperative DOM manipulation**. All HTML comes from
`maud` templates; the only DOM operations are `set_inner_html` /
`set_outer_html` / `insert_adjacent_html` targeted at fixed element
ids (HTMX-style fragment swaps). One delegated `click` listener and one
`keydown` listener at the document level handle every interaction by
reading `data-action` and `data-arg` attributes off the event target's
ancestor chain. There are zero `Closure::wrap` calls outside of those
two listeners.

Layout inside `src/app/`:

- `mod.rs` — `#[wasm_bindgen(start)]` entry, `App` state in a
  `thread_local<RefCell<App>>`, `shared_opfs()` for the one-per-tab
  `OpfsFilesystem`.
- `templates.rs` — all maud functions. Chrome, turn, text segment,
  tool-call block + result, OPFS breadcrumb/list/viewer.
- `dom.rs` — `swap_inner`, `swap_outer`, `append_html`, `by_id`,
  `set_status`. Pure web-sys, no node construction.
- `events.rs` — `Action` enum + parser + `install_delegated_listeners`.
- `chat.rs` — `run_send()`: lazy session start, then stream `StreamChunk`s
  into the assistant turn via fixed ids. Tool calls render with a
  monotonic `seg_id` and a `VecDeque` correlates `ToolResult`s back to
  their `ToolCall` block.
- `opfs.rs` — read-only file browser (read_dir, open file in preview).
  Wipe is deferred pending `Filesystem::delete`.

Build: `wasm-pack build . --target web --out-dir web/pkg --release
--no-default-features --features browser-app`. wasm-opt is disabled in
`[package.metadata.wasm-pack.profile.release]` because the wasm-pack-
bundled wasm-opt rejects post-MVP features that modern rustc emits.

## What's planned

- **Markdown rendering for assistant text segments.** Today the
  `text_segment` template emits raw text; the previous demo ran
  `marked.js` on the final string. Replacement: add `pulldown-cmark`
  behind the `browser-app` feature, render at end-of-turn, swap into
  the segment via `dom::swap_inner`.
- **`Filesystem::delete` + OPFS wipe button.** The panel's wipe action
  shows "not yet" because the trait has no remove method. Adding it
  unblocks the wipe button and also lets the `delete_file` builtin
  exist (currently absent).
- **OPFS file edit in the panel.** Today files are read-only previews;
  inline editing fits naturally as another `data-action` (e.g.
  `opfs-edit`, `opfs-save`) and a `write_atomic` call.
- **Persistent conversation history.** Refreshing the tab wipes the
  in-memory Agent. Serialise the Gemini `history` into OPFS (or
  IndexedDB) per session and reload on mount.
- **Provider-agnostic Filesystem usage.** The trait sits below
  `Connection` so any future backend (OpenAI, Anthropic, local model)
  can reuse `OpfsFilesystem` + `NativeFilesystem`. Today only
  `GeminiBackendConfig::with_filesystem` exists; the seam is ready
  when a second backend lands.
- **Backend selector in the app.** A `data-action="set-backend"`
  control once a second `ConnectionStrategy` exists.

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
