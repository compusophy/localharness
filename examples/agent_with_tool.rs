//! Register a custom tool and watch the (scripted) model call it.
//!
//! This teaches the SDK's tool loop end-to-end, OFFLINE: you register a
//! `ClosureTool`, script a model turn that REQUESTS that tool, and the mock
//! backend dispatches the call inline through the SAME tool runner + hooks +
//! policies the live Gemini/Claude backends use. The tool's body actually runs
//! (its side effect fires), and the scripted reply comes back — exactly the
//! flow a real model would drive, but deterministic and key-free.
//!
//! Two things to notice:
//!   * Registering ANY custom tool forces you to declare a safety policy — the
//!     SDK refuses to run unvetted tools silently. `allow_all()` approves every
//!     call; a real app would use `deny_all()` + `Policy::allow("name")`.
//!   * The tool call is also OBSERVABLE on the response stream
//!     (`ChatResponse::tool_calls()`), so you can assert which tool ran with
//!     which args — handy in unit tests.
//!
//! Run (no key, no network):
//!
//! ```sh
//! cargo run --example agent_with_tool
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::StreamExt;
use localharness::{allow_all, Agent, ClosureTool, MockAgentConfig, MockConnection};
use serde_json::json;

#[tokio::main]
async fn main() -> localharness::Result<()> {
    // A side effect we can observe from outside the tool: a call counter. This
    // proves the tool BODY actually executed (not just that a call was scripted).
    let calls = Arc::new(AtomicU64::new(0));

    // A custom tool = name + description + JSON-schema + async closure. Because
    // this tool captures SHARED state (the `calls` counter), we use
    // `ClosureTool::with_state`: pass the state ONCE and the framework clones a
    // fresh handle into every call — no manual clone-into-closure, no
    // double-move. The closure receives that clone as its first argument, then
    // the model's args as `serde_json::Value`, and returns the JSON the model
    // sees as the result. `with_state` hands back an `Arc<ClosureTool>` ready to
    // pass to `with_tool`. (For a STATELESS tool, reach for `ClosureTool::new`.)
    let add = ClosureTool::with_state(
        "add",
        "Add two integers and return their sum.",
        json!({
            "type": "object",
            "properties": {
                "a": { "type": "integer" },
                "b": { "type": "integer" }
            },
            "required": ["a", "b"]
        }),
        calls.clone(),
        |calls, args, _ctx| async move {
            calls.fetch_add(1, Ordering::SeqCst);
            let a = args["a"].as_i64().unwrap_or(0);
            let b = args["b"].as_i64().unwrap_or(0);
            Ok(json!({ "sum": a + b }))
        },
    );

    // Script ONE model turn: first call `add` with args, then reply with text.
    // In a live run the model would emit these; here we replay them. The mock
    // dispatches the `tool_call` inline through the runner (so `add` runs and
    // its result feeds the loop), then streams the scripted terminal text.
    let backend = MockConnection::builder()
        .turn(|t| {
            t.tool_call("add", json!({ "a": 17, "b": 25 }))
                .text("17 + 25 = 42")
        })
        .build();

    // Registering a custom tool REQUIRES a safety policy (or a pre-tool-call
    // hook) — the SDK won't run unvetted tools. `allow_all()` approves every
    // call; scope it down with `deny_all()` + `Policy::allow("add")` in a real app.
    let agent = Agent::start_mock(
        MockAgentConfig::new(backend)
            .with_tool(add)
            .with_policies(vec![allow_all()]),
    )
    .await?;

    // Drive the turn. `chat` returns once the scripted turn finishes — including
    // the inline tool dispatch.
    let response = agent.chat("What is 17 plus 25?").await?;

    // The tool call is observable on the stream: assert which tool the (mock)
    // model dispatched, with which args. `tool_calls()` is a fresh cursor.
    let mut tool_calls = response.tool_calls();
    if let Some(Ok(call)) = tool_calls.next().await {
        println!("model called tool `{}` with args {}", call.name, call.args);
    }

    // The final text reply (after the tool result fed back into the loop).
    println!("final answer: {}", response.text().await?);

    // The tool's body ran exactly once — the side effect proves it executed
    // rather than being a no-op the model merely "claimed" to call.
    println!("tool executed {} time(s)", calls.load(Ordering::SeqCst));

    agent.shutdown().await?;
    Ok(())
}
