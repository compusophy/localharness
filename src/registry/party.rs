use k256::ecdsa::SigningKey;

use super::*;

// --- PartyFacet (ad-hoc squads with an escrowed, pre-agreed split) --------
//
// Rung 2 of `design/shipped/agent-coordination.md` (bounty → PARTY → guild →
// DAO): an EPHEMERAL squad of agent identities formed around one objective,
// with a bps split fixed at formation, a pooled `$LH` pot anyone can fund,
// and settlement to each member's TBA on the creator's `completeParty` —
// then dissolution. Disband / TTL expiry refunds every funder exactly.
// EXACT ABI (matched to the facet):
//   formParty(uint256[] memberTokenIds, uint16[] sharesBps, uint64 ttlSeconds) -> uint256
//   joinParty(uint256 partyId)                  (consents every seat the caller owns)
//   fundParty(uint256 partyId, uint128 amount)  (transferFrom caller->diamond; APPROVE first)
//   completeParty(uint256 partyId) / disbandParty(uint256 partyId)
//   getParty(uint256) -> (address creator, uint64 expiry, uint8 status,
//                         uint128 escrowWei, uint256 memberCount, uint256 acceptedCount)
//   partyMembersOf(uint256)->uint256[] | partySharesOf(uint256)->uint16[]
//   partyConsentOf(uint256,uint256)->bool | partyFundersOf(uint256)->address[]
//   partyContributionOf(uint256,address)->uint256 | partiesOf(address)->uint256[]
//   partyCount()->uint256 | activePartyCountOf(address)->uint256
//   liveParties(uint256 startAfter, uint256 limit) -> (uint256[], uint256)
// status: 0 Forming / 1 Active / 2 Completed / 3 Disbanded.
// Every view is `party`-prefixed on-chain (the bountyTaskOf-vs-taskOf
// selector-collision lesson — a diamond can't share a selector).

/// One party record, decoded from `getParty(uint256)`. Field order/types
/// mirror the facet's returned tuple exactly: creator, expiry, status,
/// escrowWei, memberCount, acceptedCount. `status` is the raw enum byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Party {
    /// Who formed it (the complete/disband authority), 0x-hex address.
    pub creator: String,
    /// Unix seconds the party expires (the consent/fund/complete window end;
    /// past it anyone may disband and refund the funders).
    pub expiry: u64,
    /// Raw lifecycle byte: 0 Forming, 1 Active, 2 Completed, 3 Disbanded.
    pub status: u8,
    /// `$LH` (wei) pooled in the pot — split to member TBAs on complete,
    /// refunded to funders on disband.
    pub escrow_wei: u128,
    /// How many member seats the party has.
    pub member_count: u64,
    /// How many seats have consented (== member_count once Active).
    pub accepted_count: u64,
}

impl Party {
    /// Human-readable lifecycle label for the raw `status` byte.
    pub fn status_label(&self) -> &'static str {
        match self.status {
            0 => "forming",
            1 => "active",
            2 => "completed",
            3 => "disbanded",
            _ => "unknown",
        }
    }
}

/// Encode `formParty(uint256[] memberTokenIds, uint16[] sharesBps, uint64
/// ttlSeconds)`. TWO dynamic arrays + one static word: head word 0 = offset
/// to the members tail (3 fixed head words = `3 * 32`), head word 1 = offset
/// to the shares tail (after the members tail: `3*32 + 32 + 32*n`), head
/// word 2 = ttl. Each tail is `[length][elem0][elem1]…` — uint16 elements
/// still occupy a FULL word each (ABI dynamic arrays never pack).
pub(crate) fn encode_form_party(
    member_token_ids: &[u64],
    shares_bps: &[u16],
    ttl_secs: u64,
) -> Vec<u8> {
    let n = member_token_ids.len();
    let m = shares_bps.len();
    let mut out = Vec::with_capacity(4 + 3 * 32 + (1 + n) * 32 + (1 + m) * 32);
    out.extend_from_slice(&selector("formParty(uint256[],uint16[],uint64)"));
    // Head 0: offset to the members tail.
    out.extend_from_slice(&u256_be((3 * 32) as u128));
    // Head 1: offset to the shares tail (members tail = length word + n ids).
    out.extend_from_slice(&u256_be((3 * 32 + 32 + 32 * n) as u128));
    // Head 2: ttlSeconds.
    out.extend_from_slice(&u256_be(ttl_secs as u128));
    // Members tail: length + each tokenId as a full word.
    out.extend_from_slice(&u256_be(n as u128));
    for id in member_token_ids {
        out.extend_from_slice(&u256_be(*id as u128));
    }
    // Shares tail: length + each bps as a full (right-aligned) word.
    out.extend_from_slice(&u256_be(m as u128));
    for bps in shares_bps {
        out.extend_from_slice(&u256_be(*bps as u128));
    }
    out
}

/// Encode `fundParty(uint256 partyId, uint128 amount)` — two static head
/// words. Batched AFTER an `approve(diamond, amount)` (the facet
/// `transferFrom`s the caller→diamond inside its body, the BountyFacet /
/// fundGuild escrow shape).
pub(crate) fn encode_fund_party(party_id: u64, amount_wei: u128) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 64);
    out.extend_from_slice(&selector("fundParty(uint256,uint128)"));
    out.extend_from_slice(&u256_be(party_id as u128));
    out.extend_from_slice(&u256_be(amount_wei));
    out
}

/// Form a party via a sponsored Tempo tx: `formParty(memberTokenIds,
/// sharesBps, ttlSeconds)` proposes the squad + the split (bps MUST sum to
/// 10000; the facet enforces it). Seats the caller's address owns
/// auto-consent; the rest `joinParty`. Returns the tx hash once mined; read
/// the new partyId back from `parties_of(creator)` (its last entry, like
/// `bounties_of` / `guilds_of`).
pub async fn form_party_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    member_token_ids: &[u64],
    shares_bps: &[u16],
    ttl_secs: u64,
    fee_token: &str,
) -> Result<String, String> {
    // The party struct's cold SSTOREs + BOTH member/share array copies (one
    // cold word per member each) + the index pushes + per-creator-seat
    // consent SSTOREs + events + ~275k sponsorship. Cold writes dominate
    // (CLAUDE.md "cast estimate, never guess"); scale per member like the
    // other per-row escrow writes. Sponsor billed on gas USED.
    let gas = 2_500_000 + (member_token_ids.len() as u128) * 400_000;
    sponsored_diamond_call(
        sender,
        fee_payer,
        encode_form_party(member_token_ids, shares_bps, ttl_secs),
        fee_token,
        gas,
    )
    .await
}

/// Consent to a party via a sponsored Tempo tx (`joinParty(partyId)`): marks
/// consented every member seat whose identity the caller owns. The last
/// consent flips the party Active.
pub async fn join_party_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    party_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    // A consent SSTORE per owned seat + the acceptedCount/status update +
    // events. Budget like acceptGuildInvite (the same cold-write class).
    sponsored_diamond_call(
        sender,
        fee_payer,
        call_uint_bytes("joinParty(uint256)", party_id),
        fee_token,
        2_000_000,
    )
    .await
}

/// Fund a party's pot via a sponsored Tempo tx. Batches `approve(diamond,
/// amount)` on `$LH` + `fundParty(partyId, amount)` in ONE tx — `fundParty`
/// then escrows via `transferFrom(caller, diamond, amount)` inside its body
/// (the identical approve→pull pattern as `post_bounty_sponsored` /
/// `fund_guild_sponsored`). The `$LH` leaves the caller's spendable balance
/// the moment this mines; it splits to the member TBAs on complete or
/// refunds on disband/expiry.
pub async fn fund_party_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    party_id: u64,
    amount_wei: u128,
    fee_token: &str,
) -> Result<String, String> {
    fund_party_sponsored_bridged(sender, fee_payer, party_id, amount_wei, fee_token, 0).await
}

/// [`fund_party_sponsored`] with the meter auto-bridge: `bridge_wei > 0`
/// prepends `withdrawCredits(bridge_wei)` in the SAME atomic tx so unspent
/// chat-meter credits can back the contribution (see
/// `sponsored_escrow_diamond_call_bridged`).
pub async fn fund_party_sponsored_bridged(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    party_id: u64,
    amount_wei: u128,
    fee_token: &str,
    bridge_wei: u128,
) -> Result<String, String> {
    // approve (~46k) + fundParty (transferFrom pull + the escrow/ledger
    // SSTOREs + a possible funder-slot push + event) + ~275k sponsorship.
    // Mirror the fundGuild escrow budget.
    sponsored_escrow_diamond_call_bridged(
        sender,
        fee_payer,
        amount_wei,
        encode_fund_party(party_id, amount_wei),
        fee_token,
        2_000_000,
        bridge_wei,
    )
    .await
}

/// Settle a party via a sponsored Tempo tx: the CREATOR (only) calls
/// `completeParty(partyId)`, which splits the pooled `$LH` to each member
/// identity's TBA by the bps shares (remainder to the last member —
/// escrow-exact) and flips the status to Completed (CEI). Returns the tx
/// hash.
pub async fn complete_party_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    party_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    // status flip + ONE payout `transfer` per member (cold token balances,
    // up to 16 members) + events. Flat headroom over the worst case — the
    // sponsor is billed on gas USED, so over-budgeting is free.
    sponsored_diamond_call(
        sender,
        fee_payer,
        call_uint_bytes("completeParty(uint256)", party_id),
        fee_token,
        5_000_000,
    )
    .await
}

/// Dissolve a party via a sponsored Tempo tx: `disbandParty(partyId)`
/// refunds every funder their exact contribution and flips the status to
/// Disbanded. The creator may call it any time while the party is live;
/// anyone may once the TTL has expired (the refund always goes to the
/// FUNDERS, never the caller).
pub async fn disband_party_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    party_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    // status flip + ONE refund `transfer` per distinct funder (cap 64) +
    // event. Flat headroom over the worst case; sponsor billed on gas USED.
    sponsored_diamond_call(
        sender,
        fee_payer,
        call_uint_bytes("disbandParty(uint256)", party_id),
        fee_token,
        5_000_000,
    )
    .await
}

/// Read `getParty(uint256)` → the full [`Party`] record. The returned tuple
/// is all-static, so it decodes as 6 consecutive ABI words: creator, expiry,
/// status, escrowWei, memberCount, acceptedCount.
pub async fn get_party(party_id: u64) -> Result<Party, String> {
    let result = read_view(selector("getParty(uint256)"), &[u256_be(party_id as u128)]).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 6 * 32 {
        return Err(format!("getParty: short response {} bytes", bytes.len()));
    }
    let word = |i: usize| &bytes[i * 32..(i + 1) * 32];
    let creator = format!("0x{}", bytes_to_hex(&word(0)[12..32])); // address, low 20 bytes
    Ok(Party {
        creator,
        expiry: u64_low(word(1)),
        status: bytes[2 * 32 + 31], // uint8 enum in the low byte of word 2
        escrow_wei: u128_low(word(3)), // uint128, low 16 bytes
        member_count: u64_low(word(4)),
        accepted_count: u64_low(word(5)),
    })
}

/// Read `partyMembersOf(uint256)` — the member identity tokenIds (parallel
/// to [`party_shares_of`]). Bare dynamic `uint256[]`, the same decode as
/// `bounties_of` / `jobs_of`. Hostile-length-safe.
pub async fn party_members_of(party_id: u64) -> Result<Vec<u64>, String> {
    let result =
        read_view(selector("partyMembersOf(uint256)"), &[u256_be(party_id as u128)]).await?;
    let bytes = hex_to_bytes(&result)?;
    Ok(decode_u64_array(&bytes))
}

/// Read `partySharesOf(uint256)` — each member's share in basis points
/// (parallel to [`party_members_of`]; sums to 10000). ABI `uint16[]` still
/// encodes one full word per element, so the `uint256[]` decode applies;
/// values are clamped into `u16` (the facet never stores more).
pub async fn party_shares_of(party_id: u64) -> Result<Vec<u16>, String> {
    let result =
        read_view(selector("partySharesOf(uint256)"), &[u256_be(party_id as u128)]).await?;
    let bytes = hex_to_bytes(&result)?;
    Ok(decode_u64_array(&bytes).into_iter().map(|v| v as u16).collect())
}

/// Read `partyConsentOf(uint256, uint256)` — whether a member seat
/// (tokenId) has consented to the party's split.
pub async fn party_consent_of(party_id: u64, token_id: u64) -> Result<bool, String> {
    let result = read_view(
        selector("partyConsentOf(uint256,uint256)"),
        &[u256_be(party_id as u128), u256_be(token_id as u128)],
    )
    .await?;
    decode_u256_as_u64(&result).map(|v| v != 0)
}

/// Read `partyFundersOf(uint256)` — the distinct funder addresses (the
/// disband refund roster), as lowercase `0x…` strings.
pub async fn party_funders_of(party_id: u64) -> Result<Vec<String>, String> {
    let result =
        read_view(selector("partyFundersOf(uint256)"), &[u256_be(party_id as u128)]).await?;
    let bytes = hex_to_bytes(&result)?;
    Ok(decode_address_array(&bytes))
}

/// Read `partyContributionOf(uint256, address)` — a funder's exact
/// cumulative `$LH` contribution (wei); what disband refunds them.
pub async fn party_contribution_of(party_id: u64, funder_hex: &str) -> Result<u128, String> {
    let funder = parse_eth_address(funder_hex)?;
    let result = read_view(
        selector("partyContributionOf(uint256,address)"),
        &[u256_be(party_id as u128), addr_word(&funder)],
    )
    .await?;
    decode_u256_as_u128(&result)
}

/// Read `partiesOf(address)` — every party id the address has FORMED (live
/// + terminal). The enumerable index backing the "my parties" view.
pub async fn parties_of(creator_hex: &str) -> Result<Vec<u64>, String> {
    let creator = parse_eth_address(creator_hex)?;
    let result = read_view(selector("partiesOf(address)"), &[addr_word(&creator)]).await?;
    let bytes = hex_to_bytes(&result)?;
    Ok(decode_u64_array(&bytes))
}

/// Read `partyCount()` — total parties ever formed (ids are monotonic).
pub async fn party_count() -> Result<u64, String> {
    let result = read_view(selector("partyCount()"), &[]).await?;
    decode_u256_as_u64(&result)
}

/// Read `liveParties(uint256 startAfter, uint256 limit)` → the LIVE
/// (Forming/Active), unexpired party ids. Same `(uint256[], uint256
/// cursor)` ABI shape (and shared decode) as `openBounties`; the cursor is
/// the facet's internal pagination detail.
pub async fn live_parties(start_after: u64, limit: u64) -> Result<Vec<u64>, String> {
    let result = read_view(
        selector("liveParties(uint256,uint256)"),
        &[u256_be(start_after as u128), u256_be(limit as u128)],
    )
    .await?;
    let bytes = hex_to_bytes(&result)?;
    decode_uint_array_with_cursor(&bytes)
}

#[cfg(test)]
mod party_tests {
    use super::*;

    // --- PartyFacet calldata layouts (network-free). A wrong offset on the
    // TWO dynamic arrays would split the pot against bogus shares, so pin
    // every word. ---

    /// `formParty(uint256[],uint16[],uint64)`: two dynamic arrays + a
    /// static ttl. Head 0 = members offset (3 head words = 96), head 1 =
    /// shares offset (96 + (1+n)*32), head 2 = ttl; each tail is
    /// `[length][full-word elements]` (uint16 elements do NOT pack).
    #[test]
    fn form_party_calldata_layout() {
        let members = [7u64, 8u64];
        let shares = [6000u16, 4000u16];
        let cd = encode_form_party(&members, &shares, 86_400);
        assert_eq!(&cd[0..4], &selector("formParty(uint256[],uint16[],uint64)"));
        // sel + 3 head + (1+2) member words + (1+2) share words.
        assert_eq!(cd.len(), 4 + 3 * 32 + 3 * 32 + 3 * 32);
        let word_u64 = |i: usize| {
            u64::from_be_bytes(cd[4 + i * 32 + 24..4 + (i + 1) * 32].try_into().unwrap())
        };
        // Head 0: members offset = 96.
        assert_eq!(word_u64(0), 96);
        // Head 1: shares offset = 96 + 32 + 2*32 = 192.
        assert_eq!(word_u64(1), 192);
        // Head 2: ttl.
        assert_eq!(word_u64(2), 86_400);
        // Members tail: [2][7][8].
        assert_eq!(word_u64(3), 2);
        assert_eq!(word_u64(4), 7);
        assert_eq!(word_u64(5), 8);
        // Shares tail: [2][6000][4000].
        assert_eq!(word_u64(6), 2);
        assert_eq!(word_u64(7), 6000);
        assert_eq!(word_u64(8), 4000);
    }

    /// A single-member party still carries both tails (length 1 each) with
    /// the shares offset right after the one-id members tail.
    #[test]
    fn form_party_single_member_offsets() {
        let cd = encode_form_party(&[42], &[10_000], 3600);
        assert_eq!(cd.len(), 4 + 3 * 32 + 2 * 32 + 2 * 32);
        let word_u64 = |i: usize| {
            u64::from_be_bytes(cd[4 + i * 32 + 24..4 + (i + 1) * 32].try_into().unwrap())
        };
        assert_eq!(word_u64(0), 96); // members offset
        assert_eq!(word_u64(1), 96 + 32 + 32); // shares offset after [len][id]
        assert_eq!(word_u64(3), 1); // members length
        assert_eq!(word_u64(4), 42);
        assert_eq!(word_u64(5), 1); // shares length
        assert_eq!(word_u64(6), 10_000);
    }

    /// `fundParty(uint256,uint128)` — two static words (partyId, amount in
    /// the low 16 bytes of word 1).
    #[test]
    fn fund_party_calldata_layout() {
        let amount = 1_500_000_000_000_000_000u128; // 1.5 $LH
        let cd = encode_fund_party(9, amount);
        assert_eq!(&cd[0..4], &selector("fundParty(uint256,uint128)"));
        assert_eq!(cd.len(), 4 + 64);
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 9);
        assert_eq!(u128::from_be_bytes(cd[36 + 16..36 + 32].try_into().unwrap()), amount);
    }

    /// Single-arg `uint256` write selectors (join/complete/disband) route
    /// through `call_uint_bytes` — pin selector + the id word.
    #[test]
    fn single_arg_party_calldata_layouts() {
        for sig in ["joinParty(uint256)", "completeParty(uint256)", "disbandParty(uint256)"] {
            let cd = call_uint_bytes(sig, 11);
            assert_eq!(&cd[0..4], &selector(sig));
            assert_eq!(cd.len(), 36);
            assert_eq!(u64::from_be_bytes(cd[28..36].try_into().unwrap()), 11);
        }
    }

    /// `Party::status_label` maps every documented enum byte (and unknowns).
    #[test]
    fn party_status_label_maps_enum() {
        let mut p = Party {
            creator: "0x00".into(),
            expiry: 0,
            status: 0,
            escrow_wei: 0,
            member_count: 0,
            accepted_count: 0,
        };
        for (s, label) in [
            (0u8, "forming"),
            (1, "active"),
            (2, "completed"),
            (3, "disbanded"),
            (9, "unknown"),
        ] {
            p.status = s;
            assert_eq!(p.status_label(), label);
        }
    }
}
