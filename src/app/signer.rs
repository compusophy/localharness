//! Cross-origin signing service hosted at the apex origin.
//!
//! Subdomains can't see apex's wallet (per-origin OPFS). To prove
//! "the visitor controls the on-chain owner address", subdomains
//! create a hidden iframe pointing to `localharness.xyz/?signer=1`,
//! send a `lh-sign-challenge` postMessage, and recover the signer's
//! address from the returned signature. M8 in the design doc.
//!
//! The signer auto-approves all requests for v1 — the trust boundary
//! is "you have JS access to the apex origin, you control the wallet."
//! That's the same boundary as the apex chrome itself; an interactive
//! approval prompt can be layered on later if multi-tenant signing
//! ever matters.
//!
//! **Message protocol:**
//! ```text
//! parent  → signer: { type: "lh-sign-challenge", id, nonce }
//! signer → parent: { type: "lh-sign-response", id, address, signature }
//!                       or: { type: "lh-sign-response", id, error }
//! ```
//! `nonce` is a hex-encoded 32-byte challenge. The signer signs
//! `keccak256("localharness-auth-v0:" || nonce_bytes)` (domain-
//! separated so a captured signature can't be replayed as a real tx).

use sha3::{Digest, Keccak256};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::MessageEvent;

use crate::wallet;

const DOMAIN_TAG: &[u8] = b"localharness-auth-v0:";

/// Install the postMessage listener that turns this tab into a signer
/// service. Called once on apex mount when `?signer=1` is in the URL.
pub(crate) fn install_signer_listener() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;

    let handler = Closure::<dyn FnMut(_)>::new(move |event: MessageEvent| {
        if let Err(err) = handle_message(&event) {
            web_sys::console::warn_1(&JsValue::from_str(&format!("signer: {err}")));
        }
    });
    window.add_event_listener_with_callback("message", handler.as_ref().unchecked_ref())?;
    handler.forget();
    Ok(())
}

fn handle_message(event: &MessageEvent) -> Result<(), String> {
    let data = event.data();
    if data.is_null() || data.is_undefined() {
        return Ok(());
    }

    let msg_type = js_sys::Reflect::get(&data, &JsValue::from_str("type"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    if msg_type != "lh-sign-challenge" {
        // Not for us; ignore silently.
        return Ok(());
    }

    let origin = event.origin();
    if !is_trusted_origin(&origin) {
        return Err(format!("untrusted origin: {origin}"));
    }

    let id = js_sys::Reflect::get(&data, &JsValue::from_str("id"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    let nonce_hex = js_sys::Reflect::get(&data, &JsValue::from_str("nonce"))
        .ok()
        .and_then(|v| v.as_string())
        .ok_or_else(|| "nonce not a string".to_string())?;

    // Reply via the event source (cross-origin), echoing the id so the
    // parent can correlate request to response.
    let source = event
        .source()
        .ok_or_else(|| "no source window on the message event".to_string())?;
    let window: web_sys::Window = source
        .dyn_into()
        .map_err(|_| "source is not a Window".to_string())?;

    let reply = match build_response(&id, &nonce_hex) {
        Ok(obj) => obj,
        Err(err) => error_response(&id, &err),
    };
    window
        .post_message(&reply, &origin)
        .map_err(|e| format!("post_message: {e:?}"))?;
    Ok(())
}

fn build_response(id: &str, nonce_hex: &str) -> Result<JsValue, String> {
    let nonce = parse_nonce(nonce_hex)?;
    // Domain-separated digest the signer commits to.
    let mut hasher = Keccak256::new();
    hasher.update(DOMAIN_TAG);
    hasher.update(&nonce);
    let mut prehash = [0u8; 32];
    prehash.copy_from_slice(&hasher.finalize());

    // Pull the wallet out of App state. paint_apex caches it on mount.
    let (signer, address) = super::APP
        .with(|cell| cell.borrow().wallet.as_ref().map(|w| (w.signer.clone(), w.address)))
        .ok_or_else(|| "wallet not loaded yet — refresh and retry".to_string())?;

    let signature = wallet::sign_hash(&signer, &prehash);

    let obj = js_sys::Object::new();
    set(&obj, "type", JsValue::from_str("lh-sign-response"));
    set(&obj, "id", JsValue::from_str(id));
    set(&obj, "address", JsValue::from_str(&hex_addr(&address)));
    set(&obj, "signature", JsValue::from_str(&hex_bytes(&signature)));
    Ok(JsValue::from(obj))
}

fn error_response(id: &str, err: &str) -> JsValue {
    let obj = js_sys::Object::new();
    set(&obj, "type", JsValue::from_str("lh-sign-response"));
    set(&obj, "id", JsValue::from_str(id));
    set(&obj, "error", JsValue::from_str(err));
    JsValue::from(obj)
}

fn set(obj: &js_sys::Object, key: &str, value: JsValue) {
    let _ = js_sys::Reflect::set(obj, &JsValue::from_str(key), &value);
}

/// Accept requests only from origins we control (apex + any subdomain).
/// `localhost` is allowed too so the local-server smoke flow works.
fn is_trusted_origin(origin: &str) -> bool {
    let stripped = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .unwrap_or(origin);
    let host = stripped.split(':').next().unwrap_or(stripped);
    host == "localharness.xyz"
        || host.ends_with(".localharness.xyz")
        || host == "localhost"
        || host.ends_with(".localhost")
}

fn parse_nonce(hex: &str) -> Result<Vec<u8>, String> {
    let trimmed = hex.trim().trim_start_matches("0x").trim_start_matches("0X");
    if trimmed.len() % 2 != 0 {
        return Err("nonce hex odd length".into());
    }
    let mut out = Vec::with_capacity(trimmed.len() / 2);
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = nibble(bytes[i])?;
        let lo = nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn nibble(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(format!("non-hex byte {b}")),
    }
}

fn hex_addr(addr: &[u8; 20]) -> String {
    let mut s = String::with_capacity(42);
    s.push_str("0x");
    for b in addr {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
