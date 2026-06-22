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

/// Hard cap on the report body (mirrors `proxy/api/telemetry.ts` MAX_BODY_BYTES
/// so we never ship bytes the proxy would only truncate). Cut on a char boundary.
const MAX_BODY_BYTES: usize = 24_576;

fn clamp(mut s: String) -> String {
    if s.len() > MAX_BODY_BYTES {
        let mut cut = MAX_BODY_BYTES;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n…(truncated)");
    }
    s
}

/// POST a fully-assembled report to the proxy. Best-effort, fire-and-forget,
/// 8s timeout; no-op without an identity to sign with. Does NOT redact or dedup
/// — the public entry points own that.
async fn post(kind: String, title: String, signature: String, body: String) {
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
        "title": title,
        "signature": signature,
        "body": body,
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

/// The rich, context-stamped report — the one entry point for errors, cartridge
/// failures, and feedback. `freeform` (the error/feedback text) and the recent
/// conversation are REDACTED; the structured `raw_trailer` (e.g. an on-chain tx
/// line) and the [`context_block`] are built by us and appended RAW so tx hashes
/// and addresses survive (the 64-hex private-key filter used to nuke the tx
/// link). `code`, when present, is stamped into the title + signature so the same
/// failure GROUPS into one issue instead of one-per-fingerprint. Best-effort +
/// fire-and-forget; no-op when disabled, already-sent, or no identity.
/// Note on gating: this does NOT check [`enabled`]. The `lh_telemetry` toggle
/// governs *automatic* reports (errors, cartridge failures) — those callers
/// check `enabled()` themselves. DELIBERATE feedback always sends (the user
/// clicked submit), so it calls here directly.
pub(crate) async fn report_event(
    kind: String,
    code: Option<u16>,
    title: String,
    signature: String,
    freeform: String,
    raw_trailer: String,
) {
    let (title, signature) = match code {
        Some(c) => {
            let label = crate::error_codes::fmt_label(c);
            (format!("{label} {title}"), format!("{label}-{signature}"))
        }
        None => (title, signature),
    };
    if !SENT.with(|s| s.borrow_mut().insert(signature.clone())) {
        return; // already reported this signature this session
    }
    let mut body = redact(&freeform);
    let convo = recent_conversation();
    if !convo.trim().is_empty() {
        body.push_str("\n\nrecent conversation:\n");
        body.push_str(&redact(&convo));
    }
    if !raw_trailer.trim().is_empty() {
        body.push_str("\n\n");
        body.push_str(&raw_trailer);
    }
    body.push_str("\n\n");
    body.push_str(&context_block().await);
    post(kind, redact(&title), signature, clamp(body)).await;
}

/// A stable signature for a turn-level failure: agent + context + a short error
/// fingerprint so the same break dedups. (The `code` is prepended by
/// [`report_event`].)
pub(crate) fn signature_for(agent: &str, context: &str, err: &str) -> String {
    let fp: String = err
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    format!("{agent}-{context}-{fp}")
}

/// Rich off-chain feedback (design/telemetry-and-global-lessons.md). Off-chain is
/// now the PRIMARY path (cheap, rich); `tx_hash` is `Some` only when the owner
/// opted to ALSO mirror the short note on-chain, and is linked here RAW.
pub(crate) async fn report_feedback(agent: String, tx_hash: Option<String>, feedback: String) {
    let summary: String = feedback
        .split(['\n', '.'])
        .next()
        .unwrap_or(&feedback)
        .trim()
        .chars()
        .take(100)
        .collect();
    let title = format!("feedback ({agent}): {summary}");
    let fp: String = feedback
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(48)
        .collect();
    let signature = format!("feedback-{agent}-{fp}");
    let raw_trailer = match tx_hash {
        Some(tx) if !tx.trim().is_empty() => format!("on-chain tx: {tx}"),
        _ => String::new(),
    };
    report_event("feedback".to_string(), None, title, signature, feedback, raw_trailer).await;
}

/// Project the live agent's last few turns into a short text block for a report
/// (synchronous, already in memory — no OPFS/network). Caps turns + per-turn
/// length so a long build session can't balloon the body. "" when there's no
/// agent (e.g. a visitor).
pub(crate) fn recent_conversation() -> String {
    const MAX_TURNS: usize = 12;
    const MAX_CHARS_PER_TURN: usize = 400;
    let entries = crate::app::APP
        .with(|cell| cell.borrow().agent.as_ref().map(|a| a.transcript()))
        .unwrap_or_default();
    let start = entries.len().saturating_sub(MAX_TURNS);
    entries[start..]
        .iter()
        .filter(|e| !e.text.trim().is_empty())
        .map(|e| {
            let mut t: String = e.text.trim().chars().take(MAX_CHARS_PER_TURN).collect();
            if e.text.trim().chars().count() > MAX_CHARS_PER_TURN {
                t.push('…');
            }
            format!("{}: {}", e.role.as_str(), t)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The SAFE, non-secret context trailer stamped onto every report: who (agent +
/// identity address), what (model, chain, app version), where (device UA +
/// viewport + URL query), and the redacted-by-construction settings snapshot.
/// Built entirely from non-secret, in-memory/localStorage values (the API key is
/// NEVER read), so the whole block is appended to a report RAW.
pub(crate) async fn context_block() -> String {
    let model = crate::app::model::load().await;
    let agent = crate::app::tenant::current_name().unwrap_or_else(|| "apex".to_string());
    let address = crate::app::APP
        .with(|cell| {
            use crate::app::VerifyState;
            match &cell.borrow().verify_state {
                VerifyState::Verified { address } => Some(address.clone()),
                VerifyState::Visitor { visitor_address, .. } => Some(visitor_address.clone()),
                _ => cell.borrow().wallet.as_ref().map(|w| w.address_hex()),
            }
        })
        .unwrap_or_else(|| "—".to_string());
    let chain = if crate::registry::is_mainnet() { "mainnet" } else { "testnet" };

    let win = web_sys::window();
    let nav = win.as_ref().map(|w| w.navigator());
    let ua = nav
        .as_ref()
        .and_then(|n| n.user_agent().ok())
        .unwrap_or_default();
    let lang = nav.as_ref().and_then(|n| n.language()).unwrap_or_default();
    let vw = win
        .as_ref()
        .and_then(|w| w.inner_width().ok())
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as i32;
    let vh = win
        .as_ref()
        .and_then(|w| w.inner_height().ok())
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as i32;
    let url_q = win
        .as_ref()
        .and_then(|w| w.location().search().ok())
        .unwrap_or_default();
    let lower_ua = ua.to_lowercase();
    let mobile = lower_ua.contains("mobi") || lower_ua.contains("android") || lower_ua.contains("iphone");
    let form = if mobile { "mobile" } else { "desktop" };

    let ls = win.as_ref().and_then(|w| w.local_storage().ok().flatten());
    let get = |k: &str| ls.as_ref().and_then(|s| s.get_item(k).ok().flatten());
    let byok = get("lh_model_access").map(|v| v == "byok").unwrap_or(false);
    let key_present = get("gemini_api_key").is_some();
    let theme = get("lh-theme").unwrap_or_else(|| "dark".to_string());

    format!(
        "---\ncontext:\n  agent: {agent}\n  identity: {address}\n  model: {model}\n  \
         chain: {chain}\n  app: v{ver}\n  device: {form} · {ua}\n  viewport: {vw}x{vh}\n  \
         lang: {lang}\n  url: {url_q}\n  settings: byok={byok} key_present={key_present} \
         theme={theme} telemetry={tele} feedback_onchain={fonchain}",
        ver = env!("CARGO_PKG_VERSION"),
        tele = enabled(),
        fonchain = crate::app::feedback::feedback_onchain_enabled(),
    )
}
