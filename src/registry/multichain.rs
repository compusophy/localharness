//! Generic, READ-ONLY multi-chain EVM JSON-RPC + ENS resolution. Lets an agent
//! check balances / read contracts / resolve ENS on OTHER EVM chains (Ethereum,
//! Base, Optimism, Arbitrum, Polygon, plus the active Tempo chain) instead of
//! falling back to `web_fetch` against third-party explorer APIs. NO writes, no
//! signing — every helper here is `eth_call` / `eth_getBalance` shaped.
//!
//! Reuses the crate's canonical codecs (`crate::encoding` for hex/address,
//! `super::abi` for `selector`/`u256_be`/`encode_call_hex`) and mirrors the
//! `eth_call`/`rpc_value` pattern in `rpc.rs`, but keyed on a per-chain RPC URL
//! rather than the hardcoded Tempo `RPC_URL()`. All returned data is UNTRUSTED.

use super::abi::{encode_call_hex, selector, u256_be};
use super::rpc::{read_client, timeout_send, RpcResponse};
use crate::encoding::{hex_to_bytes, parse_address, parse_hex_quantity};
use sha3::{Digest, Keccak256};

/// A curated supported chain: human name → RPC endpoint + EIP-155 chain id. RPCs
/// are public + CORS-enabled (`Access-Control-Allow-Origin: *`, verified
/// 2026-06-18) so a browser `fetch` reaches them directly; an agent calls them
/// by `name`. `tempo` is the active localharness chain (testnet or mainnet).
#[derive(Clone, Copy)]
pub struct EvmChain {
    /// Lower-case lookup key the agent passes (e.g. `"base"`).
    pub name: &'static str,
    /// Public, CORS-enabled JSON-RPC endpoint.
    pub rpc_url: &'static str,
    /// EIP-155 chain id.
    pub chain_id: u64,
}

/// The ENS registry on Ethereum mainnet (same address across networks that
/// deploy ENS). `resolver(bytes32)` → the per-name resolver contract.
pub const ENS_REGISTRY: &str = "0x00000000000C2E074eC69A0dFb2997BA6C7d2e1e";

/// The foreign (non-Tempo) curated chains — pure constants. Public CORS RPCs
/// chosen so the browser reaches them without the proxy (all verified
/// `Access-Control-Allow-Origin: *` + full-method 2026-06-18):
/// ethereum = ethereum-rpc.publicnode.com (cloudflare-eth.com was degraded);
/// base = mainnet.base.org; optimism = mainnet.optimism.io;
/// arbitrum = arb1.arbitrum.io/rpc; polygon = polygon-bor-rpc.publicnode.com
/// (polygon-rpc.com is API-gated). The ACTIVE localharness `tempo` row is
/// appended at call time by [`chains`] off [`super::chain::active`] — its rpc/id
/// are runtime now, so they can't live in a `const` slice.
const FOREIGN_CHAINS: &[EvmChain] = &[
    EvmChain { name: "ethereum", rpc_url: "https://ethereum-rpc.publicnode.com", chain_id: 1 },
    EvmChain { name: "base", rpc_url: "https://mainnet.base.org", chain_id: 8453 },
    EvmChain { name: "optimism", rpc_url: "https://mainnet.optimism.io", chain_id: 10 },
    EvmChain { name: "arbitrum", rpc_url: "https://arb1.arbitrum.io/rpc", chain_id: 42161 },
    EvmChain {
        name: "polygon",
        rpc_url: "https://polygon-bor-rpc.publicnode.com",
        chain_id: 137,
    },
];

/// The curated chain table: the foreign chains plus the ACTIVE localharness
/// `tempo` row (testnet/mainnet, resolved at call time). A `Vec` because the
/// `tempo` row's rpc/id are now runtime.
pub fn chains() -> Vec<EvmChain> {
    let mut v: Vec<EvmChain> = FOREIGN_CHAINS
        .iter()
        .map(|c| EvmChain { name: c.name, rpc_url: c.rpc_url, chain_id: c.chain_id })
        .collect();
    let active = super::chain::active();
    v.push(EvmChain { name: "tempo", rpc_url: active.rpc_url, chain_id: active.chain_id });
    v
}

/// Look up a curated chain by (case-insensitive, trimmed) name. `eth`/`mainnet`
/// alias Ethereum; `op` aliases Optimism; `arb` aliases Arbitrum; `matic`/`pol`
/// alias Polygon — the names agents reach for.
pub fn chain_by_name(name: &str) -> Option<EvmChain> {
    let n = name.trim().to_lowercase();
    let canon = match n.as_str() {
        "eth" | "mainnet" | "l1" => "ethereum",
        "op" => "optimism",
        "arb" => "arbitrum",
        "matic" | "pol" => "polygon",
        other => other,
    };
    chains().into_iter().find(|c| c.name == canon)
}

/// One JSON-RPC call against an EXPLICIT `rpc_url` (the multi-chain twin of
/// `rpc::rpc_value`, which is pinned to the Tempo `RPC_URL()`). Races send +
/// body-read against the shared deadline so a stalled node can't hang the turn.
async fn rpc_value_at(
    rpc_url: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let client = read_client();
    let url = rpc_url.to_string();
    let parsed: RpcResponse = timeout_send(method, async {
        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("{method} send: {e}"))?;
        resp.json::<RpcResponse>()
            .await
            .map_err(|e| format!("{method} decode: {e}"))
    })
    .await??;
    if let Some(err) = parsed.error {
        return Err(format!("{method}: {}", err.message));
    }
    parsed
        .result()
        .cloned()
        .ok_or_else(|| format!("{method} returned no result"))
}

/// `eth_call` on an explicit chain URL → raw result hex.
pub async fn eth_call_at(rpc_url: &str, to: &str, data_hex: &str) -> Result<String, String> {
    let v = rpc_value_at(
        rpc_url,
        "eth_call",
        serde_json::json!([{ "to": to, "data": data_hex }, "latest"]),
    )
    .await?;
    v.as_str()
        .map(str::to_string)
        .ok_or_else(|| "eth_call: expected string result".to_string())
}

/// Native coin balance (`eth_getBalance`, latest) of `address` on `chain`, in wei.
pub async fn native_balance(chain: &EvmChain, address: &str) -> Result<u128, String> {
    let _ = parse_address(address)?; // validate shape → clear error, not opaque RPC fault
    let v = rpc_value_at(
        chain.rpc_url,
        "eth_getBalance",
        serde_json::json!([address, "latest"]),
    )
    .await?;
    parse_hex_quantity(
        v.as_str()
            .ok_or_else(|| "eth_getBalance: expected string result".to_string())?,
    )
}

/// `balanceOf(address)` of `holder` on the ERC-20 `token` on `chain`, in token
/// units (raw, undecimalled). Decode is the low-128-bits-checked path.
pub async fn erc20_balance(chain: &EvmChain, token: &str, holder: &str) -> Result<u128, String> {
    let holder_bytes = parse_address(holder)?;
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(&holder_bytes);
    let data = encode_call_hex(selector("balanceOf(address)"), &[padded]);
    let result = eth_call_at(chain.rpc_url, token, &data).await?;
    super::tx::decode_u256_as_u128(&result)
}

/// Best-effort ERC-20 `symbol()` (string) on `chain` — `None` on any decode/RPC
/// failure (a token may not implement it, or return bytes32). Read-only metadata.
pub async fn erc20_symbol(chain: &EvmChain, token: &str) -> Option<String> {
    let data = encode_call_hex(selector("symbol()"), &[]);
    let hex = eth_call_at(chain.rpc_url, token, &data).await.ok()?;
    super::rpc::decode_string(&hex).map(|s| s.trim_matches('\0').to_string())
}

/// Best-effort ERC-20 `decimals()` (uint8) on `chain` — `None` on failure.
pub async fn erc20_decimals(chain: &EvmChain, token: &str) -> Option<u32> {
    let data = encode_call_hex(selector("decimals()"), &[]);
    let hex = eth_call_at(chain.rpc_url, token, &data).await.ok()?;
    parse_hex_quantity(&hex).ok().map(|n| n as u32)
}

/// Format a raw integer `value` scaled by `decimals` into a fixed-point decimal
/// STRING (no float — exact). Trims trailing zeros in the fraction. PURE.
pub fn format_units(value: u128, decimals: u32) -> String {
    if decimals == 0 {
        return value.to_string();
    }
    let scale = 10u128.checked_pow(decimals).unwrap_or(u128::MAX);
    let whole = value / scale;
    let frac = value % scale;
    if frac == 0 {
        return whole.to_string();
    }
    let mut frac_s = format!("{frac:0width$}", width = decimals as usize);
    while frac_s.ends_with('0') {
        frac_s.pop();
    }
    format!("{whole}.{frac_s}")
}

// --- ENS ---------------------------------------------------------------

/// EIP-137 `namehash(name)` — the recursive keccak that keys every ENS record.
/// `namehash("") = 0x00…00`; for `a.b.c`, `keccak(namehash(rest) ‖ keccak(label))`
/// folding LEFT-to-RIGHT from the TLD. PURE — unit-tested against the canonical
/// `namehash("eth")` vector.
pub fn namehash(name: &str) -> [u8; 32] {
    let mut node = [0u8; 32];
    let name = name.trim().trim_end_matches('.');
    if name.is_empty() {
        return node;
    }
    // Hash labels from the RIGHT (TLD) inward: node = keccak(node ‖ keccak(label)).
    for label in name.split('.').rev() {
        let label_hash = Keccak256::digest(label.as_bytes());
        let mut h = Keccak256::new();
        h.update(node);
        h.update(label_hash);
        node.copy_from_slice(&h.finalize());
    }
    node
}

/// Resolve an ENS name to its `addr` record on Ethereum mainnet (read-only):
/// `namehash` → ENS registry `resolver(bytes32)` → resolver `addr(bytes32)`.
/// `Ok(None)` for a name with no resolver set or an unset/zero `addr` (a clean
/// "unregistered" result, not an error); `Err` only on RPC/transport failure.
pub async fn resolve_ens(name: &str) -> Result<Option<String>, String> {
    let eth = chain_by_name("ethereum").ok_or("ethereum chain missing")?;
    let node = namehash(name);
    // 1) resolver(bytes32) on the ENS registry.
    let data = encode_call_hex(selector("resolver(bytes32)"), &[node]);
    let resolver_hex = eth_call_at(eth.rpc_url, ENS_REGISTRY, &data).await?;
    let Some(resolver) = super::rpc::decode_address(&resolver_hex) else {
        return Ok(None); // no resolver set
    };
    // 2) addr(bytes32) on that resolver.
    let data = encode_call_hex(selector("addr(bytes32)"), &[node]);
    let addr_hex = eth_call_at(eth.rpc_url, &resolver, &data).await?;
    Ok(super::rpc::decode_address(&addr_hex))
}

// --- generic eth_call from a human signature ---------------------------

/// ABI-encode calldata from a HUMAN function signature (e.g.
/// `"balanceOf(address)"`, `"name()"`) + string `args`, supporting the common
/// static value types: `address`, `bool`, `uintN`/`intN` (decimal or `0x` hex),
/// and `bytes32` (`0x`-hex, left-aligned). One arg per parameter; dynamic types
/// (string/bytes/arrays) are NOT supported here. Returns `0x`-hex calldata.
/// PURE — unit-tested.
pub fn encode_function_call(signature: &str, args: &[String]) -> Result<String, String> {
    let sig = signature.trim();
    let open = sig.find('(').ok_or("signature must look like name(types)")?;
    let close = sig.rfind(')').ok_or("signature missing closing ')'")?;
    let types: Vec<&str> = sig[open + 1..close]
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    if types.len() != args.len() {
        return Err(format!(
            "signature has {} parameter(s) but {} argument(s) were given",
            types.len(),
            args.len()
        ));
    }
    // Canonical selector uses the bare signature with no spaces.
    let canonical = format!("{}({})", &sig[..open], types.join(","));
    let mut words: Vec<[u8; 32]> = Vec::with_capacity(types.len());
    for (ty, arg) in types.iter().zip(args) {
        words.push(encode_static_arg(ty, arg.trim())?);
    }
    Ok(encode_call_hex(selector(&canonical), &words))
}

/// Encode ONE static ABI argument into its 32-byte word.
fn encode_static_arg(ty: &str, arg: &str) -> Result<[u8; 32], String> {
    let mut word = [0u8; 32];
    if ty == "address" {
        let bytes = parse_address(arg)?;
        word[12..].copy_from_slice(&bytes); // right-aligned
        return Ok(word);
    }
    if ty == "bool" {
        word[31] = match arg {
            "true" | "1" => 1,
            "false" | "0" => 0,
            _ => return Err(format!("bool arg must be true/false/1/0, got {arg:?}")),
        };
        return Ok(word);
    }
    if ty == "bytes32" {
        let bytes = hex_to_bytes(arg)?;
        if bytes.len() > 32 {
            return Err("bytes32 arg longer than 32 bytes".into());
        }
        word[..bytes.len()].copy_from_slice(&bytes); // LEFT-aligned
        return Ok(word);
    }
    if ty.starts_with("uint") || ty.starts_with("int") || ty.is_empty() {
        // Decimal, or 0x-hex; right-aligned. (Negative ints unsupported here.)
        let v: u128 = if let Some(hex) = arg.strip_prefix("0x") {
            u128::from_str_radix(hex, 16).map_err(|e| format!("bad hex uint {arg:?}: {e}"))?
        } else {
            arg.parse::<u128>().map_err(|e| format!("bad uint {arg:?}: {e}"))?
        };
        return Ok(u256_be(v));
    }
    Err(format!("unsupported arg type {ty:?} (supported: address, bool, uintN, intN, bytes32)"))
}

/// Best-effort decode of a 32-byte `eth_call` return into a human reading: a
/// non-zero right-aligned address (if it looks like one), else the uint value,
/// alongside the raw hex. UNTRUSTED — purely cosmetic; the raw hex is canonical.
pub fn decode_result_hint(result_hex: &str) -> Option<String> {
    let trimmed = result_hex.trim().trim_start_matches("0x");
    if trimmed.len() != 64 {
        return None; // dynamic / multi-word return → no single-word hint
    }
    // Address-like: the high 12 bytes (24 hex) are zero AND the UPPER half of the
    // 20-byte address slot is non-zero — i.e. entropy spread across the word, not
    // a small right-aligned integer (which a real address ≥ 2^96 always has). A
    // small uint like 42 stays a uint, never a bogus "address 0x…0002a".
    let high_zero = &trimmed[..24] == "000000000000000000000000";
    let upper_addr_nonzero = trimmed[24..44].chars().any(|c| c != '0');
    if high_zero && upper_addr_nonzero {
        return Some(format!("address 0x{}", &trimmed[24..]));
    }
    parse_hex_quantity(trimmed).ok().map(|n| format!("uint {n}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::bytes_to_hex;

    #[test]
    fn namehash_eth_matches_canonical_vector() {
        // The pinned EIP-137 vector: namehash("eth").
        let h = namehash("eth");
        let hex = bytes_to_hex(&h);
        assert_eq!(
            hex,
            "93cdeb708b7545dc668eb9280176169d1c33cfd8ed6f04690a0bcc88a93fc4ae"
        );
        // namehash("") is the zero node.
        assert_eq!(namehash(""), [0u8; 32]);
        // A trailing dot (fully-qualified name) hashes identically.
        assert_eq!(namehash("eth."), namehash("eth"));
    }

    #[test]
    fn namehash_subname_is_recursive_keccak() {
        // namehash("alice.eth") = keccak( namehash("eth") ‖ keccak("alice") ).
        let mut h = Keccak256::new();
        h.update(namehash("eth"));
        h.update(Keccak256::digest(b"alice"));
        let expect: [u8; 32] = h.finalize().into();
        assert_eq!(namehash("alice.eth"), expect);
        // Known vector: namehash("foo.eth").
        assert_eq!(
            bytes_to_hex(&namehash("foo.eth")),
            "de9b09fd7c5f901e23a3f19fecc54828e9c848539801e86591bd9801b019f84f"
        );
    }

    #[test]
    fn encode_function_call_balance_of() {
        let addr = format!("0x{}", "11".repeat(20));
        let cd = encode_function_call("balanceOf(address)", &[addr]).unwrap();
        // selector(balanceOf(address)) = 0x70a08231, then the address right-aligned.
        assert!(cd.starts_with("0x70a08231"));
        assert_eq!(cd.len(), 2 + (4 + 32) * 2);
        assert!(cd.ends_with(&"11".repeat(20)));
        // Spaces in the signature are normalized away for the selector.
        let cd2 = encode_function_call("balanceOf( address )", &[format!("0x{}", "11".repeat(20))]).unwrap();
        assert_eq!(cd, cd2);
    }

    #[test]
    fn encode_function_call_arity_and_types() {
        // Arg count must match the signature.
        assert!(encode_function_call("transfer(address,uint256)", &["0x00".to_string()]).is_err());
        // uint256 decimal + hex both encode right-aligned.
        let dec = encode_function_call("f(uint256)", &["255".to_string()]).unwrap();
        let hex = encode_function_call("f(uint256)", &["0xff".to_string()]).unwrap();
        assert_eq!(dec, hex);
        assert!(dec.ends_with("ff"));
        // bool.
        let b = encode_function_call("f(bool)", &["true".to_string()]).unwrap();
        assert!(b.ends_with(&format!("{}01", "0".repeat(62))));
        // No-arg signature.
        let n = encode_function_call("decimals()", &[]).unwrap();
        assert_eq!(n.len(), 2 + 4 * 2);
        // Unsupported dynamic type errors rather than mis-encoding.
        assert!(encode_function_call("f(string)", &["hi".to_string()]).is_err());
        // Bad address shape errors.
        assert!(encode_function_call("f(address)", &["0x1234".to_string()]).is_err());
    }

    #[test]
    fn chain_lookup_and_aliases() {
        assert_eq!(chain_by_name("base").unwrap().chain_id, 8453);
        assert_eq!(chain_by_name("ETH").unwrap().name, "ethereum");
        assert_eq!(chain_by_name(" mainnet ").unwrap().name, "ethereum");
        assert_eq!(chain_by_name("op").unwrap().chain_id, 10);
        assert_eq!(chain_by_name("arb").unwrap().chain_id, 42161);
        assert_eq!(chain_by_name("matic").unwrap().chain_id, 137);
        assert!(chain_by_name("dogechain").is_none());
        // tempo tracks the active localharness chain.
        assert_eq!(chain_by_name("tempo").unwrap().chain_id, super::super::chain::active().chain_id);
    }

    #[test]
    fn format_units_is_exact_and_trims() {
        assert_eq!(format_units(1_500_000_000_000_000_000, 18), "1.5");
        assert_eq!(format_units(1_000_000_000_000_000_000, 18), "1");
        assert_eq!(format_units(1, 18), "0.000000000000000001");
        assert_eq!(format_units(0, 18), "0");
        assert_eq!(format_units(123456, 0), "123456");
        assert_eq!(format_units(250, 2), "2.5"); // trims trailing zero
        assert_eq!(format_units(1_000_000, 6), "1");
    }

    #[test]
    fn decode_result_hint_reads_address_or_uint() {
        let addr = format!("0x{}{}", "0".repeat(24), "11".repeat(20));
        assert_eq!(decode_result_hint(&addr).as_deref(), Some("address 0x1111111111111111111111111111111111111111"));
        let n = format!("0x{:064x}", 42u64);
        assert_eq!(decode_result_hint(&n).as_deref(), Some("uint 42"));
        // All-zero word reads as uint 0 (not a bogus zero address).
        assert_eq!(decode_result_hint(&format!("0x{}", "0".repeat(64))).as_deref(), Some("uint 0"));
        // Multi-word / short return → no single-word hint.
        assert_eq!(decode_result_hint("0xabcd"), None);
    }
}
