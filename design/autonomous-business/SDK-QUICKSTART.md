# localharness as a Rust library — 5-minute quickstart

`localharness` is a model-agnostic agent SDK in **one crate**. `cargo add` gives you
an agent loop with streaming text, custom tools, safety policies, hooks, triggers,
MCP, and context compaction — no external binaries, no server. This page gets you
from zero to a running agent. Everything below is checked against the real API
(crate `0.58.0`).

> The same crate, built with `--features browser-app` on `wasm32`, becomes a
> wallet-owning agent that runs in the browser. That's a separate story — this is
> the plain "use it as a library" path.

## Install

```sh
cargo add localharness
# the SDK is async — bring your own runtime for `#[tokio::main]`:
cargo add tokio --features macros,rt-multi-thread
```

The **default feature is `native`** (tokio + the `NativeFilesystem` that backs the
filesystem built-in tools + the MCP stdio bridge). That's all you need for a native
app. The optional flags you'll reach for:

| Feature | What it adds | New deps? |
|---------|--------------|-----------|
| `native` *(default)* | tokio runtime, `run_command`, MCP stdio, native filesystem tools | — |
| `anthropic` | `Agent::start_anthropic` (Claude Messages API backend) | none — additive |
| `openai` | `Agent::start_openai` (OpenAI Chat Completions backend) | none — additive |
| `wallet` | the on-chain `registry` + `wallet` surface (semver-exempt) | k256, bip39, … |

The **Gemini** and **Mock** backends are always compiled — no feature flag.

## Hello, agent (Mock backend — no key, deterministic)

The Mock backend scripts the model's turns, so it runs fully offline: no network, no
API key, no spend. It's the ideal first run and the right tool for unit-testing
agent logic — scripted tool calls still dispatch through the real hooks + policies +
tool runner that the live backends use.

```rust
use localharness::{Agent, MockAgentConfig, MockConnection};

#[tokio::main]
async fn main() -> localharness::Result<()> {
    // Script what the "model" says. Each `.turn(...)` answers one `chat(...)`.
    let backend = MockConnection::builder()
        .turn(|t| t.text("Hello from the mock backend!"))
        .build();

    let agent = Agent::start_mock(MockAgentConfig::new(backend)).await?;

    let response = agent.chat("hi").await?;
    println!("{}", response.text().await?); // -> "Hello from the mock backend!"

    agent.shutdown().await?;
    Ok(())
}
```

`agent.chat(prompt)` returns a `ChatResponse` — a lazy, multi-cursor stream.
`.text().await` drains it to the full reply; for token-by-token output use
`.text_stream()`, and `.thoughts()` / `.tool_calls()` for the other channels.

## Swap in a real backend

The shape is identical — only the config type and the `start_*` constructor change.
The **API key is passed to the config constructor** (`*AgentConfig::new(key)`); the
SDK reads it from there, not from a global.

### Gemini (always available)

```rust
use localharness::{Agent, GeminiAgentConfig};

#[tokio::main]
async fn main() -> localharness::Result<()> {
    let cfg = GeminiAgentConfig::new(std::env::var("GEMINI_API_KEY").unwrap())
        .with_model("gemini-3.5-flash")
        .with_system_instructions("You are a concise code reviewer.");

    let agent = Agent::start_gemini(cfg).await?;
    println!("{}", agent.chat("What is 2 + 2?").await?.text().await?);
    agent.shutdown().await?;
    Ok(())
}
```

### Anthropic (Claude) — `--features anthropic`

```sh
cargo add localharness --features anthropic
```

```rust
use localharness::{Agent, AnthropicAgentConfig};

let cfg = AnthropicAgentConfig::new(std::env::var("ANTHROPIC_API_KEY").unwrap())
    .with_system_instructions("You are a concise code reviewer.");
let agent = Agent::start_anthropic(cfg).await?;
```

OpenAI is the same with `--features openai`, `OpenAiAgentConfig::new(key)`, and
`Agent::start_openai`. Both are BYOK and talk directly to the provider; both are
purely additive (no extra dependencies pulled).

## Add a custom tool with `ClosureTool`

`ClosureTool` binds a Rust async closure into the agent without defining a new type.
Signature: `ClosureTool::new(name, description, json_schema, handler)`, where the
handler is `Fn(serde_json::Value, Option<Arc<ToolContext>>) -> impl Future<Output =
localharness::Result<serde_json::Value>>`. It returns an `Arc<ClosureTool>` you hand
to `.with_tool(...)`.

```rust
use std::sync::Arc;
use localharness::{Agent, GeminiAgentConfig, ClosureTool, policy};
use serde_json::json;

#[tokio::main]
async fn main() -> localharness::Result<()> {
    let weather = ClosureTool::new(
        "get_weather",
        "Look up the current weather for a city.",
        json!({
            "type": "object",
            "properties": { "city": { "type": "string" } },
            "required": ["city"]
        }),
        |args, _ctx| async move {
            let city = args["city"].as_str().unwrap_or("unknown");
            Ok(json!({ "city": city, "temp_c": 21, "sky": "clear" }))
        },
    );

    let cfg = GeminiAgentConfig::new(std::env::var("GEMINI_API_KEY").unwrap())
        .with_tool(weather)
        // Custom tools REQUIRE an explicit safety policy (or a pre-tool hook),
        // or `start_*` returns a config error. `allow_all()` approves every call;
        // `vec![policy::deny_all(), policy::Policy::allow("get_weather")]` scopes it.
        .with_policies(vec![policy::allow_all()]);

    let agent = Agent::start_gemini(cfg).await?;
    println!("{}", agent.chat("What's the weather in Oslo?").await?.text().await?);
    agent.shutdown().await?;
    Ok(())
}
```

Need shared state in the handler (a counter, a connection pool)?
`ClosureTool::with_state(name, description, schema, state, |state, args, ctx| …)`
threads a clone of `state` into every call. For anything richer, implement the
`Tool` trait directly (`name` / `description` / `input_schema` / `execute`).

> Safety gate, in one line: enabling write/custom tools **without** a policy or
> pre-tool hook is a hard error at startup — there's no silent honor-system mode.

## Where to go deeper

Each of these is a re-export off the crate root:

- **Hooks** (`localharness::hooks`) — observe/gate the lifecycle: `PreToolCallDecideHook`
  (allow/deny a call before it runs), `PostToolCallHook`, session- and turn-level
  hooks. Register via `AgentConfig::with_pre_tool_hook` / `with_post_tool_hook`.
- **Policies** (`localharness::policy`) — declarative tool-execution rules:
  `allow_all()`, `deny_all()`, `Policy::allow("tool")`, and `workspace_only(roots)`
  to sandbox the filesystem tools to a directory. Pass via `.with_policies(vec![…])`.
- **Triggers** (`localharness::triggers`) — background messages pushed into the agent;
  `every(duration, msg)` for a recurring nudge. Register via `.with_trigger(...)`.
- **MCP** (`feature = "native"`) — connect stdio MCP servers and auto-register their
  tools: `AgentConfig::with_mcp_server(McpServerConfig { … })`.
- **Context compaction** — `agent.compact().await` summarises older history into one
  synthetic turn to reclaim context budget (live backends only).
- **Session resume** — `agent.history_bytes()?` snapshots a session; feed it back via
  `GeminiAgentConfig::with_history_bytes(bytes)` (and the per-backend equivalents).

The three layers, if you want to drop below the facade: **L1** `Agent` (connect /
chat / shutdown), **L2** `Conversation` + `ChatResponse` (the streaming session),
**L3** `connections::Connection` / `ConnectionStrategy` (the transport seam every
backend implements). Full API: <https://docs.rs/localharness>.
