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
    /// Set whenever the gate DENIES a call (a fresh challenge or a
    /// not-typed-by-user re-deny). The auto-continue loop reads + clears this
    /// to STOP and wait for the owner to type the code — otherwise the denied
    /// tool call still counts as turn activity and the loop would re-issue the
    /// same (guaranteed-failing) call up to the cap, burning credits.
    static AWAITING_CONFIRMATION: RefCell<bool> = const { RefCell::new(false) };
}

/// Tools that must never execute without a user-typed confirmation:
/// irreversible burns and value moves. `spend_treasury` is here for the same
/// reason as `send_lh` — it pays an arbitrary, model-supplied `$LH` amount to
/// an arbitrary recipient and the transfer is an unconditional, non-refundable
/// outbound move (the on-chain Admin gate restricts WHO may spend, not WHETHER
/// the owner approved this specific payout, and the agent's own key IS that
/// Admin). The escrow tools (`fund_guild`/`fund_party`/`post_bounty`) stay
/// ungated by design — their funds are refundable via the disband/reclaim
/// paths — and `execute_proposal` is governance-quorum-gated.
const CONFIRM_GATED: &[&str] = &[
    "release_subdomain",
    "bulk_release_subdomains",
    "send_lh",
    "batch_send_lh",
    "spend_treasury",
];

/// Record the latest REAL user message (called by `run_send` before the turn
/// streams). Auto-continue nudges never pass through here, so the gate always
/// validates against something the user typed.
pub(crate) fn note_user_message(text: &str) {
    LAST_USER_MSG.with(|m| *m.borrow_mut() = text.to_string());
}

/// Read + clear the "a confirm-gated call was denied this turn, awaiting the
/// owner to type the code" flag. Called by `stream_turn` to convert a turn
/// whose only tool activity was a blocked confirmation into a STOP (wait for
/// the user's next message) instead of an auto-continue.
pub(crate) fn take_awaiting_confirmation() -> bool {
    AWAITING_CONFIRMATION.with(|f| f.replace(false))
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
        // A denial means the loop must STOP and wait for the owner to type the
        // code; an approval clears any stale flag from a prior turn.
        AWAITING_CONFIRMATION.with(|f| {
            *f.borrow_mut() = !matches!(outcome, ConfirmOutcome::Approved);
        });
        match outcome {
            ConfirmOutcome::Approved => Ok(HookResult::allow()),
            ConfirmOutcome::Challenge { nonce } => {
                // Surface the code DIRECTLY to the user in a bordered system
                // callout — model-independent (arrives even if the model omits
                // it) and visually distinct from chat turns so it never reads
                // as the user's own input (feedback).
                crate::app::dom::set_confirm_callout(&call.name, &nonce);
                // The callout shows the code; the model must NOT echo it (that
                // was the redundant code-spam) — keep the code out of this deny
                // text entirely so the model can't repeat it.
                Ok(HookResult::deny(format!(
                    "`{}` NOT executed — requires the owner's typed confirmation. The \
                     single-use code is shown to the owner in a confirm box; do NOT \
                     repeat the code yourself. Briefly explain what this call will do, \
                     ask the owner to type that code in chat, then STOP and wait. Once \
                     their next message contains it, retry this SAME call with \
                     `confirmation` set to the code they typed. The code is bound to \
                     these exact arguments and is replaced if you call again without it.",
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
