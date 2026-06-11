//! LIVE regression proof for the Gemini 3.x `thoughtSignature` 400.
//!
//! Gemini 3.x stamps every `functionCall` part with an opaque
//! `thoughtSignature` and rejects (HTTP 400 INVALID_ARGUMENT, "Function call
//! is missing a thought_signature in functionCall parts") any request whose
//! replayed history omits it. Before the fix in `wire.rs`/`loop.rs`, the
//! SECOND round of every tool-using turn 400'd — tool usage was bricked in
//! the CLI and the browser alike.
//!
//! This example drives a real multi-round tool turn through the credit proxy
//! (so it needs no Gemini key — only a funded localharness identity): the
//! model must call `add` at least twice, which forces the loop to replay a
//! history containing functionCall parts. If the signature is dropped, round
//! two fails with the 400; if it round-trips, the turn completes.
//!
//! Run (uses `claude`'s key from `~/.localharness/keys/`, override with
//! `LH_KEY_FILE`):
//!
//! ```sh
//! cargo run --example thought_signature_live --features wallet
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use localharness::{allow_all, Agent, ClosureTool, GeminiAgentConfig};
use serde_json::json;

#[tokio::main]
async fn main() -> localharness::Result<()> {
    let key_file = std::env::var("LH_KEY_FILE").unwrap_or_else(|_| {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .expect("no home dir");
        format!("{home}/.localharness/keys/claude.localharness.key")
    });
    let key_hex = std::fs::read_to_string(&key_file)
        .unwrap_or_else(|e| panic!("cannot read identity key {key_file}: {e}"))
        .trim()
        .to_string();
    let signer = localharness::wallet::from_private_key_hex(&key_hex)
        .expect("bad identity key");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let token = localharness::registry::proxy_auth_token(&signer, now);
    let base = url::Url::parse(localharness::registry::CREDIT_PROXY_URL).unwrap();

    let calls = Arc::new(AtomicU64::new(0));
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

    // No builtins (no filesystem access needed) — just the one custom tool.
    let caps = localharness::types::CapabilitiesConfig {
        enabled_tools: Some(Vec::new()),
        enable_subagents: false,
        ..Default::default()
    };

    let agent = Agent::start_gemini(
        GeminiAgentConfig::new(token)
            .with_base_url(base)
            .with_capabilities(caps)
            .with_tool(add)
            .with_policies(vec![allow_all()]),
    )
    .await?;

    // Two SEQUENTIAL adds: the second call's request must replay the first
    // functionCall part from history — the exact shape that 400'd.
    let response = agent
        .chat(
            "Use the add tool to compute 17 + 25. Then call add AGAIN to add \
             100 to that result. Reply with only the final number.",
        )
        .await?;
    let text = response.text().await?;
    let n = calls.load(Ordering::SeqCst);

    println!("final answer: {text}");
    println!("tool executed {n} time(s)");
    assert!(
        n >= 2,
        "expected >=2 tool rounds (the regression shape); got {n}"
    );
    assert!(
        text.contains("142"),
        "expected 142 in the final answer, got: {text}"
    );
    println!("PASS: multi-round tool turn survived history replay (no thoughtSignature 400)");

    agent.shutdown().await?;
    Ok(())
}
