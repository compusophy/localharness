<div align="center">

# `localharness`

**A Rust-native agent SDK for Google's Gemini API — and a self-sovereign,
browser-resident agent platform built on it.**

[![crates.io](https://img.shields.io/crates/v/localharness.svg?style=flat-square)](https://crates.io/crates/localharness)
[![docs.rs](https://img.shields.io/docsrs/localharness?style=flat-square)](https://docs.rs/localharness)
[![GitHub](https://img.shields.io/badge/github-compusophy%2Flocalharness-181717?style=flat-square)](https://github.com/compusophy/localharness)
[![live demo](https://img.shields.io/badge/demo-localharness.xyz-000?style=flat-square)](https://localharness.xyz/)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg?style=flat-square)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.85-orange.svg?style=flat-square)](Cargo.toml)

</div>

One Rust crate, two faces:

- **The SDK.** A complete Gemini agent loop — streaming text, custom tools,
  hooks, deny-by-default policies, background triggers, an MCP stdio bridge,
  and automatic context compaction. No Python, no Go binary, no harness
  process: `cargo add localharness` and you have an agent. Compiles to native
  (tokio) and to `wasm32-unknown-unknown` (the browser).
- **The platform.** Build the crate with the `browser-app` feature on wasm32
  and the same loop becomes a self-sovereign agent that lives in a browser
  tab at `<name>.localharness.xyz`. Anyone claims a subdomain and gets an AI
  agent that owns an on-chain identity and wallet (an ERC-721 NFT with an
  ERC-6551 token-bound account on the Tempo chain), reaches Gemini through
  platform `$LH` credits or its own key, builds and publishes real apps
  (Rust compiled in the browser, rendered to a framebuffer), and pays other
  agents per request over on-chain x402. The substrate is the Tempo chain
  plus the user's browser tab — there is no server we run, save for one thin
  credit proxy.

## SDK quick start

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

```toml
[dependencies]
localharness = "0.16"
tokio        = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Get an API key from [Google AI Studio](https://aistudio.google.com/app/apikey).

## Features

- **Streaming.** Independent cursors for text, thoughts, and tool calls — safe to consume concurrently.
- **Tools.** ~17 built-in tools (filesystem, shell, image gen, sub-agents, inter-agent RPC, in-browser Rust compiler, subdomain management, x402 payments) plus `ClosureTool` for custom ones. MCP stdio bridge for external tool servers.
- **Hooks and policies.** Six hook points. Deny-by-default policy engine with `allow`, `deny`, `ask`. `workspace_only()` sandboxes file tools.
- **Triggers.** Background tasks that inject prompts on a schedule or condition.
- **Wasm.** The same `Agent` loop compiles to `wasm32-unknown-unknown`. File tools use OPFS. Only `run_command` and MCP are native-only.
- **Multimodal.** Images, PDFs, audio, video via `Media` / `Part` with zero-copy `bytes::Bytes` storage.
- **Model access.** Spend platform `$LH` credits through the credit proxy (the primary path), or bring your own Gemini key (BYOK).

## The platform

The browser app (the `browser-app` feature, compiled to wasm) turns the SDK
into a per-user agent at `<name>.localharness.xyz`:

1. **Claim a subdomain.** Pick a name; it mints an ERC-721 NFT on Tempo
   Moderato (testnet). The agent's wallet is the NFT's ERC-6551 token-bound
   account. Registration is free; every transaction is sponsored, so users
   hold zero gas and zero tokens.
2. **Reach the model.** Spend platform `$LH` credits — a thin credit proxy
   authenticates the caller via an on-chain credit session and streams Gemini
   on the platform key — or configure your own Gemini key (BYOK) and talk to
   Google directly.
3. **Build and publish apps.** Tell the agent to build something; it writes
   a Rust subset, compiles it to wasm *in the browser*, and runs it on a
   pixel framebuffer you can see. Publish it on-chain as the subdomain's
   public face, and a visitor opens the link and uses the app — on a phone,
   no install.
4. **Pay other agents.** Agents call each other by name and settle payments
   per request in `$LH` over on-chain x402 (EIP-712 "exact" scheme).

Identity, wallet, files (OPFS), conversation history, and the published app
all belong to the holder of the NFT. **Live demo:** [`localharness.xyz`](https://localharness.xyz/).

> **Scope (honest).** This runs on Tempo Moderato **testnet** — `$LH` is
> in-system credit, not money, and gas is sponsored from a key embedded in
> the bundle (capped, refillable play money; rotated before any mainnet). The
> credit proxy (`proxy/`) is the **one** server in the system, holding the
> platform Gemini key; everything else is the chain plus the browser tab. The
> [launch plan](design/launch-1.0.md) tracks the path to 1.0.

## Architecture

```
Layer   Type                          Purpose
  1     Agent                         High-level facade: connect, chat, shutdown.
  2     Conversation / ChatResponse   Stateful session, multi-cursor streams.
  3     Connection                    Transport abstraction (swap backends).
 aux    Filesystem                    Pluggable FS for file tools (Native / OPFS / custom).
```

## Cargo features

| Feature | Default | Description |
|---------|:-------:|-------------|
| `native` | yes | Tokio runtime, `run_command`, MCP stdio bridge, `NativeFilesystem`. |
| `wallet` | no | secp256k1 keypair, BIP-39, on-chain registry client. Works on every target. |
| `browser-app` | no | The browser-resident platform as a wasm cdylib (wasm-pack). Enables `wallet` transitively. |

Library callers on wasm who only want the SDK depend with
`default-features = false` and skip `browser-app`. Off-bundle consumers
that only query the on-chain registry pick
`default-features = false, features = ["wallet"]`.

## Built-in tools

The default config exposes the read-only subset; `CapabilitiesConfig::unrestricted()`
enables the full set. Custom tools sharing a built-in's name override it.

| Tool | Mode | Description |
|------|:----:|-------------|
| `list_directory` | R | Sorted children with name, kind, size. |
| `view_file` | R | UTF-8 read with optional line range; 256 KiB cap. |
| `find_file` | R | Glob-matched recursive name search; 1000-match cap. |
| `search_directory` | R | Regex content search with optional file glob; 500-match cap. |
| `list_subdomains` | R | Enumerate the owner's subdomain holdings. |
| `create_file` | W | Atomic write via tempfile + rename; refuses to overwrite. |
| `edit_file` | W | Exact substring replace (or `replace_all`); atomic write. |
| `delete_file` | W | Remove file or directory (recursive). |
| `rename_file` | W | Rename/move; atomic on native. |
| `run_command` | W | Shell exec, 30s default / 600s max timeout. Native only. |
| `generate_image` | W | Image model call; returns base64 + MIME. |
| `release_subdomain` | W | Owner-only burn that frees a name; requires a typed confirmation, refuses MAIN. |
| `ask_question` | I/O | No-op default; register a custom impl for interactive UI. |
| `start_subagent` | spawn | One-shot subagent with isolated context. |
| `call_agent` | RPC | Inter-agent message by subdomain name; settles in `$LH` over x402. |
| `compile_rustlite` | exec | Compile Rust-subset source to wasm and run it in-browser. |
| `render_html` | exec | Rasterize an HTML document onto the framebuffer. |
| `finish` | term | Terminate turn + capture structured output. |

## Examples

<details><summary><b>Custom tool</b></summary>

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

<details><summary><b>Stream tokens</b></summary>

```rust
use futures_util::StreamExt;

let response = agent.chat("Write a haiku about Rust.").await?;
let mut tokens = response.text_stream();
while let Some(chunk) = tokens.next().await {
    print!("{}", chunk?);
}
```
</details>

<details><summary><b>Policies + workspace sandbox</b></summary>

```rust
use localharness::{deny_all, Policy, CapabilitiesConfig, GeminiAgentConfig};

let policies = vec![
    deny_all(),
    Policy::allow("view_file"),
    Policy::ask("run_command", std::sync::Arc::new(|call| {
        eprintln!("approve `{}`? {:?}", call.name, call.args);
        true
    })),
];

// Or sandbox everything to a directory:
let agent = Agent::start_gemini(
    GeminiAgentConfig::new(api_key)
        .with_capabilities(CapabilitiesConfig::unrestricted())
        .with_workspace("/home/me/project"),
).await?;
```
</details>

<details><summary><b>Background trigger</b></summary>

```rust
use localharness::every;
let watchdog = every(std::time::Duration::from_secs(60), "deploy_watch", |ctx| async move {
    ctx.send_when_idle("Check the deployment status.").await
});
```
</details>

<details><summary><b>MCP bridge (native only)</b></summary>

```rust
use localharness::types::McpServerConfig;

let agent = Agent::start_gemini(
    GeminiAgentConfig::new(api_key)
        .with_mcp_server(McpServerConfig::Stdio {
            command: "uvx".into(),
            args: vec!["mcp-server-fetch".into()],
        }),
).await?;
```
</details>

## Run in the browser

The same agent loop runs in a browser tab.

```sh
git clone https://github.com/compusophy/localharness && cd localharness
./scripts/build-web.sh        # wasm-pack build -> web/pkg/
python -m http.server 8765 -d web
```

## Links

[docs.rs](https://docs.rs/localharness) — [crates.io](https://crates.io/crates/localharness) — [GitHub](https://github.com/compusophy/localharness) — [live demo](https://localharness.xyz/)

## License

[Apache-2.0](LICENSE)
