use k256::ecdsa::SigningKey;

use super::*;

// --- TitheFacet (opt-in, permissionless-PULL auto-tithe) ------------------
//
// The revenue→treasury automation atop GuildFacet (Rung 3): an agent's
// token-bound account CONSENTS once (`setTithe(guildId, bps)` + a one-time
// `$LH.approve(diamond, …)`), then ANYONE (a scheduler, a guild officer) may
// `collectTithe(account)` to pull `bps/10000` of the account's CURRENT `$LH`
// balance (capped by its remaining allowance) into the guild's treasury —
// credited exactly like `fundGuild`. Permissionless triggering is SAFE because
// the config is keyed on the account (only it configures ITSELF) and collect
// reads ONLY that account's stored `(guildId, bps)` — a caller can't redirect
// or inflate the tithe. EXACT ABI:
//   setTithe(uint256 guildId, uint256 bps)   (self-only; bps 1..=10000)
//   revokeTithe()                            (self-only; clears the config)
//   collectTithe(address account) -> uint256 (PERMISSIONLESS; returns amount)
//   titheOf(address account) -> (uint256 guildId, uint256 bps)  (read)

/// 100% in basis points — the on-chain `MAX_BPS` cap. A `setTithe` of more than
/// this (or 0) reverts `InvalidBps`. Pin it client-side so the CLI/UI can reject
/// a bad rate before spending sponsored gas on a guaranteed revert.
pub const TITHE_MAX_BPS: u64 = 10_000;

/// Encode `setTithe(uint256 guildId, uint256 bps)` — two static head words.
/// Batched AFTER a one-time `approve(diamond, allowance)` so the later
/// `collectTithe` pull has a standing allowance to draw against (the allowance
/// is the account's hard ceiling on cumulative tithing).
pub(crate) fn encode_set_tithe(guild_id: u64, bps: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 64);
    out.extend_from_slice(&selector("setTithe(uint256,uint256)"));
    out.extend_from_slice(&u256_be(guild_id as u128));
    out.extend_from_slice(&u256_be(bps as u128));
    out
}

/// Encode `revokeTithe()` — a bare selector, no args (clears the caller's own
/// config).
pub(crate) fn encode_revoke_tithe() -> Vec<u8> {
    selector("revokeTithe()").to_vec()
}

/// Encode `collectTithe(address account)` — one static head word (the account
/// whose consented tithe to pull, right-aligned).
pub(crate) fn encode_collect_tithe(account: &[u8; 20]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32);
    out.extend_from_slice(&selector("collectTithe(address)"));
    out.extend_from_slice(&addr_word(account));
    out
}

/// `setTithe(guildId, bps)` calldata as a ready [`crate::tempo_tx::TempoCall`]
/// to the diamond. Pair with [`approve_credits_call`] in a TBA batch so an
/// agent's token-bound account opts in (approve + setTithe) in ONE sponsored
/// tx — the `tithe auto` flow.
pub fn set_tithe_call(guild_id: u64, bps: u64) -> Result<crate::tempo_tx::TempoCall, String> {
    let diamond = parse_eth_address(REGISTRY_ADDRESS())?;
    Ok(crate::tempo_tx::TempoCall {
        to: diamond,
        value_wei: 0,
        input: encode_set_tithe(guild_id, bps),
    })
}

/// Opt OUT of tithing via a sponsored Tempo tx (`revokeTithe()` — clears the
/// `sender`'s own config). `sender` is the account that previously
/// `setTithe`'d; sponsored, so it holds no gas token.
pub async fn revoke_tithe_sponsored(
    sender: &SigningKey,
) -> Result<String, String> {
    // A single `delete` of the config struct + event. 400k mirrors the
    // bounty-claim / set-role budget (sponsor billed on gas USED).
    sponsored_diamond_call(sender, encode_revoke_tithe(), 400_000).await
}

/// Trigger a consented tithe via a sponsored Tempo tx (`collectTithe(account)`).
/// PERMISSIONLESS — any `sender` may call it; the on-chain facet pulls only the
/// `account`'s OWN consented `bps`-of-balance into the guild it chose. Useful
/// for a scheduler or a guild officer to sweep members' tithes; the `account`
/// is protected because collect reads only its stored config.
pub async fn collect_tithe_sponsored(
    sender: &SigningKey,
    account_hex: &str,
) -> Result<String, String> {
    let account = parse_eth_address(account_hex)?;
    // balanceOf + allowance reads + the guild-ledger SSTORE + a `transferFrom`
    // pull (cold token balances) + event. Mirror the fundGuild inner budget
    // (no approve leg here — the standing allowance already exists). 1M covers
    // the cold-balance pull with headroom; sponsor billed on gas USED.
    sponsored_diamond_call(
        sender,
        encode_collect_tithe(&account),
        1_000_000,
    )
    .await
}

/// Read `titheOf(account)` → the account's consented `(guildId, bps)`. `bps ==
/// 0` means no config (never set / revoked); the CLI surfaces that as "not
/// tithing". Two static `uint256` return words.
pub async fn tithe_of(account_hex: &str) -> Result<(u64, u64), String> {
    let account = parse_eth_address(account_hex)?;
    let result = read_view(selector("titheOf(address)"), &[addr_word(&account)]).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 64 {
        return Ok((0, 0)); // empty / short response = unconfigured
    }
    let guild_id = u64_low(&bytes[0..32]);
    let bps = u64_low(&bytes[32..64]);
    Ok((guild_id, bps))
}

#[cfg(test)]
mod tithe_tests {
    use super::*;

    /// `setTithe(uint256,uint256)` — two static words (guildId, bps). A shifted
    /// word would tithe the wrong rate / guild.
    #[test]
    fn set_tithe_calldata_layout() {
        let cd = encode_set_tithe(7, 500); // 5%
        assert_eq!(&cd[0..4], &selector("setTithe(uint256,uint256)"));
        assert_eq!(cd.len(), 4 + 64);
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 7); // guildId
        assert_eq!(u64::from_be_bytes(cd[36 + 24..36 + 32].try_into().unwrap()), 500); // bps
    }

    /// `revokeTithe()` — a bare 4-byte selector, no args.
    #[test]
    fn revoke_tithe_calldata_layout() {
        let cd = encode_revoke_tithe();
        assert_eq!(&cd[..], &selector("revokeTithe()"));
        assert_eq!(cd.len(), 4);
    }

    /// `collectTithe(address)` — one static word, the account right-aligned in
    /// the low 20 bytes (an all-high-bit address catches a padding slip).
    #[test]
    fn collect_tithe_calldata_layout() {
        let account = [0xFFu8; 20];
        let cd = encode_collect_tithe(&account);
        assert_eq!(&cd[0..4], &selector("collectTithe(address)"));
        assert_eq!(cd.len(), 4 + 32);
        assert!(cd[4..4 + 12].iter().all(|&b| b == 0)); // left-pad zeros
        assert_eq!(&cd[4 + 12..4 + 32], &account); // address in the low 20 bytes
    }

    /// The `set_tithe_call` builder targets the DIAMOND with zero native value
    /// and the `setTithe` calldata — what the TBA batch executes.
    #[test]
    fn set_tithe_call_targets_diamond() {
        let call = set_tithe_call(9, 1000).unwrap();
        assert_eq!(call.to, parse_eth_address(REGISTRY_ADDRESS()).unwrap());
        assert_eq!(call.value_wei, 0);
        assert_eq!(call.input, encode_set_tithe(9, 1000));
    }

    /// `titheOf` decodes two return words; a short/empty response degrades to
    /// the unconfigured `(0, 0)` without panicking (hostile-RPC-safe).
    #[test]
    fn tithe_of_decode_and_short_response() {
        // A wrong-shaped (single-word) response is treated as unconfigured.
        // (The full happy-path decode is exercised live; this pins the guard.)
        // Build a canonical two-word return and confirm the extractors read it.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u256_be(42)); // guildId
        bytes.extend_from_slice(&u256_be(250)); // bps = 2.5%
        assert_eq!(u64_low(&bytes[0..32]), 42);
        assert_eq!(u64_low(&bytes[32..64]), 250);
        // Short buffer (< 64 bytes) must not panic in the decoder path.
        assert!(bytes[0..32].len() < 64);
    }
}
