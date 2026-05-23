//! API key persistence to OPFS.
//!
//! Stored next to `.lh_history.json` in the same per-origin OPFS
//! sandbox. Same security model as `sessionStorage`: anything with JS
//! access to this origin can read the file. The win over
//! `sessionStorage` is that it survives a tab close / browser restart.
//!
//! **Threat model considered:** this is no worse than session/local
//! storage (XSS-equivalent risk). It's NOT encryption at rest — the
//! key sits in plaintext bytes. Per-origin sandboxing is the only
//! protection. If untrusted JS is ever loaded into this origin, the
//! key is exposed.

use crate::filesystem::Filesystem;

const KEY_FILE: &str = ".lh_api_key";

/// Read the persisted Gemini API key, if any. Empty/missing → `None`.
pub(crate) async fn load() -> Option<String> {
    let fs = super::shared_opfs();
    let bytes = fs.read(KEY_FILE).await.ok()?;
    let s = String::from_utf8(bytes).ok()?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Persist the key. Best-effort; logs and swallows errors.
pub(crate) async fn save(key: &str) {
    let fs = super::shared_opfs();
    if let Err(err) = fs.write_atomic(KEY_FILE, key.as_bytes()).await {
        web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(&format!(
            "key save: {err}"
        )));
    }
}

/// Delete the persisted key (the "clear" button).
pub(crate) async fn clear() {
    let fs = super::shared_opfs();
    let _ = fs.delete(KEY_FILE).await;
}
