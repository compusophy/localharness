# localharness

[![crates.io](https://img.shields.io/crates/v/localharness.svg)](https://crates.io/crates/localharness)
[![docs.rs](https://img.shields.io/docsrs/localharness)](https://docs.rs/localharness)
[![license: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

Unofficial Rust client SDK for the `localharness` agent runtime — the same
backend that powers Google's [`google-antigravity`][upstream] Python SDK.
Drive Gemini-backed agents over the same wire protocol, from a Rust
codebase.

> **Status:** alpha. Tracks upstream commit
> [`d6be9ca`](UPSTREAM.md). Not affiliated with Google.

---

## Install

```toml
[dependencies]
localharness = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

You also need the `localharness` binary on `PATH`, or its location in
`ANTIGRAVITY_HARNESS_PATH`. Install the Python SDK once to obtain the
binary:

```sh
pip install google-antigravity
# then point the env var at <site-packages>/google/antigravity/bin/localharness
```

## Quickstart

```rust
use localharness::{Agent, LocalAgentConfig};

#[tokio::main]
async fn main() -> localharness::Result<()> {
    let cfg = LocalAgentConfig::new()
        .with_system_instructions("You are a concise code reviewer.")
        .with_api_key(std::env::var("GEMINI_API_KEY").unwrap());

    let agent = Agent::start_local(cfg).await?;
    let response = agent.chat("What is 2+2?").await?;
    println!("{}", response.text().await?);
    agent.shutdown().await?;
    Ok(())
}
```

### Streaming text

```rust
use futures_util::StreamExt;

let response = agent.chat("Write a haiku about Rust.").await?;
let mut tokens = response.text_stream();
while let Some(chunk) = tokens.next().await {
    print!("{}", chunk?);
}
```

### Custom tools

```rust
use localharness::{ClosureTool, LocalAgentConfig};
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

let cfg = LocalAgentConfig::new()
    .with_tool(weather)
    .with_policies(vec![localharness::allow_all()]);
```

### Policies

```rust
use localharness::{deny_all, Policy};

let policies = vec![
    deny_all(),
    Policy::allow("view_file"),
    Policy::ask("run_command", std::sync::Arc::new(|_call| {
        // Pop a UI; return true to approve.
        true
    })),
];
```

### Triggers

```rust
use std::time::Duration;
use localharness::every;

let watchdog = every(Duration::from_secs(60), "deploy_watch", |ctx| async move {
    ctx.send_when_idle("Check the deployment status.").await
});
```

## Architecture

| Layer | Type | Purpose |
|------:|------|---------|
| 1 | [`Agent`](src/agent.rs) | Builder, connect, chat, shutdown. |
| 2 | [`Conversation`](src/conversation.rs) / [`ChatResponse`](src/conversation.rs) | Stateful session, multi-cursor streams. |
| 3 | [`Connection`](src/connections/mod.rs) | Transport abstraction (`LocalConnection` over WebSocket). |

Hot-path concurrency uses `AtomicBool` for idle, `tokio::sync::broadcast`
for fan-out steps, `parking_lot::RwLock` for hook/tool registries, and
`arc_swap::ArcSwapOption` for the lock-free tool-context swap. Backpressure
is bounded at every async hop.

## Upstream sync

This crate is a translation, not a fork. See [`UPSTREAM.md`](UPSTREAM.md)
for the commit we're pinned to and how to roll the pin forward when
upstream releases.

```sh
./scripts/sync-upstream.sh        # bash
./scripts/sync-upstream.ps1       # PowerShell
```

The script clones upstream into a scratch directory, diffs against the
pinned commit, and prints the punch list of files we'd need to re-port.
It does **not** modify your working tree.

## License

Apache-2.0. Derived from Google's [`google-antigravity`][upstream] Python
SDK (same license); the upstream `LICENSE` is preserved at the root for
attribution. The original Python README is preserved as
[`PYTHON_README.md`](PYTHON_README.md).

[upstream]: https://github.com/google-antigravity/antigravity-sdk-python
