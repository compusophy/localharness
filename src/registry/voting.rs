use k256::ecdsa::SigningKey;

use super::*;

// --- VotingFacet (DAO governance — Rung 4 of the coordination ladder) ----
//
// A guild MEMBER proposes a treasury spend, members VOTE one-member-one-vote,
// and a passed measure EXECUTES from the guild's pooled treasury (the SAME
// `LibGuildStorage` ledger `spendTreasury` debits, gated on a vote not the
// Admin role). Sibling of the bounty/guild helpers; same sponsored-write +
// dynamic-bytes calldata shape. EXACT ABI (matched to VotingFacet.sol):
//   propose(uint256 guildId, address to, uint256 amount, bytes memo, uint64 votingPeriod) -> uint256
//   vote(uint256 proposalId, bool support)
//   execute(uint256 proposalId)
//   getProposal(uint256) -> (uint256 guildId, address proposer, address to,
//       uint256 amount, uint64 deadline, uint8 status, uint256 forVotes,
//       uint256 againstVotes)
//   proposalMemoOf(uint256) -> bytes
//   proposalsOf(uint256 guildId, uint256 startAfter, uint256 limit) -> (uint256[], uint256)
//   hasVoted(uint256, address) -> bool
//   tallyOf(uint256) -> (uint256 forVotes, uint256 againstVotes, uint256 quorum,
//       uint256 votesCast, bool passing)
//   proposalCount() -> uint256
// status enum (LibVotingStorage.VStatus, ABI-pinned):
//   0 Active / 1 Passed / 2 Failed / 3 Executed / 4 Expired.

/// One proposal record, decoded from `getProposal(uint256)`. Field order/types
/// mirror the facet's returned tuple exactly: guildId, proposer, to, amount,
/// deadline, status, forVotes, againstVotes. `status` is the raw `VStatus` byte
/// (see [`Proposal::status_label`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    /// The guild whose treasury this measure spends.
    pub guild_id: u64,
    /// The member who opened the proposal (the record's author), 0x-hex address.
    pub proposer: String,
    /// The spend recipient (may be a contract — a member-guild's TBA), 0x-hex.
    pub to: String,
    /// `$LH` (wei) the measure spends from the guild treasury on a pass.
    pub amount: u128,
    /// Unix seconds voting closes; `execute` opens after this.
    pub deadline: u64,
    /// Raw lifecycle byte: 0 Active, 1 Passed, 2 Failed, 3 Executed, 4 Expired.
    pub status: u8,
    /// Weighted votes in favour (== count of for-voters, MVP weight 1).
    pub for_votes: u128,
    /// Weighted votes against.
    pub against_votes: u128,
}

impl Proposal {
    /// Human-readable lifecycle label for the raw `status` byte. Mirrors
    /// `LibVotingStorage.VStatus` (Active=0 … Expired=4).
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

/// A proposal's live tally, decoded from `tallyOf(uint256)`: `(forVotes,
/// againstVotes, quorum, votesCast, passing)`. The `passing` flag is the
/// facet's read-only projection of `_passed` against the CURRENT membership
/// (the final outcome is fixed at `execute`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    /// Weighted votes in favour so far.
    pub for_votes: u128,
    /// Weighted votes against so far.
    pub against_votes: u128,
    /// Distinct members that must have voted for the proposal to be eligible
    /// (`ceil(memberCount / 2)`, min 1).
    pub quorum: u128,
    /// Votes cast so far (`for + against`).
    pub votes_cast: u128,
    /// Whether the proposal WOULD pass right now (quorum met AND `for > against`).
    pub passing: bool,
}

/// Encode `propose(uint256 guildId, address to, uint256 amount, bytes memo,
/// uint64 votingPeriod)`. `memo` is the FOURTH (dynamic `bytes`) arg, so its
/// head word holds the OFFSET to the tail past the 5 fixed head words
/// (`5 * 32` = 0xA0); the tail is `[length][padded data]` (same dynamic-bytes
/// discipline as `encode_spend_treasury`, but with FIVE head words and the
/// `votingPeriod` head word AFTER the memo offset slot).
pub(crate) fn encode_propose(guild_id: u64, to: &[u8; 20], amount_wei: u128, memo: &[u8], voting_period_secs: u64) -> Vec<u8> {
    let padded_len = memo.len().div_ceil(32) * 32;
    let mut out = Vec::with_capacity(4 + 5 * 32 + 32 + padded_len);
    out.extend_from_slice(&selector("propose(uint256,address,uint256,bytes,uint64)"));
    out.extend_from_slice(&u256_be(guild_id as u128)); // head 0: guildId
    out.extend_from_slice(&addr_word(to)); // head 1: to
    out.extend_from_slice(&u256_be(amount_wei)); // head 2: amount
    out.extend_from_slice(&u256_be(5 * 32)); // head 3: offset to memo tail (0xA0)
    out.extend_from_slice(&u256_be(voting_period_secs as u128)); // head 4: votingPeriod
    out.extend_from_slice(&u256_be(memo.len() as u128)); // tail: length
    out.extend_from_slice(memo); // tail: memo bytes
    out.resize(out.len() + (padded_len - memo.len()), 0); // right-pad
    out
}

/// Encode `vote(uint256 proposalId, bool support)` — two static head words. A
/// Solidity `bool` is a `uint256` (1 for true, 0 for false) right-aligned in
/// its word; only the low byte is non-zero.
pub(crate) fn encode_vote(proposal_id: u64, support: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 64);
    out.extend_from_slice(&selector("vote(uint256,bool)"));
    out.extend_from_slice(&u256_be(proposal_id as u128));
    out.extend_from_slice(&u256_be(if support { 1 } else { 0 }));
    out
}

/// Public ABI calldata for `vote(uint256 proposalId, bool support)` — the inner
/// call a guild's TBA executes to cast a member-guild's ballot in a PARENT
/// guild's DAO (nested divisions). Thin `pub` wrapper over the `pub(crate)`
/// `encode_vote`, so a caller (the CLI `vote cast --tba`) can route it through
/// `tba_execute_call_sponsored` without re-rolling ABI.
pub fn encode_vote_calldata(proposal_id: u64, support: bool) -> Vec<u8> {
    encode_vote(proposal_id, support)
}

/// Open a proposal via a sponsored Tempo tx (`propose(guildId, to, amount, memo,
/// votingPeriod)`): a guild member proposes spending `amount_wei` of the guild
/// treasury to `to_hex`, opening a vote that closes at `now + voting_period_secs`.
/// No escrow/approve — the spend is debited from the guild's EXISTING treasury at
/// `execute` time, not pulled from the proposer. Returns the tx hash once mined;
/// read the new proposalId back from `proposals_of(guildId, …)` (its last entry)
/// or `proposal_count()`. The `memo` `bytes` write scales the gas.
#[allow(clippy::too_many_arguments)]
pub async fn propose_sponsored(
    sender: &SigningKey,
    guild_id: u64,
    to_hex: &str,
    amount_wei: u128,
    memo: &[u8],
    voting_period_secs: u64,
) -> Result<String, String> {
    let to = parse_eth_address(to_hex)?;
    // The proposal struct's cold SSTOREs (3 packed scalar slots + the
    // proposalsOfGuild enumerable push) + the cold `memo` bytes (~9k/byte) +
    // event + ~275k sponsorship. Measured: cast estimate propose ~= 2.35M, so a 2M
    // base OOG'd live — bump to 3M (sponsor billed on gas USED, headroom is free).
    let gas = 3_000_000 + (memo.len() as u128) * 9_000;
    sponsored_diamond_call(
        sender,
        encode_propose(guild_id, &to, amount_wei, memo, voting_period_secs),
        gas,
    )
    .await
}

/// Cast one ballot on an Active proposal via a sponsored Tempo tx
/// (`vote(proposalId, support)`): `support == true` adds to `forVotes`, false to
/// `againstVotes` (one-member-one-vote MVP). Caller must be a guild member and
/// not have voted already (enforced on-chain).
pub async fn vote_sponsored(
    sender: &SigningKey,
    proposal_id: u64,
    support: bool,
) -> Result<String, String> {
    // voted-flag SSTORE + the forVotes/againstVotes tally bump + event. 800k for
    // headroom (the propose/createGuild OOGs showed estimates run high; free on USED).
    sponsored_diamond_call(
        sender,
        encode_vote(proposal_id, support),
        800_000,
    )
    .await
}

/// Resolve a proposal after its voting period ends via a sponsored Tempo tx
/// (`execute(proposalId)`): PERMISSIONLESS. On a PASS (quorum met AND strict
/// majority for) it debits the guild treasury and transfers `amount` `$LH` to
/// `to` (via the inherited `GuildFacet._spend`); otherwise it FAILS with no
/// spend. Either way the proposal becomes terminal (idempotent — a second
/// `execute` reverts `ProposalNotActive`).
pub async fn execute_proposal_sponsored(
    sender: &SigningKey,
    proposal_id: u64,
) -> Result<String, String> {
    // status flip (1 SSTORE) + on the PASS path the treasury debit + payout
    // `transfer` (cold token balances) + event. Mirror the accept-result /
    // payout budget for headroom (sponsor billed on gas USED).
    sponsored_diamond_call(
        sender,
        call_uint_bytes("execute(uint256)", proposal_id),
        3_000_000,
    )
    .await
}

/// Read `getProposal(uint256)` → the full [`Proposal`] record. The returned
/// tuple is all-static (the `memo` lives in its own mapping, read via
/// [`proposal_memo_of`]), so it decodes as 8 consecutive ABI words in struct
/// order: guildId, proposer, to, amount, deadline, status, forVotes, againstVotes.
pub async fn get_proposal(proposal_id: u64) -> Result<Proposal, String> {
    let result = read_view(
        selector("getProposal(uint256)"),
        &[u256_be(proposal_id as u128)],
    )
    .await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 8 * 32 {
        return Err(format!("getProposal: short response {} bytes", bytes.len()));
    }
    let word = |i: usize| &bytes[i * 32..(i + 1) * 32];
    Ok(Proposal {
        guild_id: u64_low(word(0)),
        proposer: format!("0x{}", bytes_to_hex(&word(1)[12..32])), // address, low 20 bytes
        to: format!("0x{}", bytes_to_hex(&word(2)[12..32])),
        amount: u128_low(word(3)),
        deadline: u64_low(word(4)),
        status: bytes[5 * 32 + 31], // uint8 enum in the low byte of word 5
        for_votes: u128_low(word(6)),
        against_votes: u128_low(word(7)),
    })
}

/// Read `tallyOf(uint256)` → the proposal's live [`Tally`]. All five returned
/// values are static `uint256`/`bool` words (forVotes, againstVotes, quorum,
/// votesCast, passing) — decode each in its native width; `passing` is the low
/// byte of word 4.
pub async fn tally_of(proposal_id: u64) -> Result<Tally, String> {
    let result = read_view(selector("tallyOf(uint256)"), &[u256_be(proposal_id as u128)]).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 5 * 32 {
        return Err(format!("tallyOf: short response {} bytes", bytes.len()));
    }
    let word = |i: usize| &bytes[i * 32..(i + 1) * 32];
    Ok(Tally {
        for_votes: u128_low(word(0)),
        against_votes: u128_low(word(1)),
        quorum: u128_low(word(2)),
        votes_cast: u128_low(word(3)),
        passing: bytes[4 * 32 + 31] != 0, // bool in the low byte of word 4
    })
}

/// Read `hasVoted(uint256 proposalId, address voter)` → whether `voter` has cast
/// a ballot on the proposal (the double-vote guard). Two static args (the
/// address right-aligned in word 1).
pub async fn has_voted(proposal_id: u64, voter_hex: &str) -> Result<bool, String> {
    let voter = parse_eth_address(voter_hex)?;
    let result = read_view(
        selector("hasVoted(uint256,address)"),
        &[u256_be(proposal_id as u128), addr_word(&voter)],
    )
    .await?;
    decode_u256_as_u64(&result).map(|v| v != 0)
}

/// Read `proposalsOf(uint256 guildId, uint256 startAfter, uint256 limit)` →
/// `(uint256[] ids, uint256 nextCursor)`. `startAfter` is a 0-based INDEX into
/// the guild's append-only proposal list (NOT a proposalId); pass 0 to begin,
/// then the returned cursor to page on. Returns only the id list (the cursor is
/// the facet's internal pagination detail), decoded via the SAME
/// `(uint256[], uint256)` cursor decoder as `open_bounties`.
pub async fn proposals_of(guild_id: u64, start_after: u64, limit: u64) -> Result<Vec<u64>, String> {
    let result = read_view(
        selector("proposalsOf(uint256,uint256,uint256)"),
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

/// Read `proposalMemoOf(uint256)` — the proposal's opaque measure `memo`,
/// decoded UTF-8 (empty if none). Same `bytes` ABI shape as a `string` return.
pub async fn proposal_memo_of(proposal_id: u64) -> Result<String, String> {
    decode_bytes_string_call("proposalMemoOf(uint256)", proposal_id, "proposalMemoOf").await
}

/// Read `proposalCount()` → total proposals ever created (== the highest
/// proposalId; ids are monotonic from 1).
pub async fn proposal_count() -> Result<u64, String> {
    let result = read_view(selector("proposalCount()"), &[]).await?;
    decode_u256_as_u64(&result)
}


#[cfg(test)]
mod voting_tests {
    use super::*;

    /// `propose(uint256,address,uint256,bytes,uint64)` — the multi-arg DYNAMIC
    /// layout. memo is the 4th of 5 args, so its offset = 5 head words (0xA0),
    /// and `votingPeriod` is head word 4 (AFTER the memo offset slot). Pins every
    /// head slot + the tail length/body so a shifted offset (the classic
    /// dynamic-encoding bug) is caught.
    #[test]
    fn propose_calldata_layout() {
        let to = [0xCDu8; 20];
        let amount = 2_000_000_000_000_000_000u128; // 2 $LH
        let memo = b"fund q3 audit"; // 13 bytes -> one padded tail word
        let period = 86_400u64; // 1 day
        let cd = encode_propose(0x10, &to, amount, memo, period);
        assert_eq!(&cd[0..4], &selector("propose(uint256,address,uint256,bytes,uint64)"));
        // head 0: guildId.
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 0x10);
        // head 1: to (right-aligned, low 20 bytes; top 12 zero).
        assert!(cd[36..36 + 12].iter().all(|&b| b == 0));
        assert_eq!(&cd[36 + 12..36 + 32], &to);
        // head 2: amount (low 16 bytes).
        assert_eq!(u128::from_be_bytes(cd[68 + 16..68 + 32].try_into().unwrap()), amount);
        // head 3: offset to memo = 5 head words = 0xA0.
        assert_eq!(u64::from_be_bytes(cd[100 + 24..100 + 32].try_into().unwrap()), 0xA0);
        // head 4: votingPeriod (right-aligned).
        assert_eq!(u64::from_be_bytes(cd[132 + 24..132 + 32].try_into().unwrap()), period);
        // tail length word at byte 4 + 0xA0 = 164.
        assert_eq!(u64::from_be_bytes(cd[164 + 24..164 + 32].try_into().unwrap()), memo.len() as u64);
        // tail body = the memo, right-padded to 32.
        assert_eq!(&cd[196..196 + memo.len()], memo);
        assert_eq!(cd.len(), 4 + 5 * 32 + 32 + 32); // sel + 5 head + len + 1 padded word
    }

    /// `propose` with an EMPTY memo: offset still 0xA0, length 0, no body.
    #[test]
    fn propose_empty_memo() {
        let to = [0x01u8; 20];
        let cd = encode_propose(1, &to, 1, b"", 3600);
        assert_eq!(u64::from_be_bytes(cd[100 + 24..100 + 32].try_into().unwrap()), 0xA0); // offset
        assert_eq!(u64::from_be_bytes(cd[132 + 24..132 + 32].try_into().unwrap()), 3600); // votingPeriod
        assert_eq!(u64::from_be_bytes(cd[164 + 24..164 + 32].try_into().unwrap()), 0); // length 0
        assert_eq!(cd.len(), 4 + 5 * 32 + 32); // sel + 5 head + length word, no body
    }

    /// `propose` with a >32-byte memo pads the tail to a 64-byte multiple (two
    /// words) — guards the `div_ceil` padding.
    #[test]
    fn propose_pads_long_memo() {
        let to = [0x02u8; 20];
        let memo = b"a-treasury-measure-memo-well-over-32-bytes!!"; // 44 bytes
        let cd = encode_propose(3, &to, 5, memo, 7200);
        assert_eq!(u64::from_be_bytes(cd[164 + 24..164 + 32].try_into().unwrap()), memo.len() as u64);
        assert_eq!(&cd[196..196 + memo.len()], memo.as_slice());
        // 44 bytes -> padded to 64; total = sel + 5 head + len + 2 tail words.
        assert_eq!(cd.len(), 4 + 5 * 32 + 32 + 64);
        assert!(cd[196 + memo.len()..].iter().all(|&b| b == 0)); // zero-padded
    }

    /// `vote(uint256,bool)` — two static words; the bool is `uint256` 1/0
    /// right-aligned in word 1 (only the low byte is non-zero). Pins both the
    /// for and against encodings so a flipped/widened bool is caught.
    #[test]
    fn vote_calldata_bool_encoding() {
        let yes = encode_vote(7, true);
        assert_eq!(&yes[0..4], &selector("vote(uint256,bool)"));
        assert_eq!(yes.len(), 4 + 64);
        assert_eq!(u64::from_be_bytes(yes[4 + 24..4 + 32].try_into().unwrap()), 7); // proposalId
        // support=true: the whole word is zero except the final byte = 1.
        assert!(yes[36..36 + 31].iter().all(|&b| b == 0));
        assert_eq!(yes[36 + 31], 1);

        let no = encode_vote(7, false);
        // support=false: the bool word is all zero.
        assert!(no[36..36 + 32].iter().all(|&b| b == 0));
    }

    /// `execute(uint256)` routes through `call_uint_bytes` — pin selector + id.
    #[test]
    fn execute_calldata_layout() {
        let cd = call_uint_bytes("execute(uint256)", 11);
        assert_eq!(&cd[0..4], &selector("execute(uint256)"));
        assert_eq!(cd.len(), 36);
        assert_eq!(u64::from_be_bytes(cd[28..36].try_into().unwrap()), 11);
    }

    /// `Proposal::status_label` maps every documented `VStatus` byte (and
    /// unknowns) — Active=0 … Expired=4 (NOT the spec-page's mistaken 3-state
    /// enum; confirmed against LibVotingStorage.VStatus).
    #[test]
    fn proposal_status_label_maps_enum() {
        let mut p = Proposal {
            guild_id: 0,
            proposer: "0x00".into(),
            to: "0x00".into(),
            amount: 0,
            deadline: 0,
            status: 0,
            for_votes: 0,
            against_votes: 0,
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

    /// `proposalsOf` returns a `(uint256[], uint256)` — the SAME cursor shape as
    /// `openBounties`; round-trip a canonical encoding through the shared decoder.
    #[test]
    fn proposals_of_cursor_decode() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u256_be(64)); // word 0: offset to the array
        bytes.extend_from_slice(&u256_be(42)); // word 1: cursor (ignored)
        bytes.extend_from_slice(&u256_be(2)); // length
        bytes.extend_from_slice(&u256_be(11));
        bytes.extend_from_slice(&u256_be(17));
        assert_eq!(decode_uint_array_with_cursor(&bytes).unwrap(), vec![11, 17]);
    }

    /// `tallyOf` decodes the five static words. `passing` is the low byte of
    /// word 4; build a canonical encoding and assert each field.
    #[test]
    fn tally_decode_fields() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u256_be(3)); // forVotes
        bytes.extend_from_slice(&u256_be(1)); // againstVotes
        bytes.extend_from_slice(&u256_be(2)); // quorum
        bytes.extend_from_slice(&u256_be(4)); // votesCast
        bytes.extend_from_slice(&u256_be(1)); // passing = true
        // Drive the pure decode by inlining the same word math the async reader
        // uses (the network half can't run in a unit test).
        let word = |i: usize| &bytes[i * 32..(i + 1) * 32];
        let u128_low = |w: &[u8]| {
            let mut b = [0u8; 16];
            b.copy_from_slice(&w[16..32]);
            u128::from_be_bytes(b)
        };
        let t = Tally {
            for_votes: u128_low(word(0)),
            against_votes: u128_low(word(1)),
            quorum: u128_low(word(2)),
            votes_cast: u128_low(word(3)),
            passing: bytes[4 * 32 + 31] != 0,
        };
        assert_eq!(t, Tally { for_votes: 3, against_votes: 1, quorum: 2, votes_cast: 4, passing: true });
    }
}
