<div align="center">

# `localharness`

**A Rust-native agent SDK for Gemini.** Build production agents with
streaming text, custom tools, safety policies, and background triggers
— all from a single `cargo add`. Zero external binaries.

[![crates.io](https://img.shields.io/crates/v/localharness.svg?style=flat-square)](https://crates.io/crates/localharness)
[![docs.rs](https://img.shields.io/docsrs/localharness?style=flat-square)](https://docs.rs/localharness)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg?style=flat-square)](LICENSE)
[![CI](https://img.shields.io/badge/MSRV-1.85-orange.svg?style=flat-square)](Cargo.toml)

</div>

```rust
use localharness::{Agent, GeminiAgentConfig};

#[tokio::main]
async fn main() -> localharness::Result<()> {
    let agent = Agent::start_gemini(
        GeminiAgentConfig::new(std::env::var("GEMINI_API_KEY").unwrap())
            .with_system_instructions("You are a concise code reviewer."),
    ).await?;

    let response = agent.chat("Review: fn add(a: i32, b: i32) -> i32 { a - b }").await?;
    println!("{}", response.text().await?);

    agent.shutdown().await?;
    Ok(())
}
```

> **Status:** 0.10.x · stable Rust-native runtime · 11/11 built-in
> tools · MCP bridge · context-window compaction · **wasm32 + browser
> IDE** · **on-chain identity** (EIP-2535 Diamond registry on Tempo
> Moderato; every claimed name is an ERC-721 NFT with an ERC-6551
> token-bound wallet).

**Try it in your browser:** [`localharness.xyz`](https://localharness.xyz/)
— pick a name, claim a subdomain. The same `Agent` loop you'd embed
in a CLI host runs in a browser tab at `<name>.localharness.xyz`,
backed by a wallet you control and an on-chain registry that proves
it's yours across devices.

---

## Contents

- [Install](#install)
- [Concepts](#concepts) — `Agent`, `Conversation`, `Connection`
- [Examples](#examples) — streaming, tools, hooks, policies, triggers, multimodal
- [Built-in tools](#built-in-tools)
- [Architecture](#architecture)
- [Run in the browser](#run-in-the-browser)
- [Design notes](#design-notes-performance--safety)
- [FAQ](#faq)
- [License](#license)

---

## Install

```toml
[dependencies]
localharness = "0.10"
tokio        = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Cargo features (off by default):

- `wallet` — `localharness::{wallet, registry}` modules: secp256k1
  keypair + BIP-39 mnemonic + RLP encoding + a JSON-RPC client for
  the on-chain `LocalharnessRegistry` Diamond. Useful for CLI tools
  and back-end indexers that want to query the registry without
  spinning up the browser app.
- `browser-app` — the in-tab HTMX-style IDE (cdylib for wasm-pack).
  Transitively enables `wallet`.

```sh
export GEMINI_API_KEY="your_api_key_here"
```

No Python install, no Go binary, no harness process — `cargo build` and
you have an agent. Get an API key from [Google AI Studio][aistudio].

[aistudio]: https://aistudio.google.com/app/apikey

---

## Concepts

| Layer | Type | Use when |
|------:|------|----------|
| **1** | [`Agent`] | One-shot or short-running scripts. Batteries included. |
| **2** | [`Conversation`] / [`ChatResponse`] | Long-lived sessions, history introspection, custom turn shapes. |
| **3** | [`Connection`] | Embed the SDK in your own runtime, swap the transport. |

[`Agent`]: https://docs.rs/localharness/latest/localharness/struct.Agent.html
[`Conversation`]: https://docs.rs/localharness/latest/localharness/struct.Conversation.html
[`ChatResponse`]: https://docs.rs/localharness/latest/localharness/struct.ChatResponse.html
[`Connection`]: https://docs.rs/localharness/latest/localharness/trait.Connection.html

---

## Examples

<details><summary><b>Stream text tokens as they arrive</b></summary>

```rust
use futures_util::StreamExt;

let response = agent.chat("Write a haiku about Rust.").await?;
let mut tokens = response.text_stream();
while let Some(chunk) = tokens.next().await {
    print!("{}", chunk?);
}
```
</details>

<details><summary><b>Stream thoughts and tool calls separately</b></summary>

Every cursor (`text_stream`, `thoughts`, `tool_calls`) replays from chunk
zero and advances independently — safe to consume concurrently from
multiple tasks.

```rust
use futures_util::StreamExt;

let response = agent.chat("What time is it in Tokyo?").await?;

let thoughts = async {
    let mut t = response.thoughts();
    while let Some(text) = t.next().await { eprint!("{}", text?); }
    Ok::<_, localharness::Error>(())
};
let calls = async {
    let mut c = response.tool_calls();
    while let Some(call) = c.next().await { println!("→ {}", call?.name); }
    Ok::<_, localharness::Error>(())
};

let (a, b) = tokio::join!(thoughts, calls);
a?; b?;
```
</details>

<details><summary><b>Register a custom tool</b></summary>

```rust
use localharness::{allow_all, Agent, ClosureTool, GeminiAgentConfig};
use serde_json::json;

let weather = ClosureTool::new(
    "get_weather",
    "Return the weather for a city.",
    json!({ "type": "object", "properties": { "city": { "type": "string" } } }),
    |args, _ctx| async move {
        let city = args["city"].as_str().unwrap_or("?");
        Ok(json!({ "weather": format!("sunny in {city}") }))
    },
);

let agent = Agent::start_gemini(
    GeminiAgentConfig::new(api_key)
        .with_tool(weather)
        .with_policies(vec![allow_all()]),
).await?;
```
</details>

<details><summary><b>Use the built-in file tools with a workspace sandbox</b></summary>

```rust
use localharness::{Agent, CapabilitiesConfig, GeminiAgentConfig};

let agent = Agent::start_gemini(
    GeminiAgentConfig::new(api_key)
        .with_capabilities(CapabilitiesConfig::unrestricted())
        .with_workspace("/home/me/project"),
).await?;

let response = agent.chat("List the Rust files under src/ and show the first 50 lines of lib.rs.").await?;
println!("{}", response.text().await?);
```

`workspace_only(...)` policies are auto-installed when `with_workspace`
is set; every file tool's path is canonicalized and rejected if it
escapes the workspace.
</details>

<details><summary><b>Policies — deny-by-default, ask before dangerous calls</b></summary>

```rust
use localharness::{deny_all, Policy};
use std::sync::Arc;

let policies = vec![
    deny_all(),                                // start from nothing
    Policy::allow("view_file"),                // safe reads ok
    Policy::ask("run_command", Arc::new(|call| {
        // Pop your own UI. Return true to approve.
        eprintln!("approve `{}`? {:?}", call.name, call.args);
        true
    })),
];
```

Precedence: `specific deny ≻ specific ask ≻ specific allow ≻ wildcard
deny ≻ wildcard ask ≻ wildcard allow`. Matches the Python SDK rule.
</details>

<details><summary><b>Structured output</b></summary>

```rust
let schema = serde_json::json!({
    "type": "object",
    "properties": {
        "summary":  { "type": "string" },
        "severity": { "type": "string", "enum": ["low", "medium", "high"] }
    },
    "required": ["summary", "severity"]
});

let agent = Agent::start_gemini(
    GeminiAgentConfig::new(api_key)
        .with_response_schema(schema.to_string()),
).await?;

let response = agent.chat("Triage this bug report: ...").await?;
let _ = response.text().await?; // drain
let out = agent.conversation().last_structured_output().unwrap();
println!("{out}");
```

The model calls the built-in `finish(output)` tool when it's done; the
agent extracts `output` into `last_structured_output()`.
</details>

<details><summary><b>Background triggers</b></summary>

```rust
use std::time::Duration;
use localharness::every;

let watchdog = every(Duration::from_secs(60), "deploy_watch", |ctx| async move {
    ctx.send_when_idle("Check the deployment status.").await
});

let agent = Agent::start_gemini(
    GeminiAgentConfig::new(api_key).with_trigger(watchdog),
).await?;
```
</details>

<details><summary><b>Multimodal input (images, PDFs, audio, video)</b></summary>

```rust
use localharness::{Content, Media, Part};

let chart = Media::from_path("./diagram.png")?
    .with_description("system architecture diagram");

let spec = Media::from_path("./spec.pdf")?;

let prompt: Content = vec![
    Part::from("List three vulnerabilities, citing the diagram and spec."),
    Part::from(chart),
    Part::from(spec),
].into();

let response = agent.chat(prompt).await?;
```

Media is stored as `Bytes` — cloning into multiple stream frames is
refcounted, so a 30 MB PDF is never copied.
</details>

<details><summary><b>Bridge an MCP server's tools</b></summary>

Connect to an external [Model Context Protocol][mcp] server and expose
its tools to the agent. The bridge spawns the server, fetches its tool
catalog, and registers each as a regular `Tool` — the model can't tell
the difference between an in-process tool and an MCP-served one.

```rust
use localharness::{Agent, GeminiAgentConfig};
use localharness::types::McpServerConfig;

let agent = Agent::start_gemini(
    GeminiAgentConfig::new(api_key)
        .with_mcp_server(McpServerConfig::Stdio {
            command: "uvx".into(),
            args: vec!["mcp-server-fetch".into()],
        }),
).await?;
```

Today: stdio transport only; SSE/HTTP variants return an error.
Tools surface only — prompts, resources, sampling, and subscriptions
are out of scope.

[mcp]: https://modelcontextprotocol.io
</details>

<details><summary><b>Compact long conversations automatically</b></summary>

When prompt tokens for a turn exceed
`CapabilitiesConfig::compaction_threshold`, the agent summarizes the
oldest history entries via a separate Gemini call and replaces them
with one synthetic user-role turn tagged `[compacted prior context]`.
The most-recent 6 user/model pairs are kept verbatim; function-call /
response pairs are kept together.

```rust
use localharness::{Agent, CapabilitiesConfig, GeminiAgentConfig};

let mut caps = CapabilitiesConfig::unrestricted();
caps.compaction_threshold = Some(60_000);

let agent = Agent::start_gemini(
    GeminiAgentConfig::new(api_key).with_capabilities(caps),
).await?;
```

Disabled by default (set to `None`). Typical values: 60-80% of your
model's max context window. Summarization failures fall back to a
drop-oldest strategy with the same tag.
</details>

<details><summary><b>Resume a conversation</b></summary>

```rust
let agent = Agent::start_gemini(
    GeminiAgentConfig::new(api_key).resume("conv-abc123"),
).await?;
```
</details>

---

## Built-in tools

The Gemini backend ships **13 tools** enabled by `BuiltinTool`,
auto-registered into the `ToolRunner` per `CapabilitiesConfig`. The
default `CapabilitiesConfig` exposes the read-only safety subset; call
`CapabilitiesConfig::unrestricted()` to enable everything.

| Tool | Read/Write | Description |
|------|:----------:|-------------|
| `list_directory` | R | Sorted children with `name`, `kind`, `size`. |
| `view_file` | R | UTF-8 lossy read with optional 1-indexed line range; 256 KiB cap. |
| `find_file` | R | Glob-matched recursive name search; 1000-match cap. |
| `search_directory` | R | Regex content search with optional file glob; 500-match cap. |
| `finish` | term | Terminate turn + capture structured output. |
| `create_file` | W | Atomic write via tempfile + rename; refuses to overwrite. |
| `edit_file` | W | Exact-once substring replace (or `replace_all`); atomic write. |
| `delete_file` | W | Remove a file or directory (recursive). Irreversible. |
| `rename_file` | W | Rename/move from → to. Atomic on Native when same filesystem; OPFS does read + write + delete. |
| `run_command` | W | Shell exec with timeout (default 30s / max 600s), 256 KiB output cap. |
| `generate_image` | W | Call the image model; returns base64 + MIME. |
| `ask_question` | I/O | Default no-op (returns `skipped: true`); register a custom `ask_question` tool for interactive UI. |
| `start_subagent` | spawn | One-shot text-only subagent with isolated context. Returns `{ final_response, finish_reason }`. |

Custom tools registered with the same name as a built-in **win** —
overrides are intentional.

---

## Architecture

```text
   ┌──────────────────────────────────────────────────────┐
   │  L1   Agent           start · chat · shutdown        │
   ├──────────────────────────────────────────────────────┤
   │  L2   Conversation    history · usage · streams      │
   │       ChatResponse    text · thoughts · tool_calls   │
   ├──────────────────────────────────────────────────────┤
   │  L3   Connection      transport abstraction          │
   │       GeminiConnection  reqwest + SSE + tool loop    │
   └──────────────────────────────────────────────────────┘
                              │
                              │  HTTPS (rustls)
                              ▼
                          Gemini API
```

Inside the Gemini agent loop:

```text
   user prompt ───►│                                         ▲
                   │ build GenerateContentRequest            │  emit Step
                   │ ───────► Gemini SSE ──────────► chunks  │  (text,
                   │             │                           │   thought,
                   │             ▼                           │   tool_call)
                   │   functionCall parts?  ────► dispatch ──┘
                   │             │              hooks→policy→tool_runner
                   │             ▼
                   │   append functionResponse ──► loop ─────┐
                   │                                         │
                   │   no more calls / finish ──► terminal Step
```

A single broadcast channel fans `Step`s out to every cursor
(`ChatResponse::chunks`, `text_stream`, `thoughts`, `tool_calls`). The
tool dispatch loop is inline inside the turn — no out-of-band
round-trip through a sidecar process.

---

## Run in the browser

`localharness` compiles to `wasm32-unknown-unknown`. The same `Agent`
loop that drives a CLI runs inside a browser tab — no server, no
backend, key stays in the page.

**Live demo:** [`localharness.xyz`](https://localharness.xyz/) — pick
a name, claim a subdomain, chat with the agent in `<name>.localharness.xyz`.

```toml
[target.'cfg(target_arch = "wasm32")'.dependencies]
localharness = { version = "0.10", default-features = false }
```

Run the demo locally:

```sh
git clone https://github.com/compusophy/localharness
cd localharness
./scripts/build-web.sh        # wasm-pack build → web/pkg/
python -m http.server 8765 -d web
```

**What works on wasm:** the full `Agent → Conversation → Connection
→ ToolRunner` chain, plus 12 of 13 built-in tools — the 4 portable
ones (`ask_question`, `finish`, `generate_image`, `start_subagent`)
and the 8 filesystem ones backed by OPFS (Origin Private File System;
per-origin sandbox, atomic writes): `list_directory`, `view_file`,
`find_file`, `search_directory`, `create_file`, `edit_file`,
`delete_file`, `rename_file`.

**What doesn't:** `run_command` and the MCP stdio bridge stay
native-only — the browser has no subprocess primitives.

```rust
use std::sync::Arc;
use localharness::filesystem::OpfsFilesystem;
use localharness::{CapabilitiesConfig, GeminiAgentConfig};

let cfg = GeminiAgentConfig::new(api_key)
    .with_capabilities(CapabilitiesConfig::unrestricted())
    .with_filesystem(Arc::new(OpfsFilesystem::new()));
```

To target a different backend (mock filesystem for tests, a custom
in-memory store), implement [`Filesystem`] and pass it to
`with_filesystem` the same way.

[`Filesystem`]: https://docs.rs/localharness/latest/localharness/filesystem/trait.Filesystem.html

### Platform: subdomains, wallets, registry

The reference browser app at `localharness.xyz` extends the SDK with
a self-sovereign identity layer:

- **Wildcard subdomain per user** (`<name>.localharness.xyz`). Each
  one is a separate origin → separate OPFS → separate working state,
  for free.
- **Master wallet** created on explicit user action (Create identity
  or Import seed) and persisted in the apex's OPFS — secp256k1 +
  BIP-39 via the `wallet` feature. The apex page hard-gates the claim
  form until an identity exists so visitors never end up with a
  silently-generated wallet they didn't ask for. Importable on any
  device via the 12-word seed phrase.
- **On-chain registry** — `LocalharnessRegistry` as an EIP-2535
  Diamond on Tempo Moderato testnet
  ([`0x6f2858…2930`](https://moderato.tempo.xyz/address/0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930)).
  Names are claimed by signing a registration transaction with the
  master wallet. Every name is an ERC-721 NFT; every NFT has an
  ERC-6551 token-bound account (the agent's wallet).
- **Gasless onboarding.** First-claim, `$LH` transfers, and on-chain
  feedback all run as **sponsored Tempo Transactions** (tx type
  `0x76` — Tempo's native account-abstraction format). A bundle-side
  sponsor wallet signs as `fee_payer` and pays fees in AlphaUSD on
  behalf of every user, so a fresh visitor can claim a subdomain
  without first acquiring native gas or any TIP-20 stablecoin.
- **Cross-origin owner verification.** Tenant subdomains embed
  `apex/?signer=1` in a hidden iframe, send a postMessage sign
  challenge, recover the address from the signature, compare it to
  the on-chain owner. Visitors who don't hold the NFT see read-only
  mode automatically.

The on-chain stack lives behind the `wallet` feature and is exposed
via `pub mod registry` for off-bundle consumers (CLI tools, indexers,
back-ends). Contract source + Foundry deploy scripts live in
[`contracts/`](contracts/); architecture write-up in
[`contracts/README.md`](contracts/README.md). The next layers on the
frontier (MPP/x402 payment hooks, ERC-8004 reputation, a second
non-Gemini backend) are tracked in `CLAUDE.md`.

---

## Design notes (performance & safety)

- **Lock-free idle polling.** `Connection::is_idle()` reads an
  `AtomicBool`. Trigger handlers can hot-loop without contention.
- **Broadcast fan-out for steps.** Cursors subscribe without blocking
  the producer; replay buffer is bounded; slow consumers fail fast.
- **Bounded backpressure everywhere.** Step broadcast cap 256.
  Function-call dispatch capped at 16 rounds per turn (`MAX_TOOL_ROUNDS`).
- **Atomic file writes.** `create_file` and `edit_file` write through
  a `tempfile::NamedTempFile` in the same directory and rename into
  place — a crash mid-write never leaves a partially written file.
- **Bounded subprocess output.** `run_command` caps each stream at
  256 KiB and kills the child on timeout with `kill_on_drop`.
- **Component-wise path containment.** `workspace_only()` defeats
  prefix tricks (`/foo/bar-evil` vs `/foo/bar`).
- **Lock-free tool-context swap.** `arc_swap::ArcSwapOption` replaces
  the runtime context atomically across concurrent tool calls.
- **Typed errors.** Flat `thiserror` enum; `io::Error`,
  `serde_json::Error`, `reqwest::Error` fold via `#[from]`.
- **API key redaction.** `Debug` for `GeminiClient` prints
  `<redacted>` for the key.
- **Zero-copy media.** `Media::data` is `bytes::Bytes`. Cloning a part
  into multiple frames is a refcount bump.

---

## FAQ

**Does this need a server?** No. The crate uses `reqwest` to call the
Gemini REST API directly. No localhost daemon, no Go binary, no Python.

**How do I get a `GEMINI_API_KEY`?** From [Google AI Studio][aistudio].
Free tier is sufficient for development.

**Which model does it use?** Default `gemini-3.5-flash` for chat,
`gemini-3.1-flash-image-preview` for `generate_image`. Override with
`GeminiBackendConfig::with_model(...)`.

**Why does write-tool access require a policy?** Enabling tools that
write to disk or run commands without a policy is almost always a bug.
Add `with_policies(vec![allow_all()])` to opt in, or
`with_workspace(...)` to scope.

**MSRV?** Rust 1.85 (edition 2024).

**Async runtime?** Tokio.

**How do I get `tracing` logs?**

```rust
tracing_subscriber::fmt().with_env_filter("localharness=debug").init();
```

---

## License

[Apache-2.0](LICENSE).
