//! Off-chain auto error reporting (browser-app) — design/telemetry-and-global-
//! lessons.md, phase 1.
//!
//! On a REAL, unexpected failure the app submits ONE redacted report to the
//! credit proxy (`/api/telemetry`), which files it as a GitHub issue in the
//! private telemetry repo. This is the rich, off-chain counterpart to the short
//! on-chain `FeedbackFacet` — we learn from failures the model didn't even
//! notice (distinct from `record_lesson`, which the model writes deliberately).
//!
//! Privacy: the body is REDACTED on this device (keys/secrets stripped) BEFORE
//! it leaves — the proxy never sees a secret. On by default; an owner can turn
//! it off (admin → telemetry). Per-session dedup so one recurring error files
//! once. Best-effort: every failure path here is swallowed — telemetry must
//! never break a turn.

use std::cell::RefCell;
use std::collections::HashSet;

thread_local! {
    /// Error signatures already reported this session — file each once.
    static SENT: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// Off switch: `localStorage["lh_telemetry"] == "off"` disables it. Anything
/// else (incl. unset) = ON — the high-signal default the owner opted into.
pub(crate) fn enabled() -> bool {
    local_storage()
        .and_then(|s| s.get_item("lh_telemetry").ok().flatten())
        .map(|v| v != "off")
        .unwrap_or(true)
}

/// Toggle + persist the setting (admin row).
pub(crate) fn set_enabled(on: bool) {
    if let Some(s) = local_storage() {
        let _ = s.set_item("lh_telemetry", if on { "on" } else { "off" });
    }
}

/// Redact obvious secrets from `s` token-by-token (no regex dep): API keys
/// (`sk-…`, `AIza…`) and 32-byte hex blobs (private keys; 20-byte addresses at
/// 40 hex are left intact). The seed never appears in chat context, but a user
/// could paste a key — strip it before it leaves the device.
pub(crate) fn redact(s: &str) -> String {
    s.split_inclusive(char::is_whitespace)
        .map(|tok| {
            let t = tok.trim();
            let hex = t.trim_start_matches("0x");
            let secret = (t.len() >= 20 && (t.starts_with("sk-") || t.starts_with("AIza")))
                || (hex.len() >= 64 && hex.chars().all(|c| c.is_ascii_hexdigit()));
            if secret {
                tok.replace(t, "[redacted]")
            } else {
                tok.to_string()
            }
        })
        .collect()
}

/// Submit a redacted report. `kind` groups it (e.g. "error"); `title` is the
/// one-line summary; `signature` dedups (same signature → filed once per
/// session); `body` is the rich context (redacted here). Best-effort + fire-and-
/// forget — call via `spawn_local`. No-op when disabled, already-sent, or no
/// identity.
pub(crate) async fn report(kind: String, title: String, signature: String, body: String) {
    if !enabled() {
        return;
    }
    let fresh = SENT.with(|s| s.borrow_mut().insert(signature.clone()));
    if !fresh {
        return; // already reported this signature this session
    }
    let Some((signer, _addr)) = crate::app::chat::credit_signer().await else {
        return; // no identity to authenticate the report
    };
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let token = crate::registry::proxy_auth_token(&signer, now);
    let endpoint = format!(
        "{}/api/telemetry",
        crate::registry::CREDIT_PROXY_URL.trim_end_matches('/')
    );
    let payload = serde_json::json!({
        "kind": kind,
        "title": redact(&title),
        "signature": signature,
        "body": redact(&body),
    });
    let _ = crate::app::net::with_timeout(8000, async {
        let _ = reqwest::Client::new()
            .post(&endpoint)
            .header("content-type", "application/json")
            .header("x-goog-api-key", token)
            .json(&payload)
            .send()
            .await;
        Ok::<(), String>(())
    })
    .await;
}

/// Convenience for a turn-level failure: a stable signature from the agent +
/// model + a short error fingerprint so the same break dedups.
pub(crate) fn signature_for(agent: &str, model: &str, err: &str) -> String {
    let fp: String = err
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    format!("{agent}-{model}-{fp}")
}

/// Phase-2 rich feedback (design/telemetry-and-global-lessons.md): the SHORT
/// note stays on-chain (the public source-of-truth task list); this fires the
/// FULL context off-chain to the telemetry repo, linked back to the on-chain
/// record by tx hash so the chain stays authoritative.
///
/// `feedback` = the on-chain text; `agent`/`model` stamp who/what; `tx_hash` is
/// the on-chain record this body links to; `context` is whatever recent
/// conversation we could cheaply reach (may be empty). The title is a short
/// summary (first line/clause of the feedback) and the signature is a stable
/// id (agent + a fingerprint of the text) so re-submits of the same note
/// collapse. Body redaction + dedup + signing happen in [`report`].
pub(crate) async fn report_feedback(
    agent: String,
    model: String,
    tx_hash: String,
    feedback: String,
    context: String,
) {
    // Title: a short, single-line summary of the feedback for the issue title.
    let summary: String = feedback
        .split(['\n', '.'])
        .next()
        .unwrap_or(&feedback)
        .trim()
        .chars()
        .take(100)
        .collect();
    let title = format!("feedback ({agent}): {summary}");
    // Signature: agent + a fingerprint of the text so the same note dedups.
    let fp: String = feedback
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(48)
        .collect();
    let signature = format!("feedback-{agent}-{fp}");
    let mut body = format!(
        "agent: {agent}\nmodel: {model}\non-chain tx: {tx_hash}\n\nfeedback:\n{feedback}\n"
    );
    if context.trim().is_empty() {
        body.push_str("\n(recent conversation context unavailable — follow up off the on-chain record above.)\n");
    } else {
        body.push_str("\nrecent conversation:\n");
        body.push_str(&context);
        body.push('\n');
    }
    report("feedback".to_string(), title, signature, body).await;
}
