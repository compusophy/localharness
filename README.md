<div align="center">

# `localharness`

**A Rust-native agent SDK for Google's Gemini API.**

Streaming text, custom tools, safety policies, hooks, background triggers,
MCP bridge, context compaction -- one crate, zero external binaries.
Compiles to native (tokio) and wasm32 (browser).

[![crates.io](https://img.shields.io/crates/v/localharness.svg?style=flat-square)](https://crates.io/crates/localharness)
[![docs.rs](https://img.shields.io/docsrs/localharness?style=flat-square)](https://docs.rs/localharness)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg?style=flat-square)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.85-orange.svg?style=flat-square)](Cargo.toml)

</div>

## Quick start

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
localharness = "0.10"
tokio        = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Get an API key from [Google AI Studio](https://aistudio.google.com/app/apikey).
No Python, no Go binary, no harness process -- `cargo build` and you have an agent.

## Features

- **Streaming.** Independent cursors for text, thoughts, and tool calls -- safe to consume concurrently.
- **Tools.** 15 built-in tools (filesystem, shell, image gen, sub-agents, inter-agent RPC, in-browser Rust compiler) plus `ClosureTool` for custom tools. MCP stdio bridge for external tool servers.
- **Hooks and policies.** Six hook points. Deny-by-default policy engine with `allow`, `deny`, `ask`. `workspace_only()` sandboxes file tools.
- **Triggers.** Background tasks that inject prompts on a schedule or condition.
- **Wasm.** Same `Agent` loop compiles to `wasm32-unknown-unknown`. File tools use OPFS. Only `run_command` and MCP are native-only.
- **Multimodal.** Images, PDFs, audio, video via `Media` / `Part` with zero-copy `bytes::Bytes` storage.
- **Model access.** Use platform `$LH` credits metered through a live credit proxy (the primary path), or bring your own Gemini key.

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
| `browser-app` | no | In-browser IDE as a wasm cdylib (wasm-pack). Enables `wallet` transitively. |

## Built-in tools

Default config exposes the read-only subset; `CapabilitiesConfig::unrestricted()` enables all 15.

| Tool | Mode | Description |
|------|:----:|-------------|
| `list_directory` | R | Sorted children with name, kind, size. |
| `view_file` | R | UTF-8 read with optional line range; 256 KiB cap. |
| `find_file` | R | Glob-matched recursive name search; 1000-match cap. |
| `search_directory` | R | Regex content search with optional file glob; 500-match cap. |
| `create_file` | W | Atomic write via tempfile + rename; refuses to overwrite. |
| `edit_file` | W | Exact substring replace (or `replace_all`); atomic write. |
| `delete_file` | W | Remove file or directory (recursive). |
| `rename_file` | W | Rename/move; atomic on native. |
| `run_command` | W | Shell exec, 30s default / 600s max timeout. Native only. |
| `generate_image` | W | Image model call; returns base64 + MIME. |
| `ask_question` | I/O | No-op default; register a custom impl for interactive UI. |
| `start_subagent` | spawn | One-shot subagent with isolated context. |
| `call_agent` | RPC | Inter-agent message by subdomain name. |
| `compile_rustlite` | exec | Compile Rust-subset source to wasm and run it in-browser. |
| `finish` | term | Terminate turn + capture structured output. |

Custom tools with the same name as a built-in override it.

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
**Live demo:** [`localharness.xyz`](https://localharness.xyz/)

```sh
git clone https://github.com/compusophy/localharness && cd localharness
./scripts/build-web.sh        # wasm-pack build -> web/pkg/
python -m http.server 8765 -d web
```

## Links

[docs.rs](https://docs.rs/localharness) -- [crates.io](https://crates.io/crates/localharness) -- [GitHub](https://github.com/compusophy/localharness) -- [live demo](https://localharness.xyz/)

## License

[Apache-2.0](LICENSE)
