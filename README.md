<div align="center">

# `localharness`

**Rust client SDK for the `localharness` agent runtime** — build
Gemini-backed agents from Rust with the same wire protocol as Google's
[`google-antigravity`][upstream] Python SDK.

[![crates.io](https://img.shields.io/crates/v/localharness.svg?style=flat-square)](https://crates.io/crates/localharness)
[![docs.rs](https://img.shields.io/docsrs/localharness?style=flat-square)](https://docs.rs/localharness)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg?style=flat-square)](LICENSE)
[![CI](https://img.shields.io/badge/MSRV-1.85-orange.svg?style=flat-square)](Cargo.toml)
[![upstream](https://img.shields.io/badge/upstream-d6be9ca-purple.svg?style=flat-square)](UPSTREAM.md)

</div>

```rust
use localharness::{Agent, LocalAgentConfig};

#[tokio::main]
async fn main() -> localharness::Result<()> {
    let agent = Agent::start_local(
        LocalAgentConfig::new()
            .with_system_instructions("You are a concise code reviewer.")
            .with_api_key(std::env::var("GEMINI_API_KEY").unwrap()),
    ).await?;

    let response = agent.chat("Review: fn add(a: i32, b: i32) -> i32 { a - b }").await?;
    println!("{}", response.text().await?);

    agent.shutdown().await?;
    Ok(())
}
```

> **Status:** alpha · pinned to upstream commit [`d6be9ca`](UPSTREAM.md) ·
> unofficial · not affiliated with Google.

---

## Contents

- [Install](#install)
- [Concepts](#concepts) — `Agent`, `Conversation`, `Connection`
- [Examples](#examples) — streaming, tools, hooks, policies, triggers, multimodal
- [Architecture](#architecture)
- [Design notes](#design-notes-performance--safety)
- [Comparison with the Python SDK](#comparison-with-the-python-sdk)
- [What's not (yet) ported](#whats-not-yet-ported)
- [Upstream sync](#upstream-sync)
- [FAQ](#faq)
- [License](#license)

---

## Install

```toml
[dependencies]
localharness = "0.1"
tokio        = { version = "1", features = ["macros", "rt-multi-thread"] }
```

The crate calls a Go binary named `localharness` over stdio + WebSocket.
The Python SDK ships it; install once to grab the binary:

```sh
pip install google-antigravity
export ANTIGRAVITY_HARNESS_PATH="$(python -c 'import importlib.resources, google.antigravity; print(importlib.resources.files(google.antigravity) / "bin" / "localharness")')"
export GEMINI_API_KEY="your_api_key_here"
```

If `localharness` is already on your `PATH`, the env var is optional.

---

## Concepts

The SDK is layered so you can pick the surface that fits the task:

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
use localharness::{allow_all, ClosureTool, LocalAgentConfig};
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

let agent = Agent::start_local(
    LocalAgentConfig::new()
        .with_tool(weather)
        .with_policies(vec![allow_all()]),
).await?;
```
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

Precedence matches the Python SDK: `specific deny ≻ specific ask ≻
specific allow ≻ wildcard deny ≻ wildcard ask ≻ wildcard allow`.
</details>

<details><summary><b>Constrain file tools to a workspace</b></summary>

```rust
use localharness::workspace_only;

let policies = workspace_only(vec!["/home/me/project".into()]);
// view_file / create_file / edit_file outside the workspace are denied.
// Component-wise comparison; "/home/me/project-evil" is NOT a match.
```
</details>

<details><summary><b>Background triggers</b></summary>

```rust
use std::time::Duration;
use localharness::every;

let watchdog = every(Duration::from_secs(60), "deploy_watch", |ctx| async move {
    ctx.send_when_idle("Check the deployment status.").await
});

let agent = Agent::start_local(
    LocalAgentConfig::new().with_trigger(watchdog),
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

<details><summary><b>Resume a conversation</b></summary>

```rust
let agent = Agent::start_local(
    LocalAgentConfig::new().resume("conv-abc123"),
).await?;
```
</details>

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
   │       LocalConnection ws + stdio handshake           │
   └──────────────────────────────────────────────────────┘
                              │
                              │  localharness (Go binary)
                              ▼
                            Gemini
```

Inside `LocalConnection`:

```text
       ┌──────────┐  inbox: mpsc(InputEvent, cap 16)  ┌────────────────┐
       │  callers │ ─────────────────────────────────►│  ws_writer     │
       └──────────┘                                   └────────┬───────┘
                                                              │
                                                       ┌──────▼──────┐
                                                       │  websocket  │
                                                       └──────▲──────┘
                                                              │
       ┌────────────┐  broadcast(Step, cap 256)  ┌────────────┴──────┐
       │ subscribers│ ◄─────────────────────────│  ws_reader        │
       └────────────┘                            └───────────────────┘
```

A single `tokio::select!` supervisor owns the WebSocket and arbitrates
inbox writes against incoming frames. A separate task supervises the
child process (`kill_on_drop` on the handle, plus an explicit
`shutdown` flag).

---

## Design notes (performance & safety)

A short tour of the load-bearing choices:

- **Lock-free idle polling.** `Connection::is_idle()` reads an
  `AtomicBool` — no mutex, no syscalls, nanoseconds. Trigger handlers
  can call it inside hot loops.
- **Broadcast fan-out for steps.** Any number of cursors can subscribe
  without blocking the producer. Replays are bounded (256 in flight); a
  slow consumer fails fast with a "lagged" error rather than ballooning
  memory.
- **Bounded backpressure everywhere.** The writer inbox is 16, the step
  broadcast is 256. There's no unbounded `Vec<Message>` waiting for the
  socket to drain.
- **Lock-free tool-context swap.** `arc_swap::ArcSwapOption` replaces
  the runtime context atomically. Concurrent tool calls never serialize
  on a mutex just to fetch the context.
- **No mutex poisoning footguns.** `parking_lot::{Mutex,RwLock}` mean
  `lock()` doesn't return `Result`; one panicking thread doesn't taint
  every other reader.
- **Typed errors, no `unwrap` on hot paths.** [`Error`] is a flat
  `thiserror` enum. `io::Error`, `serde_json::Error`, and `prost`
  errors fold into it via `#[from]`; `?` works everywhere.
- **Zero-copy media.** `Media::data` is `bytes::Bytes`. Cloning a part
  into multiple frames is a refcount bump; a 30 MB PDF is never copied.
- **Bounded resource lifetimes.** `kill_on_drop` on the child, a 10 s
  handshake timeout, idempotent `shutdown()`, and a `Drop` impl that
  flips the shutdown flag so leaked agents don't leak processes.
- **Strict policy precedence.** Component-wise path containment defeats
  prefix tricks like `/foo/bar-evil` vs `/foo/bar`. Wildcard rules
  always lose to specific rules.

[`Error`]: https://docs.rs/localharness/latest/localharness/enum.Error.html

---

## Comparison with the Python SDK

| | Python (`google-antigravity`) | Rust (`localharness`) |
|---|---|---|
| Concurrency | asyncio | tokio |
| Errors | exceptions | `Result<T, Error>` (thiserror) |
| Multi-cursor `ChatResponse` | ✓ | ✓ |
| Idle check | mutex-guarded bool | `AtomicBool` (lock-free) |
| Step fan-out | single async iterator | `broadcast` (multi-cursor) |
| Multimodal media | bytes copies | `Bytes` (refcounted) |
| Hooks (6 kinds) | ✓ | ✓ |
| Policy precedence | ✓ | ✓ + component-wise path check |
| `every()` trigger | ✓ | ✓ |
| `workspace_only()` | ✓ | ✓ |
| Conversation resume | ✓ | ✓ |
| MCP server bridge | ✓ | config-types only (see below) |
| Interactive REPL helper | ✓ | not yet |
| Bundled harness binary | ✓ (in wheel) | reuses Python install |

---

## What's not (yet) ported

- **MCP server bridge.** `McpServerConfig` exists as a type, but the
  client that connects to stdio/SSE/HTTP MCP servers and exposes their
  tools to the agent isn't implemented. The Python implementation lives
  in [`google/antigravity/mcp/bridge.py`][mcp].
- **Interactive REPL.** Python has `utils.interactive.run_interactive_loop`.
  Easy to write on top of the existing `Agent::chat` + `text_stream`
  surface; not bundled.
- **Bundled harness binary.** The crate doesn't ship a `localharness`
  binary. You need a Python install (or your own build) to provide one.

Issues / PRs welcome.

[mcp]: https://github.com/google-antigravity/antigravity-sdk-python/blob/main/google/antigravity/mcp/bridge.py

---

## Upstream sync

This is a translation, not a fork. The pinned upstream commit lives in
[`UPSTREAM.md`](UPSTREAM.md). To check what's changed since:

```sh
./scripts/sync-upstream.sh        # bash, git-bash
./scripts/sync-upstream.ps1       # PowerShell
```

The script clones upstream into a temp dir, diffs against the pinned
commit, prints a porting punch list, and **does not modify your working
tree**. Promote the pin by updating `UPSTREAM.md` once the Rust source
catches up.

---

## FAQ

**Why "unofficial"?** Google publishes the Python SDK; the Rust port is
maintained by the community. Bug reports here, not upstream.

**Where do I get the `localharness` binary?** From `pip install
google-antigravity`. The crate doesn't redistribute it.

**Does this need `GEMINI_API_KEY`?** Yes — either set the env var or
pass it via `LocalAgentConfig::with_api_key()`.

**Why does write-tool access require a policy?** Same safety check as
the Python SDK: enabling tools that write to disk or run commands
without a policy is almost always a bug. Add `policies: vec![allow_all()]`
to opt in.

**MSRV?** Rust 1.85 (edition 2024).

**Async runtime?** Tokio. The `tokio-tungstenite`, `tokio::process`, and
`tokio::sync` primitives are baked in.

**How do I get `tracing` logs?** Add `tracing-subscriber` and
initialize a subscriber early:

```rust
tracing_subscriber::fmt().with_env_filter("localharness=debug").init();
```

---

## License

[Apache-2.0](LICENSE). Derived from Google's [`google-antigravity`][upstream]
Python SDK (same license); the original `LICENSE` file is preserved at
the root for attribution. The original Python README is preserved as
[`PYTHON_README.md`](PYTHON_README.md).

[upstream]: https://github.com/google-antigravity/antigravity-sdk-python
