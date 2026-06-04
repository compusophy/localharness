//! At-rest encryption for sensitive OPFS files (the Gemini API key and
//! conversation history).
//!
//! ## Threat model — read this before assuming too much
//! The encryption key is a per-origin random AES-256-GCM key kept in
//! **localStorage**, a separate store from OPFS. So a copy of the OPFS
//! files *alone* (a future export feature, some extension scopes, casual
//! inspection) yields only ciphertext. It does **NOT** defend against
//! XSS / untrusted JS in the origin — the page can read the localStorage
//! key, so any code running here can decrypt. (The active XSS path was
//! closed separately in the security audit.) It's defense-in-depth that
//! removes plaintext secrets from OPFS at rest, nothing more.
//!
//! The **wallet seed is deliberately NOT encrypted here**: losing the
//! localStorage key would then risk locking the user out of their
//! identity. Seed protection needs its own recovery design.
//!
//! Format: `[12-byte IV][ciphertext + 16-byte GCM tag]`.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

const IV_LEN: usize = 12;
/// localStorage slot holding the hex-encoded per-origin AES key.
const STORAGE_KEY: &str = "lh_enc_key_v1";

/// Encrypt bytes for at-rest storage with the per-origin device key.
/// Returns `None` on any failure so the caller can fall back to writing
/// plaintext rather than losing data.
pub(crate) async fn seal(plaintext: &[u8]) -> Option<Vec<u8>> {
    let key = device_key().await.ok()?;
    encrypt(&key, plaintext).await.ok()
}

/// Decrypt at-rest bytes. Returns `None` if they aren't our ciphertext
/// (legacy plaintext, or a wrong/lost key) — the caller treats `None`
/// as "use the raw bytes / re-prompt", which also auto-migrates old
/// plaintext files (they re-encrypt on the next write).
pub(crate) async fn open(data: &[u8]) -> Option<Vec<u8>> {
    let key = device_key().await.ok()?;
    decrypt(&key, data).await.ok()
}

/// Derive the 32-byte AES key that seals/opens the on-chain Gemini key,
/// from a master wallet's BIP-39 entropy. Deterministic from the seed, so
/// any device holding it derives the same key. SHARED source of truth for
/// both the apex signer iframe (`signer::seed_sync_key`) and the
/// local-first path in `verify.rs` (a subdomain that pulled the seed in
/// via `seed_pull`) — they MUST agree byte-for-byte, hence one impl here.
pub(crate) fn keysync_key_from_entropy(entropy: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let mut hasher = Keccak256::new();
    hasher.update(b"localharness/v0/keysync");
    hasher.update(entropy);
    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    out
}

/// Seal with an explicit 32-byte key (e.g. a wallet-seed-derived key)
/// rather than the per-origin device key. Used by the on-chain API-key
/// sync flow so the ciphertext follows the seed, not the device.
pub(crate) async fn seal_with_raw_key(raw: &[u8; 32], plaintext: &[u8]) -> Option<Vec<u8>> {
    let key = import_aes_key(raw).await.ok()?;
    encrypt(&key, plaintext).await.ok()
}

/// Open bytes sealed by [`seal_with_raw_key`] with the same 32-byte key.
pub(crate) async fn open_with_raw_key(raw: &[u8; 32], data: &[u8]) -> Option<Vec<u8>> {
    let key = import_aes_key(raw).await.ok()?;
    decrypt(&key, data).await.ok()
}

/// ECIES seal: wrap `plaintext` to a recipient's compressed SEC1 public
/// key. Output is `ephemeral_pubkey(33) || AES-GCM(IV||ct||tag)`. The
/// recipient recovers the AES key via ECDH(their_priv, ephemeral_pub),
/// so the plaintext is readable ONLY by the holder of that device key —
/// never the desktop's seed. Used to hand a freshly-paired phone the
/// Gemini key without it ever seeing the master seed.
pub(crate) async fn ecies_seal(
    recipient_pubkey_sec1: &[u8],
    plaintext: &[u8],
) -> Option<Vec<u8>> {
    let (eph_pub, eph_signer) = crate::wallet::ephemeral_keypair();
    let key = crate::wallet::ecdh_shared_key(&eph_signer, recipient_pubkey_sec1).ok()?;
    let blob = seal_with_raw_key(&key, plaintext).await?;
    let mut out = Vec::with_capacity(eph_pub.len() + blob.len());
    out.extend_from_slice(&eph_pub);
    out.extend_from_slice(&blob);
    Some(out)
}

/// ECIES open: recover the plaintext sealed by [`ecies_seal`] using the
/// recipient's device signing key. Expects `ephemeral_pub(33) || aes_blob`.
pub(crate) async fn ecies_open(
    device: &k256::ecdsa::SigningKey,
    data: &[u8],
) -> Option<Vec<u8>> {
    // 33 (ephemeral compressed pubkey) + 12 (IV) + 16 (GCM tag) minimum.
    if data.len() < 33 + IV_LEN + 16 {
        return None;
    }
    let (eph_pub, blob) = data.split_at(33);
    let key = crate::wallet::ecdh_shared_key(device, eph_pub).ok()?;
    open_with_raw_key(&key, blob).await
}

/// Load (or generate + persist) the per-origin AES key and import it as
/// a non-extractable WebCrypto `CryptoKey`.
async fn device_key() -> Result<web_sys::CryptoKey, String> {
    let raw = load_or_create_key_bytes()?;
    import_aes_key(&raw).await
}

fn load_or_create_key_bytes() -> Result<[u8; 32], String> {
    let window = web_sys::window().ok_or("no window")?;
    let storage = window
        .local_storage()
        .map_err(|_| "no localStorage")?
        .ok_or("no localStorage")?;

    if let Ok(Some(hex)) = storage.get_item(STORAGE_KEY) {
        if let Some(bytes) = hex_to_32(&hex) {
            return Ok(bytes);
        }
    }

    // First use on this origin — generate a fresh key and persist it.
    let crypto = window.crypto().map_err(|_| "no crypto")?;
    let bytes = [0u8; 32];
    let view = unsafe { js_sys::Uint8Array::view(&bytes) };
    crypto
        .get_random_values_with_array_buffer_view(&view)
        .map_err(|_| "getRandomValues failed")?;
    drop(view);
    let _ = storage.set_item(STORAGE_KEY, &hex32(&bytes));
    Ok(bytes)
}

async fn import_aes_key(raw: &[u8]) -> Result<web_sys::CryptoKey, String> {
    let window = web_sys::window().ok_or("no window")?;
    let crypto = window.crypto().map_err(|_| "no crypto")?;
    let subtle = crypto.subtle();

    let key_data = js_sys::Uint8Array::from(raw);
    let algo = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&algo, &JsValue::from_str("name"), &JsValue::from_str("AES-GCM"));

    let usages = js_sys::Array::new();
    usages.push(&JsValue::from_str("encrypt"));
    usages.push(&JsValue::from_str("decrypt"));

    let promise = subtle
        .import_key_with_object("raw", &key_data.buffer(), &algo, false, &usages)
        .map_err(|e| format!("importKey: {e:?}"))?;
    let result = JsFuture::from(promise)
        .await
        .map_err(|e| format!("importKey await: {e:?}"))?;
    result
        .dyn_into::<web_sys::CryptoKey>()
        .map_err(|_| "importKey did not return CryptoKey".into())
}

/// Encrypt plaintext bytes. Returns `IV || ciphertext || tag`.
async fn encrypt(key: &web_sys::CryptoKey, plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let crypto = window.crypto().map_err(|_| "no crypto")?;
    let subtle = crypto.subtle();

    let iv_bytes = [0u8; IV_LEN];
    let iv_view = unsafe { js_sys::Uint8Array::view(&iv_bytes) };
    crypto
        .get_random_values_with_array_buffer_view(&iv_view)
        .map_err(|_| "getRandomValues failed")?;

    let algo = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&algo, &JsValue::from_str("name"), &JsValue::from_str("AES-GCM"));
    let _ = js_sys::Reflect::set(&algo, &JsValue::from_str("iv"), &iv_view);

    let data = plaintext.to_vec();
    let promise = subtle
        .encrypt_with_object_and_u8_array(&algo, key, &data)
        .map_err(|e| format!("encrypt: {e:?}"))?;
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
async fn decrypt(key: &web_sys::CryptoKey, encrypted: &[u8]) -> Result<Vec<u8>, String> {
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

    let ct = encrypted[IV_LEN..].to_vec();
    let promise = subtle
        .decrypt_with_object_and_u8_array(&algo, key, &ct)
        .map_err(|e| format!("decrypt: {e:?}"))?;
    let result = JsFuture::from(promise)
        .await
        .map_err(|e| format!("decrypt await: {e:?}"))?;

    let plaintext = js_sys::Uint8Array::new(&result);
    Ok(plaintext.to_vec())
}

fn hex32(b: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

fn hex_to_32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}
