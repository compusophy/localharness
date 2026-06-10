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

// The two seed-derived AES key derivations (Gemini keysync, tag
// `localharness/v0/keysync`; shared folder, tag `localharness/v0/sharedfs`)
// are pure keccak with a byte-for-byte cross-device contract, so they live
// in `crate::wallet` (beside the sibling `v0/ecies` tag in
// `ecdh_shared_key`) where native tests pin their outputs. Re-exported here
// so app call sites (`signer`, `verify`, `shared_fs`, `key_sync`) keep
// their historical `encryption::` path unchanged.
pub(crate) use crate::wallet::{keysync_key_from_entropy, sharedfs_key_from_entropy};

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
    // `get_random_values_with_u8_array` fills the buffer in place via a
    // copy-back, so we never hold a `Uint8Array::view` (a JS view aliasing
    // wasm linear memory) over a `&[u8]` while CSPRNG writes through it —
    // that pattern is UB under Rust's aliasing model and a future optimizer
    // could read a stale all-zero buffer. For a key (and the GCM IV below)
    // a zeroed value would be catastrophic, so fill safely.
    let crypto = window.crypto().map_err(|_| "no crypto")?;
    let mut bytes = [0u8; 32];
    crypto
        .get_random_values_with_u8_array(&mut bytes)
        .map_err(|_| "getRandomValues failed")?;
    // The persist MUST succeed before this key is ever used: a key that
    // fails to land in localStorage regenerates DIFFERENTLY on the next
    // load, silently orphaning everything sealed under it. Erring here
    // makes `device_key()` fail, so `seal`/`open` return `None` and the
    // callers take their documented plaintext fallback instead.
    storage
        .set_item(STORAGE_KEY, &hex32(&bytes))
        .map_err(|_| "localStorage persist of the device key failed")?;
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

    // Fresh random 96-bit GCM nonce per encryption — reuse with the same
    // key would break confidentiality + integrity. Fill in place via the
    // copy-back API (no `Uint8Array::view` aliasing a `&[u8]`, which is UB
    // and could be optimized into a stale zero IV), then hand WebCrypto a
    // SEPARATE owned copy so the value prepended to the output is exactly
    // the value used to encrypt.
    let mut iv_bytes = [0u8; IV_LEN];
    crypto
        .get_random_values_with_u8_array(&mut iv_bytes)
        .map_err(|_| "getRandomValues failed")?;
    let iv_js = js_sys::Uint8Array::from(&iv_bytes[..]);

    let algo = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&algo, &JsValue::from_str("name"), &JsValue::from_str("AES-GCM"));
    let _ = js_sys::Reflect::set(&algo, &JsValue::from_str("iv"), &iv_js);

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
    crate::encoding::bytes_to_hex(b)
}

/// Fixed-length decode — thin wrapper over [`crate::encoding::hex_to_bytes`]
/// that requires exactly 32 bytes (the persisted per-origin AES key).
fn hex_to_32(s: &str) -> Option<[u8; 32]> {
    crate::encoding::hex_to_bytes(s).ok()?.try_into().ok()
}
