use k256::ecdsa::SigningKey;

use super::*;

// --- Address-keyed Web Push subscriptions (PushFacet) -----------------
//
// The fix for cross-device notifications that never reached devices without a
// registered MAIN identity. A device self-registers its own push subscription
// keyed by ITS OWN address (`setPushSub`, msg.sender-keyed) — no name, no MAIN
// tokenId, no metadata slot required. The proxy resolves a feed subscriber's
// address → subscription directly, so even a bare device key can be buzzed.

/// Byte budget for a push-sub SLOT value. `PushFacet.setPushSub` hard-caps at
/// 4096 bytes; stay under it (and keep metadata-slot gas sane — ~8.5k gas/byte)
/// by evicting the OLDEST entries past this size.
const SLOT_BYTE_BUDGET: usize = 4000;

/// Merge THIS device's subscription JSON into a push-sub slot value, upserting
/// by `endpoint`. Slots are MULTI-DEVICE: a JSON array of subscription objects,
/// newest first (legacy single-object values are promoted to a one-element
/// array — the fix for "my phone stopped buzzing": a single-sub slot meant the
/// last device to register silently overwrote every other device's
/// subscription). Returns `None` when the slot already contains exactly this
/// subscription (no write needed), else the new slot JSON to publish.
pub fn merge_push_sub(slot: Option<&str>, current: &str) -> Option<String> {
    let cur: serde_json::Value = serde_json::from_str(current).ok()?;
    let cur_ep = cur.get("endpoint")?.as_str()?.to_string();
    let mut entries: Vec<serde_json::Value> = match slot.map(str::trim).filter(|s| !s.is_empty()) {
        None => Vec::new(),
        Some(s) => match serde_json::from_str::<serde_json::Value>(s) {
            Ok(serde_json::Value::Array(a)) => a,
            Ok(v @ serde_json::Value::Object(_)) => vec![v],
            _ => Vec::new(),
        },
    };
    entries.retain(|e| e.get("endpoint").and_then(|v| v.as_str()).is_some());
    if entries.contains(&cur) {
        return None; // this exact subscription is already published
    }
    entries.retain(|e| e.get("endpoint").and_then(|v| v.as_str()) != Some(cur_ep.as_str()));
    entries.insert(0, cur);
    // Evict oldest while over budget (never the just-added entry).
    loop {
        let json = serde_json::Value::Array(entries.clone()).to_string();
        if json.len() <= SLOT_BYTE_BUDGET || entries.len() <= 1 {
            return Some(json);
        }
        entries.pop();
    }
}

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

/// Sponsored gas for a `setPushSub` write. CRITICAL: Tempo charges ~8,500 gas
/// PER BYTE for storage writes (≈10x Ethereum — same as `setMetadata`; see
/// CLAUDE.md). A 365-byte push subscription `cast estimate`s at ~3.4-3.6M on the
/// live chain (NOT the 323k a `cast run` replay misleadingly shows). Earlier caps
/// of 2.66M and 965k both OUT-OF-GASSED — the sole reason device registration
/// never landed. There is NO sponsor ceiling here (publishing a cartridge uses
/// ~11M and works). Match the proven setMetadata formula + headroom; the sponsor
/// pays gas USED, not the cap.
fn set_push_sub_gas(len: usize) -> u128 {
    1_500_000 + (len as u128) * 9_000
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

    fn sub(ep: &str, key: &str) -> String {
        format!(r#"{{"endpoint":"https://push.example/{ep}","keys":{{"p256dh":"{key}","auth":"a"}}}}"#)
    }

    #[test]
    fn merge_into_empty_slot_makes_one_element_array() {
        let cur = sub("phone", "k1");
        let merged = merge_push_sub(None, &cur).unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0], serde_json::from_str::<serde_json::Value>(&cur).unwrap());
    }

    #[test]
    fn merge_promotes_legacy_single_object_and_appends() {
        // The desktop overwrote the phone pre-fix; now the phone ADDS itself.
        let phone = sub("phone", "k1");
        let desktop = sub("desktop", "k2");
        let merged = merge_push_sub(Some(&desktop), &phone).unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["endpoint"], "https://push.example/phone"); // newest first
        assert_eq!(arr[1]["endpoint"], "https://push.example/desktop");
    }

    #[test]
    fn merge_is_idempotent_when_already_present() {
        let phone = sub("phone", "k1");
        let slot = merge_push_sub(None, &phone).unwrap();
        assert!(merge_push_sub(Some(&slot), &phone).is_none()); // no rewrite
    }

    #[test]
    fn merge_replaces_same_endpoint_with_new_keys() {
        // Reinstall/cleared site data → same-ish endpoint slot churns keys.
        let old = sub("phone", "OLD");
        let new = sub("phone", "NEW");
        let slot = merge_push_sub(None, &old).unwrap();
        let merged = merge_push_sub(Some(&slot), &new).unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["keys"]["p256dh"], "NEW");
    }

    #[test]
    fn merge_evicts_oldest_past_byte_budget() {
        let mut slot: Option<String> = None;
        for i in 0..40 {
            // ~200 bytes each → 40 would be ~8KB; budget keeps it under 4000.
            let s = sub(&format!("dev{i}-{}", "x".repeat(120)), "k");
            if let Some(m) = merge_push_sub(slot.as_deref(), &s) {
                slot = Some(m);
            }
        }
        let out = slot.unwrap();
        assert!(out.len() <= SLOT_BYTE_BUDGET);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        // Newest entry always survives eviction.
        assert!(v[0]["endpoint"].as_str().unwrap().contains("dev39"));
    }

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
