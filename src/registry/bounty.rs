use k256::ecdsa::SigningKey;

use super::*;

// --- BountyFacet (escrowed agent-economy task board) -----------------
//
// The DEMAND primitive: a poster ESCROWS `$LH` behind a task (`transferFrom`
// poster→diamond inside `postBounty`, so the bundle batches `approve(diamond,
// reward)` + `postBounty` in ONE sponsored tx — the identical escrow shape as
// `schedule_job_sponsored` / `create_invite_sponsored`). A claimant (identified
// by THEIR OWN tokenId) claims it, submits a result, and is paid the escrow on
// the poster's `acceptResult`. EXACT ABI (matched to the parallel facet build):
//   postBounty(bytes task, uint128 rewardWei, uint64 ttlSeconds) -> uint256 bountyId
//   claimBounty(uint256 bountyId, uint256 claimantTokenId)
//   submitResult(uint256 bountyId, bytes result)
//   acceptResult(uint256 bountyId) / cancelBounty(uint256) / reclaimExpired(uint256)
//   getBounty(uint256) -> (address poster, uint128 rewardWei, uint64 expiry,
//                          uint8 status, uint256 claimantTokenId)
//   bountyTaskOf(uint256)->bytes / resultOf(uint256)->bytes
//     ⛔ NOT `taskOf(uint256)` — that selector is RESERVED by ScheduleFacet
//     (the documented diamond collision), so encoding it here would dispatch
//     to the WRONG facet. See `task_of_bounty` below.
//   openBounties(uint256 startAfter, uint256 limit) -> (uint256[], uint256)
//   bountiesOf(address) -> uint256[]
// status: 0 Open / 1 Claimed / 2 Submitted / 3 Paid / 4 Cancelled / 5 Reclaimed.

/// One bounty record, decoded from `getBounty(uint256)`. Field order/types
/// mirror the facet's returned tuple exactly: poster, rewardWei, expiry,
/// status, claimantTokenId. `status` is the raw enum byte (see the table above).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bounty {
    /// Who posted it (the escrow funder / accept authority), 0x-hex address.
    pub poster: String,
    /// `$LH` (wei) escrowed as the reward — paid to the claimant on accept.
    pub reward_wei: u128,
    /// Unix seconds the bounty expires (the `reclaimExpired` gate; 0 if unset).
    pub expiry: u64,
    /// Raw lifecycle byte: 0 Open, 1 Claimed, 2 Submitted, 3 Paid, 4 Cancelled,
    /// 5 Reclaimed.
    pub status: u8,
    /// tokenId of the agent that claimed it (0 while Open).
    pub claimant_token_id: u64,
}

impl Bounty {
    /// Human-readable lifecycle label for the raw `status` byte.
    pub fn status_label(&self) -> &'static str {
        match self.status {
            0 => "open",
            1 => "claimed",
            2 => "submitted",
            3 => "paid",
            4 => "cancelled",
            5 => "reclaimed",
            _ => "unknown",
        }
    }
}

/// Map a bounty's on-chain state to a SPECIFIC, human-readable reason a
/// `claim`/`submit`/`accept` write would fail — so callers surface "already
/// claimed by dex-qa" instead of a generic "reverted" (GitHub #50). `action` is
/// `"claim"`/`"submit"`/`"accept"`; each gates on the status the facet requires
/// (Open→claim, Claimed→submit, Submitted→accept). `claimant_label` (a resolved
/// name) sharpens the "already claimed by …" message. Pure + testable; `Ok(())`
/// means the precondition holds. Shared by the CLI and the browser bounty tools.
pub fn bounty_preflight(
    id: u64,
    b: &Bounty,
    action: &str,
    claimant_label: Option<&str>,
) -> Result<(), String> {
    // A never-posted id decodes as the zero record (poster all-zero).
    if b.poster.trim_start_matches("0x").chars().all(|c| c == '0') {
        return Err(format!("bounty #{id} doesn't exist"));
    }
    let who = || match claimant_label {
        Some(l) if !l.is_empty() => format!(" by {l}"),
        _ if b.claimant_token_id != 0 => format!(" by token #{}", b.claimant_token_id),
        _ => String::new(),
    };
    match action {
        "claim" => match b.status {
            0 => Ok(()),
            1 | 2 => Err(format!("bounty #{id} is already claimed{} — pick another", who())),
            _ => Err(format!(
                "bounty #{id} is not open (it's {}) — nothing to claim",
                b.status_label()
            )),
        },
        "submit" => match b.status {
            1 => Ok(()),
            0 => Err(format!("bounty #{id} hasn't been claimed yet — claim it first")),
            2 => Err(format!("bounty #{id} already has a submitted result")),
            _ => Err(format!(
                "bounty #{id} is {} — you can't submit a result",
                b.status_label()
            )),
        },
        "accept" => match b.status {
            2 => Ok(()),
            0 | 1 => Err(format!(
                "bounty #{id} has no submitted result to accept yet (it's {})",
                b.status_label()
            )),
            _ => Err(format!(
                "bounty #{id} is already {} — nothing to accept",
                b.status_label()
            )),
        },
        "reclaim" => match b.status {
            1 | 2 => Ok(()),
            0 => Err(format!(
                "bounty #{id} is still open — `bounty cancel` refunds an unclaimed bounty"
            )),
            _ => Err(format!(
                "bounty #{id} is already {} — nothing to reclaim",
                b.status_label()
            )),
        },
        _ => Ok(()),
    }
}

/// Seconds until `b` passes the facet's `reclaimExpired` ttl gate (None =
/// already expired / no ttl set). Pure; pair with the "reclaim" preflight arm —
/// an early reclaim submits a REVERTING tx, so callers refuse client-side.
pub fn reclaim_wait_secs(b: &Bounty, now: u64) -> Option<u64> {
    (b.expiry != 0 && b.expiry > now).then(|| b.expiry - now)
}

/// Read `id`'s state (resolving the claimant's name for a sharper message) and
/// run [`bounty_preflight`]. `Ok(())` = the precondition holds OR the read
/// failed (let the write surface the real error — a transient read must never
/// block a legitimate action). `Err(msg)` = a NAMED precondition failure.
pub async fn bounty_preflight_check(id: u64, action: &str) -> Result<(), String> {
    let Ok(b) = get_bounty(id).await else {
        return Ok(()); // read failed → let the write speak for itself
    };
    let claimant_label = if b.claimant_token_id != 0 {
        crate::registry::name_of_id(b.claimant_token_id)
            .await
            .ok()
            .filter(|n| !n.is_empty())
    } else {
        None
    };
    bounty_preflight(id, &b, action, claimant_label.as_deref())
}

/// Encode `postBounty(bytes task, uint128 rewardWei, uint64 ttlSeconds)`. `task`
/// is the FIRST (dynamic `bytes`) arg, so head word 0 holds the OFFSET to the
/// tail (3 fixed head words = `3 * 32`); words 1/2 are `rewardWei`/`ttlSeconds`
/// right-aligned; the tail is `[length][padded data]` (same dynamic-bytes layout
/// `encode_schedule_job`'s `task` uses, but the bytes arg is FIRST here).
pub(crate) fn encode_post_bounty(task: &[u8], reward_wei: u128, ttl_secs: u64) -> Vec<u8> {
    let padded_len = task.len().div_ceil(32) * 32;
    let mut out = Vec::with_capacity(4 + 3 * 32 + 32 + padded_len);
    out.extend_from_slice(&selector("postBounty(bytes,uint128,uint64)"));
    // Head word 0: offset to the `bytes task` tail — 3 fixed head words.
    out.extend_from_slice(&u256_be(3 * 32));
    // Head words 1..3: rewardWei / ttlSeconds (each right-aligned).
    out.extend_from_slice(&u256_be(reward_wei));
    out.extend_from_slice(&u256_be(ttl_secs as u128));
    // Tail: length + the task bytes, right-padded to a 32-byte multiple.
    out.extend_from_slice(&u256_be(task.len() as u128));
    out.extend_from_slice(task);
    out.resize(out.len() + (padded_len - task.len()), 0);
    out
}

/// Encode `claimBounty(uint256 bountyId, uint256 claimantTokenId)` — two static
/// head words (bountyId, then the CLAIMANT'S OWN tokenId).
pub(crate) fn encode_claim_bounty(bounty_id: u64, claimant_token_id: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 64);
    out.extend_from_slice(&selector("claimBounty(uint256,uint256)"));
    out.extend_from_slice(&u256_be(bounty_id as u128));
    out.extend_from_slice(&u256_be(claimant_token_id as u128));
    out
}

/// Encode `submitResult(uint256 bountyId, bytes result)` — a static `bountyId`
/// head word then a dynamic `bytes result` (offset = 2 head words = `2 * 32`,
/// then `[length][padded data]`).
pub(crate) fn encode_submit_result(bounty_id: u64, result: &[u8]) -> Vec<u8> {
    let padded_len = result.len().div_ceil(32) * 32;
    let mut out = Vec::with_capacity(4 + 2 * 32 + 32 + padded_len);
    out.extend_from_slice(&selector("submitResult(uint256,bytes)"));
    out.extend_from_slice(&u256_be(bounty_id as u128));
    out.extend_from_slice(&u256_be(2 * 32)); // offset to the bytes tail
    out.extend_from_slice(&u256_be(result.len() as u128));
    out.extend_from_slice(result);
    out.resize(out.len() + (padded_len - result.len()), 0);
    out
}

/// Post a bounty via a sponsored Tempo tx. Batches `approve(diamond, rewardWei)`
/// on `$LH` + `postBounty(task, rewardWei, ttlSeconds)` in ONE tx — `postBounty`
/// then escrows the reward via `transferFrom(poster, diamond, rewardWei)` inside
/// its own body (the identical approve→pull escrow pattern as
/// `schedule_job_sponsored`). The `rewardWei` leaves the poster's spendable
/// balance the moment this mines; it pays the claimant on `acceptResult` or is
/// refunded on `cancelBounty` / `reclaimExpired`. Returns the tx hash once mined;
/// read the new bounty id back from `bounties_of(poster)` (its last entry).
#[allow(clippy::too_many_arguments)]
pub async fn post_bounty_sponsored(
    sender: &SigningKey,
    task: &[u8],
    reward_wei: u128,
    ttl_secs: u64,
) -> Result<String, String> {
    post_bounty_sponsored_bridged(sender, task, reward_wei, ttl_secs, 0)
        .await
}

/// [`post_bounty_sponsored`] with the meter auto-bridge: `bridge_wei > 0`
/// prepends `withdrawCredits(bridge_wei)` in the SAME atomic tx so unspent
/// chat-meter credits can back the escrow (see
/// `sponsored_escrow_diamond_call_bridged`).
#[allow(clippy::too_many_arguments)]
pub async fn post_bounty_sponsored_bridged(
    sender: &SigningKey,
    task: &[u8],
    reward_wei: u128,
    ttl_secs: u64,
    bridge_wei: u128,
) -> Result<String, String> {
    // approve (~46k) + postBounty (transferFrom pull + the bounty struct's cold
    // SSTOREs + the cold `task` bytes ~7.6k/BYTE + the bountiesOf enumerable push
    // + event) + ~275k sponsorship overhead. Cold writes dominate (CLAUDE.md
    // "cast estimate, never guess"); budget the same 3.5M base + 9k/byte the
    // scheduleJob escrow uses (also a struct + bytes + index push). The sponsor
    // is billed on gas USED, so over-budgeting is free.
    let gas = 3_500_000 + (task.len() as u128) * 9_000;
    sponsored_escrow_diamond_call_bridged(
        sender,
        reward_wei,
        encode_post_bounty(task, reward_wei, ttl_secs),
        gas,
        bridge_wei,
    )
    .await
}

/// Claim an Open bounty via a sponsored Tempo tx. `claimant_token_id` is the
/// CLAIMANT'S OWN registered tokenId (the on-chain identity that earns the
/// reward) — resolve it from the caller's identity (see the CLI's claimant
/// resolution), NOT the bounty's poster. `claimBounty` flips the status to
/// Claimed and records the claimant.
pub async fn claim_bounty_sponsored(
    sender: &SigningKey,
    bounty_id: u64,
    claimant_token_id: u64,
) -> Result<String, String> {
    // status flip + claimant SSTORE + event. 400k mirrors the cancelJob budget.
    sponsored_diamond_call(
        sender,
        encode_claim_bounty(bounty_id, claimant_token_id),
        400_000,
    )
    .await
}

/// Submit a result for a Claimed bounty via a sponsored Tempo tx. Stores the
/// `result` bytes on-chain (cold `bytes` write, ~7.6k/BYTE) and flips the status
/// to Submitted, awaiting the poster's `acceptResult`.
pub async fn submit_result_sponsored(
    sender: &SigningKey,
    bounty_id: u64,
    result: &[u8],
) -> Result<String, String> {
    // status flip + the cold `result` bytes SSTOREs (~7.6k/byte) + event. Scale
    // the same 1.2M base + 9k/byte the on-chain `bytes` writes use elsewhere.
    let gas = 1_200_000 + (result.len() as u128) * 9_000;
    sponsored_diamond_call(
        sender,
        encode_submit_result(bounty_id, result),
        gas,
    )
    .await
}

/// Accept a Submitted bounty's result via a sponsored Tempo tx: the poster (only)
/// calls `acceptResult(bountyId)`, which pays the escrowed `$LH` out to the
/// claimant's TBA and flips the status to Paid (CEI). Returns the tx hash.
pub async fn accept_result_sponsored(
    sender: &SigningKey,
    bounty_id: u64,
) -> Result<String, String> {
    // status flip (1 SSTORE) + the payout `transfer` (cold token balances) +
    // event. Mirror the redeem/accept-invite payout budget for headroom.
    sponsored_diamond_call(
        sender,
        call_uint_bytes("acceptResult(uint256)", bounty_id),
        2_000_000,
    )
    .await
}

/// Cancel a bounty via a sponsored Tempo tx: the poster (only) calls
/// `cancelBounty(bountyId)`, which REFUNDS the full escrowed `$LH` to the poster
/// and flips the status to Cancelled (allowed before payout).
pub async fn cancel_bounty_sponsored(
    sender: &SigningKey,
    bounty_id: u64,
) -> Result<String, String> {
    // status flip + the refund `transfer` + event.
    sponsored_diamond_call(
        sender,
        call_uint_bytes("cancelBounty(uint256)", bounty_id),
        600_000,
    )
    .await
}

/// Reclaim an expired, unaccepted bounty via a sponsored Tempo tx:
/// `reclaimExpired(bountyId)` refunds the escrowed `$LH` to the poster once the
/// TTL has elapsed without an accepted result, flipping the status to Reclaimed.
pub async fn reclaim_expired_sponsored(
    sender: &SigningKey,
    bounty_id: u64,
) -> Result<String, String> {
    // status flip + the refund `transfer` + event.
    sponsored_diamond_call(
        sender,
        call_uint_bytes("reclaimExpired(uint256)", bounty_id),
        600_000,
    )
    .await
}

/// Read `openBounties(uint256 startAfter, uint256 limit)` → `(uint256[] ids,
/// uint256 nextCursor)`. The paginated open-board scan: pass `startAfter = 0` to
/// begin, then the returned cursor to page on (0 = no more). Returns only the
/// id list here (the cursor is the facet's internal pagination detail); call
/// repeatedly bumping `start_after` to the last id when walking the whole board.
/// The ABI return is a dynamic `uint256[]` (head = offset to it + the cursor
/// word) followed by the cursor; we decode the array (low 8 bytes of each id,
/// monotonic u64-scale counters).
pub async fn open_bounties(start_after: u64, limit: u64) -> Result<Vec<u64>, String> {
    let result = read_view(
        selector("openBounties(uint256,uint256)"),
        &[u256_be(start_after as u128), u256_be(limit as u128)],
    )
    .await?;
    let bytes = hex_to_bytes(&result)?;
    decode_uint_array_with_cursor(&bytes)
}

/// Decode a `(uint256[] ids, uint256 cursor)` ABI return into the id `Vec`
/// (dropping the trailing cursor word). Head layout: word 0 = OFFSET to the
/// array, word 1 = the cursor. At `offset` sits `[length][id0][id1]…`. Pure +
/// testable; hostile-length-safe (no pre-alloc; checked index math stops the
/// decode on a bogus length instead of OOMing).
pub(crate) fn decode_uint_array_with_cursor(bytes: &[u8]) -> Result<Vec<u64>, String> {
    // Need at least the two head words (array offset + cursor).
    if bytes.len() < 64 {
        return Ok(Vec::new());
    }
    // Word 0: offset to the dynamic array (low 8 bytes — never near 2^64).
    let mut off_buf = [0u8; 8];
    off_buf.copy_from_slice(&bytes[24..32]);
    let arr_off = u64::from_be_bytes(off_buf) as usize;
    // The length word sits at the array offset.
    let len_start = match arr_off.checked_add(32) {
        Some(s) if s <= bytes.len() => arr_off,
        _ => return Ok(Vec::new()),
    };
    let mut len_buf = [0u8; 8];
    len_buf.copy_from_slice(&bytes[len_start + 24..len_start + 32]);
    let len = u64::from_be_bytes(len_buf) as usize;
    let body = len_start + 32; // first id word
    let mut out = Vec::new();
    for i in 0..len {
        let start = match i.checked_mul(32).and_then(|o| o.checked_add(body)) {
            Some(s) => s,
            None => break,
        };
        let Some(word) = start.checked_add(32).and_then(|end| bytes.get(start + 24..end)) else {
            break;
        };
        let mut id_buf = [0u8; 8];
        id_buf.copy_from_slice(word);
        out.push(u64::from_be_bytes(id_buf));
    }
    Ok(out)
}

/// Read `bountiesOf(address)` — every bounty id the address has POSTED (Open +
/// terminal). The enumerable index backing the "my bounties" view. Same ABI
/// shape as `jobsOf` (a bare dynamic `uint256[]`).
pub async fn bounties_of(account_hex: &str) -> Result<Vec<u64>, String> {
    let account = parse_eth_address(account_hex)?;
    let result = read_view(selector("bountiesOf(address)"), &[addr_word(&account)]).await?;
    let bytes = hex_to_bytes(&result)?;
    // Bare dynamic uint256[]: [offset(32)][len(32)][id0(32)]… — same shared
    // decode as `jobs_of`.
    Ok(decode_u64_array(&bytes))
}

/// Read `getBounty(uint256)` → the full [`Bounty`] record. The returned tuple is
/// all-static (the `task`/`result` live in their own mappings, read via
/// [`task_of_bounty`] / [`result_of_bounty`]), so it decodes as 5 consecutive
/// ABI words: poster, rewardWei, expiry, status, claimantTokenId. Returns the
/// poster as a 0x-hex address and each numeric in its native width.
pub async fn get_bounty(bounty_id: u64) -> Result<Bounty, String> {
    let result = read_view(selector("getBounty(uint256)"), &[u256_be(bounty_id as u128)]).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 5 * 32 {
        return Err(format!("getBounty: short response {} bytes", bytes.len()));
    }
    let word = |i: usize| &bytes[i * 32..(i + 1) * 32];
    let poster = format!("0x{}", bytes_to_hex(&word(0)[12..32])); // address, low 20 bytes
    Ok(Bounty {
        poster,
        reward_wei: u128_low(word(1)), // uint128, low 16 bytes
        expiry: u64_low(word(2)),
        status: bytes[3 * 32 + 31], // uint8 enum in the low byte of word 3
        claimant_token_id: u64_low(word(4)),
    })
}

/// Read `bountyTaskOf(uint256)` — the bounty's task prompt, decoded UTF-8.
/// (`taskOf` is RESERVED by ScheduleFacet — the documented diamond selector
/// collision — hence the `bounty`-prefixed selector.) Stored as on-chain `bytes`
/// (offset + length + body, same shape as a `string` return); we interpret it as
/// UTF-8 since the MVP task is an inline prompt.
pub async fn task_of_bounty(bounty_id: u64) -> Result<String, String> {
    decode_bytes_string_call("bountyTaskOf(uint256)", bounty_id, "bountyTaskOf").await
}

/// Read `resultOf(uint256)` — the submitted result bytes, decoded UTF-8. Empty
/// until the claimant `submitResult`s. Same `bytes` ABI shape as [`task_of_bounty`].
pub async fn result_of_bounty(bounty_id: u64) -> Result<String, String> {
    decode_bytes_string_call("resultOf(uint256)", bounty_id, "resultOf").await
}

/// Shared `fn(uint256) -> bytes` reader: eth_call the selector with `id`, decode
/// the returned dynamic `bytes` (offset + length + body) as UTF-8. The decode is
/// length-checked (attacker-controlled length can't overflow the slice).
pub(crate) async fn decode_bytes_string_call(sig: &str, id: u64, what: &str) -> Result<String, String> {
    let result = read_view(selector(sig), &[u256_be(id as u128)]).await?;
    let raw = hex_to_bytes(&result)?;
    if raw.len() < 64 {
        return Err(format!("{what}: short response {} bytes", raw.len()));
    }
    let len = u64::from_be_bytes(
        raw[56..64].try_into().map_err(|e: std::array::TryFromSliceError| e.to_string())?,
    ) as usize;
    let end = len
        .checked_add(64)
        .filter(|&end| end <= raw.len())
        .ok_or_else(|| format!("{what}: truncated body (len {}, have {})", len, raw.len()))?;
    String::from_utf8(raw[64..end].to_vec()).map_err(|e| e.to_string())
}

/// Build calldata for a `fn(uint256)` selector with a single id argument,
/// returning the RAW bytes (the `Vec<u8>` twin of [`call_uint`], which returns a
/// 0x-hex string). Used for the bounty single-arg WRITE selectors that go into a
/// `TempoCall.input`.
pub(crate) fn call_uint_bytes(sig: &str, id: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector(sig));
    data.extend_from_slice(&u256_be(id as u128));
    data
}

/// An open bounty the board scan could NOT read: at least one of its two per-id
/// `eth_call`s (`bountyTaskOf` / `getBounty`) failed. It is kept OUT of the
/// entries — a transient RPC hiccup must never render a FUNDED bounty as an
/// empty task worth 0 `$LH`, which reads to an agent as unpaid work. When only
/// one half failed the other half is DISCARDED rather than listed as a
/// half-known bounty; the id is what the caller retries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadBounty {
    /// The id `openBounties` returned but this scan could not resolve. Its task,
    /// reward and current status are all unknown here — re-read it (`get_bounty`
    /// / `task_of_bounty`) before drawing any conclusion about it.
    pub id: u64,
    /// Which read failed and why (`bountyTaskOf: …` / `getBounty: …`, both when
    /// both failed). Verbatim RPC prose — for surfacing, not for matching on.
    pub error: String,
}

/// A board scan's FULL outcome: the bounties that were read (ranked) plus the
/// ids that were not. `unreadable` is what makes a partial listing visible —
/// `entries` alone cannot distinguish "the board has 3 bounties" from "the board
/// has 20 and 17 reads failed".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BountyScan {
    /// Fully-read `(id, task, reward_wei)` bounties — query-ranked as returned
    /// by [`discover_bounties_scan`] (the internal `partition_bounty_reads`
    /// leaves them in board order; ranking is a separate step).
    pub entries: Vec<(u64, String, u128)>,
    /// Open ids whose task/reward read failed — NOT in `entries`, and NOT known
    /// to be unfunded. Empty = every open id in the scanned page was read.
    pub unreadable: Vec<UnreadBounty>,
}

impl BountyScan {
    /// True when at least one scanned open bounty could not be read, i.e.
    /// `entries` is an INCOMPLETE view of the scanned page. Callers that print
    /// a count should say so ("3 of 20 could not be read").
    pub fn is_partial(&self) -> bool {
        !self.unreadable.is_empty()
    }
}

/// One open id's raw read pair as the board scan fetched it: the id, the
/// `bountyTaskOf` outcome, and the `getBounty(…).reward_wei` outcome. Both
/// halves stay `Result` up to [`partition_bounty_reads`] — that is the whole
/// fix, since a defaulted value here is unrecoverable downstream.
pub(crate) type BountyReadRow = (u64, Result<String, String>, Result<u128, String>);

/// Split one [`BountyReadRow`] per open id into a [`BountyScan`]: fully-read
/// entries (UNRANKED, in board order) + the [`UnreadBounty`]s. PURE (the
/// network-free half of [`discover_bounties_scan`], so the decision is
/// unit-testable).
///
/// An entry survives only when BOTH reads succeeded: id, task and reward are all
/// load-bearing for the act-on-it decision (claim / do the work / is it worth
/// it), and `(u64, String, u128)` has no way to say "unknown" — a defaulted
/// empty task or `0` reward is indistinguishable from a genuinely empty task or
/// a genuinely 0-reward bounty, which is the bug this replaces.
///
/// `Err` ONLY when every row failed (and there was at least one): a board that
/// returned ids but yielded nothing readable must not come back as `Ok(vec![])`
/// — "no open bounties" is a different, and false, statement. A PARTIAL failure
/// is never an error: dropping a 20-item board because one read hiccuped is
/// worse than the hiccup.
pub(crate) fn partition_bounty_reads(rows: Vec<BountyReadRow>) -> Result<BountyScan, String> {
    let total = rows.len();
    let mut entries: Vec<(u64, String, u128)> = Vec::with_capacity(total);
    let mut unreadable: Vec<UnreadBounty> = Vec::new();
    for (id, task, reward) in rows {
        match (task, reward) {
            (Ok(task), Ok(reward_wei)) => entries.push((id, task, reward_wei)),
            (task, reward) => {
                let mut why: Vec<String> = Vec::new();
                if let Err(e) = task {
                    why.push(format!("bountyTaskOf: {e}"));
                }
                if let Err(e) = reward {
                    why.push(format!("getBounty: {e}"));
                }
                unreadable.push(UnreadBounty { id, error: why.join("; ") });
            }
        }
    }
    if total > 0 && entries.is_empty() {
        let first = unreadable.first().map(|u| u.error.as_str()).unwrap_or("unknown");
        return Err(format!(
            "bounty board unreadable: all {total} open bounty read(s) failed (first: {first})"
        ));
    }
    Ok(BountyScan { entries, unreadable })
}

/// Rank fully-read board entries by query-vs-TASK relevance. PURE. Reuses the
/// agent-rank pipeline (unchanged from the pre-partition code) by putting the id
/// in the "name" slot and the task in the "persona" slot, so ranking is driven
/// by the task text — a query that is itself a digit string can still hit the id
/// slot, as it always could. Empty query keeps every entry in board order.
pub(crate) fn rank_bounty_entries(
    entries: &[(u64, String, u128)],
    query: &str,
) -> Vec<(u64, String, u128)> {
    let pairs: Vec<(String, String)> = entries
        .iter()
        .map(|(id, task, _)| (id.to_string(), task.clone()))
        .collect();
    let ranked = rank_agent_matches(&pairs, query);
    // Map the ranked (id-string, task) pairs back to the (id, task, reward)
    // entries, preserving the rank order.
    let mut out: Vec<(u64, String, u128)> = Vec::with_capacity(ranked.len());
    for (id_str, _task) in ranked {
        if let Some(entry) = entries.iter().find(|(id, _, _)| id.to_string() == id_str) {
            out.push(entry.clone());
        }
    }
    out
}

/// Discover OPEN bounties by keyword, reporting what the scan could NOT read —
/// the demand-side twin of [`discover_agents`]. Scans the open board
/// (`open_bounties`, paging up to `scan` ids), reads each one's task text +
/// reward (one un-batched read PAIR per id, so a transient RPC failure hits a
/// SUBSET), and returns the ranked matches plus the ids whose reads failed.
/// Read-only.
///
/// Prefer this over [`discover_bounties`] anywhere the result is shown or acted
/// on: `BountyScan::unreadable` is the only way the caller learns that some
/// funded bounty exists which it did not see. Errors only when the board scan
/// itself fails or when NO id was readable at all — a PARTIAL failure always
/// returns the readable entries plus the unreadable ids.
pub async fn discover_bounties_scan(query: &str, scan: u64) -> Result<BountyScan, String> {
    let ids = open_bounties(0, scan).await?;
    if ids.is_empty() {
        return Ok(BountyScan::default());
    }
    // Fetch each open bounty's task text + reward. (The board is small at launch
    // scale; one read pair per id, like `discover_agents`' persona fetch.) Keep
    // the Results — the partition below decides, never a silent default.
    let mut rows: Vec<BountyReadRow> = Vec::with_capacity(ids.len());
    for id in ids {
        let task = task_of_bounty(id).await;
        let reward = get_bounty(id).await.map(|b| b.reward_wei);
        rows.push((id, task, reward));
    }
    let mut scan = partition_bounty_reads(rows)?;
    scan.entries = rank_bounty_entries(&scan.entries, query);
    Ok(scan)
}

/// Discover OPEN bounties by keyword — `(id, task, reward_wei)` matches for
/// `query`, ranked by query-vs-task relevance. An empty query returns all
/// readable open bounties (newest-first, as the board returns them). Read-only.
///
/// ⛔ LOSSY VIEW: a bounty whose task/reward read failed is OMITTED, and this
/// signature has nowhere to say so — the returned Vec can be a strict subset of
/// the open board with no marker. It never fabricates a value (an omitted entry
/// used to come back as an empty task with a `0` reward, i.e. a funded bounty
/// that read as unpaid), and it errors rather than returning an empty Vec when
/// NO id was readable, but a caller that reports a count or a "nothing matched"
/// verdict should use [`discover_bounties_scan`] and surface
/// `BountyScan::unreadable`.
pub async fn discover_bounties(query: &str, scan: u64) -> Result<Vec<(u64, String, u128)>, String> {
    Ok(discover_bounties_scan(query, scan).await?.entries)
}


#[cfg(test)]
mod tests {
    use super::*;

    // --- BountyFacet calldata layouts (network-free). A wrong offset/length on
    // the dynamic `bytes` args would escrow against a bogus task or pay the
    // wrong claimant, so pin every word. ---

    /// `postBounty(bytes task, uint128 rewardWei, uint64 ttlSeconds)`: the
    /// dynamic `bytes task` is the FIRST arg, so head word 0 is the offset
    /// (3 fixed head words = 96) and the tail is length-prefixed + zero-padded.
    #[test]
    fn post_bounty_calldata_layout() {
        let task = b"audit my solidity contract"; // 26 bytes -> pads to 32
        let reward = 5_000_000_000_000_000_000u128; // 5 $LH
        let cd = encode_post_bounty(task, reward, 86_400);
        assert_eq!(&cd[0..4], &selector("postBounty(bytes,uint128,uint64)"));
        // 3 static head words + length word + 32 bytes padded task tail.
        assert_eq!(cd.len(), 4 + 3 * 32 + 32 + 32);
        // Word 0: offset to the bytes tail = 3*32 = 96.
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 3 * 32);
        // Word 1: rewardWei (uint128 in the low 16 bytes).
        assert_eq!(
            u128::from_be_bytes(cd[4 + 32 + 16..4 + 2 * 32].try_into().unwrap()),
            reward
        );
        // Word 2: ttlSeconds (uint64, right-aligned).
        assert_eq!(u64::from_be_bytes(cd[4 + 2 * 32 + 24..4 + 3 * 32].try_into().unwrap()), 86_400);
        // Tail word 3: bytes length = 26.
        assert_eq!(
            u64::from_be_bytes(cd[4 + 3 * 32 + 24..4 + 4 * 32].try_into().unwrap()),
            task.len() as u64
        );
        // The task bytes follow, then zero padding to the 32-byte boundary.
        assert_eq!(&cd[4 + 4 * 32..4 + 4 * 32 + task.len()], task);
        assert_eq!(&cd[4 + 4 * 32 + task.len()..], &[0u8; 32 - 26]);
    }

    /// A task that is an EXACT 32-byte multiple gets NO trailing pad word —
    /// guard the `div_ceil` boundary.
    #[test]
    fn post_bounty_task_exact_multiple_no_extra_pad() {
        let task = [0xCDu8; 64];
        let cd = encode_post_bounty(&task, 1, 60);
        // 3 head + length + exactly 64 bytes of task, no extra pad word.
        assert_eq!(cd.len(), 4 + 3 * 32 + 32 + 64);
        assert_eq!(&cd[4 + 4 * 32..], &task);
    }

    /// `claimBounty(uint256 bountyId, uint256 claimantTokenId)`: two static
    /// words — the bountyId then the CLAIMANT'S OWN tokenId. A swapped pair
    /// would claim the wrong bounty or credit the wrong identity.
    #[test]
    fn claim_bounty_calldata_layout() {
        let cd = encode_claim_bounty(7, 42);
        assert_eq!(&cd[0..4], &selector("claimBounty(uint256,uint256)"));
        assert_eq!(cd.len(), 4 + 64);
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 7); // bountyId
        assert_eq!(u64::from_be_bytes(cd[4 + 32 + 24..4 + 2 * 32].try_into().unwrap()), 42); // claimantTokenId
    }

    /// `submitResult(uint256 bountyId, bytes result)`: a static `bountyId` head
    /// word then a dynamic `bytes` (offset = 2 head words = 64).
    #[test]
    fn submit_result_calldata_layout() {
        let result = b"done: see ipfs://Qm..."; // 22 bytes -> pads to 32
        let cd = encode_submit_result(3, result);
        assert_eq!(&cd[0..4], &selector("submitResult(uint256,bytes)"));
        assert_eq!(cd.len(), 4 + 2 * 32 + 32 + 32);
        // Word 0: bountyId.
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 3);
        // Word 1: offset to the bytes tail = 2*32 = 64.
        assert_eq!(u64::from_be_bytes(cd[4 + 32 + 24..4 + 2 * 32].try_into().unwrap()), 2 * 32);
        // Word 2: bytes length = 22.
        assert_eq!(
            u64::from_be_bytes(cd[4 + 2 * 32 + 24..4 + 3 * 32].try_into().unwrap()),
            result.len() as u64
        );
        assert_eq!(&cd[4 + 3 * 32..4 + 3 * 32 + result.len()], result);
        assert_eq!(&cd[4 + 3 * 32 + result.len()..], &[0u8; 32 - 22]);
    }

    /// The single-arg bounty WRITE selectors (`acceptResult`/`cancelBounty`/
    /// `reclaimExpired`) are `fn(uint256)` — one selector + one id word.
    #[test]
    fn single_arg_bounty_calldata_layouts() {
        for sig in [
            "acceptResult(uint256)",
            "cancelBounty(uint256)",
            "reclaimExpired(uint256)",
        ] {
            let cd = call_uint_bytes(sig, 11);
            assert_eq!(&cd[0..4], &selector(sig));
            assert_eq!(cd.len(), 36);
            assert_eq!(u64::from_be_bytes(cd[28..36].try_into().unwrap()), 11);
        }
    }

    /// `decode_uint_array_with_cursor` decodes a `(uint256[], uint256)` ABI
    /// return (the `openBounties` shape): the array offset in word 0, the cursor
    /// in word 1, then `[len][id…]` at the offset. Build a canonical encoding
    /// and round-trip it; the cursor word is dropped.
    #[test]
    fn open_bounties_cursor_decode() {
        let mut bytes = Vec::new();
        // Word 0: offset to the array = 64 (the array sits after the two head
        // words). Word 1: cursor (ignored by the decoder).
        bytes.extend_from_slice(&u256_be(64));
        bytes.extend_from_slice(&u256_be(99)); // cursor
        // Array body at offset 64: length = 3, then ids 5, 8, 13.
        bytes.extend_from_slice(&u256_be(3));
        bytes.extend_from_slice(&u256_be(5));
        bytes.extend_from_slice(&u256_be(8));
        bytes.extend_from_slice(&u256_be(13));
        let ids = decode_uint_array_with_cursor(&bytes).unwrap();
        assert_eq!(ids, vec![5, 8, 13]);
    }

    /// Empty / short / hostile returns must not panic. A too-short response
    /// yields an empty list; a bogus (huge) length stops the decode at the
    /// buffer edge rather than over-reading.
    #[test]
    fn open_bounties_cursor_decode_hostile() {
        assert!(decode_uint_array_with_cursor(&[]).unwrap().is_empty());
        assert!(decode_uint_array_with_cursor(&[0u8; 32]).unwrap().is_empty());
        // Offset 64, but claim length 1000 with only one id word present →
        // decode stops at the buffer edge (no panic, partial read).
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u256_be(64));
        bytes.extend_from_slice(&u256_be(0));
        bytes.extend_from_slice(&u256_be(1000)); // lying length
        bytes.extend_from_slice(&u256_be(7)); // only one real id
        let ids = decode_uint_array_with_cursor(&bytes).unwrap();
        assert_eq!(ids, vec![7]); // stopped cleanly after the available word
    }

    /// `Bounty::status_label` maps every documented enum byte (and unknowns).
    #[test]
    fn bounty_status_label_maps_enum() {
        let mut b = Bounty {
            poster: "0x00".into(),
            reward_wei: 0,
            expiry: 0,
            status: 0,
            claimant_token_id: 0,
        };
        for (s, label) in [
            (0u8, "open"),
            (1, "claimed"),
            (2, "submitted"),
            (3, "paid"),
            (4, "cancelled"),
            (5, "reclaimed"),
            (9, "unknown"),
        ] {
            b.status = s;
            assert_eq!(b.status_label(), label);
        }
    }

    /// `discover_bounties`' ranking reuses `rank_agent_matches` over the task
    /// text (the id occupies the "name" slot, the task the "persona" slot). A
    /// query hits a bounty whose TASK contains it; an empty query keeps all.
    /// (Pure-ranking exercise of the same pipeline `discover_bounties` runs.)
    #[test]
    fn bounty_rank_over_task_text() {
        let pairs = vec![
            ("1".to_string(), "audit a solidity contract".to_string()),
            ("2".to_string(), "write a poem".to_string()),
            ("3".to_string(), "SOLIDITY gas review".to_string()),
        ];
        let hits = rank_agent_matches(&pairs, "solidity");
        // Two tasks mention solidity (case-insensitive); both rank in the
        // persona tier (ids never contain the query), input order preserved.
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "1");
        assert_eq!(hits[1].0, "3");
        // Empty query keeps the whole board.
        assert_eq!(rank_agent_matches(&pairs, "").len(), 3);
    }

    #[test]
    fn bounty_preflight_names_the_revert_cause() {
        let mk = |status: u8, claimant: u64| Bounty {
            poster: "0xabc".into(),
            reward_wei: 1,
            expiry: 0,
            status,
            claimant_token_id: claimant,
        };
        // A never-posted id decodes as the zero record → "doesn't exist".
        let ghost = Bounty {
            poster: "0x0000000000000000000000000000000000000000".into(),
            reward_wei: 0,
            expiry: 0,
            status: 0,
            claimant_token_id: 0,
        };
        assert_eq!(
            bounty_preflight(999, &ghost, "claim", None),
            Err("bounty #999 doesn't exist".to_string())
        );
        // claim: only Open passes; Claimed names the claimant, else the token id.
        assert!(bounty_preflight(1, &mk(0, 0), "claim", None).is_ok());
        let e = bounty_preflight(1, &mk(1, 7), "claim", Some("dex-qa")).unwrap_err();
        assert!(e.contains("already claimed by dex-qa"), "got: {e}");
        let e = bounty_preflight(1, &mk(1, 7), "claim", None).unwrap_err();
        assert!(e.contains("token #7"), "got: {e}");
        assert!(bounty_preflight(1, &mk(3, 7), "claim", None).is_err());
        // submit: only Claimed passes; Open coaches "claim first".
        assert!(bounty_preflight(1, &mk(1, 7), "submit", None).is_ok());
        let e = bounty_preflight(1, &mk(0, 0), "submit", None).unwrap_err();
        assert!(e.contains("hasn't been claimed"), "got: {e}");
        assert!(bounty_preflight(1, &mk(2, 7), "submit", None).is_err());
        // accept: only Submitted passes.
        assert!(bounty_preflight(1, &mk(2, 7), "accept", None).is_ok());
        let e = bounty_preflight(1, &mk(1, 7), "accept", None).unwrap_err();
        assert!(e.contains("no submitted result"), "got: {e}");
        assert!(bounty_preflight(1, &mk(3, 7), "accept", None).is_err());
        // reclaim: Claimed/Submitted pass the status gate; Open coaches cancel.
        assert!(bounty_preflight(1, &mk(1, 7), "reclaim", None).is_ok());
        assert!(bounty_preflight(1, &mk(2, 7), "reclaim", None).is_ok());
        let e = bounty_preflight(1, &mk(0, 0), "reclaim", None).unwrap_err();
        assert!(e.contains("bounty cancel"), "got: {e}");
        assert!(bounty_preflight(1, &mk(3, 7), "reclaim", None).is_err());
    }

    // --- board-scan partition: a FAILED read must never look like a genuine
    // empty task / 0 reward (a funded bounty reading as unpaid is what made an
    // agent skip paying work, or claim work it believed paid nothing). ---

    /// Both reads good → a real entry, no unreadable ids, no error.
    #[test]
    fn partition_keeps_fully_read_bounties() {
        let rows = vec![
            (1u64, Ok("audit a contract".to_string()), Ok(5_000_000_000_000_000_000u128)),
            (2, Ok(String::new()), Ok(0)), // a GENUINELY empty/0 bounty still passes
        ];
        let scan = partition_bounty_reads(rows).unwrap();
        assert_eq!(
            scan.entries,
            vec![
                (1, "audit a contract".to_string(), 5_000_000_000_000_000_000u128),
                (2, String::new(), 0),
            ]
        );
        assert!(scan.unreadable.is_empty());
        assert!(!scan.is_partial()); // every scanned id was read → COMPLETE
    }

    /// A failed TASK read drops the entry (its reward is unknowable too) and is
    /// reported by id — NOT emitted as a blank task.
    #[test]
    fn partition_reports_failed_task_read() {
        let rows = vec![
            (7u64, Err("rpc timeout".to_string()), Ok(9u128)),
            (8, Ok("write a poem".to_string()), Ok(3)),
        ];
        let scan = partition_bounty_reads(rows).unwrap();
        assert_eq!(scan.entries, vec![(8, "write a poem".to_string(), 3u128)]);
        assert_eq!(scan.unreadable.len(), 1);
        assert_eq!(scan.unreadable[0].id, 7);
        let why = &scan.unreadable[0].error;
        assert!(why.contains("bountyTaskOf: rpc timeout"), "got: {why}");
        assert!(!why.contains("getBounty"), "got: {why}");
    }

    /// A failed REWARD read is the money case: the bounty is dropped + named,
    /// never listed with a fabricated `0`.
    #[test]
    fn partition_reports_failed_reward_read() {
        let rows = vec![
            (7u64, Ok("audit a contract".to_string()), Err("eth_call reverted".to_string())),
            (8, Ok("write a poem".to_string()), Ok(3u128)),
        ];
        let scan = partition_bounty_reads(rows).unwrap();
        // The dropped id appears nowhere in the entries — least of all at 0 $LH.
        assert_eq!(scan.entries, vec![(8, "write a poem".to_string(), 3u128)]);
        assert!(!scan.entries.iter().any(|(id, _, _)| *id == 7));
        assert_eq!(scan.unreadable[0].id, 7);
        let why = &scan.unreadable[0].error;
        assert!(why.contains("getBounty: eth_call reverted"), "got: {why}");
        // A scan carrying unreadable ids reports itself as INCOMPLETE.
        assert!(scan.is_partial());
        assert!(!BountyScan::default().is_partial());
    }

    /// Both reads failing names both; and when EVERY row failed the whole scan
    /// errors — an empty Vec would assert "no open bounties", which is false.
    #[test]
    fn partition_errors_when_every_read_failed() {
        let rows = vec![
            (7u64, Err("boom".to_string()), Err("bang".to_string())),
            (8, Err("boom".to_string()), Ok(3u128)),
        ];
        let err = partition_bounty_reads(rows).unwrap_err();
        assert!(err.contains("all 2 open bounty read(s) failed"), "got: {err}");
        assert!(err.contains("bountyTaskOf: boom"), "got: {err}");
        assert!(err.contains("getBounty: bang"), "got: {err}");
        // An EMPTY board (no ids scanned) is not an error — it's an empty board.
        assert_eq!(partition_bounty_reads(Vec::new()).unwrap(), BountyScan::default());
    }

    /// `rank_bounty_entries` ranks on the TASK text and carries the reward
    /// through unchanged; an empty query keeps every entry in board order.
    #[test]
    fn rank_bounty_entries_ranks_on_task_text() {
        let entries = vec![
            (1u64, "audit a solidity contract".to_string(), 5u128),
            (2, "write a poem".to_string(), 7),
            (3, "SOLIDITY gas review".to_string(), 11),
        ];
        let hits = rank_bounty_entries(&entries, "solidity");
        assert_eq!(
            hits,
            vec![
                (1, "audit a solidity contract".to_string(), 5u128),
                (3, "SOLIDITY gas review".to_string(), 11),
            ]
        );
        assert_eq!(rank_bounty_entries(&entries, ""), entries);
    }

    #[test]
    fn reclaim_wait_gates_on_ttl() {
        let mut b = Bounty {
            poster: "0xabc".into(),
            reward_wei: 1,
            expiry: 100,
            status: 1,
            claimant_token_id: 7,
        };
        assert_eq!(reclaim_wait_secs(&b, 40), Some(60)); // not yet expired
        assert_eq!(reclaim_wait_secs(&b, 100), None); // expired exactly
        b.expiry = 0;
        assert_eq!(reclaim_wait_secs(&b, 40), None); // no ttl gate set
    }
}
