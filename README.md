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

> **Status:** 0.2.x · stable Rust-native runtime · 10/11 built-in tools shipping.

---

## Contents

- [Install](#install)
- [Concepts](#concepts) — `Agent`, `Conversation`, `Connection`
- [Examples](#examples) — streaming, tools, hooks, policies, triggers, multimodal
- [Built-in tools](#built-in-tools)
- [Architecture](#architecture)
- [Design notes](#design-notes-performance--safety)
- [FAQ](#faq)
- [License](#license)

---

## Install

```toml
[dependencies]
localharness = "0.2"
tokio        = { version = "1", features = ["macros", "rt-multi-thread"] }
```

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

<details><summary><b>Resume a conversation</b></summary>

```rust
let agent = Agent::start_gemini(
    GeminiAgentConfig::new(api_key).resume("conv-abc123"),
).await?;
```
</details>

---

## Built-in tools

The Gemini backend ships **10 of 11** tools enabled by `BuiltinTool`,
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
| `run_command` | W | Shell exec with timeout (default 30s / max 600s), 256 KiB output cap. |
| `generate_image` | W | Call the image model; returns base64 + MIME. |
| `ask_question` | I/O | Default no-op (returns `skipped: true`); register a custom `ask_question` tool for interactive UI. |
| `start_subagent` | — | **Not yet implemented** (lands in 0.3.x). |

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

**What about the 0.1.x `start_local` / Go binary?** Still works in
0.2.x but marked `#[deprecated]`; removed in 0.3.0. Migrate to
`start_gemini`. See [`UPSTREAM.md`](UPSTREAM.md) and
[`DESIGN.md`](DESIGN.md) for the historical context.

---

## License

[Apache-2.0](LICENSE).

[upstream]: https://github.com/google-antigravity/antigravity-sdk-python
