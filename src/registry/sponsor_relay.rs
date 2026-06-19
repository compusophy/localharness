//! Sponsor RELAY client — the fee_payer half, fetched server-side.
//!
//! On MAINNET the published crate holds NO fee_payer key (design/cli-mainnet-
//! relay.md §2.2). Instead this asks the credit proxy's `POST /api/sponsor`
//! route to sign the fee_payer authorization of an already-sender-signed Tempo
//! 0x76 tx. The relay re-derives the fee_payer hash from the submitted intent,
//! runs its abuse caps (selector allowlist + rate window + onboarding-only
//! gate), signs, and returns the 65-byte signature. We re-verify locally before
//! using it — the relay never hands us an opaque hash to trust.
//!
//! Auth reuses the existing personal-sign proxy token (`proxy_auth_token`); the
//! SENDER is the authenticated caller. The relay never submits — we assemble the
//! final tx and broadcast it, so a relay outage degrades to "no sponsorship",
//! never a half-sent tx (fail-closed; never silently self-pay — that hits the
//! zero-funds + native-transfer-ban trap).

use super::*;
use crate::encoding::{bytes_to_hex, hex_to_bytes};
use crate::tempo_tx::TempoTx;

/// Current unix seconds (cross-target — the relay path runs on native CLI today
/// but the function compiles on wasm too).
fn now_secs() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        (js_sys::Date::now() / 1000.0) as u64
    }
}

/// The relay endpoint: `<CREDIT_PROXY_URL>/api/sponsor`.
fn relay_url() -> String {
    format!("{}api/sponsor", CREDIT_PROXY_URL)
}

/// JSON body for `POST /api/sponsor` (mirrors `proxy/api/sponsor.ts::parseRequest`):
/// the sender-signed INTENT fields the relay re-derives the fee_payer hash from.
fn build_request_json(
    tx: &TempoTx,
    sender_address: &[u8; 20],
    sender_sig: &[u8; 65],
) -> Result<serde_json::Value, String> {
    let fee_token = tx
        .fee_token
        .ok_or_else(|| "sponsored tx must set a fee_token for the relay".to_string())?;
    let calls: Vec<serde_json::Value> = tx
        .calls
        .iter()
        .map(|c| {
            serde_json::json!({
                "to": format!("0x{}", bytes_to_hex(&c.to)),
                "value": c.value_wei.to_string(),
                "input": format!("0x{}", bytes_to_hex(&c.input)),
            })
        })
        .collect();
    Ok(serde_json::json!({
        "chainId": tx.chain_id.to_string(),
        "maxPriorityFeePerGas": tx.max_priority_fee_per_gas.to_string(),
        "maxFeePerGas": tx.max_fee_per_gas.to_string(),
        "gasLimit": tx.gas_limit.to_string(),
        "calls": calls,
        "nonceKey": tx.nonce_key.to_string(),
        "nonce": tx.nonce.to_string(),
        "validBefore": tx.valid_before,
        "validAfter": tx.valid_after,
        "feeToken": format!("0x{}", bytes_to_hex(&fee_token)),
        "senderAddress": format!("0x{}", bytes_to_hex(sender_address)),
        "senderSignature": format!("0x{}", bytes_to_hex(sender_sig)),
    }))
}

/// Parse a 65-byte hex signature (`0x` + 130 hex) from the relay reply.
fn parse_sig_65(hex: &str) -> Result<[u8; 65], String> {
    let bytes = hex_to_bytes(hex).map_err(|e| format!("bad feePayerSignature hex: {e}"))?;
    bytes
        .try_into()
        .map_err(|_| "feePayerSignature must be 65 bytes".to_string())
}

/// Ask the relay to sign the fee_payer half of `tx` (already sender-signed via
/// `sender_sig`). Returns the 65-byte fee_payer signature, VERIFIED to (a) be
/// over the exact fee_payer hash we recompute locally, and (b) recover to the
/// `feePayer` address the relay advertises. `sender` authenticates the request
/// (its personal-sign proxy token); it must be the address that produced
/// `sender_sig`.
pub async fn request_fee_payer_signature(
    sender: &k256::ecdsa::SigningKey,
    tx: &TempoTx,
    sender_address: &[u8; 20],
    sender_sig: &[u8; 65],
) -> Result<[u8; 65], String> {
    let token = proxy_auth_token(sender, now_secs());
    let body = build_request_json(tx, sender_address, sender_sig)?;

    let resp = reqwest::Client::new()
        .post(relay_url())
        .header("x-goog-api-key", token)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("sponsor relay unreachable: {e} — sponsored writes unavailable"))?;

    let status = resp.status();
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("sponsor relay returned non-JSON: {e}"))?;

    if !status.is_success() {
        let code = json.get("code").and_then(|c| c.as_str()).unwrap_or("LH_RELAY");
        let msg = json.get("error").and_then(|m| m.as_str()).unwrap_or("(no message)");
        return Err(format!("sponsor relay refused ({code}): {msg}"));
    }

    let local_hash = tx.fee_payer_hash(sender_address);
    verify_relay_reply(&json, &local_hash)
}

/// Verify a SUCCESS reply against the locally-recomputed fee_payer hash and
/// return the 65-byte signature. Defense against a buggy or hostile relay:
/// `feePayerHash` MUST equal the hash WE'D sign, and `feePayerSignature` MUST
/// recover to the advertised `feePayer`. All three fields are REQUIRED — a reply
/// that OMITS one is rejected, so the checks can't be silently bypassed by
/// leaving a field out (the prior `if let Some` form skipped the check when the
/// field was absent). Pure (no I/O) so the money-path verification is unit-tested.
fn verify_relay_reply(
    json: &serde_json::Value,
    local_hash: &[u8; 32],
) -> Result<[u8; 65], String> {
    let sig_hex = json
        .get("feePayerSignature")
        .and_then(|s| s.as_str())
        .ok_or_else(|| "relay reply missing feePayerSignature".to_string())?;
    let fp_sig = parse_sig_65(sig_hex)?;

    // No blind trust: the relay must have signed the SAME fee_payer hash we'd
    // sign locally.
    let returned = json
        .get("feePayerHash")
        .and_then(|h| h.as_str())
        .ok_or_else(|| "relay reply missing feePayerHash".to_string())?;
    let want = format!("0x{}", bytes_to_hex(local_hash));
    if !returned.eq_ignore_ascii_case(&want) {
        return Err(format!(
            "relay feePayerHash {returned} != locally-derived {want} — refusing to submit"
        ));
    }

    // The signature must recover to the fee_payer address the relay advertises
    // (a malformed/foreign sig would otherwise mine to a phantom payer and waste
    // the user's submit).
    let recovered = crate::wallet::recover_address(&fp_sig, local_hash)
        .map_err(|e| format!("relay fee_payer sig invalid: {e}"))?;
    let advertised = json
        .get("feePayer")
        .and_then(|f| f.as_str())
        .ok_or_else(|| "relay reply missing feePayer".to_string())?;
    let got = format!("0x{}", bytes_to_hex(&recovered));
    if !advertised.eq_ignore_ascii_case(&got) {
        return Err(format!(
            "relay fee_payer sig recovers {got} but advertised {advertised}"
        ));
    }
    Ok(fp_sig)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_json_shape_matches_proxy() {
        // The JSON field names/types `proxy/api/sponsor.ts::parseRequest` expects.
        let mut tx = crate::tempo_tx::TempoTxBuilder::new(42431)
            .max_priority_fee_per_gas(1_000_000_000)
            .max_fee_per_gas(2_000_000_000)
            .gas_limit(1_500_000)
            .nonce(7)
            .fee_token([0x20u8; 20])
            .call(crate::tempo_tx::TempoCall { to: [0x6c; 20], value_wei: 0, input: vec![0xde, 0xad] })
            .sponsored()
            .build();
        tx.nonce_key = 0;
        let sender = [0x11u8; 20];
        let sig = [0x22u8; 65];
        let j = build_request_json(&tx, &sender, &sig).unwrap();

        assert_eq!(j["chainId"], "42431");
        assert!(j["chainId"].is_string(), "ints are decimal STRINGS (u128 precision)");
        assert_eq!(j["maxFeePerGas"], "2000000000");
        assert_eq!(j["gasLimit"], "1500000");
        assert_eq!(j["nonce"], "7");
        assert_eq!(j["nonceKey"], "0");
        assert_eq!(j["validBefore"], serde_json::Value::Null);
        assert_eq!(j["calls"][0]["to"], "0x6c6c6c6c6c6c6c6c6c6c6c6c6c6c6c6c6c6c6c6c");
        assert_eq!(j["calls"][0]["value"], "0");
        assert_eq!(j["calls"][0]["input"], "0xdead");
        assert_eq!(j["senderAddress"], "0x1111111111111111111111111111111111111111");
        assert_eq!(j["senderSignature"].as_str().unwrap().len(), 2 + 130);
        assert_eq!(j["feeToken"], "0x2020202020202020202020202020202020202020");
    }

    #[test]
    fn relay_url_joins_without_double_slash() {
        let url = relay_url();
        assert!(url.ends_with("/api/sponsor"));
        assert!(!url.contains("//api"), "base already has a trailing slash");
    }

    #[test]
    fn verify_relay_reply_accepts_valid_and_rejects_tampering() {
        let k = crate::wallet::generate();
        let local_hash = [0x42u8; 32];
        let fp_sig = crate::wallet::sign_hash(&k.signer, &local_hash);
        let good = serde_json::json!({
            "feePayerSignature": format!("0x{}", bytes_to_hex(&fp_sig)),
            "feePayerHash": format!("0x{}", bytes_to_hex(&local_hash)),
            "feePayer": format!("0x{}", bytes_to_hex(&k.address)),
        });
        // A well-formed reply over the EXPECTED hash returns the signature.
        assert_eq!(verify_relay_reply(&good, &local_hash).unwrap(), fp_sig);

        // OMITTING any required field is rejected — the hardening: previously an
        // absent feePayerHash/feePayer SKIPPED its check (silent bypass).
        for field in ["feePayerSignature", "feePayerHash", "feePayer"] {
            let mut j = good.clone();
            j.as_object_mut().unwrap().remove(field);
            assert!(
                verify_relay_reply(&j, &local_hash).unwrap_err().contains("missing"),
                "omitting {field} must be rejected"
            );
        }

        // A returned hash that isn't the one we'd sign → refuse (even with a sig
        // valid over the REAL local hash).
        let mut wrong_hash = good.clone();
        wrong_hash["feePayerHash"] = serde_json::json!(format!("0x{}", bytes_to_hex(&[0x99u8; 32])));
        assert!(verify_relay_reply(&wrong_hash, &local_hash)
            .unwrap_err()
            .contains("!= locally-derived"));

        // An advertised feePayer that isn't the sig's actual signer → refuse.
        let mut wrong_payer = good.clone();
        wrong_payer["feePayer"] = serde_json::json!("0x00000000000000000000000000000000000000ff");
        assert!(verify_relay_reply(&wrong_payer, &local_hash)
            .unwrap_err()
            .contains("advertised"));
    }
}
