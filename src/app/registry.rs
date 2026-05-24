//! Read-side JSON-RPC client for `LocalharnessRegistry`.
//!
//! Hand-rolled instead of pulling alloy: the only operation the apex
//! chrome needs right now is `eth_call` to `idOfName(string)` and we
//! already have `reqwest` in the bundle. Sticking close to the metal
//! also avoids alloy's recent `serde::__private` compat snag (see
//! `src/wallet.rs`).
//!
//! When `REGISTRY_ADDRESS` is the zero address the contract isn't
//! deployed yet — every query returns `Status::Unknown` so the UI can
//! degrade gracefully ("(registry pending deploy)") instead of
//! erroring.

use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

/// Tempo Moderato testnet RPC. Per the tempo-x402 reference.
pub(crate) const RPC_URL: &str = "https://rpc.moderato.tempo.xyz";

/// Where `LocalharnessRegistry` lives on-chain. Replace this constant
/// with the deployed address after running `contracts/script/Deploy.s.sol`,
/// then rebuild the wasm bundle. Until then the registry checks
/// return `Status::Unknown` and the UI shows "(registry pending deploy)".
pub(crate) const REGISTRY_ADDRESS: &str = "0x0000000000000000000000000000000000000000";

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

async fn eth_call(to: &str, data_hex: &str) -> Result<String, String> {
    let body = RpcRequest {
        jsonrpc: "2.0",
        id: 1,
        method: "eth_call",
        params: serde_json::json!([
            { "to": to, "data": data_hex },
            "latest"
        ]),
    };
    let client = reqwest::Client::new();
    let resp = client
        .post(RPC_URL)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("rpc send: {e}"))?;
    let parsed: RpcResponse = resp
        .json()
        .await
        .map_err(|e| format!("rpc decode: {e}"))?;
    if let Some(err) = parsed.error {
        return Err(format!("rpc error: {}", err.message));
    }
    parsed.result.ok_or_else(|| "rpc returned no result".into())
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
