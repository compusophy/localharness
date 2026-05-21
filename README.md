<div align="center">

# `localharness`

**A Rust-native agent SDK for Gemini.** Build production agents with
streaming text, custom tools, safety policies, and background triggers
— all from a single `cargo add`.

[![crates.io](https://img.shields.io/crates/v/localharness.svg?style=flat-square)](https://crates.io/crates/localharness)
[![docs.rs](https://img.shields.io/docsrs/localharness?style=flat-square)](https://docs.rs/localharness)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg?style=flat-square)](LICENSE)
[![CI](https://img.shields.io/badge/MSRV-1.85-orange.svg?style=flat-square)](Cargo.toml)

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

> **Status:** alpha · pre-1.0 · the 0.2.x line is mid-pivot — see
> [`DESIGN.md`](DESIGN.md) for the roadmap.

---

## Roadmap (0.2.x)

`localharness` started life (0.1.x) as a Rust client for Google's
[`google-antigravity`][upstream] Python SDK, talking to a bundled Go
runtime binary. **The 0.2.x line replaces that runtime with a Rust
agent loop that hits the Gemini API directly** — no Go binary, no
Python install, no external process. The public API (`Agent`,
`Conversation`, `Tool`, `Policy`, `Hook`, `Trigger`) is preserved.

| Phase | Version | What lands |
|:-----:|---------|------------|
| 1 | `0.2.0-alpha.1` | Gemini backend, text-only chat, streaming |
| 2 | `0.2.0-alpha.2` | Tool calling + read-only built-ins |
| 3 | `0.2.0-alpha.3` | Write tools + workspace sandbox |
| 4 | `0.2.0-beta.1`  | Thoughts, structured output, image gen, ask-question |
| 5 | `0.2.0` GA      | `LocalConnectionStrategy` deprecated; Gemini default |

See [`DESIGN.md`](DESIGN.md) for the full plan with module-by-module specs.

---

## Contents

- [Install](#install)
- [Concepts](#concepts) — `Agent`, `Conversation`, `Connection`
- [Examples](#examples) — streaming, tools, hooks, policies, triggers, multimodal
- [Architecture](#architecture)
- [Design notes](#design-notes-performance--safety)
- [FAQ](#faq)
- [License](#license)

---

## Install

```toml
[dependencies]
localharness = "0.1"
tokio        = { version = "1", features = ["macros", "rt-multi-thread"] }
```

### 0.1.x (current) — Go-backed

Today's release proxies to a Go runtime binary called `localharness`.
The Python SDK ships it; install once to grab the binary:

```sh
pip install google-antigravity
export ANTIGRAVITY_HARNESS_PATH="$(python -c 'import importlib.resources, google.antigravity; print(importlib.resources.files(google.antigravity) / "bin" / "localharness")')"
export GEMINI_API_KEY="your_api_key_here"
```

If `localharness` is already on your `PATH`, the env var is optional.

### 0.2.x (in progress) — no external runtime

`Agent::start_gemini(config)` will talk to the Gemini API directly.
A single `cargo add` is all you'll need. Track progress in
[`DESIGN.md`](DESIGN.md) and [`CHANGELOG.md`](CHANGELOG.md).

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

## FAQ

**What's the Go binary I keep hearing about?** Today's 0.1.x release
talks to a runtime binary that happens to be written in Go (Google
ships it inside the `google-antigravity` Python wheel). You never write
Go; you just point an env var at the binary once. **The 0.2.x line
removes the binary entirely** — see [Roadmap](#roadmap-02x).

**Does this need `GEMINI_API_KEY`?** Yes — either set the env var or
pass it via `LocalAgentConfig::with_api_key()`.

**Why does write-tool access require a policy?** Enabling tools that
write to disk or run commands without a policy is almost always a bug.
Add `policies: vec![allow_all()]` to opt in, or use `workspace_only(…)`
to scope.

**MSRV?** Rust 1.85 (edition 2024).

**Async runtime?** Tokio.

**How do I get `tracing` logs?**

```rust
tracing_subscriber::fmt().with_env_filter("localharness=debug").init();
```

**Origin of the project.** 0.1.x began life as a port of Google's
[`google-antigravity`][upstream] Python SDK. See
[`UPSTREAM.md`](UPSTREAM.md) for the historical record and
[`DESIGN.md`](DESIGN.md) for the Rust-native pivot plan.

---

## License

[Apache-2.0](LICENSE). The `LICENSE` file is inherited from upstream
for attribution.

[upstream]: https://github.com/google-antigravity/antigravity-sdk-python
