//! Duplicate-action guard — the auto-continue double-fire fix.
//!
//! On-chain feedback #51 (plus #52-54 arriving as a TRIPLE post — the bug
//! demonstrating itself): when a turn ends with tool activity but no
//! completion signal, `run_send` auto-continues with a nudge, and the model
//! sometimes re-executes the action it already took instead of calling
//! `finish` — duplicate notifications, duplicate feedback posts, and (worst
//! case) duplicate `$LH` transfers.
//!
//! The guard is a [`PreToolCallDecideHook`]: it records a hash of every
//! GUARDED (side-effecting) call executed during one user request, and on an
//! AUTO-CONTINUED turn denies an exact repeat (same tool, same args) with a
//! message telling the model to finish instead. First turns are never
//! blocked — "notify me twice" stays expressible; only the nudge-induced
//! repeat class is suppressed.
//!
//! **Commit-on-success (issue #84).** The pre-hook records the hash at DECIDE
//! time (before the body runs) so a parallel functionCall batch can't slip the
//! same call through twice in one turn (#55). But that recording must be
//! REVERTED if execution then FAILS — otherwise a transient error (e.g. an RPC
//! blip on `send_lh`) would permanently poison the hash and the model could
//! never retry the action within the same request. The paired
//! [`DuplicateActionGuardCleanup`] [`PostToolCallHook`] removes the hash again
//! whenever the freshly-inserted call comes back with an error. A DENIED repeat
//! never re-inserts, so its denial error never triggers a cleanup.

use std::cell::RefCell;
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::error::Result;
use crate::hooks::{OperationContext, PostToolCallHook, PreToolCallDecideHook};
use crate::types::{HookResult, ToolCall, ToolResult};

/// Op-context key the pre-hook stamps with the hash it freshly inserted, so the
/// paired post-hook can revert exactly that insert on failure. Only set on the
/// FIRST occurrence (the insert path) — a denied repeat leaves it unset.
const FRESH_HASH_KEY: &str = "app::dedup::fresh_hash";

thread_local! {
    /// Hashes of guarded calls executed during the CURRENT user request
    /// (one `run_send` invocation, across all auto-continued turns).
    static RUN_HASHES: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
}

/// Tools where an exact repeat is harmful: user-visible side effects,
/// value moves, and on-chain posts. Read-only tools stay unguarded.
const GUARDED: &[&str] = &[
    "notify",
    "record_lesson",
    "set_lessons",
    "send_lh",
    "batch_send_lh",
    "post_bounty",
    "submit_feedback",
    "create_subdomain",
    "create_and_publish_app",
    "claim_bounty",
    "submit_result",
    "accept_result",
    "create_guild",
    "invite_to_guild",
    "fund_guild",
    "spend_treasury",
    "propose_measure",
    "cast_vote",
    "execute_proposal",
    "release_subdomain",
];

/// Reset at the START of each user request (`run_send`).
pub(crate) fn reset_run() {
    RUN_HASHES.with(|h| h.borrow_mut().clear());
}

fn call_hash(call: &ToolCall) -> u64 {
    let mut hasher = DefaultHasher::new();
    call.name.hash(&mut hasher);
    call.args.to_string().hash(&mut hasher);
    hasher.finish()
}

pub(crate) struct DuplicateActionGuard;

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl PreToolCallDecideHook for DuplicateActionGuard {
    fn name(&self) -> &str {
        "app::duplicate_action_guard"
    }

    async fn run(&self, ctx: &OperationContext, call: &ToolCall) -> Result<HookResult> {
        if !GUARDED.contains(&call.name.as_str()) {
            return Ok(HookResult::allow());
        }
        let h = call_hash(call);
        let already_ran = RUN_HASHES.with(|s| !s.borrow_mut().insert(h));
        // Deny exact repeats ANYWHERE in one request — originally only on
        // auto-continued turns, but feedback #55 showed the model can also
        // emit the same call TWICE in a single first turn (Gemini batches
        // parallel functionCalls), double-firing notifications. A user who
        // genuinely wants a repeat can vary the args or ask again.
        if already_ran {
            return Ok(HookResult::deny(format!(
                "duplicate suppressed: `{}` with these exact arguments already \
                 executed during this request — do NOT repeat side-effecting \
                 actions. If the user explicitly wants it again, vary the \
                 arguments; if the request is fulfilled, call `finish` now.",
                call.name
            )));
        }
        // We just inserted `h`. Hand it to the paired post-hook so a FAILED
        // execution can revert the insert (issue #84) and the model may retry.
        ctx.set(FRESH_HASH_KEY, serde_json::Value::String(h.to_string()));
        Ok(HookResult::allow())
    }
}

/// Reverts the pre-hook's optimistic hash insert when the call FAILS, so a
/// transient error doesn't permanently block a same-request retry (issue #84).
///
/// Fires for every tool call, but only acts when the pre-hook stamped a fresh
/// hash into this op context (i.e. THIS call inserted it — not a denied repeat)
/// AND the result carries an error. Read-only / never-inserted calls leave the
/// marker unset, so this is a no-op for them.
pub(crate) struct DuplicateActionGuardCleanup;

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl PostToolCallHook for DuplicateActionGuardCleanup {
    fn name(&self) -> &str {
        "app::duplicate_action_guard_cleanup"
    }

    async fn run(&self, ctx: &OperationContext, result: &ToolResult) -> Result<()> {
        if result.error.is_none() {
            return Ok(());
        }
        let Some(h) = ctx
            .get(FRESH_HASH_KEY)
            .and_then(|v| v.as_str().and_then(|s| s.parse::<u64>().ok()))
        else {
            return Ok(());
        };
        RUN_HASHES.with(|s| s.borrow_mut().remove(&h));
        Ok(())
    }
}
