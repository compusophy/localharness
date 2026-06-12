//! API key persistence to OPFS.
//!
//! Stored next to `.lh_history.json` in the same per-origin OPFS
//! sandbox. Same security model as `sessionStorage`: anything with JS
//! access to this origin can read the file. The win over
//! `sessionStorage` is that it survives a tab close / browser restart.
//!
//! **At rest:** the file is encrypted with the per-origin device key
//! (see [`super::encryption`]) — OPFS holds ciphertext. This does NOT
//! defend against XSS (the page can read the key); it removes the
//! plaintext secret from OPFS for copy/export/disk-inspection channels.
//! Legacy plaintext files are read transparently and re-encrypted on the
//! next save.


const KEY_FILE: &str = ".lh_api_key";

/// Read the persisted Gemini API key, if any. Empty/missing → `None`.
pub(crate) async fn load() -> Option<String> {
    let fs = super::shared_opfs();
    let bytes = fs.read(KEY_FILE).await.ok()?;
    // Decrypt if it's our ciphertext; otherwise it's a legacy plaintext
    // file — use the raw bytes (it re-encrypts on the next save).
    let plain = super::encryption::open(&bytes).await.unwrap_or(bytes);
    let s = String::from_utf8(plain).ok()?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Persist the key (encrypted). Best-effort; logs and swallows errors.
pub(crate) async fn save(key: &str) {
    let fs = super::shared_opfs();
    let data = super::encryption::seal(key.as_bytes())
        .await
        .unwrap_or_else(|| key.as_bytes().to_vec());
    if let Err(err) = fs.write_atomic(KEY_FILE, &data).await {
        web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(&format!(
            "key save: {err}"
        )));
    }
}
