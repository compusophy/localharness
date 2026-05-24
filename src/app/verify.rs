//! Subdomain-side companion to `signer.rs`.
//!
//! Embeds `https://localharness.xyz/?signer=1` in a hidden iframe,
//! sends a random nonce via postMessage, awaits the signed response,
//! recovers the signing address, and compares it against the on-chain
//! owner of this subdomain's name (from `registry::owner_of_name`).
//!
//! Returns:
//! - `Ok(VerifyResult::VerifiedOwner { address })` — visitor is the
//!   on-chain owner; unlock full UX.
//! - `Ok(VerifyResult::Visitor { owner_address })` — visitor signed
//!   with a different address; read-only mode.
//! - `Ok(VerifyResult::Unregistered)` — name has no on-chain owner
//!   yet; treat as a fresh subdomain.
//! - `Err(msg)` — RPC failed, iframe failed, signer didn't respond.
//!   Callers fall back to the legacy local-OPFS UUID model so the app
//!   keeps working even when the verification chain has a hiccup.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{HtmlIFrameElement, MessageEvent};

use crate::wallet;

const SIGNER_URL: &str = "https://localharness.xyz/?signer=1";
const SIGNER_ORIGIN: &str = "https://localharness.xyz";
/// How long to wait for the signer to reply before giving up.
const TIMEOUT_MS: u32 = 5_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VerifyResult {
    VerifiedOwner { address: String },
    Visitor { owner_address: String },
    Unregistered,
}

/// Run the full verify flow for `name`. Idempotent and side-effect
/// free apart from creating + removing a hidden iframe.
pub(crate) async fn verify_owner(name: &str) -> Result<VerifyResult, String> {
    // 1. Who owns this name on-chain?
    let on_chain_owner = super::registry::owner_of_name(name).await?;
    let Some(expected) = on_chain_owner else {
        return Ok(VerifyResult::Unregistered);
    };

    // 2. Get a signature from the apex signer.
    let nonce = random_nonce();
    let nonce_hex = bytes_to_hex(&nonce);
    let (signer_address, signature) = sign_via_iframe(&nonce_hex).await?;

    // 3. Recover the signer address from the signature and verify it
    //    matches what the signer claimed (basic sanity check).
    let prehash = {
        use sha3::{Digest, Keccak256};
        let mut hasher = Keccak256::new();
        hasher.update(b"localharness-auth-v0:");
        hasher.update(&nonce);
        let mut out = [0u8; 32];
        out.copy_from_slice(&hasher.finalize());
        out
    };
    let sig_bytes = hex_to_bytes(&signature)?;
    if sig_bytes.len() != 65 {
        return Err(format!("bad signature length {}", sig_bytes.len()));
    }
    let mut sig_arr = [0u8; 65];
    sig_arr.copy_from_slice(&sig_bytes);
    let recovered = wallet::recover_address(&sig_arr, &prehash).map_err(|e| e)?;
    let recovered_hex = format!("0x{}", bytes_to_hex(&recovered));

    if recovered_hex.to_lowercase() != signer_address.to_lowercase() {
        return Err(format!(
            "signature claimed address {signer_address} but recovered {recovered_hex}"
        ));
    }

    // 4. Compare against on-chain owner. Case-insensitive — addresses
    //    can come back checksummed-cased or all-lowercase from the RPC.
    if recovered_hex.to_lowercase() == expected.to_lowercase() {
        Ok(VerifyResult::VerifiedOwner {
            address: recovered_hex,
        })
    } else {
        Ok(VerifyResult::Visitor {
            owner_address: expected,
        })
    }
}

/// Embed the signer iframe, send a sign challenge, wait for the
/// reply. Cleans up the iframe before returning.
async fn sign_via_iframe(nonce_hex: &str) -> Result<(String, String), String> {
    let doc = super::dom::document().map_err(|e| format!("document: {e:?}"))?;
    let body = doc.body().ok_or_else(|| "no body".to_string())?;

    let iframe: HtmlIFrameElement = doc
        .create_element("iframe")
        .map_err(|e| format!("iframe: {e:?}"))?
        .dyn_into()
        .map_err(|_| "not an iframe".to_string())?;
    iframe.set_src(SIGNER_URL);
    let _ = iframe.set_attribute(
        "style",
        "display:none;width:0;height:0;border:0;position:absolute;",
    );
    body.append_child(&iframe)
        .map_err(|e| format!("append iframe: {e:?}"))?;

    // Wire up the listener BEFORE posting (in case the signer is fast
    // enough to reply between append and listener install — unlikely
    // but the cost is one extra await tick at worst).
    let id = format!("verify-{}", random_id_hex());
    let result_slot: Rc<RefCell<Option<Result<(String, String), String>>>> =
        Rc::new(RefCell::new(None));
    let waker_slot: Rc<RefCell<Option<js_sys::Function>>> = Rc::new(RefCell::new(None));

    let result_for_handler = result_slot.clone();
    let waker_for_handler = waker_slot.clone();
    let id_for_handler = id.clone();
    let handler = Closure::<dyn FnMut(_)>::new(move |event: MessageEvent| {
        let data = event.data();
        if data.is_null() || data.is_undefined() {
            return;
        }
        // Only respond to messages from the apex origin.
        if event.origin() != SIGNER_ORIGIN {
            return;
        }
        let msg_type = js_sys::Reflect::get(&data, &JsValue::from_str("type"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        if msg_type != "lh-sign-response" {
            return;
        }
        let id_match = js_sys::Reflect::get(&data, &JsValue::from_str("id"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        if id_match != id_for_handler {
            return;
        }
        let outcome = if let Some(err) = js_sys::Reflect::get(&data, &JsValue::from_str("error"))
            .ok()
            .and_then(|v| v.as_string())
        {
            Err(format!("signer: {err}"))
        } else {
            let address = js_sys::Reflect::get(&data, &JsValue::from_str("address"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            let signature = js_sys::Reflect::get(&data, &JsValue::from_str("signature"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            if address.is_empty() || signature.is_empty() {
                Err("signer reply missing address or signature".into())
            } else {
                Ok((address, signature))
            }
        };
        *result_for_handler.borrow_mut() = Some(outcome);
        if let Some(waker) = waker_for_handler.borrow_mut().take() {
            let _ = waker.call0(&JsValue::NULL);
        }
    });
    let window = super::dom::window().map_err(|e| format!("window: {e:?}"))?;
    window
        .add_event_listener_with_callback("message", handler.as_ref().unchecked_ref())
        .map_err(|e| format!("add listener: {e:?}"))?;

    // Wait for the iframe to load. content_window is None until the
    // iframe has navigated — poll briefly. Most browsers expose the
    // window immediately for cross-origin iframes, so this is cheap.
    let mut content_window: Option<web_sys::Window> = None;
    for _ in 0..50 {
        if let Some(w) = iframe.content_window() {
            content_window = Some(w);
            break;
        }
        sleep_ms(50).await;
    }
    let target = content_window.ok_or_else(|| "iframe content window never available".to_string())?;

    // Build the request payload.
    let payload = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("type"),
        &JsValue::from_str("lh-sign-challenge"),
    );
    let _ = js_sys::Reflect::set(&payload, &JsValue::from_str("id"), &JsValue::from_str(&id));
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("nonce"),
        &JsValue::from_str(nonce_hex),
    );

    // Give the signer a moment to register its listener after load.
    sleep_ms(200).await;
    target
        .post_message(&payload, SIGNER_ORIGIN)
        .map_err(|e| format!("postMessage: {e:?}"))?;

    // Race the reply against a timeout.
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let resolve_clone = resolve.clone();
        *waker_slot.borrow_mut() = Some(resolve_clone);
        // Timeout wakes the same resolver; the outcome slot stays None
        // and we error below.
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                &resolve,
                TIMEOUT_MS as i32,
            );
        }
    });
    let _ = JsFuture::from(promise).await;

    // Clean up the iframe + listener.
    let _ = window.remove_event_listener_with_callback(
        "message",
        handler.as_ref().unchecked_ref(),
    );
    drop(handler);
    let _ = body.remove_child(&iframe);

    result_slot
        .borrow_mut()
        .take()
        .unwrap_or_else(|| Err("signer did not respond within timeout".into()))
}

fn random_nonce() -> [u8; 32] {
    use rand_core::RngCore;
    let mut bytes = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut bytes);
    bytes
}

fn random_id_hex() -> String {
    use rand_core::RngCore;
    let mut bytes = [0u8; 8];
    rand_core::OsRng.fill_bytes(&mut bytes);
    bytes_to_hex(&bytes)
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_to_bytes(hex: &str) -> Result<Vec<u8>, String> {
    let trimmed = hex.trim().trim_start_matches("0x").trim_start_matches("0X");
    if trimmed.len() % 2 != 0 {
        return Err("hex odd length".into());
    }
    let mut out = Vec::with_capacity(trimmed.len() / 2);
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = match bytes[i] {
            b'0'..=b'9' => bytes[i] - b'0',
            b'a'..=b'f' => bytes[i] - b'a' + 10,
            b'A'..=b'F' => bytes[i] - b'A' + 10,
            _ => return Err(format!("non-hex byte {}", bytes[i])),
        };
        let lo = match bytes[i + 1] {
            b'0'..=b'9' => bytes[i + 1] - b'0',
            b'a'..=b'f' => bytes[i + 1] - b'a' + 10,
            b'A'..=b'F' => bytes[i + 1] - b'A' + 10,
            _ => return Err(format!("non-hex byte {}", bytes[i + 1])),
        };
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

async fn sleep_ms(ms: u32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                &resolve,
                ms as i32,
            );
        }
    });
    let _ = JsFuture::from(promise).await;
}
