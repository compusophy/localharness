use k256::ecdsa::SigningKey;

use super::*;

// --- WeightedVotingFacet (share-weighted governance — a cap-table board) --
//
// A SHARE-WEIGHTED board that runs IN PARALLEL to VotingFacet (one-member-one-
// vote) over the SAME guild treasury. A guild Admin assigns SHARES; a member
// proposes a treasury spend; members vote with weight == their shares; a passed
// measure executes from the guild's pooled treasury (the SAME `LibGuildStorage`
// ledger `guild spend` / `vote execute` debit, gated on a WEIGHTED vote).
// Sibling of `voting.rs`; same sponsored-write + dynamic-bytes calldata shape.
// EXACT ABI (matched to WeightedVotingFacet.sol):
//   setShares(uint256 guildId, address member, uint256 shares)
//   proposeWeighted(uint256 guildId, address to, uint256 amount, uint256 period, string memo) -> uint256
//   voteWeighted(uint256 proposalId, bool support)
//   executeWeighted(uint256 proposalId)
//   sharesOf(uint256, address) -> uint256
//   totalSharesOf(uint256) -> uint256
//   weightedProposal(uint256) -> (uint256 guildId, address proposer, address to,
//       uint256 amount, uint64 deadline, uint8 status, uint256 forShares,
//       uint256 againstShares, uint256 snapshotTotalShares)
//   weightedProposalMemoOf(uint256) -> bytes
//   weightedProposalsOf(uint256 guildId, uint256 startAfter, uint256 limit) -> (uint256[], uint256)
//   hasVotedWeighted(uint256, address) -> bool
//   weightedTallyOf(uint256) -> (uint256 forShares, uint256 againstShares,
//       uint256 quorumShares, uint256 castShares, bool passing)
//   weightedProposalCount() -> uint256
// status enum (LibWeightedVotingStorage.WStatus, ABI-pinned, mirrors VStatus):
//   0 Active / 1 Passed / 2 Failed / 3 Executed / 4 Expired.

/// One weighted-proposal record, decoded from `weightedProposal(uint256)`.
/// Field order/types mirror the facet's returned tuple exactly: guildId,
/// proposer, to, amount, deadline, status, forShares, againstShares,
/// snapshotTotalShares. `status` is the raw `WStatus` byte (see
/// [`WeightedProposal::status_label`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeightedProposal {
    /// The guild whose treasury this measure spends.
    pub guild_id: u64,
    /// The member who opened the proposal (the record's author), 0x-hex address.
    pub proposer: String,
    /// The spend recipient (may be a contract — a member-guild's TBA), 0x-hex.
    pub to: String,
    /// `$LH` (wei) the measure spends from the guild treasury on a pass.
    pub amount: u128,
    /// Unix seconds voting closes; `executeWeighted` opens after this.
    pub deadline: u64,
    /// Raw lifecycle byte: 0 Active, 1 Passed, 2 Failed, 3 Executed, 4 Expired.
    pub status: u8,
    /// Sum of for-voters' shares (NOT a headcount).
    pub for_shares: u128,
    /// Sum of against-voters' shares.
    pub against_shares: u128,
    /// Total shares FROZEN at propose — the quorum denominator.
    pub snapshot_total_shares: u128,
}

impl WeightedProposal {
    /// Human-readable lifecycle label for the raw `status` byte. Mirrors
    /// `LibWeightedVotingStorage.WStatus` (Active=0 … Expired=4).
    pub fn status_label(&self) -> &'static str {
        match self.status {
            0 => "active",
            1 => "passed",
            2 => "failed",
            3 => "executed",
            4 => "expired",
            _ => "unknown",
        }
    }
}

/// A weighted proposal's live tally, decoded from `weightedTallyOf(uint256)`:
/// `(forShares, againstShares, quorumShares, castShares, passing)`. `quorumShares`
/// is the DISPLAY "shares needed" (`snapshot/2 + 1`); `passing` is the facet's
/// read-only projection of the share-weighted pass test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeightedTally {
    /// Sum of for-voters' shares so far.
    pub for_shares: u128,
    /// Sum of against-voters' shares so far.
    pub against_shares: u128,
    /// Shares needed for quorum (`snapshotTotalShares / 2 + 1`).
    pub quorum_shares: u128,
    /// Shares cast so far (`for + against`).
    pub cast_shares: u128,
    /// Whether the proposal WOULD pass right now (quorum met AND `for > against`).
    pub passing: bool,
}

/// Encode `setShares(uint256 guildId, address member, uint256 shares)` — three
/// static head words (no dynamic args).
pub(crate) fn encode_set_shares(guild_id: u64, member: &[u8; 20], shares: u128) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 3 * 32);
    out.extend_from_slice(&selector("setShares(uint256,address,uint256)"));
    out.extend_from_slice(&u256_be(guild_id as u128)); // head 0: guildId
    out.extend_from_slice(&addr_word(member)); // head 1: member
    out.extend_from_slice(&u256_be(shares)); // head 2: shares
    out
}

/// Encode `proposeWeighted(uint256 guildId, address to, uint256 amount,
/// uint256 period, string memo)`. `memo` is the FIFTH (dynamic `string`) arg,
/// so its head word holds the OFFSET to the tail past the 5 fixed head words
/// (`5 * 32` = 0xA0); the tail is `[length][padded data]`. NOTE the layout
/// differs from `encode_propose` (VotingFacet): here `period` is head word 3
/// (BEFORE the memo offset in word 4), whereas VotingFacet's `votingPeriod` sat
/// AFTER the memo offset. A `string` ABI-encodes identically to `bytes`.
pub(crate) fn encode_propose_weighted(
    guild_id: u64,
    to: &[u8; 20],
    amount_wei: u128,
    period_secs: u64,
    memo: &[u8],
) -> Vec<u8> {
    let padded_len = memo.len().div_ceil(32) * 32;
    let mut out = Vec::with_capacity(4 + 5 * 32 + 32 + padded_len);
    out.extend_from_slice(&selector("proposeWeighted(uint256,address,uint256,uint256,string)"));
    out.extend_from_slice(&u256_be(guild_id as u128)); // head 0: guildId
    out.extend_from_slice(&addr_word(to)); // head 1: to
    out.extend_from_slice(&u256_be(amount_wei)); // head 2: amount
    out.extend_from_slice(&u256_be(period_secs as u128)); // head 3: period
    out.extend_from_slice(&u256_be(5 * 32)); // head 4: offset to memo tail (0xA0)
    out.extend_from_slice(&u256_be(memo.len() as u128)); // tail: length
    out.extend_from_slice(memo); // tail: memo bytes
    out.resize(out.len() + (padded_len - memo.len()), 0); // right-pad
    out
}

/// Encode `voteWeighted(uint256 proposalId, bool support)` — two static head
/// words (the bool is `uint256` 1/0 right-aligned in word 1).
pub(crate) fn encode_vote_weighted(proposal_id: u64, support: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 64);
    out.extend_from_slice(&selector("voteWeighted(uint256,bool)"));
    out.extend_from_slice(&u256_be(proposal_id as u128));
    out.extend_from_slice(&u256_be(if support { 1 } else { 0 }));
    out
}

/// Public ABI calldata for `voteWeighted(uint256 proposalId, bool support)` —
/// the inner call a sub-guild's TBA executes to cast its member-guild ballot in
/// a PARENT guild's weighted board (nested divisions). Thin `pub` wrapper over
/// the `pub(crate)` `encode_vote_weighted`, mirroring `encode_vote_calldata`.
pub fn encode_vote_weighted_calldata(proposal_id: u64, support: bool) -> Vec<u8> {
    encode_vote_weighted(proposal_id, support)
}

/// Admin sets a member's share weight via a sponsored Tempo tx
/// (`setShares(guildId, member, shares)`). One SSTORE + the running-total delta
/// + event; 800k for headroom (sponsor billed on gas USED).
pub async fn set_shares_sponsored(
    sender: &SigningKey,
    guild_id: u64,
    member_hex: &str,
    shares: u128,
) -> Result<String, String> {
    let member = parse_eth_address(member_hex)?;
    sponsored_diamond_call(
        sender,
        encode_set_shares(guild_id, &member, shares),
        800_000,
    )
    .await
}

/// Open a share-weighted proposal via a sponsored Tempo tx
/// (`proposeWeighted(guildId, to, amount, period, memo)`). No escrow — the
/// spend is debited from the guild's EXISTING treasury at `executeWeighted`
/// time. Returns the tx hash; read the new proposalId from
/// `weighted_proposals_of(guildId, …)` (its last entry). The `memo` write
/// scales the gas (mirrors `propose_sponsored`: 3M base + 9k/byte).
#[allow(clippy::too_many_arguments)]
pub async fn propose_weighted_sponsored(
    sender: &SigningKey,
    guild_id: u64,
    to_hex: &str,
    amount_wei: u128,
    period_secs: u64,
    memo: &[u8],
) -> Result<String, String> {
    let to = parse_eth_address(to_hex)?;
    let gas = 3_000_000 + (memo.len() as u128) * 9_000;
    sponsored_diamond_call(
        sender,
        encode_propose_weighted(guild_id, &to, amount_wei, period_secs, memo),
        gas,
    )
    .await
}

/// Cast one share-weighted ballot via a sponsored Tempo tx
/// (`voteWeighted(proposalId, support)`): `support == true` adds the voter's
/// shares to `forShares`, false to `againstShares`. Caller must be a guild
/// member with > 0 shares and not have voted (enforced on-chain). 800k headroom.
pub async fn vote_weighted_sponsored(
    sender: &SigningKey,
    proposal_id: u64,
    support: bool,
) -> Result<String, String> {
    sponsored_diamond_call(
        sender,
        encode_vote_weighted(proposal_id, support),
        800_000,
    )
    .await
}

/// Resolve a weighted proposal after its voting period ends via a sponsored
/// Tempo tx (`executeWeighted(proposalId)`): PERMISSIONLESS. On a PASS (quorum
/// met AND strict majority of cast shares for) it debits the guild treasury and
/// transfers `amount` `$LH` to `to` (via the inherited `GuildFacet._spendCore`);
/// otherwise FAILS with no spend. Idempotent. 3M for the cold token transfer.
pub async fn execute_weighted_proposal_sponsored(
    sender: &SigningKey,
    proposal_id: u64,
) -> Result<String, String> {
    sponsored_diamond_call(
        sender,
        call_uint_bytes("executeWeighted(uint256)", proposal_id),
        3_000_000,
    )
    .await
}

/// Read `sharesOf(uint256 guildId, address member)` → the member's cap-table
/// share weight (0 default). Two static args (the address right-aligned in
/// word 1).
pub async fn shares_of(guild_id: u64, member_hex: &str) -> Result<u128, String> {
    let member = parse_eth_address(member_hex)?;
    let result = read_view(
        selector("sharesOf(uint256,address)"),
        &[u256_be(guild_id as u128), addr_word(&member)],
    )
    .await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 32 {
        return Err(format!("sharesOf: short response {} bytes", bytes.len()));
    }
    Ok(u128_low(&bytes[0..32]))
}

/// Read `totalSharesOf(uint256 guildId)` → the guild's live total shares (the
/// quorum denominator source).
pub async fn total_shares_of(guild_id: u64) -> Result<u128, String> {
    let result = read_view(selector("totalSharesOf(uint256)"), &[u256_be(guild_id as u128)]).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 32 {
        return Err(format!("totalSharesOf: short response {} bytes", bytes.len()));
    }
    Ok(u128_low(&bytes[0..32]))
}

/// Read `weightedProposal(uint256)` → the full [`WeightedProposal`] record. The
/// returned tuple is all-static (the `memo` lives in its own mapping, read via
/// [`weighted_proposal_memo_of`]), so it decodes as 9 consecutive ABI words in
/// struct order: guildId, proposer, to, amount, deadline, status, forShares,
/// againstShares, snapshotTotalShares. NOTE the extra `snapshotTotalShares`
/// word vs VotingFacet's 8-word `getProposal`.
pub async fn get_weighted_proposal(proposal_id: u64) -> Result<WeightedProposal, String> {
    let result = read_view(
        selector("weightedProposal(uint256)"),
        &[u256_be(proposal_id as u128)],
    )
    .await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 9 * 32 {
        return Err(format!("weightedProposal: short response {} bytes", bytes.len()));
    }
    let word = |i: usize| &bytes[i * 32..(i + 1) * 32];
    Ok(WeightedProposal {
        guild_id: u64_low(word(0)),
        proposer: format!("0x{}", bytes_to_hex(&word(1)[12..32])), // address, low 20 bytes
        to: format!("0x{}", bytes_to_hex(&word(2)[12..32])),
        amount: u128_low(word(3)),
        deadline: u64_low(word(4)),
        status: bytes[5 * 32 + 31], // uint8 enum in the low byte of word 5
        for_shares: u128_low(word(6)),
        against_shares: u128_low(word(7)),
        snapshot_total_shares: u128_low(word(8)),
    })
}

/// Read `weightedTallyOf(uint256)` → the proposal's live [`WeightedTally`]. All
/// five returned values are static `uint256`/`bool` words (forShares,
/// againstShares, quorumShares, castShares, passing); `passing` is the low byte
/// of word 4.
pub async fn weighted_tally_of(proposal_id: u64) -> Result<WeightedTally, String> {
    let result = read_view(selector("weightedTallyOf(uint256)"), &[u256_be(proposal_id as u128)]).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 5 * 32 {
        return Err(format!("weightedTallyOf: short response {} bytes", bytes.len()));
    }
    let word = |i: usize| &bytes[i * 32..(i + 1) * 32];
    Ok(WeightedTally {
        for_shares: u128_low(word(0)),
        against_shares: u128_low(word(1)),
        quorum_shares: u128_low(word(2)),
        cast_shares: u128_low(word(3)),
        passing: bytes[4 * 32 + 31] != 0, // bool in the low byte of word 4
    })
}

/// Read `hasVotedWeighted(uint256 proposalId, address voter)` → whether `voter`
/// has cast a ballot (the double-vote guard). Two static args.
pub async fn has_voted_weighted(proposal_id: u64, voter_hex: &str) -> Result<bool, String> {
    let voter = parse_eth_address(voter_hex)?;
    let result = read_view(
        selector("hasVotedWeighted(uint256,address)"),
        &[u256_be(proposal_id as u128), addr_word(&voter)],
    )
    .await?;
    decode_u256_as_u64(&result).map(|v| v != 0)
}

/// Read `weightedProposalsOf(uint256 guildId, uint256 startAfter, uint256
/// limit)` → `(uint256[] ids, uint256 nextCursor)`. `startAfter` is a 0-based
/// INDEX into the guild's append-only proposal list (NOT a proposalId); pass 0
/// to begin. Returns the id list (decoded via the shared `(uint256[], uint256)`
/// cursor decoder).
pub async fn weighted_proposals_of(guild_id: u64, start_after: u64, limit: u64) -> Result<Vec<u64>, String> {
    let result = read_view(
        selector("weightedProposalsOf(uint256,uint256,uint256)"),
        &[
            u256_be(guild_id as u128),
            u256_be(start_after as u128),
            u256_be(limit as u128),
        ],
    )
    .await?;
    let bytes = hex_to_bytes(&result)?;
    decode_uint_array_with_cursor(&bytes)
}

/// Read `weightedProposalMemoOf(uint256)` — the proposal's opaque measure
/// `memo`, decoded UTF-8 (empty if none). Same `bytes` ABI shape as a `string`.
pub async fn weighted_proposal_memo_of(proposal_id: u64) -> Result<String, String> {
    decode_bytes_string_call("weightedProposalMemoOf(uint256)", proposal_id, "weightedProposalMemoOf").await
}

/// Read `weightedProposalCount()` → total weighted proposals ever created
/// (== the highest proposalId; ids monotonic from 1).
pub async fn weighted_proposal_count() -> Result<u64, String> {
    let result = read_view(selector("weightedProposalCount()"), &[]).await?;
    decode_u256_as_u64(&result)
}

#[cfg(test)]
mod weighted_voting_tests {
    use super::*;

    /// `setShares(uint256,address,uint256)` — three static words, no dynamic
    /// tail. Pins selector + each word so a shifted/widened arg is caught.
    #[test]
    fn set_shares_calldata_layout() {
        let member = [0xABu8; 20];
        let cd = encode_set_shares(0x10, &member, 60);
        assert_eq!(&cd[0..4], &selector("setShares(uint256,address,uint256)"));
        assert_eq!(cd.len(), 4 + 3 * 32);
        // head 0: guildId.
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 0x10);
        // head 1: member (right-aligned, low 20 bytes; top 12 zero).
        assert!(cd[36..36 + 12].iter().all(|&b| b == 0));
        assert_eq!(&cd[36 + 12..36 + 32], &member);
        // head 2: shares (low 16 bytes).
        assert_eq!(u128::from_be_bytes(cd[68 + 16..68 + 32].try_into().unwrap()), 60);
    }

    /// `proposeWeighted(uint256,address,uint256,uint256,string)` — the 5-arg
    /// DYNAMIC layout. memo is the 5th arg, so its offset = 5 head words (0xA0),
    /// and `period` is head word 3 (BEFORE the memo offset slot in word 4 — the
    /// difference from VotingFacet's `propose`, where the period sat AFTER the
    /// offset). Pins every head slot + the tail length/body so a shifted offset
    /// (the classic dynamic-encoding bug) is caught.
    #[test]
    fn propose_weighted_calldata_layout() {
        let to = [0xCDu8; 20];
        let amount = 2_000_000_000_000_000_000u128; // 2 $LH
        let period = 86_400u64; // 1 day
        let memo = b"fund q3 audit"; // 13 bytes -> one padded tail word
        let cd = encode_propose_weighted(0x10, &to, amount, period, memo);
        assert_eq!(&cd[0..4], &selector("proposeWeighted(uint256,address,uint256,uint256,string)"));
        // head 0: guildId.
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 0x10);
        // head 1: to (right-aligned, low 20 bytes).
        assert!(cd[36..36 + 12].iter().all(|&b| b == 0));
        assert_eq!(&cd[36 + 12..36 + 32], &to);
        // head 2: amount (low 16 bytes).
        assert_eq!(u128::from_be_bytes(cd[68 + 16..68 + 32].try_into().unwrap()), amount);
        // head 3: period (right-aligned) — BEFORE the offset word.
        assert_eq!(u64::from_be_bytes(cd[100 + 24..100 + 32].try_into().unwrap()), period);
        // head 4: offset to memo = 5 head words = 0xA0.
        assert_eq!(u64::from_be_bytes(cd[132 + 24..132 + 32].try_into().unwrap()), 0xA0);
        // tail length word at byte 4 + 0xA0 = 164.
        assert_eq!(u64::from_be_bytes(cd[164 + 24..164 + 32].try_into().unwrap()), memo.len() as u64);
        // tail body = the memo, right-padded to 32.
        assert_eq!(&cd[196..196 + memo.len()], memo);
        assert_eq!(cd.len(), 4 + 5 * 32 + 32 + 32); // sel + 5 head + len + 1 padded word
    }

    /// `proposeWeighted` with an EMPTY memo: offset still 0xA0, length 0, no body.
    #[test]
    fn propose_weighted_empty_memo() {
        let to = [0x01u8; 20];
        let cd = encode_propose_weighted(1, &to, 1, 3600, b"");
        assert_eq!(u64::from_be_bytes(cd[100 + 24..100 + 32].try_into().unwrap()), 3600); // period
        assert_eq!(u64::from_be_bytes(cd[132 + 24..132 + 32].try_into().unwrap()), 0xA0); // offset
        assert_eq!(u64::from_be_bytes(cd[164 + 24..164 + 32].try_into().unwrap()), 0); // length 0
        assert_eq!(cd.len(), 4 + 5 * 32 + 32); // sel + 5 head + length word, no body
    }

    /// `proposeWeighted` with a >32-byte memo pads the tail to a 64-byte multiple
    /// (two words) — guards the `div_ceil` padding.
    #[test]
    fn propose_weighted_pads_long_memo() {
        let to = [0x02u8; 20];
        let memo = b"a-treasury-measure-memo-well-over-32-bytes!!"; // 44 bytes
        let cd = encode_propose_weighted(3, &to, 5, 7200, memo);
        assert_eq!(u64::from_be_bytes(cd[164 + 24..164 + 32].try_into().unwrap()), memo.len() as u64);
        assert_eq!(&cd[196..196 + memo.len()], memo.as_slice());
        // 44 bytes -> padded to 64; total = sel + 5 head + len + 2 tail words.
        assert_eq!(cd.len(), 4 + 5 * 32 + 32 + 64);
        assert!(cd[196 + memo.len()..].iter().all(|&b| b == 0)); // zero-padded
    }

    /// `voteWeighted(uint256,bool)` — two static words; the bool is `uint256`
    /// 1/0 right-aligned in word 1. Pins both the for and against encodings.
    #[test]
    fn vote_weighted_calldata_bool_encoding() {
        let yes = encode_vote_weighted(7, true);
        assert_eq!(&yes[0..4], &selector("voteWeighted(uint256,bool)"));
        assert_eq!(yes.len(), 4 + 64);
        assert_eq!(u64::from_be_bytes(yes[4 + 24..4 + 32].try_into().unwrap()), 7); // proposalId
        // support=true: the whole word is zero except the final byte = 1.
        assert!(yes[36..36 + 31].iter().all(|&b| b == 0));
        assert_eq!(yes[36 + 31], 1);

        let no = encode_vote_weighted(7, false);
        assert!(no[36..36 + 32].iter().all(|&b| b == 0)); // all zero
        // The public wrapper produces the same bytes.
        assert_eq!(encode_vote_weighted_calldata(7, true), yes);
    }

    /// `executeWeighted(uint256)` routes through `call_uint_bytes` — pin selector + id.
    #[test]
    fn execute_weighted_calldata_layout() {
        let cd = call_uint_bytes("executeWeighted(uint256)", 11);
        assert_eq!(&cd[0..4], &selector("executeWeighted(uint256)"));
        assert_eq!(cd.len(), 36);
        assert_eq!(u64::from_be_bytes(cd[28..36].try_into().unwrap()), 11);
    }

    /// `WeightedProposal::status_label` maps every documented `WStatus` byte
    /// (and unknowns) — Active=0 … Expired=4 (mirrors VStatus).
    #[test]
    fn weighted_proposal_status_label_maps_enum() {
        let mut p = WeightedProposal {
            guild_id: 0,
            proposer: "0x00".into(),
            to: "0x00".into(),
            amount: 0,
            deadline: 0,
            status: 0,
            for_shares: 0,
            against_shares: 0,
            snapshot_total_shares: 0,
        };
        for (s, label) in [
            (0u8, "active"),
            (1, "passed"),
            (2, "failed"),
            (3, "executed"),
            (4, "expired"),
            (9, "unknown"),
        ] {
            p.status = s;
            assert_eq!(p.status_label(), label);
        }
    }

    /// `weightedProposalsOf` returns a `(uint256[], uint256)` — the SAME cursor
    /// shape as `proposalsOf`; round-trip through the shared decoder.
    #[test]
    fn weighted_proposals_of_cursor_decode() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u256_be(64)); // word 0: offset to the array
        bytes.extend_from_slice(&u256_be(42)); // word 1: cursor (ignored)
        bytes.extend_from_slice(&u256_be(2)); // length
        bytes.extend_from_slice(&u256_be(11));
        bytes.extend_from_slice(&u256_be(17));
        assert_eq!(decode_uint_array_with_cursor(&bytes).unwrap(), vec![11, 17]);
    }

    /// `weightedTallyOf` decodes the five static words. `passing` is the low byte
    /// of word 4; build a canonical encoding and assert each field.
    #[test]
    fn weighted_tally_decode_fields() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u256_be(60)); // forShares
        bytes.extend_from_slice(&u256_be(10)); // againstShares
        bytes.extend_from_slice(&u256_be(51)); // quorumShares (snapshot 100 -> 50+1)
        bytes.extend_from_slice(&u256_be(70)); // castShares
        bytes.extend_from_slice(&u256_be(1)); // passing = true
        let word = |i: usize| &bytes[i * 32..(i + 1) * 32];
        let t = WeightedTally {
            for_shares: u128_low(word(0)),
            against_shares: u128_low(word(1)),
            quorum_shares: u128_low(word(2)),
            cast_shares: u128_low(word(3)),
            passing: bytes[4 * 32 + 31] != 0,
        };
        assert_eq!(
            t,
            WeightedTally { for_shares: 60, against_shares: 10, quorum_shares: 51, cast_shares: 70, passing: true }
        );
    }

    /// `weightedProposal` decodes the 9 static words — one MORE than
    /// VotingFacet's `getProposal` (the trailing `snapshotTotalShares`). Build a
    /// canonical encoding and assert the extra word lands.
    #[test]
    fn weighted_proposal_decode_has_snapshot_word() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u256_be(5)); // guildId
        let mut proposer = [0u8; 32];
        proposer[12..].copy_from_slice(&[0x11u8; 20]);
        bytes.extend_from_slice(&proposer); // proposer
        let mut to = [0u8; 32];
        to[12..].copy_from_slice(&[0x22u8; 20]);
        bytes.extend_from_slice(&to); // to
        bytes.extend_from_slice(&u256_be(2_000_000_000_000_000_000)); // amount
        bytes.extend_from_slice(&u256_be(1_700_000_000)); // deadline
        bytes.extend_from_slice(&u256_be(3)); // status = executed
        bytes.extend_from_slice(&u256_be(60)); // forShares
        bytes.extend_from_slice(&u256_be(10)); // againstShares
        bytes.extend_from_slice(&u256_be(100)); // snapshotTotalShares
        let word = |i: usize| &bytes[i * 32..(i + 1) * 32];
        let p = WeightedProposal {
            guild_id: u64_low(word(0)),
            proposer: format!("0x{}", bytes_to_hex(&word(1)[12..32])),
            to: format!("0x{}", bytes_to_hex(&word(2)[12..32])),
            amount: u128_low(word(3)),
            deadline: u64_low(word(4)),
            status: bytes[5 * 32 + 31],
            for_shares: u128_low(word(6)),
            against_shares: u128_low(word(7)),
            snapshot_total_shares: u128_low(word(8)),
        };
        assert_eq!(p.guild_id, 5);
        assert_eq!(p.proposer, format!("0x{}", "11".repeat(20)));
        assert_eq!(p.to, format!("0x{}", "22".repeat(20)));
        assert_eq!(p.amount, 2_000_000_000_000_000_000);
        assert_eq!(p.status, 3);
        assert_eq!(p.for_shares, 60);
        assert_eq!(p.against_shares, 10);
        assert_eq!(p.snapshot_total_shares, 100);
        assert_eq!(p.status_label(), "executed");
    }
}
