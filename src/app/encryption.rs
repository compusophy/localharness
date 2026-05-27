//! At-rest encryption for OPFS contents.
//!
//! Derives an AES-256-GCM key from the wallet's private key via
//! `keccak256(privkey || "localharness-opfs-encrypt-v1")`. Sensitive
//! files (API key, wallet seed, conversation history) are encrypted
//! before writing and decrypted on read.
//!
//! Format: `[12 bytes IV][ciphertext + 16 bytes GCM tag]`.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

const DOMAIN_TAG: &[u8] = b"localharness-opfs-encrypt-v1";
const IV_LEN: usize = 12;

/// Derive an AES-256-GCM CryptoKey from wallet private key bytes.
pub(crate) async fn derive_key(privkey_bytes: &[u8; 32]) -> Result<web_sys::CryptoKey, String> {
    use sha3::{Digest, Keccak256};

    let mut hasher = Keccak256::new();
    hasher.update(privkey_bytes);
    hasher.update(DOMAIN_TAG);
    let key_material = hasher.finalize();

    let window = web_sys::window().ok_or("no window")?;
    let crypto = window.crypto().map_err(|_| "no crypto")?;
    let subtle = crypto.subtle();

    let key_data = js_sys::Uint8Array::from(&key_material[..]);
    let algo = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&algo, &JsValue::from_str("name"), &JsValue::from_str("AES-GCM"));

    let usages = js_sys::Array::new();
    usages.push(&JsValue::from_str("encrypt"));
    usages.push(&JsValue::from_str("decrypt"));

    let promise = subtle.import_key_with_object(
        "raw",
        &key_data.buffer(),
        &algo,
        false,
        &usages,
    ).map_err(|e| format!("importKey: {e:?}"))?;

    let result = JsFuture::from(promise)
        .await
        .map_err(|e| format!("importKey await: {e:?}"))?;

    result.dyn_into::<web_sys::CryptoKey>()
        .map_err(|_| "importKey did not return CryptoKey".into())
}

/// Encrypt plaintext bytes. Returns `IV || ciphertext || tag`.
pub(crate) async fn encrypt(key: &web_sys::CryptoKey, plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let crypto = window.crypto().map_err(|_| "no crypto")?;
    let subtle = crypto.subtle();

    let mut iv_bytes = [0u8; IV_LEN];
    let iv_view = unsafe { js_sys::Uint8Array::view(&mut iv_bytes) };
    crypto.get_random_values_with_array_buffer_view(&iv_view)
        .map_err(|_| "getRandomValues failed")?;

    let algo = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&algo, &JsValue::from_str("name"), &JsValue::from_str("AES-GCM"));
    let _ = js_sys::Reflect::set(&algo, &JsValue::from_str("iv"), &iv_view);

    let mut data = plaintext.to_vec();
    let promise = subtle.encrypt_with_object_and_u8_array(
        &algo, key, &mut data,
    ).map_err(|e| format!("encrypt: {e:?}"))?;

    let result = JsFuture::from(promise)
        .await
        .map_err(|e| format!("encrypt await: {e:?}"))?;

    let ciphertext = js_sys::Uint8Array::new(&result);
    let ct_bytes = ciphertext.to_vec();

    let mut out = Vec::with_capacity(IV_LEN + ct_bytes.len());
    out.extend_from_slice(&iv_bytes);
    out.extend_from_slice(&ct_bytes);
    Ok(out)
}

/// Decrypt `IV || ciphertext || tag` back to plaintext.
pub(crate) async fn decrypt(key: &web_sys::CryptoKey, encrypted: &[u8]) -> Result<Vec<u8>, String> {
    if encrypted.len() < IV_LEN + 16 {
        return Err("ciphertext too short".into());
    }

    let window = web_sys::window().ok_or("no window")?;
    let crypto = window.crypto().map_err(|_| "no crypto")?;
    let subtle = crypto.subtle();

    let iv = js_sys::Uint8Array::from(&encrypted[..IV_LEN]);

    let algo = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&algo, &JsValue::from_str("name"), &JsValue::from_str("AES-GCM"));
    let _ = js_sys::Reflect::set(&algo, &JsValue::from_str("iv"), &iv);

    let mut ct = encrypted[IV_LEN..].to_vec();
    let promise = subtle.decrypt_with_object_and_u8_array(
        &algo, key, &mut ct,
    ).map_err(|e| format!("decrypt: {e:?}"))?;

    let result = JsFuture::from(promise)
        .await
        .map_err(|e| format!("decrypt await: {e:?}"))?;

    let plaintext = js_sys::Uint8Array::new(&result);
    Ok(plaintext.to_vec())
}
