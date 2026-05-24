//! Cross-origin signing service hosted at the apex origin.
//!
//! Subdomains can't see apex's wallet (per-origin OPFS). To prove
//! "the visitor controls the on-chain owner address", subdomains
//! create a hidden iframe pointing to `localharness.xyz/?signer=1`,
//! send a `lh-sign-challenge` postMessage, and recover the signer's
//! address from the returned signature. M8 in the design doc.
//!
//! Challenge-signing auto-approves for v1 (verification is read-only,
//! the trust boundary is "you have JS access to the apex origin").
//! Transaction-signing **always asks consent** via a synchronous
//! `window.confirm()` dialog — txs move real value and we don't want
//! a captured-from-XSS subdomain draining the wallet without the user
//! seeing it.
//!
//! **Message protocol:**
//! ```text
//! Verification (auto-approved):
//!   parent  → signer: { type: "lh-sign-challenge", id, nonce }
//!   signer → parent:  { type: "lh-sign-response",  id, address, signature }
//!                or:  { type: "lh-sign-response",  id, error }
//!
//! Payments (consent-prompted):
//!   parent  → signer: { type: "lh-sign-tx", id, tx: {
//!                         to, value, nonce, gas, gasPrice, chainId
//!                       }, purpose }
//!   signer → parent:  { type: "lh-sign-response", id, address, raw_tx_hex }
//!                or:  { type: "lh-sign-response", id, error }
//! ```
//! `nonce` (challenge) is a hex-encoded 32-byte challenge. The signer
//! signs `keccak256("localharness-auth-v0:" || nonce_bytes)` (domain-
//! separated so a captured signature can't be replayed as a real tx).
//!
//! Tx fields are hex-encoded (`0x...`) except `nonce` / `gas` which can
//! be either a hex string or a JS number. `chainId` must match
//! [`crate::registry::CHAIN_ID`] (42431); otherwise the signer rejects
//! to avoid a replay-on-a-different-chain footgun. `purpose` is the
//! human-readable description shown in the consent dialog.

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

    let origin = event.origin();
    if !is_trusted_origin(&origin) {
        // Drop silently on truly unknown message types; only error on
        // recognised types from untrusted origins so we don't log
        // noise from unrelated postMessage chatter.
        if matches!(msg_type.as_str(), "lh-sign-challenge" | "lh-sign-tx") {
            return Err(format!("untrusted origin: {origin}"));
        }
        return Ok(());
    }

    let id = js_sys::Reflect::get(&data, &JsValue::from_str("id"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();

    let source = event
        .source()
        .ok_or_else(|| "no source window on the message event".to_string())?;
    let window: web_sys::Window = source
        .dyn_into()
        .map_err(|_| "source is not a Window".to_string())?;

    let reply = match msg_type.as_str() {
        "lh-sign-challenge" => {
            let nonce_hex = js_sys::Reflect::get(&data, &JsValue::from_str("nonce"))
                .ok()
                .and_then(|v| v.as_string())
                .ok_or_else(|| "nonce not a string".to_string())?;
            match build_challenge_response(&id, &nonce_hex) {
                Ok(obj) => obj,
                Err(err) => error_response(&id, &err),
            }
        }
        "lh-sign-tx" => {
            let purpose = js_sys::Reflect::get(&data, &JsValue::from_str("purpose"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_else(|| "sign transaction".into());
            let tx = js_sys::Reflect::get(&data, &JsValue::from_str("tx"))
                .map_err(|_| "tx field missing".to_string())?;
            match build_tx_response(&id, &tx, &purpose) {
                Ok(obj) => obj,
                Err(err) => error_response(&id, &err),
            }
        }
        _ => return Ok(()), // not for us
    };

    window
        .post_message(&reply, &origin)
        .map_err(|e| format!("post_message: {e:?}"))?;
    Ok(())
}

fn build_challenge_response(id: &str, nonce_hex: &str) -> Result<JsValue, String> {
    let nonce = parse_nonce(nonce_hex)?;
    // Domain-separated digest the signer commits to.
    let mut hasher = Keccak256::new();
    hasher.update(DOMAIN_TAG);
    hasher.update(&nonce);
    let mut prehash = [0u8; 32];
    prehash.copy_from_slice(&hasher.finalize());

    let (signer, address) = wallet_handle()?;
    let signature = wallet::sign_hash(&signer, &prehash);

    let obj = js_sys::Object::new();
    set(&obj, "type", JsValue::from_str("lh-sign-response"));
    set(&obj, "id", JsValue::from_str(id));
    set(&obj, "address", JsValue::from_str(&hex_addr(&address)));
    set(&obj, "signature", JsValue::from_str(&hex_bytes(&signature)));
    Ok(JsValue::from(obj))
}

/// Sign an EIP-155 native-ETH transfer after explicit user consent.
/// The consent dialog spells out the recipient, value (in test ETH),
/// and the human-readable purpose so the user knows what they're
/// authorizing — postMessage from a compromised subdomain can't move
/// funds without this.
fn build_tx_response(id: &str, tx: &JsValue, purpose: &str) -> Result<JsValue, String> {
    let to_hex = field_string(tx, "to")?;
    let value_wei = field_u128(tx, "value")?;
    let nonce = field_u128(tx, "nonce")?;
    let gas_limit = field_u128(tx, "gas")?;
    let gas_price = field_u128(tx, "gasPrice")?;
    let chain_id = field_u128(tx, "chainId")?;

    if chain_id != crate::registry::CHAIN_ID as u128 {
        return Err(format!(
            "chainId mismatch: tx wants {chain_id}, signer is locked to {}",
            crate::registry::CHAIN_ID
        ));
    }
    if !is_address_shape(&to_hex) {
        return Err(format!("`to` doesn't look like a 20-byte address: {to_hex}"));
    }

    let (signer, address) = wallet_handle()?;
    let from_hex = hex_addr(&address);

    let prompt = format!(
        "Sign transaction?\n\n\
         purpose: {purpose}\n\
         from:    {from_hex}\n\
         to:      {to_hex}\n\
         value:   {} test ETH ({value_wei} wei)\n\
         gas:     {gas_limit} @ {gas_price} wei\n\
         chain:   {chain_id}\n\
         nonce:   {nonce}",
        super::format_wei_as_test_eth(value_wei),
    );
    let consent = web_sys::window()
        .and_then(|w| w.confirm_with_message(&prompt).ok())
        .unwrap_or(false);
    if !consent {
        return Err("user denied signing".into());
    }

    let unsigned = crate::registry::rlp_native_transfer_unsigned(
        &to_hex, value_wei, nonce, gas_price, gas_limit,
    )?;
    let mut hasher = Keccak256::new();
    hasher.update(&unsigned);
    let mut prehash = [0u8; 32];
    prehash.copy_from_slice(&hasher.finalize());
    let sig = wallet::sign_hash(&signer, &prehash);

    let raw_hex = crate::registry::rlp_native_transfer_signed(
        &to_hex, value_wei, nonce, gas_price, gas_limit, &sig,
    )?;

    let obj = js_sys::Object::new();
    set(&obj, "type", JsValue::from_str("lh-sign-response"));
    set(&obj, "id", JsValue::from_str(id));
    set(&obj, "address", JsValue::from_str(&from_hex));
    set(&obj, "raw_tx_hex", JsValue::from_str(&raw_hex));
    Ok(JsValue::from(obj))
}

fn wallet_handle() -> Result<(k256::ecdsa::SigningKey, [u8; 20]), String> {
    super::APP
        .with(|cell| {
            cell.borrow()
                .wallet
                .as_ref()
                .map(|w| (w.signer.clone(), w.address))
        })
        .ok_or_else(|| "no identity on this device — create one at the apex".to_string())
}

fn field_string(obj: &JsValue, key: &str) -> Result<String, String> {
    js_sys::Reflect::get(obj, &JsValue::from_str(key))
        .ok()
        .and_then(|v| v.as_string())
        .ok_or_else(|| format!("tx.{key} not a string"))
}

/// Read a numeric tx field that may arrive as either a hex string
/// (`"0x..."`) or a JS number. JSON-from-JS objects often serialize
/// small ints as numbers; hex strings are the canonical eth-RPC form.
fn field_u128(obj: &JsValue, key: &str) -> Result<u128, String> {
    let raw = js_sys::Reflect::get(obj, &JsValue::from_str(key))
        .map_err(|_| format!("tx.{key} missing"))?;
    if let Some(n) = raw.as_f64() {
        if n.is_finite() && n >= 0.0 {
            return Ok(n as u128);
        }
        return Err(format!("tx.{key} non-finite or negative: {n}"));
    }
    if let Some(s) = raw.as_string() {
        let trimmed = s.trim_start_matches("0x").trim_start_matches("0X");
        if trimmed.is_empty() {
            return Ok(0);
        }
        return u128::from_str_radix(trimmed, 16)
            .map_err(|e| format!("tx.{key} bad hex: {e}"));
    }
    Err(format!("tx.{key} must be hex string or number"))
}

fn is_address_shape(hex: &str) -> bool {
    let stripped = hex.trim_start_matches("0x").trim_start_matches("0X");
    stripped.len() == 40 && stripped.bytes().all(|b| b.is_ascii_hexdigit())
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
