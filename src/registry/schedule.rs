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
    schedule_job_sponsored_bridged(
        sender, fee_payer, target_id, task, interval_secs, budget_wei, max_runs, fee_token, 0,
    )
    .await
}

/// [`schedule_job_sponsored`] with the meter auto-bridge: `bridge_wei > 0`
/// prepends `withdrawCredits(bridge_wei)` in the SAME atomic tx so unspent
/// chat-meter credits can back the escrow (see
/// `sponsored_escrow_diamond_call_bridged`).
#[allow(clippy::too_many_arguments)]
pub async fn schedule_job_sponsored_bridged(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    target_id: u64,
    task: &[u8],
    interval_secs: u64,
    budget_wei: u128,
    max_runs: u32,
    fee_token: &str,
    bridge_wei: u128,
) -> Result<String, String> {
    // approve (~46k) + scheduleJob + ~275k sponsorship overhead. MEASURED via
    // `cast estimate`: scheduleJob alone is ~2.88M for a ~45-byte task (3 packed
    // cold job slots + the cold `task` bytes ~7.6k/BYTE + the two enumerable-index
    // pushes jobIds/jobsOfOwner + transferFrom + event). The old 1.5M base
    // OUT-OF-GASSED at ~1.9M (receipt status=false). 3.5M base + 9k/byte gives
    // comfortable headroom; the sponsor only pays gas USED, so over-budgeting is
    // free. (See the CLAUDE.md "cast estimate, never guess" gotcha.)
    let gas = 3_500_000 + (task.len() as u128) * 9_000;
    sponsored_escrow_diamond_call_bridged(
        sender,
        fee_payer,
        budget_wei,
        encode_schedule_job(target_id, task, interval_secs, budget_wei, max_runs),
        fee_token,
        gas,
        bridge_wei,
    )
    .await
}

/// Cancel a scheduled job via a sponsored Tempo tx — REFUNDS the job's full
/// remaining `budgetWei` to the owner (`cancelJob` is owner-gated on-chain).
pub async fn cancel_job_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    job_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    // status flip + budget zero (1 SSTORE) + the refund `transfer` + event.
    sponsored_diamond_call(sender, fee_payer, encode_cancel_job(job_id), fee_token, 400_000).await
}

// --- OFF-CHAIN scheduling (GitHub store via the proxy) ----------------------
//
// NEW scheduled jobs live OFF-CHAIN: the browser/CLI POST `/api/schedule` and the
// proxy keeps the job as a GitHub-store record (no ScheduleFacet escrow, no gas,
// no sponsor — the same off-chain model as the app store / telemetry). A
// `reminder` is a future web-push (zero $LH); an `agent` job runs the target each
// fire, billed per run from the OWNER's meter (same cost as an interactive
// message). The cron worker fires both stores. The on-chain helpers ABOVE stay
// for the browser [⇪ background] goal-escrow + any in-flight legacy jobs. Auth =
// the personal-sign proxy token; the signer is the job OWNER (billing + push).

/// Create an off-chain scheduled job via `POST /api/schedule`. `kind` =
/// `"reminder"` | `"agent"`; `target` is the subdomain to run (agent only —
/// ignored/empty for a reminder). Returns the new job id. `now_secs` is the
/// caller's UNIX time, passed in so this is cross-target (native CLI =
/// `SystemTime`, browser = `js_sys::Date::now()`).
pub async fn create_offchain_job(
    signer: &SigningKey,
    now_secs: u64,
    kind: &str,
    target: &str,
    task: &str,
    interval_secs: u64,
    runs: u32,
) -> Result<String, String> {
    let token = proxy_auth_token(signer, now_secs);
    let url = format!("{CREDIT_PROXY_URL}api/schedule");
    let mut body = serde_json::json!({
        "action": "create",
        "kind": kind,
        "task": task,
        "intervalSecs": interval_secs,
        "runs": runs,
    });
    if !target.is_empty() {
        body["target"] = serde_json::Value::String(target.to_string());
    }
    let resp = http_post_json_authed_returning(&url, &token, &body).await?;
    resp.get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("schedule: unexpected response {resp}"))
}

/// Cancel an off-chain scheduled job by id (owner-gated server-side: the proxy
/// only deletes a job whose stored owner matches the token's signer). `Ok(())`
/// on success; an unknown id / not-yours surfaces the proxy's error.
pub async fn cancel_offchain_job(signer: &SigningKey, now_secs: u64, id: &str) -> Result<(), String> {
    let token = proxy_auth_token(signer, now_secs);
    let url = format!("{CREDIT_PROXY_URL}api/schedule");
    let body = serde_json::json!({ "action": "cancel", "id": id });
    http_post_json_authed_returning(&url, &token, &body).await.map(|_| ())
}

/// List the caller's off-chain scheduled jobs (the `jobs` array). Read-only.
pub async fn list_offchain_jobs(
    signer: &SigningKey,
    now_secs: u64,
) -> Result<Vec<serde_json::Value>, String> {
    let token = proxy_auth_token(signer, now_secs);
    let url = format!("{CREDIT_PROXY_URL}api/schedule");
    let body = serde_json::json!({ "action": "list" });
    let resp = http_post_json_authed_returning(&url, &token, &body).await?;
    Ok(resp
        .get("jobs")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default())
}

/// Read `jobsOf(address)` — every job id the owner has scheduled (Active +
/// terminal). The enumerable index backing the "my jobs" view.
pub async fn jobs_of(owner_hex: &str) -> Result<Vec<u64>, String> {
    let owner = parse_eth_address(owner_hex)?;
    let result = read_view(selector("jobsOf(address)"), &[addr_word(&owner)]).await?;
    let bytes = hex_to_bytes(&result)?;
    // ABI dynamic uint256[]: [offset(32)][len(32)][id0(32)]... — shared decode.
    Ok(decode_u64_array(&bytes))
}

/// Read `getJob(uint256)` → the full [`ScheduledJob`] record. The returned
/// tuple is all-static (the `task` lives in its own mapping, read via
/// [`task_of`]), so it decodes as 7 consecutive ABI words in `Job` order:
/// owner, interval, status, nextRun, budgetWei, runsLeft, targetId.
pub async fn get_job(job_id: u64) -> Result<ScheduledJob, String> {
    let result = read_view(selector("getJob(uint256)"), &[u256_be(job_id as u128)]).await?;
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

/// Read `jobsDue(startAfter, limit) -> (uint256[] ids, uint256 nextCursor)`: the
/// DUE jobs in the index window `[startAfter, startAfter+limit)` across ALL
/// owners, plus the cursor after the window. This is the cross-owner enumeration
/// a decentralized keeper needs — the same view the Vercel scheduler worker pages
/// through. The return is a TUPLE, so it decodes explicitly (NOT via
/// `decode_u64_array`, which assumes the length sits at word 1 — here word 1 is
/// `nextCursor`): word0 = offset to the ids array, word1 = nextCursor, and at
/// that offset `[len][id0]…`.
pub async fn jobs_due(start_after: u64, limit: u64) -> Result<(Vec<u64>, u64), String> {
    let result = read_view(
        selector("jobsDue(uint256,uint256)"),
        &[u256_be(start_after as u128), u256_be(limit as u128)],
    )
    .await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 64 {
        return Err(format!("jobsDue: short response {} bytes", bytes.len()));
    }
    Ok(decode_jobs_due(&bytes))
}

/// Pure decode of a `jobsDue` return — the `(uint256[] ids, uint256 nextCursor)`
/// tuple. Factored out so the offset arithmetic is unit-testable without an RPC:
/// word0 = offset to the ids array, word1 = nextCursor, and `[len][id0]…` at that
/// offset. Bounds-safe (truncates rather than panics on a short/hostile blob).
fn decode_jobs_due(bytes: &[u8]) -> (Vec<u64>, u64) {
    if bytes.len() < 64 {
        return (Vec::new(), 0);
    }
    let next_cursor = u64_low(&bytes[32..64]); // word1 = nextCursor (static)
    let off = u64_low(&bytes[0..32]) as usize; // word0 = offset to the ids array
    let mut ids = Vec::new();
    if bytes.len() >= off + 32 {
        let len = u64_low(&bytes[off..off + 32]) as usize;
        for i in 0..len {
            let s = off + 32 + i * 32;
            let Some(word) = bytes.get(s..s + 32) else { break };
            ids.push(u64_low(word));
        }
    }
    (ids, next_cursor)
}

#[cfg(test)]
mod jobs_due_tests {
    use super::*;

    #[test]
    fn decode_jobs_due_tuple_array_then_cursor() {
        // (uint256[] ids = [5, 9], uint256 nextCursor = 128):
        //   word0 = offset to ids = 0x40 ; word1 = 128 ; at 0x40 → [len=2][5][9]
        let mut b = Vec::new();
        b.extend_from_slice(&u256_be(0x40));
        b.extend_from_slice(&u256_be(128));
        b.extend_from_slice(&u256_be(2));
        b.extend_from_slice(&u256_be(5));
        b.extend_from_slice(&u256_be(9));
        assert_eq!(decode_jobs_due(&b), (vec![5, 9], 128));

        // empty array, nonzero cursor.
        let mut e = Vec::new();
        e.extend_from_slice(&u256_be(0x40));
        e.extend_from_slice(&u256_be(64));
        e.extend_from_slice(&u256_be(0)); // len 0
        assert_eq!(decode_jobs_due(&e), (Vec::new(), 64));

        // short / hostile blobs never panic.
        assert_eq!(decode_jobs_due(&[]), (Vec::new(), 0));
        assert_eq!(decode_jobs_due(&[0u8; 40]), (Vec::new(), 0));
        // length claims 3 ids but only 1 word follows → truncates, no panic.
        let mut t = Vec::new();
        t.extend_from_slice(&u256_be(0x40));
        t.extend_from_slice(&u256_be(0));
        t.extend_from_slice(&u256_be(3));
        t.extend_from_slice(&u256_be(7));
        assert_eq!(decode_jobs_due(&t), (vec![7], 0));
    }
}

/// Collect the FULL cross-owner due set by following [`jobs_due`]'s cursor across
/// pages (bounded at 64 pages of 64 so a huge job index can't spin — mirrors the
/// scheduler worker's scan). The decentralized keeper reads this each tick, then
/// `keeper::jobs_to_fire` decides which of these IDs THIS peer should trigger.
pub async fn all_due_job_ids() -> Result<Vec<u64>, String> {
    let mut all = Vec::new();
    let mut cursor = 0u64;
    for _ in 0..64 {
        let (ids, next) = jobs_due(cursor, 64).await?;
        all.extend(ids);
        if next <= cursor {
            break; // cursor didn't advance → index fully scanned
        }
        cursor = next;
    }
    Ok(all)
}

/// Read `lastRunOf(uint256)` → (unix timestamp, status byte) of the job's most
/// recent `recordRun`, or `(0, 0)` if it has never run (GitHub #52). The view
/// returns two ABI words: the `uint64` timestamp then the `uint8` status enum
/// (each right-aligned). Lets `jobs`/`status` show "last run: <when> [status]"
/// without scraping the `JobRan` event log.
pub async fn last_run_of(job_id: u64) -> Result<(u64, u8), String> {
    let result = read_view(selector("lastRunOf(uint256)"), &[u256_be(job_id as u128)]).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 2 * 32 {
        return Err(format!("lastRunOf: short response {} bytes", bytes.len()));
    }
    let timestamp = u64_low(&bytes[0..32]);
    let status = bytes[2 * 32 - 1]; // low byte of word 1
    Ok((timestamp, status))
}

/// Read `taskOf(uint256)` — the job's task prompt, decoded UTF-8. Stored as
/// on-chain `bytes` (same ABI shape as a `string` return: offset + length +
/// body); we interpret it as UTF-8 since the MVP task is an inline prompt.
pub async fn task_of(job_id: u64) -> Result<String, String> {
    let result = read_view(selector("taskOf(uint256)"), &[u256_be(job_id as u128)]).await?;
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
