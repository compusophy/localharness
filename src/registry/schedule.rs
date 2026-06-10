use k256::ecdsa::SigningKey;

use super::*;

// --- Scheduling (ScheduleFacet on the diamond) -----------------------
//
// The durable, tab-independent job registry: a holder ESCROWS `$LH` to
// back a recurring job that runs a `<name>.localharness.xyz` agent on a
// fixed interval (the on-chain answer to "persistent without keeping the
// tab open"). Escrow is the same approve-then-call shape `depositCredits`
// uses — `scheduleJob` does `transferFrom(caller -> diamond, budgetWei)`
// inside its body, so the bundle batches `approve(diamond, budgetWei)` on
// `$LH` + `scheduleJob(...)` into ONE sponsored Tempo tx. See
// `contracts/src/facets/ScheduleFacet.sol`.

/// One scheduled job, decoded from `getJob(uint256)`. Field order/types
/// mirror `LibScheduleStorage.Job` exactly (the ABI tuple `getJob` returns).
/// `status` is the raw `Status` enum byte: 0 Active / 1 Paused / 2 Cancelled
/// / 3 Exhausted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledJob {
    /// Who scheduled it (refund recipient / billing identity), 0x-hex.
    pub owner: String,
    /// Seconds between runs (the cadence).
    pub interval: u64,
    /// Raw lifecycle byte: 0 Active, 1 Paused, 2 Cancelled, 3 Exhausted.
    pub status: u8,
    /// Unix seconds of the next due fire (the CAS key; 0 once terminal).
    pub next_run: u64,
    /// `$LH` (wei) still escrowed for this job — debited per run, refundable.
    pub budget_wei: u128,
    /// Remaining runs (the hard count cap); hitting 0 → Exhausted.
    pub runs_left: u32,
    /// tokenId of the agent run each tick (name resolved off-chain).
    pub target_id: u64,
}

impl ScheduledJob {
    /// Human-readable lifecycle label for the raw `status` byte.
    pub fn status_label(&self) -> &'static str {
        match self.status {
            0 => "active",
            1 => "paused",
            2 => "cancelled",
            3 => "exhausted",
            _ => "unknown",
        }
    }
}

/// Encode `scheduleJob(uint256 targetId, bytes task, uint64 interval,
/// uint128 budgetWei, uint32 maxRuns)` calldata. `task` is a DYNAMIC `bytes`
/// arg, so the head holds an OFFSET to a tail of `[length][padded data]`
/// (same dynamic-bytes layout `encode_settle`'s signature uses). The four
/// scalars are static head words (uint64/uint128/uint32 right-aligned, the
/// 5-word fixed head means the bytes offset is always `5 * 32`).
pub(crate) fn encode_schedule_job(
    target_id: u64,
    task: &[u8],
    interval_secs: u64,
    budget_wei: u128,
    max_runs: u32,
) -> Vec<u8> {
    let padded_len = task.len().div_ceil(32) * 32;
    let mut out = Vec::with_capacity(4 + 5 * 32 + 32 + padded_len);
    out.extend_from_slice(&selector("scheduleJob(uint256,bytes,uint64,uint128,uint32)"));
    // Head word 0: targetId (uint256).
    out.extend_from_slice(&u256_be(target_id as u128));
    // Head word 1: offset to the `bytes task` tail — 5 fixed head words.
    out.extend_from_slice(&u256_be(5 * 32));
    // Head words 2..5: interval / budgetWei / maxRuns (each right-aligned).
    out.extend_from_slice(&u256_be(interval_secs as u128));
    out.extend_from_slice(&u256_be(budget_wei));
    out.extend_from_slice(&u256_be(max_runs as u128));
    // Tail: length + the task bytes, right-padded to a 32-byte multiple.
    out.extend_from_slice(&u256_be(task.len() as u128));
    out.extend_from_slice(task);
    out.resize(out.len() + (padded_len - task.len()), 0);
    out
}

pub(crate) fn encode_cancel_job(job_id: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32);
    out.extend_from_slice(&selector("cancelJob(uint256)"));
    out.extend_from_slice(&u256_be(job_id as u128));
    out
}

/// Schedule a recurring job via a sponsored Tempo tx. Batches
/// `approve(diamond, budgetWei)` on `$LH` + `scheduleJob(targetId, task,
/// interval, budgetWei, maxRuns)` in ONE tx — `scheduleJob` then escrows the
/// budget via `transferFrom` inside its own body (same cost-gate shape as
/// `deposit_credits_sponsored`). Returns the tx hash once mined; read the new
/// job id back from `jobs_of(owner)` (its last entry) or the `JobScheduled`
/// event.
#[allow(clippy::too_many_arguments)]
pub async fn schedule_job_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    target_id: u64,
    task: &[u8],
    interval_secs: u64,
    budget_wei: u128,
    max_runs: u32,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let token_addr = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS)?;
    let approve_call = crate::tempo_tx::TempoCall {
        to: token_addr,
        value_wei: 0,
        input: encode_approve(&diamond_addr, budget_wei),
    };
    let schedule_call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_schedule_job(target_id, task, interval_secs, budget_wei, max_runs),
    };
    // approve (~46k) + scheduleJob + ~275k sponsorship overhead. MEASURED via
    // `cast estimate`: scheduleJob alone is ~2.88M for a ~45-byte task (3 packed
    // cold job slots + the cold `task` bytes ~7.6k/BYTE + the two enumerable-index
    // pushes jobIds/jobsOfOwner + transferFrom + event). The old 1.5M base
    // OUT-OF-GASSED at ~1.9M (receipt status=false). 3.5M base + 9k/byte gives
    // comfortable headroom; the sponsor only pays gas USED, so over-budgeting is
    // free. (See the CLAUDE.md "cast estimate, never guess" gotcha.)
    let gas = 3_500_000 + (task.len() as u128) * 9_000;
    submit_tempo_sponsored(sender, fee_payer, vec![approve_call, schedule_call], fee_token, gas).await
}

/// Cancel a scheduled job via a sponsored Tempo tx — REFUNDS the job's full
/// remaining `budgetWei` to the owner (`cancelJob` is owner-gated on-chain).
pub async fn cancel_job_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    job_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_cancel_job(job_id),
    };
    // status flip + budget zero (1 SSTORE) + the refund `transfer` + event.
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 400_000).await
}

/// Read `jobsOf(address)` — every job id the owner has scheduled (Active +
/// terminal). The enumerable index backing the "my jobs" view.
pub async fn jobs_of(owner_hex: &str) -> Result<Vec<u64>, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(Vec::new());
    }
    let owner = parse_eth_address(owner_hex)?;
    let mut calldata = selector("jobsOf(address)").to_vec();
    calldata.extend_from_slice(&addr_word(&owner));
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    let bytes = hex_to_bytes(&result)?;
    // ABI dynamic uint256[]: [offset(32)][len(32)][id0(32)]... — shared decode.
    Ok(decode_u64_array(&bytes))
}

/// Read `getJob(uint256)` → the full [`ScheduledJob`] record. The returned
/// tuple is all-static (the `task` lives in its own mapping, read via
/// [`task_of`]), so it decodes as 7 consecutive ABI words in `Job` order:
/// owner, interval, status, nextRun, budgetWei, runsLeft, targetId.
pub async fn get_job(job_id: u64) -> Result<ScheduledJob, String> {
    let calldata = call_uint("getJob(uint256)", job_id);
    let result = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 7 * 32 {
        return Err(format!("getJob: short response {} bytes", bytes.len()));
    }
    let word = |i: usize| &bytes[i * 32..(i + 1) * 32];
    let owner = format!("0x{}", bytes_to_hex(&word(0)[12..32])); // address, low 20 bytes
    Ok(ScheduledJob {
        owner,
        interval: u64_low(word(1)),
        status: bytes[2 * 32 + 31], // Status enum in the low byte of word 2
        next_run: u64_low(word(3)),
        budget_wei: u128_low(word(4)),
        runs_left: u64_low(word(5)) as u32,
        target_id: u64_low(word(6)),
    })
}

/// Read `taskOf(uint256)` — the job's task prompt, decoded UTF-8. Stored as
/// on-chain `bytes` (same ABI shape as a `string` return: offset + length +
/// body); we interpret it as UTF-8 since the MVP task is an inline prompt.
pub async fn task_of(job_id: u64) -> Result<String, String> {
    let calldata = call_uint("taskOf(uint256)", job_id);
    let result = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    let raw = hex_to_bytes(&result)?;
    if raw.len() < 64 {
        return Err(format!("taskOf: short response {} bytes", raw.len()));
    }
    let len = u64::from_be_bytes(
        raw[56..64].try_into().map_err(|e: std::array::TryFromSliceError| e.to_string())?,
    ) as usize;
    // `len` is attacker-controlled — checked add so a huge length errors
    // instead of overflowing the slice.
    let end = len
        .checked_add(64)
        .filter(|&end| end <= raw.len())
        .ok_or_else(|| format!("taskOf: truncated body (len {}, have {})", len, raw.len()))?;
    String::from_utf8(raw[64..end].to_vec()).map_err(|e| e.to_string())
}


#[cfg(test)]
mod tests {
    use super::*;

    /// `scheduleJob(uint256,bytes,uint64,uint128,uint32)` calldata: the
    /// dynamic `bytes task` is the 2nd arg, so head word 1 must hold the
    /// offset (5 fixed head words = 160) and the tail must be length-prefixed
    /// then zero-padded to a 32-byte multiple. The four scalars are static head
    /// words AFTER the offset. A wrong offset/length would make the facet read a
    /// bogus task (or revert), and a mis-placed scalar would escrow the wrong
    /// budget / set the wrong interval — so pin every word.
    #[test]
    fn schedule_job_calldata_layout() {
        let task = b"ping the oracle"; // 15 bytes -> pads to 32
        let cd = encode_schedule_job(0x42, task, 300, 1_500_000_000_000_000_000u128, 100);
        assert_eq!(&cd[0..4], &selector("scheduleJob(uint256,bytes,uint64,uint128,uint32)"));
        // 5 static head words + length word + 32 bytes of padded task tail.
        assert_eq!(cd.len(), 4 + 5 * 32 + 32 + 32);
        // Word 0: targetId.
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 0x42);
        // Word 1: offset to the bytes tail = 5*32 = 160.
        assert_eq!(u64::from_be_bytes(cd[4 + 32 + 24..4 + 2 * 32].try_into().unwrap()), 5 * 32);
        // Word 2: interval (uint64, right-aligned).
        assert_eq!(u64::from_be_bytes(cd[4 + 2 * 32 + 24..4 + 3 * 32].try_into().unwrap()), 300);
        // Word 3: budgetWei (uint128 in the low 16 bytes).
        assert_eq!(
            u128::from_be_bytes(cd[4 + 3 * 32 + 16..4 + 4 * 32].try_into().unwrap()),
            1_500_000_000_000_000_000u128
        );
        // Word 4: maxRuns (uint32, right-aligned).
        assert_eq!(u64::from_be_bytes(cd[4 + 4 * 32 + 24..4 + 5 * 32].try_into().unwrap()), 100);
        // Tail word 5: bytes length = 15.
        assert_eq!(
            u64::from_be_bytes(cd[4 + 5 * 32 + 24..4 + 6 * 32].try_into().unwrap()),
            task.len() as u64
        );
        // The task bytes follow, then zero padding to the 32-byte boundary.
        assert_eq!(&cd[4 + 6 * 32..4 + 6 * 32 + task.len()], task);
        assert_eq!(&cd[4 + 6 * 32 + task.len()..], &[0u8; 32 - 15]);
    }

    /// A task that is an EXACT 32-byte multiple needs NO trailing padding —
    /// guard the `div_ceil` boundary (a 32-byte task must not gain a phantom
    /// 32-byte zero word).
    #[test]
    fn schedule_job_task_exact_multiple_no_extra_pad() {
        let task = [0xABu8; 32];
        let cd = encode_schedule_job(1, &task, 60, 1, 1);
        // 5 head + length + exactly 32 bytes of task, no extra pad word.
        assert_eq!(cd.len(), 4 + 5 * 32 + 32 + 32);
        assert_eq!(&cd[4 + 6 * 32..], &task);
    }

    #[test]
    fn cancel_job_calldata_layout() {
        let cd = encode_cancel_job(9);
        assert_eq!(&cd[0..4], &selector("cancelJob(uint256)"));
        assert_eq!(cd.len(), 36);
        assert_eq!(u64::from_be_bytes(cd[28..36].try_into().unwrap()), 9);
    }

    #[test]
    fn scheduled_job_status_label_maps_enum() {
        let mut j = ScheduledJob {
            owner: "0x00".into(),
            interval: 60,
            status: 0,
            next_run: 0,
            budget_wei: 0,
            runs_left: 0,
            target_id: 0,
        };
        assert_eq!(j.status_label(), "active");
        j.status = 1;
        assert_eq!(j.status_label(), "paused");
        j.status = 2;
        assert_eq!(j.status_label(), "cancelled");
        j.status = 3;
        assert_eq!(j.status_label(), "exhausted");
        j.status = 9;
        assert_eq!(j.status_label(), "unknown");
    }
}
