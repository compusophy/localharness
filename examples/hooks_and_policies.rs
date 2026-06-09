//! Attach a hook (observe tool calls) and a policy (gate them) — and watch both
//! fire, OFFLINE.
//!
//! The agent loop runs every tool call through two real extension points:
//!   * **Policies** — a deny-by-default allowlist that decides whether a call is
//!     allowed BEFORE it runs. Here `deny_all()` blocks everything, then
//!     `Policy::allow("ping")` re-opens exactly one tool. The other scripted
//!     call (`secret`) is denied — its body never executes.
//!   * **Hooks** — observers/gates at six lifecycle points. We implement the
//!     real `PostToolCallHook` trait as a logger that records every tool result
//!     (allowed OR denied) as it flows back through the loop.
//!
//! Both are driven by the same scripted `MockConnection`, so this is a faithful,
//! deterministic exercise of the policy + hook pipeline — no key, no network.
//!
//! Run:
//!
//! ```sh
//! cargo run --example hooks_and_policies
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use localharness::{
    deny_all, Agent, ClosureTool, MockAgentConfig, MockConnection, OperationContext,
    PostToolCallHook, Policy, ToolResult,
};
use parking_lot::Mutex;
use serde_json::json;

/// A real hook: implements `PostToolCallHook` to OBSERVE (not block) every tool
/// result. It pushes a one-line summary into a shared log we read at the end.
/// Inspect-only hooks like this can't change the outcome — they watch it.
struct ToolCallLogger {
    log: Arc<Mutex<Vec<String>>>,
}

// `#[async_trait]` on native; the SDK cfg-gates this to `?Send` on wasm. Mirror
// that pattern in your own hooks if you target the browser too.
#[async_trait]
impl PostToolCallHook for ToolCallLogger {
    fn name(&self) -> &str {
        "tool_call_logger"
    }

    async fn run(&self, _ctx: &OperationContext, result: &ToolResult) -> localharness::Result<()> {
        // `ToolResult` carries either `result` (success) or `error` (a denied or
        // failed call). We summarise both so the log shows what happened.
        let outcome = match (&result.result, &result.error) {
            (Some(v), _) => format!("ok -> {v}"),
            (_, Some(e)) => format!("blocked/failed -> {e}"),
            _ => "no output".to_string(),
        };
        self.log.lock().push(format!("{}: {}", result.name, outcome));
        Ok(())
    }
}

#[tokio::main]
async fn main() -> localharness::Result<()> {
    let log = Arc::new(Mutex::new(Vec::<String>::new()));

    // Two custom tools. `ping` is allowed by policy; `secret` is denied — its
    // body must NEVER run, so we make it scream if it does.
    let ping = ClosureTool::new(
        "ping",
        "Return pong.",
        json!({ "type": "object" }),
        |_args, _ctx| async move { Ok(json!({ "reply": "pong" })) },
    );
    let secret = ClosureTool::new(
        "secret",
        "Should be blocked by policy.",
        json!({ "type": "object" }),
        |_args, _ctx| async move {
            // If policy enforcement worked, this line never prints.
            println!("!! `secret` tool body RAN — policy failed to block it");
            Ok(json!({ "leaked": true }))
        },
    );

    // Script one turn that requests BOTH tools, then replies. The mock runs each
    // call through the policy gate inline: `ping` is approved and executes;
    // `secret` is denied and yields an error result WITHOUT running its body.
    let backend = MockConnection::builder()
        .turn(|t| {
            t.tool_call("ping", json!({}))
                .tool_call("secret", json!({}))
                .text("done")
        })
        .build();

    // Policies: deny everything, then re-allow exactly one tool. A specific
    // allow plus the wildcard deny makes this an allowlist (deny-by-default).
    let agent = Agent::start_mock(
        MockAgentConfig::new(backend)
            .with_tool(ping)
            .with_tool(secret)
            .with_policies(vec![deny_all(), Policy::allow("ping")]),
    )
    .await?;

    // Register the observability hook AFTER start, via the live hook runner.
    // (You can also register hooks before start; either works.)
    agent
        .hooks()
        .register_post_tool_call(Arc::new(ToolCallLogger { log: log.clone() }));

    // Drive the turn. Policies gate each call; the hook logs each result.
    let response = agent.chat("ping then run secret").await?;
    println!("agent said: {}", response.text().await?);

    // What the post-tool-call hook observed: `ping` ran (pong), `secret` was
    // blocked by the deny-by-default policy (its body never executed).
    println!("--- tool-call log (from the hook) ---");
    for line in log.lock().iter() {
        println!("  {line}");
    }

    agent.shutdown().await?;
    Ok(())
}
