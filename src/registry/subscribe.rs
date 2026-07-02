use k256::ecdsa::SigningKey;

use super::*;

// --- Subscriptions (SubscribeFacet on the diamond) -------------------
//
// Per-subdomain notification subscriber sets — the on-chain half of the
// cartridge "Ready Up" feed (`host::agent::subscribe` / `broadcast`).
// Permissionless: any identity may subscribe to any subdomain's `targetId`.
// Writes are sponsored Tempo txs (the viewer holds zero gas); the push
// delivery itself is off-chain (the credit proxy's `/api/broadcast` reads
// `subscribersOf` + each subscriber's published push subscription).

fn encode_target_call(sig: &str, target_id: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32);
    out.extend_from_slice(&selector(sig));
    out.extend_from_slice(&u256_be(target_id as u128));
    out
}

/// Subscribe the sender to `target_id`'s notification feed via a sponsored
/// Tempo tx. Idempotent on-chain (a repeat is a no-op, not a revert).
pub async fn subscribe_sponsored(
    sender: &SigningKey,
    target_id: u64,
) -> Result<String, String> {
    // `cast estimate` (never guess): the FIRST subscriber to a feed creates the
    // dynamic array (length + element + 1-based index mapping = several cold
    // SSTOREs) and costs ~1.05M gas INNER; even joining a populated feed is
    // ~800k. Tempo sponsorship adds ~275k on top → ~1.33M real. 300k then 600k
    // both OUT-OF-GASSED (receipt reverted → the toggle snapped back, count
    // stuck at 0). 2M is the limit; the sponsor pays gas USED, not the cap, so
    // the headroom is free (CLAUDE.md "the bug is always an under-set cap").
    sponsored_diamond_call(
        sender,
        encode_target_call("subscribe(uint256)", target_id),
        2_000_000,
    )
    .await
}

/// Unsubscribe the sender from `target_id`'s feed (sponsored). Idempotent.
pub async fn unsubscribe_sponsored(
    sender: &SigningKey,
    target_id: u64,
) -> Result<String, String> {
    sponsored_diamond_call(
        sender,
        encode_target_call("unsubscribe(uint256)", target_id),
        600_000,
    )
    .await
}

/// Read `isSubscribed(targetId, who)` — true iff `who_hex` subscribes to
/// `target_id`'s feed.
pub async fn is_subscribed(target_id: u64, who_hex: &str) -> Result<bool, String> {
    let who = parse_eth_address(who_hex)?;
    let result = read_view(
        selector("isSubscribed(uint256,address)"),
        &[u256_be(target_id as u128), addr_word(&who)],
    )
    .await?;
    decode_u256_as_u64(&result).map(|v| v != 0)
}

/// Read `subscriberCount(targetId)` — how many identities subscribe to the
/// feed (the "member count" for a Ready-Up app).
pub async fn subscriber_count(target_id: u64) -> Result<u128, String> {
    let result = read_view(
        selector("subscriberCount(uint256)"),
        &[u256_be(target_id as u128)],
    )
    .await?;
    decode_u256_as_u128(&result)
}
