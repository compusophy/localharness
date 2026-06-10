use k256::ecdsa::SigningKey;

use super::*;

// --- InviteFacet (user-funded, refundable $LH invite codes) ----------
//
// A holder escrows their OWN $LH behind a bearer code; an accepter redeems it
// (paid out from escrow); the funder reclaims it after expiry if unclaimed.
// Mirrors `RedeemFacet`'s `keccak256(bytes(code))` hashing — only the hash is
// on-chain, the plaintext is the bearer secret distributed off-chain. See
// `design/invites.md`. EXACT ABI (matched to the parallel facet build):
//   createInvite(bytes32 codeHash, uint256 amount, uint64 ttlSeconds)
//   acceptInvite(string code)
//   reclaimInvite(bytes32 codeHash)
//   getInvite(bytes32) -> (address funder, uint128 amount, uint64 expiry, uint8 status)
//   escrowedOf(address) -> uint256

/// `keccak256(bytes(code))` — the on-chain invite key. IDENTICAL primitive to
/// `RedeemFacet.redeem`'s hash (`keccak_key(code.as_bytes())`), so a code
/// hashed here matches what `acceptInvite(string)` recomputes on-chain. NOT
/// trimmed: the facet hashes the exact string passed to `acceptInvite`, and
/// generated invite codes never carry whitespace — trimming here would diverge
/// from the chain for a code that legitimately contained leading/trailing space.
pub fn invite_code_hash(code: &str) -> [u8; 32] {
    keccak_key(code.as_bytes())
}

/// Encode `createInvite(bytes32 codeHash, uint256 amount, uint64 ttlSeconds)` —
/// three static head words (`amount`/`ttlSeconds` right-aligned in their words).
pub(crate) fn encode_create_invite(code_hash: &[u8; 32], amount_wei: u128, ttl_secs: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 3 * 32);
    out.extend_from_slice(&selector("createInvite(bytes32,uint256,uint64)"));
    out.extend_from_slice(code_hash);
    out.extend_from_slice(&u256_be(amount_wei));
    out.extend_from_slice(&u256_be(ttl_secs as u128));
    out
}

/// Encode `acceptInvite(string code)` — one dynamic-string arg (offset 0x20 +
/// length + right-padded bytes), the SAME ABI shape as `encode_redeem`.
pub(crate) fn encode_accept_invite(code: &str) -> Vec<u8> {
    let bytes = code.as_bytes();
    let len = bytes.len();
    let padded_len = len.div_ceil(32) * 32;
    let mut out = Vec::with_capacity(4 + 32 + 32 + padded_len);
    out.extend_from_slice(&selector("acceptInvite(string)"));
    out.extend_from_slice(&u256_be(0x20));
    out.extend_from_slice(&u256_be(len as u128));
    out.extend_from_slice(bytes);
    out.resize(4 + 32 + 32 + padded_len, 0);
    out
}

/// Encode `reclaimInvite(bytes32 codeHash)` — one static word.
pub(crate) fn encode_reclaim_invite(code_hash: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32);
    out.extend_from_slice(&selector("reclaimInvite(bytes32)"));
    out.extend_from_slice(code_hash);
    out
}

/// Create a refundable invite via a sponsored Tempo tx. Batches
/// `approve(diamond, amount)` on `$LH` + `createInvite(codeHash, amount, ttl)`
/// in ONE tx — `createInvite` then escrows the `$LH` via `transferFrom(caller,
/// diamond, amount)` inside its own body (the identical approve→pull escrow
/// pattern as `schedule_job_sponsored` / `deposit_credits_sponsored`). The
/// `amount` leaves the funder's spendable balance the moment this mines; it is
/// paid to the accepter on `acceptInvite` or refunded to the funder on
/// `reclaimInvite` after expiry. Returns the tx hash once mined.
#[allow(clippy::too_many_arguments)]
pub async fn create_invite_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    code_hash: [u8; 32],
    amount_wei: u128,
    ttl_secs: u64,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let token_addr = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS)?;
    let approve_call = crate::tempo_tx::TempoCall {
        to: token_addr,
        value_wei: 0,
        input: encode_approve(&diamond_addr, amount_wei),
    };
    let create_call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_create_invite(&code_hash, amount_wei, ttl_secs),
    };
    // approve (~46k) + createInvite (transferFrom pull + the invite struct's TWO
    // cold SSTOREs + the `escrowedOf` SSTORE + event) + ~275k sponsorship. These
    // are cold writes (CLAUDE.md "cold SSTOREs dominate; never guess — cast
    // estimate"); budget generously at 2.5M. The sponsor is billed on gas USED,
    // not the limit, so over-budgeting is free.
    submit_tempo_sponsored(sender, fee_payer, vec![approve_call, create_call], fee_token, 2_500_000)
        .await
}

/// Accept an invite via a sponsored Tempo tx: `acceptInvite(code)` pays the
/// escrowed `$LH` out to the CALLER (`sender`). The plaintext `code` is hashed
/// on-chain (`keccak256(bytes(code))`) to find the invite; the facet flips its
/// status to `Accepted` before the payout (CEI), so a replay reverts. Returns
/// the tx hash once mined.
pub async fn accept_invite_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    code: &str,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_accept_invite(code),
    };
    // status flip (1 SSTORE) + the payout `transfer` + `escrowedOf` decrement +
    // event — cheaper than create. Mirror redeem's mint-path budget for
    // headroom (cold token-balance SSTOREs on the payout).
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 2_000_000).await
}

/// Reclaim an expired, unclaimed invite via a sponsored Tempo tx:
/// `reclaimInvite(codeHash)` refunds the escrowed `$LH` to the FUNDER. The call
/// is permissionless (anyone can poke it; the `$LH` only ever goes to the
/// recorded funder), but the funder's own front-end / CLI normally triggers it.
/// Returns the tx hash once mined.
pub async fn reclaim_invite_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    code_hash: [u8; 32],
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_reclaim_invite(&code_hash),
    };
    // status flip + the refund `transfer` + `escrowedOf` decrement + event.
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 600_000).await
}

/// Read `escrowedOf(address)` — total `$LH` (18-decimal wei) the funder
/// currently has locked across all their `Open` invites (the running sum the
/// facet maintains on create/accept/reclaim). The "pending invites" total.
pub async fn escrowed_of(account_hex: &str) -> Result<u128, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(0);
    }
    let account = parse_eth_address(account_hex)?;
    let mut calldata = selector("escrowedOf(address)").to_vec();
    calldata.extend_from_slice(&addr_word(&account));
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    decode_u256_as_u128(&result)
}

/// Read `getInvite(bytes32) -> (address funder, uint128 amount, uint64 expiry,
/// uint8 status)`. Status: 0=Open, 1=Accepted, 2=Reclaimed. An empty/unknown
/// invite returns the zero record (funder `0x0…0`, amount 0). All four fields
/// pack into 4 consecutive ABI words (each value right-aligned in its word).
pub async fn get_invite(code_hash: [u8; 32]) -> Result<(String, u128, u64, u8), String> {
    let mut calldata = selector("getInvite(bytes32)").to_vec();
    calldata.extend_from_slice(&code_hash);
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 4 * 32 {
        return Err(format!("getInvite: short response {} bytes", bytes.len()));
    }
    let word = |i: usize| &bytes[i * 32..(i + 1) * 32];
    let funder = format!("0x{}", bytes_to_hex(&word(0)[12..32])); // address, low 20 bytes
    let amount = u128_low(word(1)); // uint128, low 16 bytes
    let expiry = u64_low(word(2)); // uint64, low 8 bytes
    let status = word(3)[31]; // uint8 enum in the low byte
    Ok((funder, amount, expiry, status))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_code_hash_matches_keccak256_of_code_bytes() {
        // The on-chain `acceptInvite(string)` recomputes keccak256(bytes(code));
        // our `invite_code_hash` MUST produce the same 32 bytes. Known vector
        // for the empty string: keccak256("") = c5d2460186f7...
        let h_empty = invite_code_hash("");
        let hex_empty: String = h_empty.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex_empty,
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
        // It IS exactly keccak_key(code.as_bytes()) — the same primitive
        // RedeemFacet's code hashing uses (so an invite code and a redeem code
        // hash identically), and matches `cast keccak "<code>"`.
        let code = "inv-100-A8kZqM2pQr";
        assert_eq!(invite_code_hash(code), keccak_key(code.as_bytes()));
        // Distinct codes hash distinctly; the hash is deterministic.
        assert_ne!(invite_code_hash("inv-10-aaaa"), invite_code_hash("inv-10-aaab"));
        assert_eq!(invite_code_hash(code), invite_code_hash(code));
    }

    #[test]
    fn encode_create_invite_layout() {
        let code_hash = invite_code_hash("inv-100-deadbeef01");
        let amount: u128 = 100 * 1_000_000_000_000_000_000; // 100 $LH in wei
        let ttl: u64 = 7 * 24 * 3600; // 7d
        let cd = encode_create_invite(&code_hash, amount, ttl);
        // selector(4) + 3 static head words = 100 bytes, no dynamic tail.
        assert_eq!(cd.len(), 4 + 3 * 32);
        // Selector pins the EXACT ABI signature the facet exposes.
        assert_eq!(&cd[..4], &selector("createInvite(bytes32,uint256,uint64)"));
        // Word 0 is the raw codeHash (bytes32 is NOT right-aligned — it occupies
        // the whole word as-is).
        assert_eq!(&cd[4..36], &code_hash[..]);
        // Word 1 = amount, right-aligned (low 16 bytes carry the u128).
        assert_eq!(&cd[36..68], &u256_be(amount)[..]);
        // Word 2 = ttlSeconds, right-aligned in its word.
        assert_eq!(&cd[68..100], &u256_be(ttl as u128)[..]);
    }

    #[test]
    fn encode_accept_invite_dynamic_string_layout() {
        let code = "inv-1000-Qm2pZ8kXaa"; // 19 bytes -> 1 padded word
        let cd = encode_accept_invite(code);
        assert_eq!(&cd[..4], &selector("acceptInvite(string)"));
        // Head word 0 = offset 0x20 to the string tail.
        assert_eq!(&cd[4..36], &u256_be(0x20)[..]);
        // Head word 1 = the string byte length.
        assert_eq!(&cd[36..68], &u256_be(code.len() as u128)[..]);
        // Tail = the bytes, then zero-padded to a 32-byte multiple.
        assert_eq!(&cd[68..68 + code.len()], code.as_bytes());
        let padded = code.len().div_ceil(32) * 32;
        assert_eq!(cd.len(), 4 + 32 + 32 + padded);
        // The padding bytes are zero.
        assert!(cd[68 + code.len()..].iter().all(|&b| b == 0));
        // Same dynamic-string ABI shape as `encode_redeem` (offset/len/body),
        // so the facet decodes it identically to RedeemFacet's `redeem(string)`.
        assert_eq!(&cd[4..36], &encode_redeem(code)[4..36]);
        assert_eq!(&cd[36..], &encode_redeem(code)[36..]);
    }

    #[test]
    fn encode_reclaim_invite_layout() {
        let code_hash = invite_code_hash("inv-10-cafef00d11");
        let cd = encode_reclaim_invite(&code_hash);
        assert_eq!(cd.len(), 4 + 32);
        assert_eq!(&cd[..4], &selector("reclaimInvite(bytes32)"));
        assert_eq!(&cd[4..36], &code_hash[..]);
    }
}
