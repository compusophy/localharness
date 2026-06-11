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

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::error::Result;
use crate::hooks::{OperationContext, PreToolCallDecideHook};
use crate::types::{HookResult, ToolCall};

thread_local! {
    /// Hashes of guarded calls executed during the CURRENT user request
    /// (one `run_send` invocation, across all auto-continued turns).
    static RUN_HASHES: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
    /// True once the current request has entered an auto-continuation.
    static IN_CONTINUATION: Cell<bool> = const { Cell::new(false) };
}

/// Tools where an exact repeat is harmful: user-visible side effects,
/// value moves, and on-chain posts. Read-only tools stay unguarded.
const GUARDED: &[&str] = &[
    "notify",
    "record_lesson",
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
    IN_CONTINUATION.with(|c| c.set(false));
}

/// Mark that the current request is entering an auto-continued turn.
pub(crate) fn mark_continuation() {
    IN_CONTINUATION.with(|c| c.set(true));
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

    async fn run(&self, _ctx: &OperationContext, call: &ToolCall) -> Result<HookResult> {
        if !GUARDED.contains(&call.name.as_str()) {
            return Ok(HookResult::allow());
        }
        let h = call_hash(call);
        let already_ran = RUN_HASHES.with(|s| !s.borrow_mut().insert(h));
        if already_ran && IN_CONTINUATION.with(|c| c.get()) {
            return Ok(HookResult::deny(format!(
                "duplicate suppressed: `{}` with these exact arguments already \
                 executed during this request — do NOT repeat side-effecting \
                 actions. If the user's request is fulfilled, call `finish` now.",
                call.name
            )));
        }
        Ok(HookResult::allow())
    }
}
