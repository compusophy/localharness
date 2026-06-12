use k256::ecdsa::SigningKey;

use super::*;

// --- ValidationFacet (ERC-8004-style validation staking) -------------------
//
// The money-backed half of the reputation system (ReputationFacet attestations
// are the free-signal half): a VALIDATOR escrows `$LH` behind a verdict about a
// subject identity's `workRef`; a challenger counter-stakes the SAME amount
// behind the opposite verdict; the work's bounty POSTER (the platform
// convention is `workRef = bytes32(bountyId)`) or the diamond owner resolves,
// and the loser's stake pays the winner. Unchallenged stakes reclaim after the
// challenge window; unresolved challenges auto-draw (both refunded). EXACT ABI
// (mirrors contracts/src/facets/ValidationFacet.sol):
//   stakeValidation(bytes32 workRef, uint256 subjectTokenId, bool valid,
//                   uint256 stakeWei) -> uint256 id
//   challengeValidation(uint256 id)
//   resolveValidation(uint256 id, bool validatorWins)
//   reclaimStake(uint256 id)
//   reclaimUnresolved(uint256 id)
//   getValidation(uint256 id) -> (address validator, address challenger,
//     uint256 subjectTokenId, bytes32 workRef, uint128 stakeWei,
//     uint64 challengeDeadline, uint64 resolveDeadline, uint8 status,
//     bool verdictValid)
//   hasValidated(address, uint256, bytes32) -> bool
//   validationCount() -> uint256
//
// NOTE: the facet is built + tested but NOT yet cut into the live diamond —
// these helpers go live the moment script/AddValidationFacet.s.sol runs.

/// One decoded `getValidation` record. `status` is the ABI-pinned enum:
/// 0 Open, 1 Challenged, 2 Reclaimed, 3 ValidatorWon, 4 ChallengerWon,
/// 5 Drawn. Addresses are `0x`-lowercase hex; `work_ref_hex` is the raw
/// 32-byte ref as `0x`-hex. A zero `validator` means the id is unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validation {
    /// Who staked the verdict (the escrow's owner while Open).
    pub validator: String,
    /// Who counter-staked; the zero address until challenged.
    pub challenger: String,
    /// The identity whose work is being validated.
    pub subject_token_id: u64,
    /// The work pointer (`bytes32(bountyId)` couples the resolver).
    pub work_ref_hex: String,
    /// `$LH` each side escrows (18-dec wei).
    pub stake_wei: u128,
    /// Unix seconds; the challenge/reclaim window boundary.
    pub challenge_deadline: u64,
    /// Unix seconds; the resolve/draw window boundary (0 until challenged).
    pub resolve_deadline: u64,
    /// The lifecycle status enum (see the struct doc).
    pub status: u8,
    /// The validator's claim: true = "this work is valid".
    pub verdict_valid: bool,
}

/// Encode `stakeValidation(bytes32 workRef, uint256 subjectTokenId, bool valid,
/// uint256 stakeWei)` — four STATIC head words: the raw `bytes32 workRef`
/// (occupies its whole word as-is, NOT right-aligned), the subject tokenId
/// (right-aligned), the bool (low byte 0/1), and the stake in wei. Returns raw
/// calldata for a `TempoCall.input`. The `stake_validation_calldata_layout`
/// test pins this byte-for-byte.
pub(crate) fn encode_stake_validation(
    work_ref: &[u8; 32],
    subject_token_id: u64,
    valid: bool,
    stake_wei: u128,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 4 * 32);
    out.extend_from_slice(&selector("stakeValidation(bytes32,uint256,bool,uint256)"));
    out.extend_from_slice(work_ref); // word 0: bytes32 workRef (full word, as-is)
    out.extend_from_slice(&u256_be(subject_token_id as u128)); // word 1: subjectTokenId
    out.extend_from_slice(&u256_be(valid as u128)); // word 2: bool (low byte)
    out.extend_from_slice(&u256_be(stake_wei)); // word 3: stakeWei
    out
}

/// Stake `stake_wei` `$LH` behind a verdict (`valid`) about
/// `subject_token_id`'s `work_ref`, via ONE sponsored Tempo tx that batches
/// `$LH.approve(diamond, stakeWei)` + `stakeValidation(...)` (the facet pulls
/// the escrow via `transferFrom` inside its own body — the identical
/// approve→pull shape as `post_bounty_sponsored` / `create_invite`). The stake
/// leaves the validator's spendable balance the moment this mines; it comes
/// home on `reclaim_stake_sponsored` (unchallenged) or doubles/forfeits on
/// resolution. Reverts (surfaced to the caller) on a zero/over-cap stake, an
/// unknown subject, a self-validation, or a duplicate (validator, subject,
/// workRef). Read the new id back from `validation_count()` after mining.
pub async fn stake_validation_sponsored(
    validator_signer: &SigningKey,
    fee_payer: &SigningKey,
    work_ref: [u8; 32],
    subject_token_id: u64,
    valid: bool,
    stake_wei: u128,
    fee_token: &str,
) -> Result<String, String> {
    // approve (~46k) + stakeValidation: the transferFrom pull + a 5-slot cold
    // record + dedup flag + two enumerable index pushes + two counters + event.
    // Cold SSTOREs dominate (CLAUDE.md "cast estimate, never guess"); budget
    // the same 3.5M base the other struct-writing escrows (postBounty /
    // scheduleJob) use — the sponsor is billed on gas USED, over-budget is free.
    sponsored_escrow_diamond_call(
        validator_signer,
        fee_payer,
        stake_wei,
        encode_stake_validation(&work_ref, subject_token_id, valid, stake_wei),
        fee_token,
        3_500_000,
    )
    .await
}

/// Challenge an Open validation by counter-staking ITS `stakeWei` behind the
/// opposite verdict, via ONE sponsored Tempo tx (approve + challenge batched —
/// the facet pulls the counter-stake via `transferFrom`). `stake_wei` MUST be
/// the validation's own stake (read it from [`get_validation`] first); the
/// approve is for exactly that amount. Only valid while the validation is Open
/// and `now <= challengeDeadline`, and never by the validator themself.
pub async fn challenge_validation_sponsored(
    challenger_signer: &SigningKey,
    fee_payer: &SigningKey,
    validation_id: u64,
    stake_wei: u128,
    fee_token: &str,
) -> Result<String, String> {
    // approve + status flip + challenger/deadline SSTOREs + the transferFrom
    // pull + event. The record's slots are warm-ish but the token balances are
    // cold; 1.5M gives the same headroom the deposit-shaped escrows use.
    sponsored_escrow_diamond_call(
        challenger_signer,
        fee_payer,
        stake_wei,
        call_uint_bytes("challengeValidation(uint256)", validation_id),
        fee_token,
        1_500_000,
    )
    .await
}

/// Resolve a Challenged validation via a sponsored Tempo tx. RESOLVER-ONLY on
/// chain: the poster of bounty `uint256(workRef)` when one exists, or the
/// diamond owner (arbiter fallback). `validator_wins = true` sides with the
/// staked verdict; the winner is paid BOTH stakes.
pub async fn resolve_validation_sponsored(
    resolver_signer: &SigningKey,
    fee_payer: &SigningKey,
    validation_id: u64,
    validator_wins: bool,
    fee_token: &str,
) -> Result<String, String> {
    let mut input = Vec::with_capacity(4 + 2 * 32);
    input.extend_from_slice(&selector("resolveValidation(uint256,bool)"));
    input.extend_from_slice(&u256_be(validation_id as u128));
    input.extend_from_slice(&u256_be(validator_wins as u128));
    // status flip + two stakedOf decrements + the payout `transfer` (cold token
    // balances) + event — mirror the acceptResult payout budget.
    sponsored_diamond_call(resolver_signer, fee_payer, input, fee_token, 2_000_000).await
}

/// Reclaim an UNCHALLENGED validation's stake after the challenge window, via
/// a sponsored Tempo tx. Permissionless poke — the refund always goes to the
/// VALIDATOR regardless of who calls.
pub async fn reclaim_stake_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    validation_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    // status flip + accounting decrements + the refund `transfer` + event.
    sponsored_diamond_call(
        sender,
        fee_payer,
        call_uint_bytes("reclaimStake(uint256)", validation_id),
        fee_token,
        600_000,
    )
    .await
}

/// Refund a Challenged validation whose resolver never ruled (past the resolve
/// deadline), via a sponsored Tempo tx: BOTH sides take their own stake back (a
/// draw). Permissionless poke; the AWOL-resolver hard stop.
pub async fn reclaim_unresolved_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    validation_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    // status flip + accounting + TWO refund `transfer`s + event.
    sponsored_diamond_call(
        sender,
        fee_payer,
        call_uint_bytes("reclaimUnresolved(uint256)", validation_id),
        fee_token,
        800_000,
    )
    .await
}

/// Read `getValidation(uint256 id)` → the full [`Validation`] record, or
/// `Ok(None)` for an unknown id (a zero `validator`). Read-only, no `$LH`.
pub async fn get_validation(validation_id: u64) -> Result<Option<Validation>, String> {
    let result = read_view(
        selector("getValidation(uint256)"),
        &[u256_be(validation_id as u128)],
    )
    .await?;
    let bytes = hex_to_bytes(&result)?;
    Ok(decode_validation(&bytes))
}

/// Decode the `getValidation` return — nine STATIC words in declaration order:
/// validator, challenger, subjectTokenId, workRef, stakeWei, challengeDeadline,
/// resolveDeadline, status, verdictValid. Pure + bounds-checked: a short buffer
/// or a zero validator decodes to `None`, never a panic.
pub(crate) fn decode_validation(bytes: &[u8]) -> Option<Validation> {
    if bytes.len() < 9 * 32 {
        return None;
    }
    let word = |i: usize| &bytes[i * 32..(i + 1) * 32];
    let validator_bytes = &word(0)[12..32];
    if validator_bytes.iter().all(|&b| b == 0) {
        return None; // unknown id — the facet returns all-zeros
    }
    Some(Validation {
        validator: format!("0x{}", bytes_to_hex(validator_bytes)),
        challenger: format!("0x{}", bytes_to_hex(&word(1)[12..32])),
        subject_token_id: u64_low(word(2)),
        work_ref_hex: format!("0x{}", bytes_to_hex(word(3))),
        stake_wei: u128_low(word(4)),
        challenge_deadline: u64_low(word(5)),
        resolve_deadline: u64_low(word(6)),
        status: word(7)[31],
        verdict_valid: word(8)[31] != 0,
    })
}

/// Read `hasValidated(address validator, uint256 subjectTokenId, bytes32
/// workRef)` → bool — whether `validator_hex` already staked a verdict about
/// (`subject`, `work_ref`) (the facet's `AlreadyValidated` dedup, queryable up
/// front to skip a doomed write). Read-only.
pub async fn has_validated(
    validator_hex: &str,
    subject: u64,
    work_ref: [u8; 32],
) -> Result<bool, String> {
    let validator = parse_eth_address(validator_hex)?;
    let result = read_view(
        selector("hasValidated(address,uint256,bytes32)"),
        &[addr_word(&validator), u256_be(subject as u128), work_ref],
    )
    .await?;
    let bytes = hex_to_bytes(&result)?;
    Ok(bytes.last().is_some_and(|&b| b != 0))
}

/// Read `validationCount()` → total validations ever staked (== the highest
/// id; ids are monotonic from 1). Read-only.
pub async fn validation_count() -> Result<u64, String> {
    let result = read_view(selector("validationCount()"), &[]).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 32 {
        return Ok(0);
    }
    Ok(u64_low(&bytes[0..32]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stake_validation_calldata_layout() {
        // Pin the EXACT `stakeValidation(bytes32,uint256,bool,uint256)` wire
        // bytes — the bytes32-as-whole-word and bool packing are the easy-to-
        // get-wrong parts, so assert each word.
        let mut work_ref = [0u8; 32];
        work_ref[0] = 0xAB; // a recognisable high byte
        work_ref[24..32].copy_from_slice(&42u64.to_be_bytes()); // bounty-id-style low word
        let cd = encode_stake_validation(&work_ref, 7, true, 100_000_000_000_000_000_000);
        // selector(4) + 4 static head words = 132 bytes, no dynamic tail.
        assert_eq!(cd.len(), 4 + 4 * 32);
        assert_eq!(
            &cd[..4],
            &selector("stakeValidation(bytes32,uint256,bool,uint256)")
        );
        // Word 0 = the raw bytes32 workRef, occupying the WHOLE word as-is.
        assert_eq!(&cd[4..36], &work_ref[..]);
        assert_eq!(cd[4], 0xAB);
        // Word 1 = subjectTokenId, right-aligned.
        assert_eq!(&cd[36..68], &u256_be(7)[..]);
        // Word 2 = the bool, low byte 1, leading 31 bytes zero.
        assert!(cd[68..99].iter().all(|&b| b == 0), "bool word must be zero-padded");
        assert_eq!(cd[99], 1, "true must occupy the low byte of word 2");
        // Word 3 = stakeWei (100 $LH in 18-dec wei), right-aligned.
        assert_eq!(&cd[100..132], &u256_be(100_000_000_000_000_000_000)[..]);
        // false encodes a zero word 2.
        let cd_false = encode_stake_validation(&work_ref, 7, false, 1);
        assert!(cd_false[68..100].iter().all(|&b| b == 0), "false must be an all-zero word");
    }

    #[test]
    fn decode_validation_nine_static_words() {
        // Hand-build a `getValidation` return: 9 static words.
        let word = |hex: &str| -> String {
            assert!(hex.len() <= 64);
            format!("{:0>64}", hex)
        };
        let mut hex = String::from("0x");
        hex.push_str(&word("1111111111111111111111111111111111111111")); // validator
        hex.push_str(&word("2222222222222222222222222222222222222222")); // challenger
        hex.push_str(&word("7")); // subjectTokenId
        let mut wref = String::from("ab");
        wref.push_str(&"0".repeat(60));
        wref.push_str("2a");
        hex.push_str(&wref); // workRef: high byte AB, low byte 0x2a (bounty 42)
        hex.push_str(&word("56bc75e2d63100000")); // stakeWei = 100e18
        hex.push_str(&word("f4240")); // challengeDeadline = 1_000_000
        hex.push_str(&word("f9060")); // resolveDeadline = 1_020_000
        hex.push_str(&word("1")); // status = Challenged
        hex.push_str(&word("1")); // verdictValid = true
        let bytes = hex_to_bytes(&hex).unwrap();
        let v = decode_validation(&bytes).expect("decodes");
        assert_eq!(v.validator, "0x1111111111111111111111111111111111111111");
        assert_eq!(v.challenger, "0x2222222222222222222222222222222222222222");
        assert_eq!(v.subject_token_id, 7);
        assert!(v.work_ref_hex.starts_with("0xab"));
        assert!(v.work_ref_hex.ends_with("2a"));
        assert_eq!(v.stake_wei, 100_000_000_000_000_000_000);
        assert_eq!(v.challenge_deadline, 1_000_000);
        assert_eq!(v.resolve_deadline, 1_020_000);
        assert_eq!(v.status, 1);
        assert!(v.verdict_valid);

        // A zero validator (unknown id) decodes to None; short/empty buffers
        // decode to None (no panic).
        let zeros = vec![0u8; 9 * 32];
        assert!(decode_validation(&zeros).is_none());
        assert!(decode_validation(&[]).is_none());
        assert!(decode_validation(&[0u8; 64]).is_none());
    }
}
