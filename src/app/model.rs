//! Per-tenant LLM model selection — which backend the in-tab agent uses.
//!
//! The choice is a single model ID persisted to `.lh_model` in this
//! origin's OPFS root (same pattern as [`super::key_store`] /
//! `.lh_system_prompt.txt`), read on session start by
//! [`super::chat::start_session`]. A `gemini-*` id routes to the Gemini
//! backend; a `claude-*` id routes to the Anthropic backend. Both reach
//! the model through the credit proxy in credits mode (the proxy is
//! multi-provider) and BYOK still works for Gemini.
//!
//! Unlike the encrypted `.lh_api_key`, the model id is not a secret, so
//! it's stored as plaintext UTF-8.

use crate::filesystem::Filesystem;

const MODEL_FILE: &str = ".lh_model";

/// Default model id when none is persisted — the Gemini flash model the
/// platform has always defaulted to.
pub(crate) const DEFAULT_MODEL: &str = "gemini-3.5-flash";

/// The selectable models, as `(id, label)` pairs. Drives the admin
/// selector template AND is the allowlist [`save`] validates against, so a
/// stale/garbage `.lh_model` can never route to an unknown model.
pub(crate) const MODELS: &[(&str, &str)] = &[
    ("gemini-3.5-flash", "Gemini"),
    ("claude-haiku-4-5-20251001", "Claude Haiku"),
    ("claude-sonnet-4-6", "Claude Sonnet"),
    ("claude-opus-4-8", "Claude Opus"),
];

/// True for a Claude/Anthropic model id (`claude-*`). Everything else is
/// treated as a Gemini id by [`super::chat::start_session`].
pub(crate) fn is_anthropic(model: &str) -> bool {
    model.starts_with("claude-")
}

/// Read the persisted model id, validated against [`MODELS`]. A missing,
/// empty, or unrecognised file falls back to [`DEFAULT_MODEL`] — the
/// selector is never left pointing at a model the bundle can't route.
pub(crate) async fn load() -> String {
    let fs = super::shared_opfs();
    let chosen = fs
        .read(MODEL_FILE)
        .await
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if MODELS.iter().any(|(id, _)| *id == chosen) {
        chosen
    } else {
        DEFAULT_MODEL.to_string()
    }
}

/// Persist `model` as the new selection. Rejects an id not in [`MODELS`]
/// so the file can only ever hold a routable model. Best-effort write.
pub(crate) async fn save(model: &str) {
    if !MODELS.iter().any(|(id, _)| *id == model) {
        return;
    }
    let fs = super::shared_opfs();
    if let Err(err) = fs.write_atomic(MODEL_FILE, model.as_bytes()).await {
        web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(&format!(
            "model save: {err}"
        )));
    }
}
