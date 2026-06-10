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

use crate::runtime::sleep_ms;
use crate::wallet;

const SIGNER_URL: &str = "https://localharness.xyz/?signer=1";
const SIGNER_ORIGIN: &str = "https://localharness.xyz";
/// How long to wait for the signer to reply before giving up.
const TIMEOUT_MS: u32 = 5_000;

/// Overall ceiling for a [`verify_owner`] call, used by `kick_verification`
/// to cap the flow. The internal iframe wait is `TIMEOUT_MS` (5s), but the
/// first step is an on-chain `owner_of_name` read whose transport (browser
/// `fetch`) has no timeout — so without an outer cap a dead RPC hangs the
/// whole verification (and the verify pill) indefinitely. Generous enough to
/// absorb the iframe round-trip on top of a slow-but-alive read.
pub(crate) const VERIFY_BUDGET_MS: u32 = 15_000;

/// The local master wallet IF this origin holds the seed — apex always,
/// or a subdomain that pulled it in via [`super::seed_pull`]. When
/// present, seed-derived ops (owner proof, tempo-tx sign, key seal/open)
/// run **locally** and skip the cross-origin signer iframe entirely. That
/// iframe is the dead path on mobile, where browsers partition
/// cross-origin iframe storage so the embedded apex sees an empty OPFS.
/// Returns `(signer, address, bip39_entropy)`; `None` on a seedless
/// origin → callers fall back to the iframe.
fn local_master() -> Option<(k256::ecdsa::SigningKey, [u8; 20], Vec<u8>)> {
    super::APP.with(|cell| {
        cell.borrow().wallet.as_ref().map(|w| {
            (w.signer.clone(), w.address, w.mnemonic.to_entropy())
        })
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VerifyResult {
    VerifiedOwner { address: String },
    Visitor {
        owner_address: String,
        /// Recovered address that signed the challenge — the visitor's
        /// master-wallet address. The payment flow uses it as the
        /// `from` of the on-chain payment tx so the iframe signer can
        /// query the correct nonce + gas-price + balance.
        visitor_address: String,
    },
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

    // Local-first: if this origin holds the seed, we ARE the holder of our
    // own address — no challenge round-trip needed. Skips the iframe (dead
    // on mobile). Compare the local address to the on-chain owner directly.
    if let Some((_, address, _)) = local_master() {
        let local_hex = format!("0x{}", bytes_to_hex(&address));
        return Ok(if local_hex.eq_ignore_ascii_case(&expected) {
            VerifyResult::VerifiedOwner { address: local_hex }
        } else {
            VerifyResult::Visitor {
                owner_address: expected,
                visitor_address: local_hex,
            }
        });
    }

    // 2. Get a signature from the apex signer. The signer's `paint_signer`
    // is async — it might not have loaded the master wallet by the time
    // we post the first challenge. Retry once after a short backoff so a
    // simple race condition doesn't surface as "verify failed".
    let nonce = random_nonce();
    let nonce_hex = bytes_to_hex(&nonce);
    let (signer_address, signature) = match sign_via_iframe(&nonce_hex, name).await {
        Ok(pair) => pair,
        Err(first_err) => {
            sleep_ms(1500).await;
            sign_via_iframe(&nonce_hex, name).await
                .map_err(|second_err| format!(
                    "signer didn't respond — first attempt: {first_err}; retry: {second_err}"
                ))?
        }
    };

    // 3. Recover the signer address from the signature and verify it
    //    matches what the signer claimed (basic sanity check). The
    //    preimage binds the subdomain `name` so a signature proving
    //    ownership of one name can't be replayed as proof for a different
    //    name held by the same address. MUST match `signer.rs`
    //    `build_challenge_response` byte-for-byte.
    let prehash = challenge_prehash(name, &nonce);
    let sig_bytes = hex_to_bytes(&signature)?;
    if sig_bytes.len() != 65 {
        return Err(format!("bad signature length {}", sig_bytes.len()));
    }
    let mut sig_arr = [0u8; 65];
    sig_arr.copy_from_slice(&sig_bytes);
    let recovered = wallet::recover_address(&sig_arr, &prehash)?;
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
            visitor_address: recovered_hex,
        })
    }
}

/// keccak256("localharness-auth-v0:" || name || ":" || nonce). Binds the
/// owner-proof to BOTH the subdomain name and a random nonce, so a
/// signature proving ownership of one name can't be replayed as proof for
/// a different name held by the same address. MUST stay byte-for-byte
/// identical to `signer.rs::build_challenge_response`.
fn challenge_prehash(name: &str, nonce: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let mut hasher = Keccak256::new();
    hasher.update(b"localharness-auth-v0:");
    hasher.update(name.as_bytes());
    hasher.update(b":");
    hasher.update(nonce);
    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    out
}

/// Embed the signer iframe, send a sign challenge, wait for the
/// reply. Cleans up the iframe before returning. `name` is the subdomain
/// being verified — sent so the signer binds it into the signed preimage.
async fn sign_via_iframe(nonce_hex: &str, name: &str) -> Result<(String, String), String> {
    let id = format!("verify-{}", random_id_hex());
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
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("name"),
        &JsValue::from_str(name),
    );

    let data = signer_iframe_request(&id, &payload.into(), TIMEOUT_MS).await?;
    let address = js_sys::Reflect::get(&data, &JsValue::from_str("address"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    let signature = js_sys::Reflect::get(&data, &JsValue::from_str("signature"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    if address.is_empty() || signature.is_empty() {
        return Err("signer reply missing address or signature".into());
    }
    Ok((address, signature))
}

const TX_TIMEOUT_MS: u32 = 90_000;

/// Ask the apex signer to sign a sponsored Tempo tx with the master
/// wallet. Returns `(signer_address, 65-byte signature)` over the tx's
/// sender_hash.
///
/// SECURITY: we send the tx's **structured fields** (chain id, fees,
/// nonce, fee token, and every call's `to`/`value`/`input`), not just an
/// opaque 32-byte digest. The apex signer reconstructs the sender_hash
/// from these, enforces a call-target allowlist (registry diamond + $LH
/// token, zero native value), and refuses to sign anything else — so a
/// hostile subdomain can no longer get the master wallet to sign an
/// arbitrary transaction (the confused-deputy fund-drain vector). The
/// `digest` is still sent as a cross-check; the caller re-verifies the
/// returned signature against its own `tx.sender_hash()` (fail-closed).
pub(crate) async fn sign_tempo_tx_via_iframe(
    tx: &crate::tempo_tx::TempoTx,
    purpose: &str,
) -> Result<(String, [u8; 65]), String> {
    let digest = tx.sender_hash();

    // Local-first: hold the seed here → sign the sender_hash directly,
    // skip the iframe (dead on mobile). The caller re-recovers + checks
    // the address against the expected sender, so this stays fail-closed.
    if let Some((signer, address, _)) = local_master() {
        let sig = wallet::sign_hash(&signer, &digest);
        let _ = purpose;
        return Ok((format!("0x{}", bytes_to_hex(&address)), sig));
    }

    let id = format!("digest-{}", random_id_hex());
    let digest_hex = format!("0x{}", bytes_to_hex(&digest));

    let payload = js_sys::Object::new();
    let set_str = |obj: &js_sys::Object, k: &str, v: &str| {
        let _ = js_sys::Reflect::set(obj, &JsValue::from_str(k), &JsValue::from_str(v));
    };
    set_str(&payload, "type", "lh-sign-digest");
    set_str(&payload, "id", &id);
    set_str(&payload, "digest", &digest_hex);
    set_str(&payload, "purpose", purpose);

    // Structured fields the signer reconstructs + validates against.
    let txo = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &txo,
        &JsValue::from_str("chainId"),
        &JsValue::from_f64(tx.chain_id as f64),
    );
    set_str(&txo, "maxPriorityFeePerGas", &format!("0x{:x}", tx.max_priority_fee_per_gas));
    set_str(&txo, "maxFeePerGas", &format!("0x{:x}", tx.max_fee_per_gas));
    set_str(&txo, "gasLimit", &format!("0x{:x}", tx.gas_limit));
    set_str(&txo, "nonce", &format!("0x{:x}", tx.nonce));
    match tx.fee_token {
        Some(addr) => set_str(&txo, "feeToken", &format!("0x{}", bytes_to_hex(&addr))),
        None => {
            let _ = js_sys::Reflect::set(&txo, &JsValue::from_str("feeToken"), &JsValue::NULL);
        }
    }
    let _ = js_sys::Reflect::set(
        &txo,
        &JsValue::from_str("sponsored"),
        &JsValue::from_bool(tx.sponsored),
    );
    let calls = js_sys::Array::new();
    for c in &tx.calls {
        let co = js_sys::Object::new();
        set_str(&co, "to", &format!("0x{}", bytes_to_hex(&c.to)));
        set_str(&co, "value", &format!("0x{:x}", c.value_wei));
        set_str(&co, "input", &format!("0x{}", bytes_to_hex(&c.input)));
        calls.push(&co);
    }
    let _ = js_sys::Reflect::set(&txo, &JsValue::from_str("calls"), &calls);
    let _ = js_sys::Reflect::set(&payload, &JsValue::from_str("tx"), &txo);

    let data = signer_iframe_request(&id, &payload.into(), TX_TIMEOUT_MS).await?;
    let address = js_sys::Reflect::get(&data, &JsValue::from_str("address"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    let sig_hex = js_sys::Reflect::get(&data, &JsValue::from_str("signature"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    if address.is_empty() || sig_hex.is_empty() {
        return Err("signer reply missing address or signature".into());
    }
    let sig_bytes = hex_to_bytes(&sig_hex)?;
    if sig_bytes.len() != 65 {
        return Err(format!("signature must be 65 bytes, got {}", sig_bytes.len()));
    }
    let mut sig = [0u8; 65];
    sig.copy_from_slice(&sig_bytes);
    Ok((address, sig))
}

/// Generous timeout for OPFS-touching ops at apex (create wallet,
/// import seed). The actual work is a single file write; the budget
/// is generous to absorb wasm-bundle cold-load and a slow disk.
const IDENTITY_TIMEOUT_MS: u32 = 20_000;

/// Ask the apex signer for the cached mnemonic. Returns the 12-word
/// phrase on success, or an error if no identity exists at apex.
pub(crate) async fn reveal_seed_via_iframe() -> Result<String, String> {
    let id = format!("reveal-{}", random_id_hex());
    let payload = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("type"),
        &JsValue::from_str("lh-reveal-seed"),
    );
    let _ = js_sys::Reflect::set(&payload, &JsValue::from_str("id"), &JsValue::from_str(&id));
    let data = signer_iframe_request(&id, &payload.into(), TIMEOUT_MS).await?;
    let phrase = js_sys::Reflect::get(&data, &JsValue::from_str("phrase"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    if phrase.is_empty() {
        return Err("signer reply missing phrase".into());
    }
    Ok(phrase)
}

/// Ensure the apex origin has a master wallet, returning its address.
/// If `overwrite` is false (the default in tenant-side flows), an
/// existing wallet is preserved — only a brand-new origin gets a fresh
/// keypair. Pass `overwrite=true` from the explicit apex "create
/// identity" path where the user is asking for a fresh wallet.
pub(crate) async fn create_wallet_via_iframe(overwrite: bool) -> Result<String, String> {
    let id = format!("create-{}", random_id_hex());
    let payload = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("type"),
        &JsValue::from_str("lh-create-wallet"),
    );
    let _ = js_sys::Reflect::set(&payload, &JsValue::from_str("id"), &JsValue::from_str(&id));
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("overwrite"),
        &JsValue::from_bool(overwrite),
    );
    let data = signer_iframe_request(&id, &payload.into(), IDENTITY_TIMEOUT_MS).await?;
    let address = js_sys::Reflect::get(&data, &JsValue::from_str("address"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    if address.is_empty() {
        return Err("signer reply missing address".into());
    }
    Ok(address)
}

/// Run the full apex claim flow (faucet → register → wait receipt) from
/// the apex signer iframe. Long timeout because waiting for a receipt
/// can take ~10s and the faucet drip adds another ~5s. Returns
/// `(owner_address, tx_hash)`.
pub(crate) async fn claim_name_via_iframe(name: &str) -> Result<(String, String), String> {
    let id = format!("claim-{}", random_id_hex());
    let payload = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("type"),
        &JsValue::from_str("lh-claim-name"),
    );
    let _ = js_sys::Reflect::set(&payload, &JsValue::from_str("id"), &JsValue::from_str(&id));
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("name"),
        &JsValue::from_str(name),
    );
    let data = signer_iframe_request(&id, &payload.into(), CLAIM_TIMEOUT_MS).await?;
    let address = js_sys::Reflect::get(&data, &JsValue::from_str("address"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    let tx_hash = js_sys::Reflect::get(&data, &JsValue::from_str("tx_hash"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    if address.is_empty() || tx_hash.is_empty() {
        return Err("signer reply missing address or tx_hash".into());
    }
    Ok((address, tx_hash))
}

const CLAIM_TIMEOUT_MS: u32 = 90_000;

/// Ask the apex signer to seal `plaintext` (the Gemini key) with the
/// seed-derived key. Returns ciphertext hex for on-chain storage.
pub(crate) async fn seal_key_via_iframe(plaintext: &str) -> Result<String, String> {
    // Local-first: hold the seed here → derive the keysync key + seal
    // locally, skip the iframe (dead on mobile). Same derivation as the
    // signer's `seed_sync_key` (shared in encryption.rs).
    if let Some((_, _, entropy)) = local_master() {
        let key = super::encryption::keysync_key_from_entropy(&entropy);
        let ct = super::encryption::seal_with_raw_key(&key, plaintext.as_bytes())
            .await
            .ok_or_else(|| "seal failed".to_string())?;
        return Ok(format!("0x{}", bytes_to_hex(&ct)));
    }
    let id = format!("seal-{}", random_id_hex());
    let payload = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("type"),
        &JsValue::from_str("lh-seal-key"),
    );
    let _ = js_sys::Reflect::set(&payload, &JsValue::from_str("id"), &JsValue::from_str(&id));
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("plaintext"),
        &JsValue::from_str(plaintext),
    );
    let data = signer_iframe_request(&id, &payload.into(), IDENTITY_TIMEOUT_MS).await?;
    js_sys::Reflect::get(&data, &JsValue::from_str("ciphertext"))
        .ok()
        .and_then(|v| v.as_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "signer reply missing ciphertext".to_string())
}

/// Ask the apex signer to open seed-sealed `ciphertext_hex` → plaintext.
pub(crate) async fn open_key_via_iframe(ciphertext_hex: &str) -> Result<String, String> {
    // Local-first: hold the seed here → derive the keysync key + open
    // locally, skip the iframe (dead on mobile).
    if let Some((_, _, entropy)) = local_master() {
        let key = super::encryption::keysync_key_from_entropy(&entropy);
        let ct = hex_to_bytes(ciphertext_hex)?;
        let pt = super::encryption::open_with_raw_key(&key, &ct)
            .await
            .ok_or_else(|| "open failed (wrong seed?)".to_string())?;
        return String::from_utf8(pt).map_err(|_| "decrypted value not utf-8".to_string());
    }
    let id = format!("open-{}", random_id_hex());
    let payload = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("type"),
        &JsValue::from_str("lh-open-key"),
    );
    let _ = js_sys::Reflect::set(&payload, &JsValue::from_str("id"), &JsValue::from_str(&id));
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("ciphertext"),
        &JsValue::from_str(ciphertext_hex),
    );
    let data = signer_iframe_request(&id, &payload.into(), IDENTITY_TIMEOUT_MS).await?;
    js_sys::Reflect::get(&data, &JsValue::from_str("plaintext"))
        .ok()
        .and_then(|v| v.as_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "signer reply missing plaintext".to_string())
}

/// Ask the apex signer to import a user-supplied seed phrase and
/// persist it. Returns the new address. Overwrites any existing wallet.
pub(crate) async fn import_seed_via_iframe(phrase: &str) -> Result<String, String> {
    let id = format!("import-{}", random_id_hex());
    let payload = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("type"),
        &JsValue::from_str("lh-import-seed"),
    );
    let _ = js_sys::Reflect::set(&payload, &JsValue::from_str("id"), &JsValue::from_str(&id));
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("phrase"),
        &JsValue::from_str(phrase),
    );
    let data = signer_iframe_request(&id, &payload.into(), IDENTITY_TIMEOUT_MS).await?;
    let address = js_sys::Reflect::get(&data, &JsValue::from_str("address"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    if address.is_empty() {
        return Err("signer reply missing address".into());
    }
    Ok(address)
}

/// How long to wait for the signer iframe's `lh-signer-ready` ping
/// before posting the challenge anyway. The wasm bundle in a cold
/// iframe can take a couple of seconds to compile + install its
/// postMessage listener; this window covers that. If the ready ping
/// never arrives we post anyway as best-effort.
const READY_TIMEOUT_MS: u32 = 15_000;

/// Shared iframe-lifecycle dance — load the apex signer in a hidden
/// iframe, attach a correlation-id-filtered listener, wait for the
/// `lh-signer-ready` ping, post `payload`, race the reply against
/// `timeout_ms`, tear down. Returns the raw response `JsValue` (a
/// `{type:"lh-sign-response", id, ...}` object); callers parse the
/// variant-specific fields.
async fn signer_iframe_request(
    expected_id: &str,
    payload: &JsValue,
    timeout_ms: u32,
) -> Result<JsValue, String> {
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

    let result_slot: Rc<RefCell<Option<Result<JsValue, String>>>> =
        Rc::new(RefCell::new(None));
    let waker_slot: Rc<RefCell<Option<js_sys::Function>>> = Rc::new(RefCell::new(None));
    // Separate ready slot: signer posts `{type:"lh-signer-ready"}` once
    // its postMessage listener is installed. Verify-side gates on this
    // instead of a fixed sleep, so a slow wasm-compile doesn't race.
    let ready_slot: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let ready_waker: Rc<RefCell<Option<js_sys::Function>>> = Rc::new(RefCell::new(None));

    let result_for_handler = result_slot.clone();
    let waker_for_handler = waker_slot.clone();
    let ready_for_handler = ready_slot.clone();
    let ready_waker_for_handler = ready_waker.clone();
    let id_for_handler = expected_id.to_string();
    let handler = Closure::<dyn FnMut(_)>::new(move |event: MessageEvent| {
        let data = event.data();
        if data.is_null() || data.is_undefined() {
            return;
        }
        if event.origin() != SIGNER_ORIGIN {
            return;
        }
        let msg_type = js_sys::Reflect::get(&data, &JsValue::from_str("type"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();

        if msg_type == "lh-signer-ready" {
            *ready_for_handler.borrow_mut() = true;
            if let Some(waker) = ready_waker_for_handler.borrow_mut().take() {
                let _ = waker.call0(&JsValue::NULL);
            }
            return;
        }

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
            Ok(data.clone())
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

    // Wait for the iframe content_window to materialize.
    let mut content_window: Option<web_sys::Window> = None;
    for _ in 0..50 {
        if let Some(w) = iframe.content_window() {
            content_window = Some(w);
            break;
        }
        sleep_ms(50).await;
    }
    let target = content_window
        .ok_or_else(|| "iframe content window never available".to_string())?;

    // Wait for the signer to send its `lh-signer-ready` ping (set by
    // paint_signer once the wasm bundle has compiled + the listener
    // is installed + the wallet is loaded-or-known-absent). Falls back
    // to posting anyway after READY_TIMEOUT_MS so a missing ping
    // doesn't deadlock — though every shipped signer paints one.
    if !*ready_slot.borrow() {
        let ready_promise = js_sys::Promise::new(&mut |resolve, _reject| {
            *ready_waker.borrow_mut() = Some(resolve.clone());
            if let Some(window) = web_sys::window() {
                let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                    &resolve,
                    READY_TIMEOUT_MS as i32,
                );
            }
        });
        let _ = JsFuture::from(ready_promise).await;
    }

    target
        .post_message(payload, SIGNER_ORIGIN)
        .map_err(|e| format!("postMessage: {e:?}"))?;

    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let resolve_clone = resolve.clone();
        *waker_slot.borrow_mut() = Some(resolve_clone);
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                &resolve,
                timeout_ms as i32,
            );
        }
    });
    let _ = JsFuture::from(promise).await;

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
