use k256::ecdsa::SigningKey;

use super::*;

// --- Address-keyed Web Push subscriptions (PushFacet) -----------------
//
// The fix for cross-device notifications that never reached devices without a
// registered MAIN identity. A device self-registers its own push subscription
// keyed by ITS OWN address (`setPushSub`, msg.sender-keyed) — no name, no MAIN
// tokenId, no metadata slot required. The proxy resolves a feed subscriber's
// address → subscription directly, so even a bare device key can be buzzed.

/// Encode `PushFacet.setPushSub(bytes)` — register the CALLER's push
/// subscription JSON, keyed by `msg.sender`'s address.
pub fn encode_set_addr_push_sub(sub_json: &[u8]) -> Vec<u8> {
    let len = sub_json.len();
    let padded = len.div_ceil(32) * 32;
    let mut buf = Vec::with_capacity(4 + 32 + 32 + padded);
    buf.extend_from_slice(&selector("setPushSub(bytes)"));
    buf.extend_from_slice(&u256_be(0x20)); // offset to the single dynamic `bytes` arg
    buf.extend_from_slice(&u256_be(len as u128));
    buf.extend_from_slice(sub_json);
    buf.resize(4 + 32 + 32 + padded, 0);
    buf
}

/// Read a device's address-keyed push subscription JSON (`pushSubOf(address)`),
/// or `None` if the device never registered one.
pub async fn addr_push_sub_of(addr_hex: &str) -> Result<Option<String>, String> {
    let addr = parse_eth_address(addr_hex)?;
    let result = read_view(selector("pushSubOf(address)"), &[addr_word(&addr)]).await?;
    Ok(decode_string(&result)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty()))
}

/// Sponsored gas for a `setPushSub` write. LIVE-MEASURED: a 365-byte
/// subscription's INNER call is ~323k gas (debug trace); plus ~275k Tempo
/// sponsorship overhead → ~600k real. The cap MUST stay well under ~2M — a
/// proven subscribe runs at 2M, but ~2.66M trips the sponsor's per-tx ceiling
/// and the whole sponsored tx REVERTS (out-of-gas, ~2.6M burned) even though
/// the inner call would succeed. So: tight, length-scaled, comfortably < 2M.
fn set_push_sub_gas(len: usize) -> u128 {
    600_000 + (len as u128) * 1_000
}

/// Publish the CALLER's Web Push subscription on-chain (address-keyed),
/// sponsored — the device holds zero gas. Called from the browser the moment a
/// viewer grants notification permission (e.g. on the subscribe gesture), so a
/// later broadcast can reach this exact device.
pub async fn set_push_sub_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    sub_json: &[u8],
    fee_token: &str,
) -> Result<String, String> {
    sponsored_diamond_call(
        sender,
        fee_payer,
        encode_set_addr_push_sub(sub_json),
        fee_token,
        set_push_sub_gas(sub_json.len()),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_push_sub_calldata_layout() {
        let sub = br#"{"endpoint":"https://x"}"#;
        let cd = encode_set_addr_push_sub(sub);
        assert_eq!(&cd[0..4], &selector("setPushSub(bytes)"));
        // selector + offset(32) + len(32) + padded payload
        assert_eq!(&cd[4..36], &u256_be(0x20));
        assert_eq!(&cd[36..68], &u256_be(sub.len() as u128));
        assert_eq!(cd.len(), 4 + 32 + 32 + sub.len().div_ceil(32) * 32);
    }
}
