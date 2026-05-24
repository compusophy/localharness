//! JSON-RPC client for `LocalharnessRegistry` — read AND write.
//!
//! Hand-rolled instead of pulling alloy: the apex chrome only needs a
//! handful of methods (`eth_call`, `eth_chainId`, `eth_gasPrice`,
//! `eth_getTransactionCount`, `eth_estimateGas`,
//! `eth_sendRawTransaction`, `eth_getTransactionReceipt`) and we
//! already have `reqwest` in the bundle. Avoiding alloy also sidesteps
//! the `serde::__private` compat snag we hit during the M6 spike.
//!
//! When `REGISTRY_ADDRESS` is the zero address the contract isn't
//! deployed yet — every query returns `Status::Unknown` so the UI can
//! degrade gracefully ("(registry pending deploy)") instead of
//! erroring.

use k256::ecdsa::SigningKey;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

use crate::wallet;

/// Tempo Moderato testnet RPC. Per the tempo-x402 reference.
pub(crate) const RPC_URL: &str = "https://rpc.moderato.tempo.xyz";

/// `LocalharnessRegistry` on Tempo Moderato testnet (chain id 42431).
/// Deployed 2026-05-23 from `contracts/script/Deploy.s.sol`. Update
/// by re-deploying + bumping this constant + rebuilding the bundle.
pub(crate) const REGISTRY_ADDRESS: &str = "0x42c8D4EaF99bA80F6B6FCA8E163E077D9FC2F9db";

/// Tempo Moderato chain id — used in EIP-155 v computation.
pub(crate) const CHAIN_ID: u64 = 42431;

/// What we can learn about a name without touching the wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    /// Registry isn't deployed (or address still set to zero).
    Unknown,
    /// `idOfName` returned 0 — free to register.
    Available,
    /// `idOfName` returned a non-zero agentId.
    Taken { agent_id: u64 },
}

/// `eth_call idOfName(name)` and classify the result. Single round trip.
pub(crate) async fn check_name(name: &str) -> Result<Status, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(Status::Unknown);
    }

    let calldata = encode_id_of_name(name);
    let result_hex = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    let id = decode_u256_as_u64(&result_hex)?;
    Ok(if id == 0 {
        Status::Available
    } else {
        Status::Taken { agent_id: id }
    })
}

/// Register `name` on the contract under the given signer's address.
/// Returns the transaction hash once it's been included in a block.
/// The wallet needs testnet TMP for gas — the apex page is expected
/// to faucet-fund it on first claim attempt.
pub(crate) async fn claim_name(signer: &SigningKey, name: &str) -> Result<String, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Err("registry not deployed".into());
    }
    let from = wallet::address(signer);
    let from_hex = address_to_hex(&from);

    // Pull live tx parameters in parallel-ish (they're cheap reads).
    let nonce = eth_get_transaction_count(&from_hex).await?;
    let gas_price = eth_gas_price().await?;
    let calldata_hex = encode_register(name);
    let gas_limit = eth_estimate_gas(&from_hex, REGISTRY_ADDRESS, &calldata_hex).await?;

    // EIP-155 legacy tx: keccak the unsigned RLP, sign, RLP the
    // signed envelope. v = chain_id*2 + 35 + recoveryId.
    let calldata_bytes = hex_to_bytes(&calldata_hex)?;
    let unsigned = rlp_legacy_unsigned(
        nonce,
        gas_price,
        gas_limit,
        REGISTRY_ADDRESS,
        0,
        &calldata_bytes,
        CHAIN_ID,
    )?;
    let mut hasher = Keccak256::new();
    hasher.update(&unsigned);
    let mut prehash = [0u8; 32];
    prehash.copy_from_slice(&hasher.finalize());

    let sig = wallet::sign_hash(signer, &prehash);
    let r = &sig[..32];
    let s = &sig[32..64];
    // sig[64] is 27 + recoveryId in our wallet's output; lift it back
    // to a 0/1 recovery id for EIP-155 v derivation.
    let rec_id = (sig[64] - 27) as u64;
    let v = CHAIN_ID * 2 + 35 + rec_id;

    let signed = rlp_legacy_signed(
        nonce,
        gas_price,
        gas_limit,
        REGISTRY_ADDRESS,
        0,
        &calldata_bytes,
        v,
        r,
        s,
    )?;
    let raw_hex = format!("0x{}", bytes_to_hex(&signed));

    let tx_hash = eth_send_raw_transaction(&raw_hex).await?;
    // Wait for the receipt — claim should be confirmed before the
    // UI navigates the user away.
    wait_for_receipt(&tx_hash).await?;
    Ok(tx_hash)
}

/// Best-effort: hit the Tempo `tempo_fundAddress` faucet for the
/// supplied address. The faucet returns the funding tx hashes on
/// success. Bundled here because the apex claim flow uses it
/// pre-emptively before a brand-new wallet tries to send its first tx.
pub(crate) async fn request_faucet_funds(address_hex: &str) -> Result<(), String> {
    let body = RpcRequest {
        jsonrpc: "2.0",
        id: 1,
        method: "tempo_fundAddress",
        params: serde_json::json!([address_hex]),
    };
    let client = reqwest::Client::new();
    let resp = client
        .post(RPC_URL)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("faucet send: {e}"))?;
    let parsed: RpcResponse = resp
        .json()
        .await
        .map_err(|e| format!("faucet decode: {e}"))?;
    if let Some(err) = parsed.error {
        return Err(format!("faucet: {}", err.message));
    }
    Ok(())
}

// --- ABI encoding -------------------------------------------------------

/// Function selector = first 4 bytes of keccak256("<sig>").
fn selector(signature: &str) -> [u8; 4] {
    let mut h = Keccak256::new();
    h.update(signature.as_bytes());
    let digest = h.finalize();
    let mut out = [0u8; 4];
    out.copy_from_slice(&digest[..4]);
    out
}

/// Encode `idOfName(string)` calldata. ABI layout:
///   [0..4]     selector
///   [4..36]    offset to string head (always 0x20 for one dynamic arg)
///   [36..68]   string length (uint256, big-endian)
///   [68..]     string bytes, right-padded to 32-byte multiple
fn encode_id_of_name(name: &str) -> String {
    let sel = selector("idOfName(string)");
    let bytes = name.as_bytes();
    let len = bytes.len();
    let padded_len = ((len + 31) / 32) * 32;

    let mut buf = Vec::with_capacity(4 + 32 + 32 + padded_len);
    buf.extend_from_slice(&sel);
    buf.extend_from_slice(&u256_be(0x20));
    buf.extend_from_slice(&u256_be(len as u128));
    buf.extend_from_slice(bytes);
    buf.resize(4 + 32 + 32 + padded_len, 0);

    let mut out = String::with_capacity(2 + buf.len() * 2);
    out.push_str("0x");
    for b in &buf {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Encode `register(string)` calldata. Same shape as `idOfName`.
fn encode_register(name: &str) -> String {
    let sel = selector("register(string)");
    let bytes = name.as_bytes();
    let len = bytes.len();
    let padded_len = ((len + 31) / 32) * 32;

    let mut buf = Vec::with_capacity(4 + 32 + 32 + padded_len);
    buf.extend_from_slice(&sel);
    buf.extend_from_slice(&u256_be(0x20));
    buf.extend_from_slice(&u256_be(len as u128));
    buf.extend_from_slice(bytes);
    buf.resize(4 + 32 + 32 + padded_len, 0);

    let mut out = String::with_capacity(2 + buf.len() * 2);
    out.push_str("0x");
    for b in &buf {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn u256_be(value: u128) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&value.to_be_bytes());
    out
}

fn decode_u256_as_u64(hex: &str) -> Result<u64, String> {
    let stripped = hex.trim().trim_start_matches("0x");
    if stripped.is_empty() {
        return Ok(0);
    }
    if stripped.len() > 64 {
        return Err(format!("u256 hex too long: {}", stripped.len()));
    }
    // High bytes must be zero for u64.
    let high_end = stripped.len().saturating_sub(16);
    if stripped[..high_end].chars().any(|c| c != '0') {
        return Err("u256 exceeds u64 range".into());
    }
    let tail = &stripped[high_end..];
    u64::from_str_radix(tail, 16).map_err(|e| e.to_string())
}

fn zero_address() -> &'static str {
    "0x0000000000000000000000000000000000000000"
}

fn address_to_hex(addr: &[u8; 20]) -> String {
    let mut s = String::with_capacity(42);
    s.push_str("0x");
    for b in addr {
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
        let hi = nibble_value(bytes[i])?;
        let lo = nibble_value(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn nibble_value(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(format!("non-hex byte {b}")),
    }
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn parse_hex_quantity(hex: &str) -> Result<u128, String> {
    let trimmed = hex.trim().trim_start_matches("0x");
    if trimmed.is_empty() {
        return Ok(0);
    }
    u128::from_str_radix(trimmed, 16).map_err(|e| e.to_string())
}

// --- legacy / EIP-155 transaction RLP --------------------------------

#[allow(clippy::too_many_arguments)]
fn rlp_legacy_unsigned(
    nonce: u128,
    gas_price: u128,
    gas_limit: u128,
    to_hex: &str,
    value: u128,
    data: &[u8],
    chain_id: u64,
) -> Result<Vec<u8>, String> {
    let to_bytes = hex_to_bytes(to_hex)?;
    // EIP-155: rlp([nonce, gasPrice, gasLimit, to, value, data, chainId, 0, 0])
    let items = vec![
        wallet::rlp_uint(nonce),
        wallet::rlp_uint(gas_price),
        wallet::rlp_uint(gas_limit),
        wallet::rlp_bytes(&to_bytes),
        wallet::rlp_uint(value),
        wallet::rlp_bytes(data),
        wallet::rlp_uint(chain_id as u128),
        wallet::rlp_uint(0),
        wallet::rlp_uint(0),
    ];
    Ok(wallet::rlp_list(&items))
}

#[allow(clippy::too_many_arguments)]
fn rlp_legacy_signed(
    nonce: u128,
    gas_price: u128,
    gas_limit: u128,
    to_hex: &str,
    value: u128,
    data: &[u8],
    v: u64,
    r: &[u8],
    s: &[u8],
) -> Result<Vec<u8>, String> {
    let to_bytes = hex_to_bytes(to_hex)?;
    // r and s are 32 bytes each; RLP wants minimal-leading-zero
    // representations. Strip leading zeros (but not all if the value is 0).
    let r_min = strip_leading_zeros(r);
    let s_min = strip_leading_zeros(s);
    let items = vec![
        wallet::rlp_uint(nonce),
        wallet::rlp_uint(gas_price),
        wallet::rlp_uint(gas_limit),
        wallet::rlp_bytes(&to_bytes),
        wallet::rlp_uint(value),
        wallet::rlp_bytes(data),
        wallet::rlp_uint(v as u128),
        wallet::rlp_bytes(r_min),
        wallet::rlp_bytes(s_min),
    ];
    Ok(wallet::rlp_list(&items))
}

fn strip_leading_zeros(bytes: &[u8]) -> &[u8] {
    let first_nz = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len() - 1);
    &bytes[first_nz..]
}

// --- JSON-RPC plumbing --------------------------------------------------

#[derive(Serialize)]
struct RpcRequest<'a> {
    jsonrpc: &'a str,
    id: u32,
    method: &'a str,
    params: serde_json::Value,
}

#[derive(Deserialize)]
struct RpcResponse {
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    #[allow(dead_code)]
    code: i64,
    message: String,
}

async fn rpc(method: &str, params: serde_json::Value) -> Result<String, String> {
    let body = RpcRequest {
        jsonrpc: "2.0",
        id: 1,
        method,
        params,
    };
    let client = reqwest::Client::new();
    let resp = client
        .post(RPC_URL)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("{method} send: {e}"))?;
    let parsed: RpcResponse = resp
        .json()
        .await
        .map_err(|e| format!("{method} decode: {e}"))?;
    if let Some(err) = parsed.error {
        return Err(format!("{method}: {}", err.message));
    }
    parsed
        .result
        .ok_or_else(|| format!("{method} returned no result"))
}

async fn eth_call(to: &str, data_hex: &str) -> Result<String, String> {
    rpc(
        "eth_call",
        serde_json::json!([{ "to": to, "data": data_hex }, "latest"]),
    )
    .await
}

async fn eth_get_transaction_count(addr: &str) -> Result<u128, String> {
    let hex = rpc(
        "eth_getTransactionCount",
        serde_json::json!([addr, "pending"]),
    )
    .await?;
    parse_hex_quantity(&hex)
}

async fn eth_gas_price() -> Result<u128, String> {
    let hex = rpc("eth_gasPrice", serde_json::json!([])).await?;
    parse_hex_quantity(&hex)
}

async fn eth_estimate_gas(from: &str, to: &str, data_hex: &str) -> Result<u128, String> {
    let hex = rpc(
        "eth_estimateGas",
        serde_json::json!([{ "from": from, "to": to, "data": data_hex }]),
    )
    .await?;
    // Add a 25% buffer so we don't get caught by gas-estimation jitter.
    let estimate = parse_hex_quantity(&hex)?;
    Ok(estimate + estimate / 4)
}

async fn eth_send_raw_transaction(raw_hex: &str) -> Result<String, String> {
    rpc("eth_sendRawTransaction", serde_json::json!([raw_hex])).await
}

/// Poll `eth_getTransactionReceipt` until the receipt resolves. Errors
/// after ~30 seconds — Tempo Moderato blocks are ~1s so 30 attempts
/// is more than enough headroom.
async fn wait_for_receipt(tx_hash: &str) -> Result<(), String> {
    for _ in 0..30 {
        let body = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "eth_getTransactionReceipt",
            params: serde_json::json!([tx_hash]),
        };
        let client = reqwest::Client::new();
        let resp = client
            .post(RPC_URL)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("receipt poll: {e}"))?;
        // Receipt comes back as an object or null — bypass the
        // RpcResponse string-only deserializer.
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("receipt parse: {e}"))?;
        if let Some(receipt) = json.get("result").filter(|v| !v.is_null()) {
            let status = receipt
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            if status == "0x1" {
                return Ok(());
            } else if status == "0x0" {
                return Err(format!("tx reverted: {tx_hash}"));
            }
        }
        // Wait ~1s before next poll. spawn_local + a 1s timer would
        // be cleaner; gloo_timers is an option if this becomes a
        // bottleneck. For now: a busy yield via JS microtask.
        sleep_ms(1000).await;
    }
    Err(format!("receipt timeout for {tx_hash}"))
}

async fn sleep_ms(ms: u32) {
    use wasm_bindgen_futures::JsFuture;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_matches_known_value() {
        // keccak256("idOfName(string)") = 0x127c388a...
        // Verified independently: `cast sig "idOfName(string)"`.
        let sel = selector("idOfName(string)");
        let hex: String = sel.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "127c388a");
    }

    #[test]
    fn encode_short_name_layout() {
        let cd = encode_id_of_name("abc");
        // selector + 0x20 offset + 0x03 length + "abc" + padding
        assert!(cd.starts_with("0x127c388a"));
        // Total length: "0x" + (4 + 32 + 32 + 32) bytes * 2 chars/byte
        assert_eq!(cd.len(), 2 + (4 + 32 + 32 + 32) * 2);
    }

    #[test]
    fn decode_zero_means_available() {
        // 32-byte zero word
        let z = format!("0x{}", "0".repeat(64));
        assert_eq!(decode_u256_as_u64(&z).unwrap(), 0);
    }

    #[test]
    fn decode_normal_id() {
        // agentId = 7
        let mut s = "0".repeat(63);
        s.push('7');
        let hex = format!("0x{s}");
        assert_eq!(decode_u256_as_u64(&hex).unwrap(), 7);
    }

    #[test]
    fn decode_oversize_errors() {
        // Bit set in the upper 192 bits — can't fit in u64.
        let mut s = String::from("1");
        s.push_str(&"0".repeat(63));
        let hex = format!("0x{s}");
        assert!(decode_u256_as_u64(&hex).is_err());
    }
}
