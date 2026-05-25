//! Conversation history persistence to OPFS.
//!
//! On mount: read `HISTORY_FILE` from OPFS. If present and non-empty,
//! stash the bytes in `App::pending_history` so the next
//! `start_session` seeds the new agent via
//! `GeminiAgentConfig::with_history_bytes`. We also project the
//! history into a flat user/assistant transcript and paint it into
//! `#transcript` so the user actually sees what was restored.
//!
//! After every successful turn: snapshot the agent's history and
//! atomically rewrite `HISTORY_FILE`. Best-effort — failures log to
//! the console but don't bubble up to the UI.

use maud::html;

use crate::backends::gemini::decode_transcript_bytes;
use crate::filesystem::Filesystem;
use crate::types::TranscriptRole;

use super::dom;
use super::templates;
use super::APP;

const HISTORY_FILE: &str = ".lh_history.json";

/// Load history bytes from OPFS into `App::pending_history`. Called
/// once at mount time. If the bytes parse, paints the prior
/// user/assistant turns into `#transcript` so the user can see what
/// the restored session contains — the agent itself isn't built yet
/// (no key applied) but the model's context will match once they send.
pub(crate) async fn load_into_pending() {
    let fs = super::shared_opfs();
    let bytes = match fs.read(HISTORY_FILE).await {
        Ok(b) if !b.is_empty() => b,
        // Empty or missing — fresh session.
        _ => return,
    };

    // Project the bytes into a transcript and paint each entry.
    match decode_transcript_bytes(&bytes) {
        Ok(entries) if !entries.is_empty() => {
            for entry in &entries {
                let turn_id = APP.with(|cell| cell.borrow_mut().alloc_id());
                let role = entry.role.as_str();
                let body = match entry.role {
                    TranscriptRole::User => html! { (entry.text) },
                    TranscriptRole::Assistant => templates::rendered_markdown(&entry.text),
                };
                // `streaming = false` so the replayed turns don't show
                // the "· streaming" suffix.
                let html_str =
                    templates::turn(turn_id, role, body, false).into_string();
                dom::append_html("transcript", &html_str);
            }
            // No status write — restoring the transcript is silent
            // per the minimalism pass; the terminal stays empty until
            // the user actually triggers something.
        }
        Ok(_) => {
            // Empty transcript — bytes existed but no user-visible content.
        }
        Err(err) => {
            // Corrupt bytes — surface but don't crash. The bytes are
            // still stashed for restore; if the model rejects them at
            // session start the user will see the error there.
            web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(&format!(
                "history decode: {err}"
            )));
        }
    }

    APP.with(|cell| cell.borrow_mut().pending_history = Some(bytes));
}

/// Snapshot the agent's history and persist it. Best-effort; logs but
/// doesn't surface errors.
pub(crate) async fn save_from_agent() {
    let bytes = APP.with(|cell| {
        cell.borrow()
            .agent
            .as_ref()
            .and_then(|a| a.history_bytes().ok().flatten())
    });
    let Some(bytes) = bytes else { return };
    let fs = super::shared_opfs();
    if let Err(err) = fs.write_atomic(HISTORY_FILE, &bytes).await {
        web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(&format!(
            "history save: {err}"
        )));
    }
}

/// Take any pending restored history out of the App state. The first
/// `start_session` consumes it; subsequent calls return `None`.
pub(crate) fn take_pending() -> Option<Vec<u8>> {
    APP.with(|cell| cell.borrow_mut().pending_history.take())
}

/// Delete the history file from OPFS. Used by the "reset" action so a
/// new conversation doesn't auto-restore the old one on reload.
pub(crate) async fn clear() {
    let fs = super::shared_opfs();
    let _ = fs.delete(HISTORY_FILE).await;
}
