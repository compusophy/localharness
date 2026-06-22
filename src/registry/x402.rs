use k256::ecdsa::SigningKey;
use sha3::{Digest, Keccak256};

use super::*;

// --- x402 payment authorization (settled in $LH via X402Facet) -------
//
// EIP-712 "exact"-scheme settlement for agent-to-agent payments. The
// payer signs a `PaymentAuthorization` (gasless); the payee submits
// `settle`. Domain/typehash MUST match `contracts/src/facets/X402Facet.sol`
// — the `x402_domain_matches_live_facet` test pins it to the deployed
// diamond.

pub(crate) fn keccak32(data: &[u8]) -> [u8; 32] {
    let mut h = Keccak256::new();
    h.update(data);
    let d = h.finalize();
    let mut o = [0u8; 32];
    o.copy_from_slice(&d);
    o
}

/// EIP-712 domain separator for the x402 facet (name "localharness-x402",
/// version "1", `CHAIN_ID()`, diamond). Matches `x402DomainSeparator()`.
pub fn x402_domain_separator() -> Result<[u8; 32], String> {
    domain_separator_for(CHAIN_ID(), REGISTRY_ADDRESS())
}

/// The pure core of [`x402_domain_separator`] for an EXPLICIT (chain_id, diamond)
/// — so a test can pin one chain's separator without depending on which chain
/// `active()` happens to resolve to (the CLI default is mainnet; testnet is opt-in).
fn domain_separator_for(chain_id: u64, diamond_hex: &str) -> Result<[u8; 32], String> {
    let diamond = parse_eth_address(diamond_hex)?;
    let mut dom = Vec::with_capacity(160);
    dom.extend_from_slice(&keccak32(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    ));
    dom.extend_from_slice(&keccak32(b"localharness-x402"));
    dom.extend_from_slice(&keccak32(b"1"));
    dom.extend_from_slice(&u256_be(chain_id as u128));
    dom.extend_from_slice(&addr_word(&diamond));
    Ok(keccak32(&dom))
}

/// EIP-712 digest of an x402 `PaymentAuthorization` (what the payer signs).
pub fn x402_digest(
    from: &[u8; 20],
    to: &[u8; 20],
    value_wei: u128,
    valid_after: u64,
    valid_before: u64,
    nonce: &[u8; 32],
) -> Result<[u8; 32], String> {
    let mut st = Vec::with_capacity(224);
    st.extend_from_slice(&keccak32(
        b"PaymentAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)",
    ));
    st.extend_from_slice(&addr_word(from));
    st.extend_from_slice(&addr_word(to));
    st.extend_from_slice(&u256_be(value_wei));
    st.extend_from_slice(&u256_be(valid_after as u128));
    st.extend_from_slice(&u256_be(valid_before as u128));
    st.extend_from_slice(nonce);
    let struct_hash = keccak32(&st);

    let mut pre = Vec::with_capacity(66);
    pre.extend_from_slice(&[0x19, 0x01]);
    pre.extend_from_slice(&x402_domain_separator()?);
    pre.extend_from_slice(&struct_hash);
    Ok(keccak32(&pre))
}

/// Sign an x402 authorization with an EOA key — the 65-byte sig that
/// goes in the `X-PAYMENT` payload. (k256 emits low-s, which the facet
/// requires.) Agents paying from a contract TBA sign via EIP-1271 paths
/// instead; this is the EOA fast path.
pub fn sign_x402(
    signer: &SigningKey,
    from: &[u8; 20],
    to: &[u8; 20],
    value_wei: u128,
    valid_after: u64,
    valid_before: u64,
    nonce: &[u8; 32],
) -> Result<[u8; 65], String> {
    let digest = x402_digest(from, to, value_wei, valid_after, valid_before, nonce)?;
    Ok(crate::wallet::sign_hash(signer, &digest))
}

pub(crate) fn encode_settle(
    from: &[u8; 20],
    to: &[u8; 20],
    value_wei: u128,
    valid_after: u64,
    valid_before: u64,
    nonce: &[u8; 32],
    signature: &[u8; 65],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32 * 9 + 96);
    out.extend_from_slice(&selector(
        "settle(address,address,uint256,uint256,uint256,bytes32,bytes)",
    ));
    out.extend_from_slice(&addr_word(from));
    out.extend_from_slice(&addr_word(to));
    out.extend_from_slice(&u256_be(value_wei));
    out.extend_from_slice(&u256_be(valid_after as u128));
    out.extend_from_slice(&u256_be(valid_before as u128));
    out.extend_from_slice(nonce);
    out.extend_from_slice(&u256_be(7 * 32)); // offset to the `bytes` arg
    out.extend_from_slice(&u256_be(signature.len() as u128)); // 65
    out.extend_from_slice(signature);
    out.resize(out.len() + 31, 0); // pad 65 -> 96 (32-byte multiple)
    out
}

/// Submit an x402 settlement (sponsored). `submitter` is the payee /
/// facilitator (signs the Tempo tx); fees paid by `fee_payer`. Moves
/// `value_wei` `$LH` from the signed authorization's payer to `to`.
/// The payer must have `approve`d the diamond for `$LH` once.
#[allow(clippy::too_many_arguments)]
pub async fn settle_x402_sponsored(
    submitter: &SigningKey,
    fee_payer: &SigningKey,
    from: &[u8; 20],
    to: &[u8; 20],
    value_wei: u128,
    valid_after: u64,
    valid_before: u64,
    nonce: &[u8; 32],
    signature: &[u8; 65],
    fee_token: &str,
) -> Result<String, String> {
    // ecrecover + one-shot nonce SSTORE + TIP-20 transferFrom. The first
    // payment INTO a fresh TBA is all cold zero→nonzero SSTOREs and reverted
    // at 392k/400k live (2026-06-10); warm-path E2Es masked it. ~275k of the
    // limit is Tempo sponsorship overhead before the inner call even runs.
    sponsored_diamond_call(
        submitter,
        fee_payer,
        encode_settle(from, to, value_wei, valid_after, valid_before, nonce, signature),
        fee_token,
        1_200_000,
    )
    .await
}

/// Metadata label for an agent's advertised per-call `$LH` price — the
/// payload is the wei amount as a decimal UTF-8 string. Read by the
/// hosted `ask_agent` gate (and anyone choosing what to pay).
pub const X402_PRICE_LABEL: &[u8] = b"localharness.x402_price";

/// Floor applied by the hosted `ask_agent` path when an agent has NOT
/// advertised a price on-chain: 0.01 `$LH`. Advertising a price (any
/// value > 0) overrides it; the proxy enforces `max(advertised, …)` — an
/// unpriced agent still earns something instead of answering for tips.
pub const DEFAULT_ASK_PRICE_WEI: u128 = 10_000_000_000_000_000;

/// Ceiling on what `call_agent` will pay WITHOUT a human in the loop:
/// 1 `$LH`. The target's advertised price is on-chain data a FOREIGN owner
/// controls, so an unbounded auto-pay would let a malicious agent drain
/// the caller one tool-call at a time. Enforced by [`auto_pay_amount`];
/// above the cap the call errs with the price so the owner can decide.
pub const REMOTE_CALL_MAX_AUTO_PAY_WEI: u128 = 1_000_000_000_000_000_000;

/// Resolve what an UNATTENDED agent-to-agent call may auto-pay: the
/// advertised on-chain price (pass [`x402_price_of`]'s output — `None` /
/// zero are already filtered to `None` there) falling back to
/// [`DEFAULT_ASK_PRICE_WEI`], then REFUSED when above `cap` — `Err`
/// carries the over-cap price so the caller can surface it to the human.
/// Fallback-then-cap order matters: a missing price must never bypass the
/// cap check.
pub fn auto_pay_amount(advertised: Option<u128>, cap: u128) -> Result<u128, u128> {
    let pay_wei = advertised.unwrap_or(DEFAULT_ASK_PRICE_WEI);
    if pay_wei > cap {
        Err(pay_wei)
    } else {
        Ok(pay_wei)
    }
}

/// Price-lock overpay tolerance, in basis points (1% = 100 bps). The x402
/// `PaymentAuthorization` signs an EXACT `value`, and on-chain `settle` moves
/// EXACTLY that — a facilitator can never settle LESS than the signed amount,
/// so "pay the cheaper of signed/current" is impossible without a re-sign.
/// Instead the gate locks the signed value to the live price: an underpay is
/// rejected (the floor), and an overpay BEYOND this slack is rejected too so a
/// caller whose quote went stale (price dropped after they signed) re-quotes at
/// the current price instead of SILENTLY overpaying. The band absorbs benign
/// drift / deliberate small tips; only a real, money-losing gap rejects.
pub const PRICE_LOCK_OVERPAY_TOLERANCE_BPS: u128 = 1000; // 10%

/// The maximum `value` the price-lock gate will settle against a live
/// `required` price: `required + tolerance`, where the tolerance is
/// [`PRICE_LOCK_OVERPAY_TOLERANCE_BPS`] of `required` (saturating). A signed
/// authorization at or below this ceiling settles as signed; above it the gate
/// rejects and returns `required` so the caller re-quotes — preventing a stale
/// quote from silently overpaying when the price dropped. Pure; the proxy's
/// `priceLockCeiling` in `proxy/api/mcp.ts` mirrors it (kept in lockstep).
pub fn price_lock_ceiling(required: u128) -> u128 {
    let slack = required.saturating_mul(PRICE_LOCK_OVERPAY_TOLERANCE_BPS) / 10_000;
    required.saturating_add(slack)
}

/// Read an agent's advertised per-call price (wei). `None` when never
/// set (callers should fall back to [`DEFAULT_ASK_PRICE_WEI`]).
pub async fn x402_price_of(token_id: u64) -> Result<Option<u128>, String> {
    match metadata_bytes_of(token_id, keccak_key(X402_PRICE_LABEL)).await? {
        Some(b) => Ok(String::from_utf8(b)
            .ok()
            .and_then(|s| s.trim().parse::<u128>().ok())
            .filter(|w| *w > 0)),
        None => Ok(None),
    }
}

/// An agent's effective per-call price: advertised, else the default.
pub async fn x402_ask_price_of(token_id: u64) -> Result<u128, String> {
    Ok(x402_price_of(token_id).await?.unwrap_or(DEFAULT_ASK_PRICE_WEI))
}

/// Encode `setMetadata` publishing the advertised price. `wei == 0`
/// writes an empty payload (clears the slot → callers see the default).
pub fn encode_set_x402_price(token_id: u64, wei: u128) -> Vec<u8> {
    let payload = if wei == 0 { String::new() } else { wei.to_string() };
    encode_set_metadata_bytes(token_id, keccak_key(X402_PRICE_LABEL), payload.as_bytes())
}

/// Read `authorizationState(from, nonce)` — true if that x402 nonce was
/// already settled (lets a payee detect replays before serving).
pub async fn x402_authorization_state(
    from_hex: &str,
    nonce: &[u8; 32],
) -> Result<bool, String> {
    let from = parse_eth_address(from_hex)?;
    let result = read_view(
        selector("authorizationState(address,bytes32)"),
        &[addr_word(&from), *nonce],
    )
    .await?;
    Ok(decode_u256_as_u64(&result).map(|v| v != 0).unwrap_or(false))
}

/// A fresh random 32-byte x402 nonce (CSPRNG via `getrandom`). Each
/// `PaymentAuthorization` needs a unique nonce — the on-chain `settle`
/// records it one-shot, so a replayed nonce reverts.
pub fn random_x402_nonce() -> [u8; 32] {
    use rand_core::RngCore;
    let mut n = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut n);
    n
}

// --- the hosted x402-gated MCP endpoint (`<proxy>/mcp`) ---------------
//
// The credit proxy exposes an `ask_agent` tool over MCP-over-HTTP: the
// caller signs a `PaymentAuthorization` paying the TARGET's TBA, the proxy
// verifies + settles it on-chain, then answers under the target's
// published persona with the proxy's own model key — so neither side
// needs a model key of its own. These builders/parser are shared by the
// CLI `mcp-call` client and the browser `call_agent` proxy fallback.

/// The hosted MCP endpoint URL: `<CREDIT_PROXY_URL>/mcp`. Joins safely
/// whether or not the base has a trailing slash.
pub fn mcp_endpoint_url() -> String {
    let base = CREDIT_PROXY_URL.trim_end_matches('/');
    format!("{base}/mcp")
}

/// Build the JSON the `x-x402-authorization` header carries, matching the
/// shape `proxy/api/mcp.ts::parseAuth` expects EXACTLY: addresses as 0x-hex,
/// `value` as a decimal string of `$LH` wei, `nonce` as 0x + 32-byte hex,
/// `signature` as 0x + 65-byte hex, `validAfter`/`validBefore` as numbers.
/// Pure — the signature/nonce are passed in so this is deterministic.
#[allow(clippy::too_many_arguments)]
pub fn x402_authorization_json(
    from_hex: &str,
    to_hex: &str,
    value_wei: u128,
    valid_after: u64,
    valid_before: u64,
    nonce: &[u8; 32],
    signature: &[u8; 65],
) -> serde_json::Value {
    serde_json::json!({
        "from": from_hex,
        "to": to_hex,
        "value": value_wei.to_string(),
        "validAfter": valid_after,
        "validBefore": valid_before,
        "nonce": format!("0x{}", bytes_to_hex(nonce)),
        "signature": format!("0x{}", bytes_to_hex(signature)),
    })
}

/// The `tools/call` JSON-RPC body the hosted endpoint expects: it routes
/// the `ask_agent` tool, with the target name + message in `arguments`
/// (see `proxy/api/mcp.ts` — `params.name` is the TOOL name "ask_agent",
/// and `params.arguments = { name: <target>, message }`).
pub fn x402_ask_agent_body(target: &str, message: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "ask_agent",
            "arguments": { "name": target, "message": message }
        }
    })
}

/// Parse the hosted endpoint's JSON-RPC reply for a `tools/call`: the text
/// content on success, `Err` for protocol errors, missing content, or a
/// tool-level failure (`result.isError` with the reason in
/// `content[0].text`). Pure.
pub fn parse_mcp_tool_reply(json: &serde_json::Value) -> Result<String, String> {
    if let Some(err) = json.get("error") {
        let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("(no message)");
        return Err(format!("mcp error {code}: {msg}"));
    }
    let result = json
        .get("result")
        .ok_or_else(|| format!("response has neither result nor error: {json}"))?;
    let text = result
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| format!("response had no text content: {result}"))?;
    if result.get("isError").and_then(|b| b.as_bool()).unwrap_or(false) {
        return Err(text.trim().to_string());
    }
    Ok(text.trim().to_string())
}


#[cfg(test)]
mod x402_tests {
    use super::*;

    #[test]
    fn x402_domain_matches_live_facet() {
        // Pinned to the deployed Moderato X402Facet's `x402DomainSeparator()` —
        // cross-checks the Rust EIP-712 encoding against the live contract. The
        // domain binds chainId + diamond. We compute it for the MODERATO preset
        // EXPLICITLY (not via `active()`, whose CLI default is now mainnet) so the
        // pin holds regardless of `LH_CHAIN`. The mainnet X402Facet is not yet cut
        // (mainnet is the on-ramp slice only), so there is no live mainnet
        // separator to pin against until the economy ladder deploys there.
        let expected =
            "54530933a67f96286ac528dbff39d00c0ea49f4c6bd0f034343a0c78927f0b7a";
        let got =
            domain_separator_for(crate::registry::chain::MODERATO.chain_id, crate::registry::chain::MODERATO.diamond)
                .unwrap();
        assert_eq!(bytes_to_hex(&got), expected);
    }

    // Chain-agnostic: signs and recovers over a self-consistent EIP-712 digest
    // built off the ACTIVE chain's domain separator — needs only a non-empty
    // diamond, which every preset now has. (No live facet required.)
    #[test]
    fn x402_sign_recovers_payer() {
        let w = crate::wallet::generate();
        let from = w.address;
        let to = [0x11u8; 20];
        let nonce = [0x22u8; 32];
        let sig = sign_x402(&w.signer, &from, &to, 1_000, 0, 9_999_999_999, &nonce).unwrap();
        let digest = x402_digest(&from, &to, 1_000, 0, 9_999_999_999, &nonce).unwrap();
        // EIP-712 digest is signed directly (no personal-sign prefix).
        let recovered = crate::wallet::recover_address(&sig, &digest).unwrap();
        assert_eq!(recovered, from);
    }

    /// x402 `settle(...)` calldata: the dynamic-`bytes` signature must be
    /// pointed at by the right offset (7 words) and length-prefixed with 65,
    /// then zero-padded to a 32-byte multiple. A wrong offset/length makes the
    /// facet read a bogus signature → reject (or worse, accept the wrong one).
    #[test]
    fn settle_calldata_layout() {
        let from = [0x11u8; 20];
        let to = [0x22u8; 20];
        let nonce = [0x33u8; 32];
        let sig = [0x44u8; 65];
        let value = 7_000u128;
        let cd = encode_settle(&from, &to, value, 1, 2, &nonce, &sig);
        assert_eq!(
            &cd[0..4],
            &selector("settle(address,address,uint256,uint256,uint256,bytes32,bytes)")
        );
        // 6 static words + offset word + length word + 65 sig + 31 pad = 96 tail.
        assert_eq!(cd.len(), 4 + 6 * 32 + 32 + 32 + 96);
        assert_eq!(&cd[4 + 12..4 + 32], &from); // word 0
        assert_eq!(&cd[4 + 44..4 + 64], &to); // word 1
        assert_eq!(u128::from_be_bytes(cd[4 + 80..4 + 96].try_into().unwrap()), value); // word 2
        assert_eq!(&cd[4 + 5 * 32..4 + 6 * 32], &nonce); // word 5
        // Word 6 = offset to the bytes arg = 7*32 = 224.
        assert_eq!(u64::from_be_bytes(cd[4 + 6 * 32 + 24..4 + 7 * 32].try_into().unwrap()), 7 * 32);
        // Word 7 = bytes length = 65.
        assert_eq!(u64::from_be_bytes(cd[4 + 7 * 32 + 24..4 + 8 * 32].try_into().unwrap()), 65);
        // The 65 signature bytes follow, then zero padding to a 32-multiple.
        assert_eq!(&cd[4 + 8 * 32..4 + 8 * 32 + 65], &sig);
        assert_eq!(&cd[4 + 8 * 32 + 65..], &[0u8; 31]);
    }

    #[test]
    fn x402_authorization_json_matches_proxy_shape() {
        // The exact field names + types `proxy/api/mcp.ts::parseAuth` requires.
        let from = "0x00000000000000000000000000000000000000aa";
        let to = "0x00000000000000000000000000000000000000bb";
        let nonce = [0x11u8; 32];
        let sig = [0x22u8; 65];
        let j = x402_authorization_json(from, to, 1_000_000_000_000_000, 0, 1_999_999_999, &nonce, &sig);

        assert_eq!(j["from"], from);
        assert_eq!(j["to"], to);
        // value is a DECIMAL STRING of $LH wei (not a number).
        assert_eq!(j["value"], "1000000000000000");
        assert!(j["value"].is_string());
        assert_eq!(j["validAfter"], 0);
        assert_eq!(j["validBefore"], 1_999_999_999u64);
        // nonce: 0x + 32 bytes = 64 hex chars. signature: 0x + 65 bytes = 130 hex.
        let nonce_s = j["nonce"].as_str().unwrap();
        let sig_s = j["signature"].as_str().unwrap();
        assert_eq!(nonce_s.len(), 2 + 64);
        assert!(nonce_s.starts_with("0x"));
        assert_eq!(sig_s.len(), 2 + 130);
        assert!(sig_s.starts_with("0x"));
    }

    #[test]
    fn x402_ask_agent_body_is_jsonrpc_tools_call() {
        let b = x402_ask_agent_body("claude", "hello");
        assert_eq!(b["jsonrpc"], "2.0");
        assert_eq!(b["method"], "tools/call");
        // params.name is the TOOL ("ask_agent"); the target rides in arguments.
        assert_eq!(b["params"]["name"], "ask_agent");
        assert_eq!(b["params"]["arguments"]["name"], "claude");
        assert_eq!(b["params"]["arguments"]["message"], "hello");
    }

    #[test]
    fn parse_mcp_tool_reply_extracts_text_and_failures() {
        // Success: trimmed text content.
        let ok = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "content": [{ "type": "text", "text": "  hi there\n" }], "isError": false }
        });
        assert_eq!(parse_mcp_tool_reply(&ok).unwrap(), "hi there");

        // Tool-level failure rides in isError + the text.
        let tool_err = serde_json::json!({
            "result": { "content": [{ "type": "text", "text": "agent exploded" }], "isError": true }
        });
        assert_eq!(parse_mcp_tool_reply(&tool_err).unwrap_err(), "agent exploded");

        // Protocol error envelope.
        let rpc_err = serde_json::json!({ "error": { "code": -32602, "message": "payment required" } });
        let e = parse_mcp_tool_reply(&rpc_err).unwrap_err();
        assert!(e.contains("-32602") && e.contains("payment required"), "{e}");

        // Neither result nor error, and result-without-text, both fail.
        assert!(parse_mcp_tool_reply(&serde_json::json!({})).is_err());
        assert!(parse_mcp_tool_reply(&serde_json::json!({ "result": {} })).is_err());
    }

    /// The auto-pay cap is the ONLY guard between a foreign agent's
    /// advertised on-chain price and the caller's $LH balance — `call_agent`
    /// pays it unattended. Pins the exact boundary (at-cap pays, one wei
    /// over refuses), the None → default fallback, and that the default
    /// itself fits under the cap (otherwise every unpriced agent would be
    /// unreachable).
    #[test]
    fn auto_pay_cap_boundary() {
        let cap = REMOTE_CALL_MAX_AUTO_PAY_WEI;
        // Exactly at the cap: pays.
        assert_eq!(auto_pay_amount(Some(cap), cap), Ok(cap));
        // One wei over: refused, Err carries the over-cap price.
        assert_eq!(auto_pay_amount(Some(cap + 1), cap), Err(cap + 1));
        // No advertised price: falls back to the platform default…
        assert_eq!(auto_pay_amount(None, cap), Ok(DEFAULT_ASK_PRICE_WEI));
        // …which must itself be payable under the cap.
        assert!(DEFAULT_ASK_PRICE_WEI <= cap);
        // The fallback is still capped (fallback-then-cap order).
        assert_eq!(auto_pay_amount(None, 1), Err(DEFAULT_ASK_PRICE_WEI));
    }

    /// The price-lock ceiling pins the overpay-acceptance band. A signed value
    /// in `[required, required + tolerance]` settles as signed; above it the
    /// gate must reject (the caller's quote went stale → re-quote, never silent
    /// overpay). Checks the exact boundary, the default-price band, and that
    /// saturating math can't panic / wrap on extreme inputs.
    #[test]
    fn price_lock_ceiling_band() {
        let p = DEFAULT_ASK_PRICE_WEI; // 0.01 $LH
                                       // Default tolerance is 10%, so the ceiling is required * 1.10.
        assert_eq!(price_lock_ceiling(p), p + p / 10);
        // Exactly at the price and exactly at the ceiling are both inside the band.
        assert!(p <= price_lock_ceiling(p));
        assert!(price_lock_ceiling(p) >= p + p * PRICE_LOCK_OVERPAY_TOLERANCE_BPS / 10_000);
        // One wei over the ceiling is OUTSIDE the band (the reject case).
        assert!(price_lock_ceiling(p) + 1 > price_lock_ceiling(p));
        // A zero price (unreachable in practice — the default floor applies) has
        // a zero-width band: only an exact-zero value is inside.
        assert_eq!(price_lock_ceiling(0), 0);
        // Saturating: u128::MAX must not panic or wrap below the input.
        assert!(price_lock_ceiling(u128::MAX) >= u128::MAX - 1);
    }

    #[test]
    fn mcp_endpoint_is_proxy_slash_mcp() {
        let url = mcp_endpoint_url();
        assert!(url.ends_with("/mcp"));
        assert!(!url.contains("//mcp")); // no double slash from the base
    }
}
