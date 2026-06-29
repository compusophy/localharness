//! On-chain feedback: the write-only `[feedback]` modal + its OPFS mirror and
//! `FeedbackFacet.submitFeedback` submission. Lifted out of `events.rs` as a
//! self-contained feature (step 2 of breaking up the app monolith). The event
//! dispatcher in `events` calls these; `chat.rs` calls `submit_feedback_onchain`
//! for the agent-authored path. Behavior is unchanged — the proof-of-spec gate
//! confirms it.

use wasm_bindgen::prelude::*;

use crate::encoding::{parse_address, tx_short_hash};
use super::dom;

/// Whether to ALSO mirror the short feedback note on-chain (FeedbackFacet).
/// Off-chain (the rich telemetry issue) is the PRIMARY, default path — cheap and
/// context-rich; the on-chain write costs sponsor gas (~7.6k/byte) and is now
/// opt-in via `localStorage["lh_feedback_onchain"] == "on"` (admin toggle).
pub(crate) fn feedback_onchain_enabled() -> bool {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("lh_feedback_onchain").ok().flatten())
        .map(|v| v == "on")
        .unwrap_or(false)
}

/// Validate + rate-limit the feedback textarea, mirror it to OPFS, and submit it
/// on-chain (signed by the apex iframe wallet, sponsor-paid).
pub(crate) fn feedback_submit() {
    let Some(textarea) = dom::textarea_by_id("feedback-text") else {
        return;
    };
    let text = textarea.value().trim().to_string();
    if text.is_empty() {
        return; // silent no-op per [[feedback-no-explanatory-validation]]
    }
    if text.len() > 2048 {
        dom::swap_inner(
            "feedback-msg",
            &format!(
                "<span style=\"color:var(--error)\">feedback too long: {} bytes (max 2048) — please shorten</span>",
                text.len()
            ),
        );
        return;
    }

    // Client-side rate limit: one submission per 60 seconds.
    thread_local! {
        static LAST_FEEDBACK_MS: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
    }
    let now = js_sys::Date::now();
    let elapsed = LAST_FEEDBACK_MS.with(|c| now - c.get());
    if elapsed < 60_000.0 {
        let remaining = ((60_000.0 - elapsed) / 1000.0).ceil() as u32;
        dom::swap_inner(
            "feedback-msg",
            &dom::msg_span(dom::Msg::Muted, &format!("wait {remaining}s")),
        );
        return;
    }
    LAST_FEEDBACK_MS.with(|c| c.set(now));

    // Need an apex wallet to sign. The visitor address from verify
    // state is what the iframe signer controls.
    let from_hex = super::APP.with(|cell| {
        use super::VerifyState;
        match &cell.borrow().verify_state {
            VerifyState::Verified { address } => Some(address.clone()),
            VerifyState::Visitor { visitor_address, .. } => Some(visitor_address.clone()),
            _ => cell.borrow().wallet.as_ref().map(|w| w.address_hex()),
        }
    });
    let Some(from_hex) = from_hex else {
        dom::swap_inner(
            "feedback-msg",
            "<span style=\"color:var(--error)\">claim an identity first</span>",
        );
        return;
    };

    let onchain = feedback_onchain_enabled();
    dom::swap_inner(
        "feedback-msg",
        &dom::msg_span(dom::Msg::Muted, if onchain { "submitting…" } else { "sending…" }),
    );
    wasm_bindgen_futures::spawn_local(async move {
        // Mirror to local OPFS first so the user always has a copy even if the
        // network leg fails. Best-effort — log and continue on error.
        if let Err(err) = append_feedback_local(&text).await {
            web_sys::console::warn_1(&JsValue::from_str(&format!("feedback local copy: {err}")));
        }
        let agent = super::tenant::current_name().unwrap_or_else(|| "apex".to_string());
        // On-chain ONLY when the owner opted in (default off). Off-chain is the
        // primary path; the on-chain write costs sponsor gas.
        let tx = if onchain {
            match submit_feedback_onchain(&from_hex, &text).await {
                Ok(h) => Some(h),
                Err(err) => {
                    web_sys::console::warn_1(&JsValue::from_str(&format!("feedback on-chain: {err}")));
                    None
                }
            }
        } else {
            None
        };
        // The rich off-chain report is the PRIMARY record — full device/settings/
        // conversation context, linked to the on-chain tx when present. This is a
        // deliberate user action, so it always sends (independent of the auto-
        // telemetry toggle).
        super::telemetry::report_feedback(agent, tx.clone(), text.clone()).await;
        let receipt = match (onchain, &tx) {
            (true, Some(h)) => format!("✓ sent (on-chain tx {})", tx_short_hash(h)),
            (true, None) => "✓ sent (on-chain leg failed; saved off-chain + locally)".to_string(),
            _ => "✓ sent".to_string(),
        };
        dom::swap_inner("feedback-msg", &dom::msg_span(dom::Msg::Accent, &receipt));
        // Clear the textarea — leaving the sent text in place made a second
        // SUBMIT click double-file the same note.
        if let Some(textarea) = dom::textarea_by_id("feedback-text") {
            textarea.set_value("");
        }
    });
}

/// Sign + submit `FeedbackFacet.submitFeedback(text)` on the diamond via the apex
/// iframe signer. The on-chain `Entry[]` + event log is the canonical store;
/// the developer harvests via `scripts/harvest-feedback`. Caller's gas is paid
/// by the sponsor (Tempo allows contract calls).
pub(crate) async fn submit_feedback_onchain(from_hex: &str, text: &str) -> Result<String, String> {
    let calldata = encode_submit_feedback_calldata(text);
    let registry_addr = parse_address(super::registry::REGISTRY_ADDRESS())?;
    let call = crate::tempo_tx::TempoCall {
        to: registry_addr,
        value_wei: 0,
        input: calldata,
    };
    // FeedbackFacet APPENDS an Entry{sender,timestamp,text} to an on-chain
    // array (storing the full string in cold SSTOREs) AND emits an event.
    // Live estimate: ~1.3M gas for a short note, ~17M near the 2048-byte cap
    // — the cost scales steeply with byte length, so a flat cap can't cover
    // both. The old flat 800k out-of-gassed and reverted SILENTLY on EVERY
    // submission (local mirror succeeded, on-chain leg failed → permanently
    // stuck at feedbackCount=0). Size the cap to the text length with
    // generous per-byte headroom plus ~300k Tempo sponsorship overhead.
    let gas = 1_500_000u128 + (text.len() as u128) * 9_000;
    super::events::run_sponsored_tempo_call(from_hex, vec![call], gas, "submit feedback").await
}

/// ABI-encode `submitFeedback(string)`. Layout: selector + offset(0x20) + length
/// + bytes (right-padded to a 32-byte multiple).
fn encode_submit_feedback_calldata(text: &str) -> Vec<u8> {
    use sha3::{Digest, Keccak256};
    let mut hasher = Keccak256::new();
    hasher.update(b"submitFeedback(string)");
    let digest = hasher.finalize();
    let mut selector = [0u8; 4];
    selector.copy_from_slice(&digest[..4]);

    let bytes = text.as_bytes();
    let len = bytes.len();
    let padded_len = len.div_ceil(32) * 32;

    let mut out = Vec::with_capacity(4 + 32 + 32 + padded_len);
    out.extend_from_slice(&selector);
    // offset to dynamic head — 0x20 (one dynamic arg after a 32-byte slot)
    let mut offset = [0u8; 32];
    offset[31] = 0x20;
    out.extend_from_slice(&offset);
    // length
    let mut len_bytes = [0u8; 32];
    len_bytes[24..].copy_from_slice(&(len as u64).to_be_bytes());
    out.extend_from_slice(&len_bytes);
    // payload + zero-pad
    out.extend_from_slice(bytes);
    out.resize(4 + 32 + 32 + padded_len, 0);
    out
}

/// Append a feedback entry to `.lh_feedback.txt` in this origin's OPFS as a
/// local-first mirror (the canonical store is on-chain). One line per entry:
/// `ISO-timestamp\tTEXT\n`.
async fn append_feedback_local(text: &str) -> Result<(), String> {
    let fs = super::shared_opfs();
    let existing = fs.read(".lh_feedback.txt").await.unwrap_or_default();
    let now = js_sys::Date::new_0().to_iso_string().as_string().unwrap_or_default();
    let entry = format!("{now}\t{text}\n");
    let mut combined = existing;
    combined.extend_from_slice(entry.as_bytes());
    fs.write_atomic(".lh_feedback.txt", &combined)
        .await
        .map_err(|e| format!("{e}"))
}
