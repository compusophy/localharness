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
