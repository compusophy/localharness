# src/builtins — backend-neutral builtin tools subsystem spec

> Module-owned context (auto-loaded when an agent works in `src/builtins/`). These
> are the backend-NEUTRAL tools the agent loop ships. Adding/editing one? The schema
> rule below is load-bearing — get it wrong and you brick ALL chat on Gemini.

## ⛔ Tool `input_schema` must be GEMINI-SAFE (a violation 400s → bricks ALL chat)
Gemini rejects union-type / JSON-Schema-meta fields. Every builtin's `input_schema`:
- SINGLE `type` per field — NEVER a union like `["string","null"]` (use one type;
  make optionality a non-required field, not a null union).
- NO `additionalProperties`, `$schema`, `$ref`, `oneOf`, `anyOf`, `allOf`.
- Nested objects/arrays + `minimum`/`maximum` ARE fine.
Two schema-lint GUARD TESTS enforce this (`builtin_tool_schemas_have_no_union_types`
+ the sibling guard). Run `cargo test` after touching any schema — a red guard here
means a brick, not a nitpick. **New tools declare args via `crate::tool_params!`
FIRST** (see `src/tool_params.rs` — one table generates struct + schema, and its
grammar CANNOT express the brick shapes; pattern: `delete_file.rs`/`run_command.rs`
in-file, chat tools hoist tables to `tool_params.rs`). The migration sweep is
COMPLETE (71/90); hand-write a schema ONLY for resident-class shapes — nested-object
arrays, runtime-derived enums, any-typed fields, literal `required:[]`, zero-arg —
and add it to the PERMANENT-RESIDENTS roster atop `src/tool_params.rs` with its
reason. If you migrate a resident anyway, keep a frozen byte-identity test.

## Which tools run where
- **8 fs builtins** (`list_directory view_file find_file search_directory create_file
  edit_file delete_file rename_file`): gate on a SUPPLIED `Filesystem`
  (`BuiltinDeps.fs`), NOT `feature=native` — so they run on wasm/OPFS too. Guard:
  `fs_builtins_gate_on_filesystem_not_native`. Don't re-gate them on `native`.
- **Client-free** (`ask_question finish start_subagent generate_image`): work on both
  native + wasm, no filesystem needed.
- **Native-only** (`feature=native`): `run_command` + the MCP stdio bridge.

## Tool semantics that bite
- **`finish`** is the ABSOLUTE END of the turn — the `run_send` loop stops on it
  (no auto-continue) and NO trailing sign-off is emitted. Don't make it return text
  the loop would render after the turn (the "calling finish…" then more text bug).
- **`ask_question`** blocks for the user — only when genuinely needing a decision.
- **`start_subagent`** spawns a scoped subagent; it wraps the model stream in a
  bounded RETRY (transport/5xx/timeout only — auth/credits/rate-limit fail fast).
- **`run_cartridge`/`render_html`** drive the display framebuffer; `compile_rustlite`
  STUBS host imports so a compile-only check needs no run.

## Errors
Arg rejections (args-JSON parse + face-value validation like an empty
`old_string` / bad glob) are `Error::bad_args(tool, msg)` — Display verbatim,
CORE_TOOL_FAILED structurally (never substring-classified; model-echoed arg
text must not read as a backend failure). State-dependent refusals (caps,
not-found, overwrite) stay `Error::other`; the protected-path refusal stays
`other` too (PolicyDenied's prefix would change model-visible text).

## Registration
The default tool SETS + `Step` constructors live in `src/types.rs` (no hand-written
wire literals). A builtin is registered there; `BuiltinDeps` carries its deps (fs,
etc.). The old `backends/gemini/tools` back-compat shim is GONE — import from
`crate::builtins` directly.
