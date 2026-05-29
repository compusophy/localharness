//! Cross-origin signing service hosted at the apex origin.
//!
//! Subdomains can't see apex's wallet (per-origin OPFS). To prove
//! "the visitor controls the on-chain owner address", subdomains
//! create a hidden iframe pointing to `localharness.xyz/?signer=1`,
//! send a `lh-sign-challenge` postMessage, and recover the signer's
//! address from the returned signature. M8 in the design doc.
//!
//! Challenge-signing auto-approves for v1 (verification is read-only).
//!
//! **Trust model (hardened — see the per-message notes below).** A
//! trusted origin is any `*.localharness.xyz`, but since registration is
//! free that is NOT a sufficient boundary on its own. So:
//!   - **Tx signing** (`lh-sign-digest`) no longer signs an opaque
//!     digest. The tenant sends the tx's structured fields; the signer
//!     reconstructs the sender_hash, enforces a call-target allowlist
//!     (registry diamond + $LH token, zero native value), and refuses
//!     anything else — a hostile subdomain can't get the master wallet
//!     to sign an arbitrary fund-moving tx.
//!   - **Identity ops** (`lh-reveal-seed`, `lh-import-seed`,
//!     `lh-create-wallet{overwrite}`) are **apex-origin only** — a
//!     subdomain iframe cannot exfiltrate or overwrite the master seed.
//!
//! **Message protocol:**
//! ```text
//! Verification (auto-approved):
//!   parent  → signer: { type: "lh-sign-challenge", id, nonce }
//!   signer → parent:  { type: "lh-sign-response",  id, address, signature }
//!                or:  { type: "lh-sign-response",  id, error }
//!
//! Sponsored-Tempo-tx signing (structured + allowlisted — NOT opaque):
//!   parent  → signer: { type: "lh-sign-digest", id, purpose, digest,
//!                       tx: { chainId, maxPriorityFeePerGas, maxFeePerGas,
//!                             gasLimit, nonce, feeToken, sponsored,
//!                             calls: [{ to, value, input }, ...] } }
//!   signer → parent:  { type: "lh-sign-response", id, address, signature }
//!                or:  { type: "lh-sign-response", id, error }
//!   The signer reconstructs the sender_hash from `tx`, requires every
//!   `call.to` ∈ {registry diamond, $LH token} with value 0, cross-checks
//!   against `digest`, and signs only its own reconstruction.
//!   `signature` is 65 bytes hex (r ‖ s ‖ v with v ∈ {27,28}).
//!
//! Identity management (lh-reveal-seed / lh-import-seed are APEX-ORIGIN
//! ONLY; lh-create-wallet overwrite=true is apex-only, ensure is open):
//!   parent  → signer: { type: "lh-create-wallet", id, overwrite: bool? }
//!   signer → parent:  { type: "lh-sign-response", id, address }
//!                or:  { type: "lh-sign-response", id, error }
//!   Default semantics: ENSURE — returns existing wallet's address if
//!   one is present, only generates fresh on a wallet-less origin. Pass
//!   overwrite=true to force regeneration (the apex "create identity"
//!   button uses this; tenant first-claim uses default ensure).
//!
//!   parent  → signer: { type: "lh-reveal-seed", id }
//!   signer → parent:  { type: "lh-sign-response", id, phrase }
//!                or:  { type: "lh-sign-response", id, error }
//!
//!   parent  → signer: { type: "lh-import-seed", id, phrase }
//!   signer → parent:  { type: "lh-sign-response", id, address }
//!                or:  { type: "lh-sign-response", id, error }
//!
//!   parent  → signer: { type: "lh-claim-name", id, name }
//!   signer → parent:  { type: "lh-sign-response", id, address, tx_hash }
//!                or:  { type: "lh-sign-response", id, error }
//!   Runs the full apex claim flow (faucet → register → wait receipt)
//!   from apex origin. Long-running — callers should set a generous
//!   timeout (60s+).
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

    // Early-return for message types we don't handle BEFORE doing any
    // source/origin work. Pages run lots of incidental postMessage
    // chatter (Vercel's lockdown.js, browser extensions, dev tooling)
    // and we don't want to log "source is not a Window" for any of it.
    if !matches!(
        msg_type.as_str(),
        "lh-sign-challenge"
            | "lh-sign-digest"
            | "lh-create-wallet"
            | "lh-reveal-seed"
            | "lh-import-seed"
            | "lh-claim-name"
    ) {
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

    // Don't `dyn_into::<Window>` here — cross-origin parent
    // references are `WindowProxy` objects which fail strict
    // wasm-bindgen type checks even though they expose `postMessage`.
    // Hold the source as a generic JsValue and call postMessage via
    // Reflect on the way out.
    let source = event
        .source()
        .ok_or_else(|| "no source window on the message event".to_string())?;
    let source_jsval: JsValue = source.into();

    let reply = match msg_type.as_str() {
        "lh-sign-challenge" => {
            let nonce_hex = js_sys::Reflect::get(&data, &JsValue::from_str("nonce"))
                .ok()
                .and_then(|v| v.as_string())
                .ok_or_else(|| "nonce not a string".to_string())?;
            // The subdomain being verified. Bound into the signed preimage
            // so an owner-proof can't be replayed across names (verify.rs
            // sends it). Default empty for resilience if an old client
            // omits it — that client also omits it on the verify side, so
            // the two still agree.
            let name = js_sys::Reflect::get(&data, &JsValue::from_str("name"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            match build_challenge_response(&id, &nonce_hex, &name) {
                Ok(obj) => obj,
                Err(err) => error_response(&id, &err),
            }
        }
        "lh-sign-digest" => {
            let purpose = js_sys::Reflect::get(&data, &JsValue::from_str("purpose"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_else(|| "sign digest".into());
            match build_sponsored_tx_response(&id, &data, &purpose) {
                Ok(obj) => obj,
                Err(err) => error_response(&id, &err),
            }
        }
        "lh-reveal-seed" => {
            // APEX-ONLY. Revealing the master mnemonic to a tenant
            // subdomain iframe is the confused-deputy seed-exfiltration
            // vector — any free-to-claim subdomain could request it
            // silently. Seed reveal happens only on the apex page itself
            // (which reads the wallet from local state directly, never
            // through this iframe), so gating here breaks no legit flow.
            if !super::tenant::is_apex_origin(&origin) {
                error_response(&id, "seed reveal is only available at localharness.xyz")
            } else {
                match build_reveal_seed_response(&id) {
                    Ok(obj) => obj,
                    Err(err) => error_response(&id, &err),
                }
            }
        }
        "lh-create-wallet" => {
            // Default semantics: ENSURE (return existing if any, else
            // generate). Explicit overwrite=true requests regeneration.
            let overwrite = js_sys::Reflect::get(&data, &JsValue::from_str("overwrite"))
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // ENSURE (overwrite=false) is safe cross-origin — it's how a
            // tenant first-claim makes sure a master wallet exists. But
            // OVERWRITE regenerates (destroys) the master wallet, so it's
            // apex-only; a subdomain must not be able to brick the
            // identity.
            if overwrite && !super::tenant::is_apex_origin(&origin) {
                let reply = error_response(
                    &id,
                    "creating a fresh identity is only available at localharness.xyz",
                );
                post_reply(&source_jsval, &reply, &origin)?;
                return Ok(());
            }
            spawn_create_wallet(
                id.clone(),
                overwrite,
                source_jsval.clone(),
                origin.clone(),
            );
            return Ok(());
        }
        "lh-import-seed" => {
            // APEX-ONLY. Importing overwrites the master wallet; honoring
            // it cross-origin lets a subdomain silently replace the user's
            // identity with an attacker-controlled key. Import is done on
            // the apex page (writes local state directly).
            if !super::tenant::is_apex_origin(&origin) {
                let reply = error_response(
                    &id,
                    "seed import is only available at localharness.xyz",
                );
                post_reply(&source_jsval, &reply, &origin)?;
                return Ok(());
            }
            let phrase = js_sys::Reflect::get(&data, &JsValue::from_str("phrase"))
                .ok()
                .and_then(|v| v.as_string())
                .ok_or_else(|| "phrase not a string".to_string())?;
            spawn_import_seed(id.clone(), phrase, source_jsval.clone(), origin.clone());
            return Ok(());
        }
        "lh-claim-name" => {
            let name = js_sys::Reflect::get(&data, &JsValue::from_str("name"))
                .ok()
                .and_then(|v| v.as_string())
                .ok_or_else(|| "name not a string".to_string())?;
            spawn_claim_name(id.clone(), name, source_jsval.clone(), origin.clone());
            return Ok(());
        }
        _ => return Ok(()), // not for us
    };

    post_reply(&source_jsval, &reply, &origin)?;
    Ok(())
}

/// Reflect-based postMessage on the source (which may be a cross-origin
/// WindowProxy, not a strict Window). Equivalent to
/// `source.postMessage(reply, origin)` in JS. Shared by the sync reply
/// path and the spawn_local async reply paths.
fn post_reply(source: &JsValue, reply: &JsValue, origin: &str) -> Result<(), String> {
    let post_msg = js_sys::Reflect::get(source, &JsValue::from_str("postMessage"))
        .map_err(|_| "source has no postMessage".to_string())?;
    let post_fn: js_sys::Function = post_msg
        .dyn_into()
        .map_err(|_| "source.postMessage isn't a function".to_string())?;
    post_fn
        .call2(source, reply, &JsValue::from_str(origin))
        .map_err(|e| format!("postMessage call: {e:?}"))?;
    Ok(())
}

fn build_reveal_seed_response(id: &str) -> Result<JsValue, String> {
    let phrase = super::APP
        .with(|cell| {
            cell.borrow()
                .wallet
                .as_ref()
                .map(|w| w.mnemonic.to_string())
        })
        .ok_or_else(|| "no identity on this device".to_string())?;
    let obj = js_sys::Object::new();
    set(&obj, "type", JsValue::from_str("lh-sign-response"));
    set(&obj, "id", JsValue::from_str(id));
    set(&obj, "phrase", JsValue::from_str(&phrase));
    Ok(JsValue::from(obj))
}

fn spawn_create_wallet(id: String, overwrite: bool, source: JsValue, origin: String) {
    wasm_bindgen_futures::spawn_local(async move {
        // Ensure-semantic when overwrite is false: if a wallet already
        // exists at apex, return its address without regenerating.
        // Protects users from accidentally nuking the master wallet
        // (and all the NFT ownership it tracks) during a tenant-side
        // first-claim flow.
        if !overwrite {
            let existing = super::APP
                .with(|cell| cell.borrow().wallet.as_ref().map(|w| w.address_hex()));
            if let Some(addr) = existing {
                let obj = js_sys::Object::new();
                set(&obj, "type", JsValue::from_str("lh-sign-response"));
                set(&obj, "id", JsValue::from_str(&id));
                set(&obj, "address", JsValue::from_str(&addr));
                let reply = JsValue::from(obj);
                if let Err(err) = post_reply(&source, &reply, &origin) {
                    web_sys::console::warn_1(&JsValue::from_str(&format!(
                        "signer: create-wallet (cached) reply: {err}"
                    )));
                }
                return;
            }
        }
        let reply = match super::wallet_store::create_and_persist().await {
            Ok(wallet) => {
                let addr = wallet.address_hex();
                super::APP.with(|cell| cell.borrow_mut().wallet = Some(wallet));
                let obj = js_sys::Object::new();
                set(&obj, "type", JsValue::from_str("lh-sign-response"));
                set(&obj, "id", JsValue::from_str(&id));
                set(&obj, "address", JsValue::from_str(&addr));
                JsValue::from(obj)
            }
            Err(err) => error_response(&id, &err),
        };
        if let Err(err) = post_reply(&source, &reply, &origin) {
            web_sys::console::warn_1(&JsValue::from_str(&format!(
                "signer: create-wallet reply: {err}"
            )));
        }
    });
}

/// Long-running: faucet-fund the apex wallet, then `register(name)` on
/// the registry, then wait for the receipt. Posts a single reply at the
/// end with the tx hash. Tenant first-claim sets this off and shows a
/// progress placeholder until the reply lands.
fn spawn_claim_name(id: String, name: String, source: JsValue, origin: String) {
    wasm_bindgen_futures::spawn_local(async move {
        let reply = match run_claim_name(&name).await {
            Ok((address, tx_hash)) => {
                let obj = js_sys::Object::new();
                set(&obj, "type", JsValue::from_str("lh-sign-response"));
                set(&obj, "id", JsValue::from_str(&id));
                set(&obj, "address", JsValue::from_str(&address));
                set(&obj, "tx_hash", JsValue::from_str(&tx_hash));
                JsValue::from(obj)
            }
            Err(err) => error_response(&id, &err),
        };
        if let Err(err) = post_reply(&source, &reply, &origin) {
            web_sys::console::warn_1(&JsValue::from_str(&format!(
                "signer: claim-name reply: {err}"
            )));
        }
    });
}

async fn run_claim_name(name: &str) -> Result<(String, String), String> {
    let (signer, address) = wallet_handle()?;
    let address_hex = hex_addr(&address);
    // Sponsored path: sender (user's wallet) holds zero, fee_payer
    // (bundle's sponsor) pays gas in AlphaUSD. No faucet drip
    // required — users get on-chain in one click with no native gas.
    let fee_payer = super::sponsor::signer()?;
    let tx_hash = crate::registry::claim_and_maybe_set_main_sponsored(
        &signer,
        &fee_payer,
        name,
        crate::registry::ALPHA_USD_ADDRESS,
    )
    .await?;
    Ok((address_hex, tx_hash))
}

fn spawn_import_seed(id: String, phrase: String, source: JsValue, origin: String) {
    wasm_bindgen_futures::spawn_local(async move {
        let reply = match super::wallet_store::import(&phrase).await {
            Ok(wallet) => {
                let addr = wallet.address_hex();
                super::APP.with(|cell| cell.borrow_mut().wallet = Some(wallet));
                let obj = js_sys::Object::new();
                set(&obj, "type", JsValue::from_str("lh-sign-response"));
                set(&obj, "id", JsValue::from_str(&id));
                set(&obj, "address", JsValue::from_str(&addr));
                JsValue::from(obj)
            }
            Err(err) => error_response(&id, &err),
        };
        if let Err(err) = post_reply(&source, &reply, &origin) {
            web_sys::console::warn_1(&JsValue::from_str(&format!(
                "signer: import-seed reply: {err}"
            )));
        }
    });
}

fn build_challenge_response(id: &str, nonce_hex: &str, name: &str) -> Result<JsValue, String> {
    let nonce = parse_nonce(nonce_hex)?;
    // Domain-separated digest the signer commits to. Binds the subdomain
    // `name` (and a random nonce) so a captured owner-proof for one name
    // can't be replayed as proof for another name held by the same
    // address. MUST stay byte-for-byte identical to `verify.rs`
    // `challenge_prehash`: DOMAIN_TAG || name || ":" || nonce.
    let mut hasher = Keccak256::new();
    hasher.update(DOMAIN_TAG);
    hasher.update(name.as_bytes());
    hasher.update(b":");
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

/// Sign a sponsored Tempo tx for a tenant. SECURITY-CRITICAL: we do NOT
/// sign an opaque caller-supplied digest (that let any subdomain get the
/// master wallet to sign an arbitrary transaction — e.g. drain a
/// token-bound account). Instead the tenant sends the tx's structured
/// fields; we independently reconstruct the sender_hash, enforce that
/// every call targets an allowlisted contract (the registry diamond or
/// the $LH credits token) with zero native value, cross-check the
/// reconstruction against the claimed digest, and only then sign. The
/// cross-origin sponsored path is only ever used for register /
/// setMetadata / submitFeedback (diamond) and approve / transfer ($LH);
/// TBA-touching flows run apex-side with the wallet directly, never here.
fn build_sponsored_tx_response(id: &str, data: &JsValue, purpose: &str) -> Result<JsValue, String> {
    let tx_obj = js_sys::Reflect::get(data, &JsValue::from_str("tx"))
        .ok()
        .filter(|v| !v.is_undefined() && !v.is_null())
        .ok_or_else(|| "refusing to sign: missing structured tx fields".to_string())?;
    let get = |k: &str| js_sys::Reflect::get(&tx_obj, &JsValue::from_str(k)).ok();
    let get_str = |k: &str| get(k).and_then(|v| v.as_string());

    // chain_id must match — no cross-chain replay.
    let chain_id = get("chainId")
        .and_then(|v| v.as_f64())
        .map(|f| f as u64)
        .ok_or_else(|| "tx.chainId missing".to_string())?;
    if chain_id != crate::registry::CHAIN_ID {
        return Err(format!("chainId {chain_id} != {}", crate::registry::CHAIN_ID));
    }

    let fee_priority =
        parse_u128_hex(&get_str("maxPriorityFeePerGas").ok_or("maxPriorityFeePerGas missing")?)?;
    let fee_max = parse_u128_hex(&get_str("maxFeePerGas").ok_or("maxFeePerGas missing")?)?;
    let gas_limit = parse_u128_hex(&get_str("gasLimit").ok_or("gasLimit missing")?)?;
    let nonce = parse_u128_hex(&get_str("nonce").ok_or("nonce missing")?)?;
    let fee_token = match get_str("feeToken") {
        Some(s) if !s.trim().trim_start_matches("0x").is_empty() => Some(parse_addr20(&s)?),
        _ => None,
    };
    let sponsored = get("sponsored").and_then(|v| v.as_bool()).unwrap_or(false);

    // Call-target allowlist — the heart of the fix.
    let registry_addr = parse_addr20(crate::registry::REGISTRY_ADDRESS)?;
    let token_addr = parse_addr20(crate::registry::LOCALHARNESS_TOKEN_ADDRESS)?;

    let calls_val = get("calls").ok_or_else(|| "tx.calls missing".to_string())?;
    let calls_arr: js_sys::Array = calls_val
        .dyn_into()
        .map_err(|_| "tx.calls not an array".to_string())?;
    if calls_arr.length() == 0 {
        return Err("tx.calls empty".into());
    }
    let mut calls = Vec::with_capacity(calls_arr.length() as usize);
    for i in 0..calls_arr.length() {
        let c = calls_arr.get(i);
        let cto = js_sys::Reflect::get(&c, &JsValue::from_str("to"))
            .ok()
            .and_then(|v| v.as_string())
            .ok_or_else(|| "call.to missing".to_string())?;
        let cval = js_sys::Reflect::get(&c, &JsValue::from_str("value"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| "0x0".into());
        let cinput = js_sys::Reflect::get(&c, &JsValue::from_str("input"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        let to = parse_addr20(&cto)?;
        if to != registry_addr && to != token_addr {
            return Err(format!(
                "refusing to sign: call target {} is not allowlisted",
                hex_addr(&to)
            ));
        }
        let value_wei = parse_u128_hex(&cval)?;
        if value_wei != 0 {
            return Err("refusing to sign: native value transfer not permitted".into());
        }
        let input = if cinput.trim().trim_start_matches("0x").is_empty() {
            Vec::new()
        } else {
            decode_hex(&cinput)?
        };
        calls.push(crate::tempo_tx::TempoCall { to, value_wei, input });
    }

    // Rebuild the tx EXACTLY as `events::run_sponsored_tempo_call` does so
    // our reconstructed sender_hash matches the tenant's (whose recover-
    // check fails closed on any mismatch). Mirror any builder change there.
    let mut builder = crate::tempo_tx::TempoTxBuilder::new(chain_id)
        .max_priority_fee_per_gas(fee_priority)
        .max_fee_per_gas(fee_max)
        .gas_limit(gas_limit)
        .nonce(nonce)
        .calls(calls);
    if let Some(ft) = fee_token {
        builder = builder.fee_token(ft);
    }
    if sponsored {
        builder = builder.sponsored();
    }
    let rebuilt = builder.build();
    let sender_hash = rebuilt.sender_hash();

    // Cross-check: the digest the tenant claims must equal our independent
    // reconstruction. A mismatch means the structured fields and the
    // digest disagree — refuse rather than sign the unknown one.
    if let Some(claimed) = js_sys::Reflect::get(data, &JsValue::from_str("digest"))
        .ok()
        .and_then(|v| v.as_string())
    {
        if let Ok(claimed_bytes) = decode_hex(&claimed) {
            if claimed_bytes.as_slice() != sender_hash {
                return Err("provided digest does not match reconstructed sender_hash".into());
            }
        }
    }

    let (signer, address) = wallet_handle()?;
    web_sys::console::log_1(&JsValue::from_str(&format!(
        "lh-sign-digest: signed reconstructed sponsored tx ({purpose}, {} allowlisted call(s))",
        rebuilt.calls.len(),
    )));
    let sig = wallet::sign_hash(&signer, &sender_hash);

    let obj = js_sys::Object::new();
    set(&obj, "type", JsValue::from_str("lh-sign-response"));
    set(&obj, "id", JsValue::from_str(id));
    set(&obj, "address", JsValue::from_str(&hex_addr(&address)));
    set(&obj, "signature", JsValue::from_str(&hex_bytes(&sig)));
    Ok(JsValue::from(obj))
}

/// Parse a `0x`-optional hex string into a `u128`. Empty ⇒ 0.
fn parse_u128_hex(s: &str) -> Result<u128, String> {
    let t = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    if t.is_empty() {
        return Ok(0);
    }
    u128::from_str_radix(t, 16).map_err(|e| format!("bad u128 hex '{s}': {e}"))
}

/// Parse a `0x`-optional 20-byte hex address.
fn parse_addr20(s: &str) -> Result<[u8; 20], String> {
    let bytes = decode_hex(s)?;
    if bytes.len() != 20 {
        return Err(format!("address must be 20 bytes, got {}", bytes.len()));
    }
    let mut a = [0u8; 20];
    a.copy_from_slice(&bytes);
    Ok(a)
}

fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
    let trimmed = hex.trim().trim_start_matches("0x").trim_start_matches("0X");
    if trimmed.len() % 2 != 0 {
        return Err("data hex odd length".into());
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
    // Centralised, hardened check (see tenant::is_trusted_lh_origin):
    // localhost is honoured only in dev, and matching is host-exact so a
    // page like localharness.xyz.evil.com can't request a signature.
    super::tenant::is_trusted_lh_origin(origin)
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
