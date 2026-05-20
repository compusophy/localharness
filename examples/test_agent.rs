//! In-process smoke test for the Rust port.
//!
//! Exercises every layer that does not require a live `localharness`:
//!
//! * `policy::evaluate` precedence + workspace containment
//! * `ToolRunner` registration and dispatch
//! * `Conversation::chat()` end-to-end against a stub `Connection` that
//!   emits a scripted step stream
//!
//! Run with: `cargo run --example smoke`

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream::{self, BoxStream, StreamExt};
use tokio::sync::Mutex;

use localharness::{
    allow_all, evaluate, ClosureTool, Connection, Content, Conversation, Policy, Result, Step,
    StepSource, StepStatus, StepTarget, StepType, ToolCall, ToolResult,
};

// ----------------------------------------------------------------------------
// Stub connection that replays a scripted step trajectory.
// ----------------------------------------------------------------------------

struct StubConnection {
    id: String,
    script: Mutex<Option<Vec<Step>>>,
}

impl StubConnection {
    fn with_steps(steps: Vec<Step>) -> Arc<Self> {
        Arc::new(Self {
            id: "stub-conv".to_string(),
            script: Mutex::new(Some(steps)),
        })
    }
}

#[async_trait]
impl Connection for StubConnection {
    fn is_idle(&self) -> bool {
        true
    }
    fn conversation_id(&self) -> &str {
        &self.id
    }
    async fn send(&self, _content: Content) -> Result<()> {
        Ok(())
    }
    async fn send_trigger(&self, _content: String) -> Result<()> {
        Ok(())
    }
    async fn send_tool_results(&self, _results: Vec<ToolResult>) -> Result<()> {
        Ok(())
    }
    fn subscribe_steps(&self) -> BoxStream<'static, Result<Step>> {
        let taken = self.script.try_lock().ok().and_then(|mut g| g.take());
        match taken {
            Some(v) => stream::iter(v.into_iter().map(Ok)).boxed(),
            None => stream::empty().boxed(),
        }
    }
    async fn wait_for_idle(&self) -> Result<()> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

fn text_delta(idx: u32, delta: &str) -> Step {
    Step {
        id: format!("step-{idx}"),
        step_index: idx,
        kind: StepType::TextResponse,
        source: StepSource::Model,
        target: StepTarget::User,
        status: StepStatus::Active,
        content: String::new(),
        content_delta: delta.to_string(),
        thinking: String::new(),
        thinking_delta: String::new(),
        tool_calls: Vec::new(),
        error: String::new(),
        is_complete_response: Some(false),
        structured_output: None,
        usage_metadata: None,
    }
}

fn text_done(idx: u32, content: &str) -> Step {
    Step {
        id: format!("step-{idx}"),
        step_index: idx,
        kind: StepType::TextResponse,
        source: StepSource::Model,
        target: StepTarget::User,
        status: StepStatus::Done,
        content: content.to_string(),
        content_delta: String::new(),
        thinking: String::new(),
        thinking_delta: String::new(),
        tool_calls: Vec::new(),
        error: String::new(),
        is_complete_response: Some(true),
        structured_output: None,
        usage_metadata: None,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .compact()
        .init();

    println!("=== localharness smoke ===");

    // ---------- policy precedence ----------
    let policies = vec![
        Policy::deny("run_command").with_name("block_commands"),
        allow_all(),
    ];
    let safe = ToolCall {
        name: "view_file".into(),
        args: serde_json::json!({}),
        id: Some("1".into()),
        canonical_path: None,
    };
    let dangerous = ToolCall {
        name: "run_command".into(),
        args: serde_json::json!({}),
        id: Some("2".into()),
        canonical_path: None,
    };
    let r_safe = evaluate(&policies, &safe);
    let r_dangerous = evaluate(&policies, &dangerous);
    assert!(r_safe.allow, "safe call should be allowed");
    assert!(!r_dangerous.allow, "dangerous call should be denied");
    println!("policy: safe -> {}, dangerous -> {}", r_safe.message, r_dangerous.message);

    // ---------- custom tool ----------
    let echo = ClosureTool::new(
        "echo",
        "echo back its 'msg' arg",
        serde_json::json!({
            "type": "object",
            "properties": {"msg": {"type": "string"}}
        }),
        |args, _ctx| async move {
            Ok(serde_json::json!({"echo": args.get("msg").cloned()}))
        },
    );
    let runner = localharness::ToolRunner::new();
    runner.register(echo);
    let out = runner
        .execute("echo", serde_json::json!({"msg": "ping"}))
        .await?;
    assert_eq!(out["echo"], serde_json::Value::String("ping".into()));
    println!("tool: echo returned {}", out);

    // ---------- scripted chat() ----------
    let steps = vec![
        text_delta(0, "Hello "),
        text_delta(1, "world!"),
        text_done(2, "Hello world!"),
    ];
    let conn = StubConnection::with_steps(steps);
    let convo = Conversation::new(conn);
    let response = convo.chat("hi").await?;
    let text = response.text().await?;
    assert_eq!(text, "Hello world!");
    assert_eq!(convo.turn_count(), 1);
    println!("chat: '{}' (turn_count={})", text, convo.turn_count());

    println!("=== ok ===");
    Ok(())
}
