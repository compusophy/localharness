//! Conversation history persistence to OPFS.
//!
//! On mount: read `HISTORY_FILE` from OPFS. If present and non-empty,
//! stash the bytes in `App::pending_history` so the next
//! `start_session` seeds the new agent via
//! `GeminiAgentConfig::with_history_bytes`.
//!
//! After every successful turn: snapshot the agent's history and
//! atomically rewrite `HISTORY_FILE`. Best-effort — failures log to
//! the console but don't bubble up to the UI.

use crate::filesystem::Filesystem;

use super::APP;

const HISTORY_FILE: &str = ".lh_history.json";

/// Load history bytes from OPFS into `App::pending_history`. Called
/// once at mount time.
pub(crate) async fn load_into_pending() {
    let fs = super::shared_opfs();
    let bytes = match fs.read(HISTORY_FILE).await {
        Ok(b) if !b.is_empty() => b,
        // Empty or missing — fresh session.
        _ => return,
    };
    APP.with(|cell| cell.borrow_mut().pending_history = Some(bytes));
    super::dom::set_status("ready · restored prior session", false);
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
