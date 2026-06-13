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


const MODEL_FILE: &str = ".lh_model";

/// Default model id when none is persisted — the platform's Gemini default.
/// Aliases the crate-canonical [`crate::types::DEFAULT_MODEL`] so a model-id
/// flip in ONE place propagates here (no re-typed literal to drift).
pub(crate) const DEFAULT_MODEL: &str = crate::types::DEFAULT_MODEL;

/// The selectable models, as `(id, label)` pairs. Drives the admin
/// selector template AND is the allowlist [`save`] validates against, so a
/// stale/garbage `.lh_model` can never route to an unknown model.
///
/// The ids REFERENCE the canonical backend constants rather than re-typing
/// literals — a rename in `types`/`anthropic::wire` auto-propagates here, so
/// the selector can never advertise a dead id (the model-id-flip drift trap;
/// browser-app always pulls the `anthropic` feature, so the consts resolve).
/// `gemma-3-270m` stays a literal (the `local` feature/backend isn't always
/// present to const-reference); `gpt-*` is intentionally absent until the
/// OpenAI selector path is wired (proxy `OPENAI_API_KEY`).
pub(crate) const MODELS: &[(&str, &str)] = &[
    (crate::types::DEFAULT_MODEL, "Gemini"),
    (crate::backends::anthropic::DEFAULT_MODEL, "Claude Haiku"),
    (crate::backends::anthropic::SONNET_MODEL, "Claude Sonnet"),
    (crate::backends::anthropic::OPUS_MODEL, "Claude Opus"),
    ("gemma-3-270m", "Local (Gemma)"),
];

/// True for a Claude/Anthropic model id (`claude-*`). Everything else is
/// treated as a Gemini id by [`super::chat::start_session`].
pub(crate) fn is_anthropic(model: &str) -> bool {
    model.starts_with("claude-")
}

/// True for the in-browser local model id (`gemma-*`). Routes to the local
/// (Burn-wgpu) backend rather than the credit proxy / a network API.
pub(crate) fn is_local(model: &str) -> bool {
    model.starts_with("gemma-")
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
