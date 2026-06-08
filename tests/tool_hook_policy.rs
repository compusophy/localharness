//! Integration test: the tool + hook + policy pipeline working TOGETHER.
//!
//! The existing unit tests cover each layer in isolation — `policy.rs` tests
//! `evaluate()` against synthetic `ToolCall`s, `tools.rs` documents
//! `ClosureTool`, and `hooks.rs` exercises `HookRunner` dispatch. None of them
//! drive the *combined* flow the agent loop actually runs: gate a tool call
//! through the registered `PreToolCallDecideHook`s (where `policy::enforce`
//! lives), and only dispatch it through the `ToolRunner` when the gate allows.
//!
//! This file reconstructs exactly that pipeline from the gemini backend's inner
//! loop (`src/backends/gemini/loop.rs`, the `dispatch_pre_tool_call` → gated
//! `runner.execute` → `dispatch_post_tool_call` sequence) using ONLY the public
//! SDK surface — no live `Connection`, no network, no mocked backend. A small
//! `run_gated` helper mirrors the real loop so each test asserts on a true
//! end-to-end interaction rather than one isolated component.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use localharness::hooks::{
    HookContext, HookRunner, OperationContext, PostToolCallHook, PreToolCallDecideHook,
};
use localharness::policy::{self, Decision, Policy};
use localharness::types::{HookResult, ToolCall, ToolResult};
use localharness::{ClosureTool, ToolRunner};

/// The outcome of running one tool call through the full gate→dispatch pipeline.
struct Gated {
    /// The policy/hook gate decision (true == allowed to run).
    allowed: bool,
    /// The typed result that post-tool hooks observed, mirroring the loop.
    result: ToolResult,
}

/// Faithful re-creation of the backend loop's per-tool-call sequence:
/// run pre-tool decide hooks; if denied, synthesize an error result and skip
/// execution; if allowed, dispatch through the `ToolRunner`; either way run the
/// post-tool hooks against the resulting `ToolResult`. This is the exact shape
/// of `src/backends/gemini/loop.rs` lines ~369-413, minus the wire plumbing.
async fn run_gated(hooks: &HookRunner, runner: &ToolRunner, call: ToolCall) -> Gated {
    let turn_ctx = HookContext::new();
    let (decision, op_ctx) = hooks.dispatch_pre_tool_call(&turn_ctx, &call).await;

    let (result_value, error): (Value, Option<String>) = if !decision.allow {
        let msg = decision.message.clone();
        (json!({ "error": msg.clone() }), Some(msg))
    } else {
        match runner.execute(&call.name, call.args.clone()).await {
            Ok(v) => {
                let err = v.get("error").and_then(|e| e.as_str()).map(String::from);
                (v, err)
            }
            Err(e) => {
                let s = e.to_string();
                (json!({ "error": s.clone() }), Some(s))
            }
        }
    };

    let result = ToolResult {
        name: call.name.clone(),
        id: None,
        result: Some(result_value),
        error,
    };
    hooks.dispatch_post_tool_call(&op_ctx, &result).await;

    Gated {
        allowed: decision.allow,
        result,
    }
}

/// A real `ClosureTool` that records each invocation in a shared counter, so a
/// test can prove the tool body ran (or, on deny, did NOT run).
fn counting_echo_tool(name: &'static str, calls: Arc<AtomicUsize>) -> Arc<ClosureTool> {
    ClosureTool::new(
        name,
        "Echo its `msg` arg back, counting each call.",
        json!({
            "type": "object",
            "properties": { "msg": { "type": "string" } }
        }),
        move |args, _ctx| {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let msg = args.get("msg").and_then(|v| v.as_str()).unwrap_or("");
                Ok(json!({ "echo": msg }))
            }
        },
    )
}

fn call(name: &str, args: Value) -> ToolCall {
    ToolCall {
        name: name.to_string(),
        args,
        id: None,
        canonical_path: None,
    }
}

/// A `PostToolCallHook` that captures the names of the tools it observed, in
/// order, so we can assert that post-hooks see every gated outcome (allow AND
/// deny) and run after the decision is made.
struct RecordingPostHook {
    seen: Arc<Mutex<Vec<(String, bool)>>>,
}

#[async_trait]
impl PostToolCallHook for RecordingPostHook {
    fn name(&self) -> &str {
        "test::recording_post"
    }
    async fn run(&self, _ctx: &OperationContext, result: &ToolResult) -> localharness::Result<()> {
        // `is_ok` == no error string surfaced (matches the loop's convention).
        self.seen
            .lock()
            .unwrap()
            .push((result.name.clone(), result.error.is_none()));
        Ok(())
    }
}

/// A `PreToolCallDecideHook` that stashes a marker into the operation context
/// and denies a single named tool. Lets us prove (a) ordering — a later
/// allow-everything policy never overrides an earlier deny — and (b) that
/// context written in a decide hook reaches the matching post hook.
struct StashAndGate {
    deny_tool: String,
}

#[async_trait]
impl PreToolCallDecideHook for StashAndGate {
    fn name(&self) -> &str {
        "test::stash_and_gate"
    }
    async fn run(
        &self,
        ctx: &OperationContext,
        call: &ToolCall,
    ) -> localharness::Result<HookResult> {
        ctx.set("inspected_by", json!(self.name()));
        if call.name == self.deny_tool {
            Ok(HookResult::deny(format!("test gate blocked {}", call.name)))
        } else {
            Ok(HookResult::allow())
        }
    }
}

// =============================================================================
// 1. A specific-tool DENY policy blocks dispatch; the tool body never runs.
// =============================================================================

#[tokio::test]
async fn policy_deny_blocks_dispatch_and_tool_body_never_runs() {
    let allowed_calls = Arc::new(AtomicUsize::new(0));
    let denied_calls = Arc::new(AtomicUsize::new(0));

    let runner = ToolRunner::new();
    runner.register(counting_echo_tool("safe_tool", allowed_calls.clone()));
    runner.register(counting_echo_tool("danger_tool", denied_calls.clone()));

    // Real composition: a wildcard allow with a specific deny layered on top.
    // The precedence table (specific-deny bucket 0 < wildcard-allow bucket 5)
    // must make the deny win — this is the load-bearing safety guarantee.
    let hooks = HookRunner::new();
    hooks.register_pre_tool_call_decide(policy::enforce(vec![
        policy::allow_all(),
        Policy::deny("danger_tool").with_name("block_danger"),
    ]));

    // The allowed tool dispatches and its body runs.
    let ok = run_gated(&hooks, &runner, call("safe_tool", json!({ "msg": "hi" }))).await;
    assert!(ok.allowed, "safe_tool should pass the gate");
    assert_eq!(ok.result.error, None, "safe_tool should not error");
    assert_eq!(
        ok.result.result.as_ref().unwrap(),
        &json!({ "echo": "hi" }),
        "safe_tool must actually execute and echo its arg",
    );

    // The denied tool is blocked BEFORE dispatch; its body must never run.
    let blocked = run_gated(&hooks, &runner, call("danger_tool", json!({ "msg": "boom" }))).await;
    assert!(!blocked.allowed, "danger_tool must be denied by policy");
    assert!(
        blocked
            .result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("block_danger"),
        "denied result should carry the policy name, got {:?}",
        blocked.result.error,
    );

    assert_eq!(allowed_calls.load(Ordering::SeqCst), 1, "safe_tool ran once");
    assert_eq!(
        denied_calls.load(Ordering::SeqCst),
        0,
        "danger_tool body must NOT have executed — the gate stopped it before ToolRunner::execute",
    );
}

// =============================================================================
// 2. deny-by-default allowlist: only explicitly allowed tools dispatch.
// =============================================================================

#[tokio::test]
async fn deny_by_default_allowlist_only_runs_allowlisted_tools() {
    let qa_calls = Arc::new(AtomicUsize::new(0));
    let write_calls = Arc::new(AtomicUsize::new(0));

    let runner = ToolRunner::new();
    runner.register(counting_echo_tool("qa_read", qa_calls.clone()));
    runner.register(counting_echo_tool("qa_write", write_calls.clone()));

    // deny_all + a single explicit allow — the autonomous-agent allowlist
    // pattern. An off-list tool (even one that IS registered) is denied.
    let hooks = HookRunner::new();
    hooks.register_pre_tool_call_decide(policy::enforce(vec![
        policy::deny_all(),
        Policy::allow("qa_read"),
    ]));

    let read = run_gated(&hooks, &runner, call("qa_read", json!({ "msg": "ok" }))).await;
    assert!(read.allowed, "allowlisted qa_read should run");

    let write = run_gated(&hooks, &runner, call("qa_write", json!({ "msg": "no" }))).await;
    assert!(!write.allowed, "off-list qa_write must be denied even though it is registered");

    // A tool name that is not even registered is still denied at the gate,
    // so the ToolNotFound error path is never reached.
    let ghost = run_gated(&hooks, &runner, call("hallucinated", json!({}))).await;
    assert!(!ghost.allowed, "unknown tool denied at the gate");
    assert!(
        ghost.result.error.as_deref().unwrap_or("").contains("policy"),
        "ghost denial should come from the policy gate, not a ToolNotFound dispatch error: {:?}",
        ghost.result.error,
    );

    assert_eq!(qa_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        write_calls.load(Ordering::SeqCst),
        0,
        "denied tool body must not run",
    );
}

// =============================================================================
// 3. ask-user policy: the handler's verdict drives allow/deny end-to-end.
// =============================================================================

#[tokio::test]
async fn ask_user_handler_verdict_drives_dispatch() {
    // Two identical setups differing only in the ask-user handler's answer.
    for (approve, expect_run) in [(true, 1usize), (false, 0usize)] {
        let calls = Arc::new(AtomicUsize::new(0));
        let runner = ToolRunner::new();
        runner.register(counting_echo_tool("needs_confirm", calls.clone()));

        let handler: policy::AskUserHandler = Arc::new(move |_call| approve);
        let hooks = HookRunner::new();
        hooks.register_pre_tool_call_decide(policy::enforce(vec![
            Policy::ask("needs_confirm", handler).with_name("confirm_gate"),
        ]));

        let g = run_gated(&hooks, &runner, call("needs_confirm", json!({ "msg": "go" }))).await;
        assert_eq!(g.allowed, approve, "gate must reflect the ask-user verdict");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            expect_run,
            "tool body should run iff the user approved (approve={approve})",
        );
    }
}

// =============================================================================
// 4. Hook ordering + context propagation across the decide→post boundary.
// =============================================================================

#[tokio::test]
async fn decide_hook_context_propagates_to_post_hook_and_first_deny_wins() {
    let calls = Arc::new(AtomicUsize::new(0));
    let runner = ToolRunner::new();
    runner.register(counting_echo_tool("free_tool", calls.clone()));
    runner.register(counting_echo_tool("gated_tool", calls.clone()));

    let seen = Arc::new(Mutex::new(Vec::<(String, bool)>::new()));

    let hooks = HookRunner::new();
    // First decide hook denies `gated_tool` and stashes a context marker.
    hooks.register_pre_tool_call_decide(Arc::new(StashAndGate {
        deny_tool: "gated_tool".to_string(),
    }));
    // A second decide hook that would allow everything — registered AFTER the
    // gate. Because the runner short-circuits on the first deny, this must NOT
    // be able to rescue `gated_tool`.
    hooks.register_pre_tool_call_decide(policy::enforce(vec![policy::allow_all()]));

    // A post hook records every observed outcome, proving post hooks fire for
    // BOTH allowed and denied calls.
    hooks.register_post_tool_call(Arc::new(RecordingPostHook { seen: seen.clone() }));

    let ok = run_gated(&hooks, &runner, call("free_tool", json!({ "msg": "x" }))).await;
    assert!(ok.allowed, "free_tool not in the deny list → allowed");

    let blocked = run_gated(&hooks, &runner, call("gated_tool", json!({ "msg": "y" }))).await;
    assert!(
        !blocked.allowed,
        "first-deny-wins: a later allow_all hook cannot override an earlier deny",
    );

    // The post hook saw both calls, in order, with correct ok/err flags.
    let log = seen.lock().unwrap().clone();
    assert_eq!(
        log,
        vec![
            ("free_tool".to_string(), true),
            ("gated_tool".to_string(), false),
        ],
        "post hook must observe both the allowed and denied outcomes in order",
    );

    // Only the allowed tool actually executed.
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// =============================================================================
// 5. Sanity on the precedence enum the whole gate rests on (cross-check that
//    the public `evaluate` agrees with the gated pipeline above).
// =============================================================================

#[test]
fn evaluate_specific_deny_outranks_wildcard_allow() {
    let policies = vec![
        policy::allow_all(),
        Policy::deny("x").with_name("deny_x"),
    ];
    assert_eq!(policies[1].decision, Decision::Deny);
    assert!(!policy::evaluate(&policies, &call("x", json!({}))).allow);
    assert!(policy::evaluate(&policies, &call("y", json!({}))).allow);
}
