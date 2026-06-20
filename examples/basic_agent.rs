//! Minimal getting-started example: a single agent turn with one custom tool.
//!
//! Unlike the other examples (`tempo_tx_live`, `create_subagent_live`), this
//! touches no chain and needs no wallet — just a Gemini API key and the
//! default features. It's the smallest end-to-end use of the core agent loop.
//!
//! Run:
//!
//! ```sh
//! GEMINI_API_KEY=your-key cargo run --example basic_agent
//! ```
//!
//! Get a key from https://aistudio.google.com/app/apikey.

use localharness::{deny_all, Agent, ClosureTool, GeminiAgentConfig, Policy};
use serde_json::json;

#[tokio::main]
async fn main() -> localharness::Result<()> {
    // Missing key is a normal "you haven't set it up yet" state, not an SDK
    // failure — exit cleanly with instructions instead of a panic + backtrace
    // (this example IS getting-started documentation; a crash reads as a bug).
    let Ok(api_key) = std::env::var("GEMINI_API_KEY") else {
        eprintln!("This example needs a Gemini API key. Set it and re-run:");
        eprintln!("  GEMINI_API_KEY=your-key cargo run --example basic_agent");
        eprintln!("Get a key at https://aistudio.google.com/app/apikey");
        return Ok(());
    };

    // A custom tool is any name + description + JSON-schema + async closure.
    // `ClosureTool::new` returns an `Arc<ClosureTool>` ready for `with_tool`.
    // The closure receives the model's arguments as `serde_json::Value` and
    // returns a JSON `Value` the model sees as the tool's result.
    let add = ClosureTool::new(
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
        |args, _ctx| async move {
            let a = args["a"].as_i64().unwrap_or(0);
            let b = args["b"].as_i64().unwrap_or(0);
            Ok(json!({ "sum": a + b }))
        },
    );

    // Because we register a custom tool, the SDK requires an explicit safety
    // policy (or a pre-tool-call hook) — it won't run unvetted tools silently.
    // `deny_all()` makes the agent an allowlist: only the named tools run.
    // We allow our `add` tool and the built-in `finish` (how the model ends a
    // turn). Everything else is denied.
    let agent = Agent::start_gemini(
        GeminiAgentConfig::new(api_key)
            .with_system_instructions("You are a calculator. Use the `add` tool to compute sums.")
            .with_tool(add)
            .with_policies(vec![
                deny_all(),
                Policy::allow("add"),
                Policy::allow("finish"),
            ]),
    )
    .await?;

    // One prompt drives the turn: the model calls `add`, reads the result,
    // then answers. `chat` returns once the turn completes.
    let response = agent.chat("What is 17 plus 25?").await?;
    println!("{}", response.text().await?);

    // Clean teardown stops the background tool dispatcher.
    agent.shutdown().await?;
    Ok(())
}
