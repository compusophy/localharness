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
//! [`crate::registry::CHAIN_ID()`] (42431); otherwise the signer rejects
//! to avoid a replay-on-a-different-chain footgun. `purpose` is the
//! human-readable description shown in the consent dialog.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::MessageEvent;

use crate::encoding::{bytes_to_hex_str, hex_to_bytes, parse_hex_quantity};
use crate::wallet;

use super::signer_protocol::{
    challenge_prehash, MSG_CLAIM_NAME, MSG_CREATE_WALLET, MSG_IMPORT_SEED, MSG_OPEN_KEY,
    MSG_REVEAL_SEED, MSG_SEAL_KEY, MSG_SIGN_CHALLENGE, MSG_SIGN_DIGEST, MSG_SIGN_RESPONSE,
};

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
        MSG_SIGN_CHALLENGE
            | MSG_SIGN_DIGEST
            | MSG_CREATE_WALLET
            | MSG_REVEAL_SEED
            | MSG_IMPORT_SEED
            | MSG_CLAIM_NAME
            | MSG_SEAL_KEY
            | MSG_OPEN_KEY
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

    // Field accessors over the request object.
    let get_str = |k: &str| {
        js_sys::Reflect::get(&data, &JsValue::from_str(k))
            .ok()
            .and_then(|v| v.as_string())
    };
    let require_str =
        |k: &str| get_str(k).ok_or_else(|| format!("{k} not a string"));

    let reply = match msg_type.as_str() {
        MSG_SIGN_CHALLENGE => {
            let nonce_hex = require_str("nonce")?;
            // The subdomain being verified. Bound into the signed preimage
            // so an owner-proof can't be replayed across names (verify.rs
            // sends it). Default empty for resilience if an old client
            // omits it — that client also omits it on the verify side, so
            // the two still agree.
            let name = get_str("name").unwrap_or_default();
            match build_challenge_response(&id, &nonce_hex, &name) {
                Ok(obj) => obj,
                Err(err) => error_response(&id, &err),
            }
        }
        MSG_SIGN_DIGEST => {
            // Async now: a value-moving $LH transfer is only signed for a
            // subdomain the master OWNS (on-chain `ownerOfName` check), which
            // needs `.await`. The reply is posted by the spawned task.
            let purpose = get_str("purpose").unwrap_or_else(|| "sign digest".into());
            spawn_reply(
                "sign-digest",
                id,
                source_jsval,
                origin.clone(),
                build_sponsored_tx_response(data.clone(), purpose, origin),
            );
            return Ok(());
        }
        MSG_REVEAL_SEED => {
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
        MSG_CREATE_WALLET => {
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
                error_response(
                    &id,
                    "creating a fresh identity is only available at localharness.xyz",
                )
            } else {
                spawn_reply("create-wallet", id, source_jsval, origin, run_create_wallet(overwrite));
                return Ok(());
            }
        }
        MSG_IMPORT_SEED => {
            // APEX-ONLY. Importing overwrites the master wallet; honoring
            // it cross-origin lets a subdomain silently replace the user's
            // identity with an attacker-controlled key. Import is done on
            // the apex page (writes local state directly).
            if !super::tenant::is_apex_origin(&origin) {
                error_response(&id, "seed import is only available at localharness.xyz")
            } else {
                let phrase = require_str("phrase")?;
                spawn_reply("import-seed", id, source_jsval, origin, run_import_seed(phrase));
                return Ok(());
            }
        }
        MSG_CLAIM_NAME => {
            let name = require_str("name")?;
            spawn_reply("claim-name", id, source_jsval, origin, run_claim_name_op(name));
            return Ok(());
        }
        MSG_SEAL_KEY => {
            // Encrypt a value (the tenant's Gemini key) with a key derived
            // from the master seed, so the ciphertext can be stored on-chain
            // and any seed-linked device can decrypt it. See note in
            // `seed_sync_key`.
            let plaintext = require_str("plaintext")?;
            spawn_reply("seal-key", id, source_jsval, origin, run_seal_key(plaintext));
            return Ok(());
        }
        MSG_OPEN_KEY => {
            // Owner-gated inside run_open_key — the requesting subdomain must be
            // one THIS identity owns, so a hostile origin a victim merely visits
            // can't have the apex signer decrypt the victim's on-chain key.
            let ciphertext = require_str("ciphertext")?;
            let req_origin = origin.clone();
            spawn_reply("open-key", id, source_jsval, origin, run_open_key(req_origin, ciphertext));
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

/// The success-reply fields of one async signer op: `(key, value)` pairs
/// set on the `lh-sign-response` object next to `type` + `id`.
type ReplyFields = Vec<(&'static str, String)>;

/// The one async-op scaffold every spawned handler used to copy-paste:
/// run `op`, turn its `Ok(fields)` / `Err(msg)` into a success / error
/// reply, post it back to `source`, and warn (with the op `name`) if the
/// post itself fails.
fn spawn_reply<F>(name: &'static str, id: String, source: JsValue, origin: String, op: F)
where
    F: std::future::Future<Output = Result<ReplyFields, String>> + 'static,
{
    wasm_bindgen_futures::spawn_local(async move {
        let reply = match op.await {
            Ok(fields) => success_response(&id, &fields),
            Err(err) => error_response(&id, &err),
        };
        if let Err(err) = post_reply(&source, &reply, &origin) {
            web_sys::console::warn_1(&JsValue::from_str(&format!(
                "signer: {name} reply: {err}"
            )));
        }
    });
}

/// `{type: "lh-sign-response", id, ...fields}` — the success-reply shape
/// shared by every signer op (sync builders and spawned ops alike).
fn success_response(id: &str, fields: &[(&'static str, String)]) -> JsValue {
    let obj = js_sys::Object::new();
    set(&obj, "type", JsValue::from_str(MSG_SIGN_RESPONSE));
    set(&obj, "id", JsValue::from_str(id));
    for (k, v) in fields {
        set(&obj, k, JsValue::from_str(v));
    }
    JsValue::from(obj)
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
    Ok(success_response(id, &[("phrase", phrase)]))
}

/// Ensure-semantic when `overwrite` is false: if a wallet already exists
/// at apex, return its address without regenerating. Protects users from
/// accidentally nuking the master wallet (and all the NFT ownership it
/// tracks) during a tenant-side first-claim flow.
async fn run_create_wallet(overwrite: bool) -> Result<ReplyFields, String> {
    if !overwrite {
        let existing = super::APP
            .with(|cell| cell.borrow().wallet.as_ref().map(|w| w.address_hex()));
        if let Some(addr) = existing {
            return Ok(vec![("address", addr)]);
        }
    }
    let wallet = super::wallet_store::create_and_persist().await?;
    let addr = wallet.address_hex();
    super::APP.with(|cell| cell.borrow_mut().wallet = Some(wallet));
    Ok(vec![("address", addr)])
}

/// Derive the 32-byte AES key used to seal/open the on-chain Gemini key,
/// from the master wallet's BIP-39 entropy. Deterministic from the seed,
/// so any device that imports the seed derives the same key and can
/// decrypt the on-chain ciphertext.
///
/// SECURITY: this key opens ANY seed-sealed ciphertext, so the cross-origin
/// `lh-open-key` op ([`run_open_key`]) is gated on the requesting subdomain
/// being one THIS identity OWNS on-chain (the #81 ownership pattern) — a
/// hostile `*.localharness.xyz` a victim merely visits can no longer have the
/// apex signer decrypt the victim's on-chain Gemini key. Sealing
/// ([`run_seal_key`]) stays open cross-origin: it only returns ciphertext of
/// the CALLER's own value and writes nothing.
fn seed_sync_key() -> Result<[u8; 32], String> {
    let entropy = super::APP
        .with(|cell| cell.borrow().wallet.as_ref().map(|w| w.mnemonic.to_entropy()))
        .ok_or_else(|| "no identity on this device".to_string())?;
    // Single derivation, shared with the local-first path in verify.rs.
    Ok(super::encryption::keysync_key_from_entropy(&entropy))
}

/// Seal a plaintext (the tenant's Gemini key) with the seed-derived key
/// and return the ciphertext hex.
async fn run_seal_key(plaintext: String) -> Result<ReplyFields, String> {
    let key = seed_sync_key()?;
    let ct = super::encryption::seal_with_raw_key(&key, plaintext.as_bytes())
        .await
        .ok_or_else(|| "seal failed".to_string())?;
    Ok(vec![("ciphertext", bytes_to_hex_str(&ct))])
}

/// Open seed-sealed ciphertext → plaintext (the Gemini key), but ONLY for a
/// subdomain THIS identity owns on-chain. The seed-derived keysync key opens
/// ANY ciphertext, so without this gate a hostile `*.localharness.xyz` a victim
/// merely VISITS could ask the apex signer to decrypt the victim's (public,
/// on-chain) sealed key — the confused-deputy the `seed_sync_key` note flagged.
/// Mirrors the #81 tx-ownership gate: the legit cross-origin restore is the
/// user's OWN subdomain pulling its own key (passes); a foreign/missing owner or
/// an RPC error fails CLOSED. The local-first path (`verify::open_key_via_iframe`)
/// never reaches here on a seed-bearing device.
async fn run_open_key(origin: String, ciphertext_hex: String) -> Result<ReplyFields, String> {
    let (_, address) = wallet_handle()?;
    let sub = super::tenant::tenant_name_from_origin(&origin).ok_or_else(|| {
        "refusing to open a sealed key: request is not from a tenant subdomain".to_string()
    })?;
    match crate::registry::owner_of_name(&sub).await {
        Ok(Some(owner)) if owner.eq_ignore_ascii_case(&bytes_to_hex_str(&address)) => {}
        Ok(_) => {
            return Err(format!(
                "refusing to open a sealed key for '{sub}': that subdomain is not owned by this identity"
            ));
        }
        Err(e) => return Err(format!("open-key ownership check failed: {e}")),
    }
    let key = seed_sync_key()?;
    let ct = hex_to_bytes(&ciphertext_hex)?;
    let pt = super::encryption::open_with_raw_key(&key, &ct)
        .await
        .ok_or_else(|| "open failed (wrong seed?)".to_string())?;
    let s = String::from_utf8(pt).map_err(|_| "decrypted value not utf-8".to_string())?;
    Ok(vec![("plaintext", s)])
}

/// Long-running: `register(name)` on the registry (sponsored), then wait
/// for the receipt. Replies once with the address + tx hash. Tenant
/// first-claim sets this off and shows a progress placeholder until the
/// reply lands.
async fn run_claim_name_op(name: String) -> Result<ReplyFields, String> {
    let (address_hex, tx_hash) = run_claim_name(&name).await?;
    Ok(vec![("address", address_hex), ("tx_hash", tx_hash)])
}

async fn run_claim_name(name: &str) -> Result<(String, String), String> {
    let (signer, address) = wallet_handle()?;
    let address_hex = bytes_to_hex_str(&address);
    // Sponsored path: sender (user's wallet) holds zero, fee_payer
    // (bundle's sponsor) pays gas in AlphaUSD. No faucet drip
    // required — users get on-chain in one click with no native gas.
    let fee_payer = super::sponsor::signer()?;
    let tx_hash = crate::registry::claim_and_maybe_set_main_sponsored(
        &signer,
        &fee_payer,
        name,
        crate::registry::ALPHA_USD_ADDRESS(),
    )
    .await?;
    Ok((address_hex, tx_hash))
}

async fn run_import_seed(phrase: String) -> Result<ReplyFields, String> {
    let wallet = super::wallet_store::import(&phrase).await?;
    let addr = wallet.address_hex();
    super::APP.with(|cell| cell.borrow_mut().wallet = Some(wallet));
    Ok(vec![("address", addr)])
}

fn build_challenge_response(id: &str, nonce_hex: &str, name: &str) -> Result<JsValue, String> {
    let nonce = hex_to_bytes(nonce_hex)?;
    // Domain-separated digest the signer commits to. Binds the subdomain
    // `name` (and a random nonce) so a captured owner-proof for one name
    // can't be replayed as proof for another name held by the same
    // address. ONE definition shared with the verifying side — see
    // `signer_protocol::challenge_prehash`.
    let prehash = challenge_prehash(name, &nonce);

    let (signer, address) = wallet_handle()?;
    let signature = wallet::sign_hash(&signer, &prehash);

    Ok(success_response(
        id,
        &[
            ("address", bytes_to_hex_str(&address)),
            ("signature", bytes_to_hex_str(&signature)),
        ],
    ))
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
async fn build_sponsored_tx_response(
    data: JsValue,
    purpose: String,
    origin: String,
) -> Result<ReplyFields, String> {
    let tx_obj = js_sys::Reflect::get(&data, &JsValue::from_str("tx"))
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
    if chain_id != crate::registry::CHAIN_ID() {
        return Err(format!("chainId {chain_id} != {}", crate::registry::CHAIN_ID()));
    }

    let fee_priority =
        parse_hex_quantity(&get_str("maxPriorityFeePerGas").ok_or("maxPriorityFeePerGas missing")?)?;
    let fee_max = parse_hex_quantity(&get_str("maxFeePerGas").ok_or("maxFeePerGas missing")?)?;
    let gas_limit = parse_hex_quantity(&get_str("gasLimit").ok_or("gasLimit missing")?)?;
    let nonce = parse_hex_quantity(&get_str("nonce").ok_or("nonce missing")?)?;
    let fee_token = match get_str("feeToken") {
        Some(s) if !s.trim().trim_start_matches("0x").is_empty() => Some(parse_addr20(&s)?),
        _ => None,
    };
    let sponsored = get("sponsored").and_then(|v| v.as_bool()).unwrap_or(false);

    // Call-target allowlist — the heart of the fix.
    let registry_addr = parse_addr20(crate::registry::REGISTRY_ADDRESS())?;
    let token_addr = parse_addr20(crate::registry::LOCALHARNESS_TOKEN_ADDRESS())?;

    let calls_val = get("calls").ok_or_else(|| "tx.calls missing".to_string())?;
    let calls_arr: js_sys::Array = calls_val
        .dyn_into()
        .map_err(|_| "tx.calls not an array".to_string())?;
    if calls_arr.length() == 0 {
        return Err("tx.calls empty".into());
    }
    // #81: allowlisting the $LH token by TARGET alone is a confused-deputy
    // drain — the token accepts value-moving calls, so any trusted subdomain
    // (incl. a hostile one the victim merely VISITS) could have the master sign
    // `transfer(attacker, balance)`. Gate the token's calldata: `approve` may
    // only target the diamond (escrow), `transferFrom` is never signed here, and
    // a `transfer` (send_lh) is signed ONLY for a subdomain the master OWNS
    // (the on-chain ownership check below, after the loop).
    let mut needs_owner_check = false;
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
                bytes_to_hex_str(&to)
            ));
        }
        let value_wei = parse_hex_quantity(&cval)?;
        if value_wei != 0 {
            return Err("refusing to sign: native value transfer not permitted".into());
        }
        let input = if cinput.trim().trim_start_matches("0x").is_empty() {
            Vec::new()
        } else {
            hex_to_bytes(&cinput)?
        };
        // Per-call $LH-token calldata policy (#81).
        if to == token_addr {
            match input.get(0..4) {
                // transferFrom(address,address,uint256) — never via this path
                // (the master pushes funds with `transfer`; `transferFrom` is the
                // diamond pulling, not a cross-origin-signable master action).
                Some([0x23, 0xb8, 0x72, 0xdd]) => {
                    return Err("refusing to sign: $LH transferFrom is not permitted via the cross-origin signer".into());
                }
                // approve(address,uint256) — spender (arg0, input[16..36]) must be
                // the diamond, so a hostile page can't approve itself to drain.
                Some([0x09, 0x5e, 0xa7, 0xb3]) => {
                    if input.get(16..36) != Some(registry_addr.as_slice()) {
                        return Err("refusing to sign: $LH approve must target the diamond".into());
                    }
                }
                // transfer(address,uint256) — send_lh. Only legit from a subdomain
                // the master owns; verified on-chain after the loop.
                Some([0xa9, 0x05, 0x9c, 0xbb]) => {
                    needs_owner_check = true;
                }
                // Other selectors move no $LH (e.g. accidental no-ops) — allowed.
                _ => {}
            }
        }
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
    if let Some(claimed) = js_sys::Reflect::get(&data, &JsValue::from_str("digest"))
        .ok()
        .and_then(|v| v.as_string())
    {
        if let Ok(claimed_bytes) = hex_to_bytes(&claimed) {
            if claimed_bytes.as_slice() != sender_hash {
                return Err("provided digest does not match reconstructed sender_hash".into());
            }
        }
    }

    let (signer, address) = wallet_handle()?;

    // #81 ownership gate: a $LH `transfer` is only signed for a subdomain THIS
    // identity owns on-chain — closing the confused-deputy where visiting a
    // hostile `*.localharness.xyz` (which is `is_trusted_origin`) makes the
    // master sign `transfer(attacker, balance)`. Fails CLOSED (refuse) on a
    // missing/foreign owner or an RPC error — send_lh from the user's own
    // subdomain still works; a drain from someone else's does not.
    if needs_owner_check {
        let sub = super::tenant::tenant_name_from_origin(&origin).ok_or_else(|| {
            "refusing to sign a $LH transfer: request is not from a tenant subdomain".to_string()
        })?;
        match crate::registry::owner_of_name(&sub).await {
            Ok(Some(owner)) if owner.eq_ignore_ascii_case(&bytes_to_hex_str(&address)) => {}
            Ok(_) => {
                return Err(format!(
                    "refusing to sign a $LH transfer for '{sub}': that subdomain is not owned by this identity"
                ));
            }
            Err(e) => return Err(format!("$LH transfer ownership check failed: {e}")),
        }
    }

    web_sys::console::log_1(&JsValue::from_str(&format!(
        "lh-sign-digest: signed reconstructed sponsored tx ({purpose}, {} allowlisted call(s))",
        rebuilt.calls.len(),
    )));
    let sig = wallet::sign_hash(&signer, &sender_hash);

    Ok(vec![
        ("address", bytes_to_hex_str(&address)),
        ("signature", bytes_to_hex_str(&sig)),
    ])
}

/// Parse a `0x`-optional 20-byte hex address. Thin fixed-length wrapper over
/// [`crate::encoding::parse_address`] that also tolerates surrounding
/// whitespace (the fields arrive as JS strings).
fn parse_addr20(s: &str) -> Result<[u8; 20], String> {
    crate::encoding::parse_address(s.trim())
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
    set(&obj, "type", JsValue::from_str(MSG_SIGN_RESPONSE));
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

