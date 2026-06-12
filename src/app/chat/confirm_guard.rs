//! Typed-confirmation guard — DISPATCH-LAYER enforcement of the
//! destructive-action convention (challenge-nonce pattern).
//!
//! The convention used to live only in the prompt + a `confirmation == name`
//! arg check, and a live E2E showed the model auto-filling it
//! (`release_subdomain(name: "fsmoke", confirmation: "fsmoke")` in the SAME
//! turn as the request). This [`PreToolCallDecideHook`] makes that
//! impossible: the pure state machine is [`crate::confirm`]; this guard
//! supplies the CSPRNG nonce, the latest USER message (recorded by
//! `run_send`), and the user-facing surfacing (the code is painted into the
//! status line so the user sees it even if the model paraphrases — the deny
//! message also lands verbatim in the inline tool-result pill).
//!
//! Flow: first call → denied with a single-use code bound to those exact
//! arguments; the agent relays the code; the OWNER types it in chat; the
//! retry with `confirmation: <code>` passes only because the code appears in
//! the latest user message. A model echoing the code by itself is denied.

use std::cell::RefCell;

use crate::confirm::{fingerprint, nonce_from_bytes, ConfirmGate, ConfirmOutcome, NONCE_LEN};
use crate::error::Result;
use crate::hooks::{OperationContext, PreToolCallDecideHook};
use crate::types::{HookResult, ToolCall};

thread_local! {
    /// The single pending challenge (at most one destructive action awaits
    /// confirmation at a time). Survives across turns within the tab —
    /// the user types the code in their NEXT message.
    static GATE: RefCell<ConfirmGate> = RefCell::new(ConfirmGate::new());
    /// Text of the most recent REAL user message (never an internal nudge),
    /// recorded by `run_send`. The confirming call is only accepted when the
    /// pending code appears here — i.e. the user actually typed it.
    static LAST_USER_MSG: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Tools that must never execute without a user-typed confirmation:
/// irreversible burns and value moves.
const CONFIRM_GATED: &[&str] = &[
    "release_subdomain",
    "bulk_release_subdomains",
    "send_lh",
    "batch_send_lh",
];

/// Record the latest REAL user message (called by `run_send` before the turn
/// streams). Auto-continue nudges never pass through here, so the gate always
/// validates against something the user typed.
pub(crate) fn note_user_message(text: &str) {
    LAST_USER_MSG.with(|m| *m.borrow_mut() = text.to_string());
}

/// A fresh single-use confirmation code from the platform CSPRNG
/// (`OsRng` → `getrandom/js` on wasm) — NOT derivable from the conversation.
fn fresh_nonce() -> String {
    let mut bytes = [0u8; NONCE_LEN];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
    nonce_from_bytes(&bytes)
}

pub(crate) struct TypedConfirmationGuard;

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl PreToolCallDecideHook for TypedConfirmationGuard {
    fn name(&self) -> &str {
        "app::typed_confirmation_guard"
    }

    async fn run(&self, _ctx: &OperationContext, call: &ToolCall) -> Result<HookResult> {
        if !CONFIRM_GATED.contains(&call.name.as_str()) {
            return Ok(HookResult::allow());
        }
        let confirmation = call
            .args
            .get("confirmation")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let fp = fingerprint(&call.name, &call.args);
        let last_user = LAST_USER_MSG.with(|m| m.borrow().clone());
        let outcome =
            GATE.with(|g| g.borrow_mut().check(&fp, confirmation, &last_user, fresh_nonce()));
        match outcome {
            ConfirmOutcome::Approved => Ok(HookResult::allow()),
            ConfirmOutcome::Challenge { nonce } => {
                // Surface the code DIRECTLY to the user — the status line is
                // model-independent, so the code arrives even if the model
                // paraphrases or omits it.
                crate::app::dom::set_status(
                    &format!("confirm {}: type {nonce} to proceed", call.name),
                    false,
                );
                Ok(HookResult::deny(format!(
                    "`{}` NOT executed — this action requires a typed confirmation. \
                     A single-use code has been issued and shown to the owner: {nonce}. \
                     Explain exactly what this call will do, ask the owner to TYPE the \
                     code {nonce} in chat, then STOP and wait. Only after a user message \
                     containing the code, retry this call with the SAME arguments plus \
                     `confirmation: \"{nonce}\"`. The code is bound to these exact \
                     arguments and is replaced if you call again without it.",
                    call.name
                )))
            }
            ConfirmOutcome::NotTypedByUser => Ok(HookResult::deny(format!(
                "`{}` NOT executed — the confirmation code is correct but the OWNER has \
                 not typed it: it does not appear in their latest chat message, and \
                 echoing it yourself does not count. STOP, ask the owner to type the \
                 code, and retry only after a user message containing it (the same code \
                 stays valid).",
                call.name
            ))),
        }
    }
}
