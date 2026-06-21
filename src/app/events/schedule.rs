//! Scheduled jobs — escrow-backed recurring agent runs (ScheduleFacet).
//!
//! The admin schedule UI was removed; what remains is the escrow core the
//! `schedule_task` chat tool reuses: parse a cadence + submit a `scheduleJob`.

/// ScheduleFacet's on-chain minimum cadence (mirrors the CLI's
/// `SCHEDULE_MIN_INTERVAL_SECS`). The facet rejects anything faster.
const SCHEDULE_MIN_INTERVAL_SECS: u64 = 60;

/// Parse a human cadence (`60s` / `5m` / `1h`, bare number = seconds) into
/// seconds, enforcing the 60s floor. Mirrors the CLI's `parse_interval`
/// EXACTLY so the browser + CLI accept the same strings. `None` on garbage
/// or sub-minimum (handled by a silent no-op — no explanatory-validation).
pub(crate) fn parse_schedule_interval(raw: &str) -> Option<u64> {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    let (num_part, mult) = match s.strip_suffix('s') {
        Some(n) => (n, 1u64),
        None => match s.strip_suffix('m') {
            Some(n) => (n, 60u64),
            None => match s.strip_suffix('h') {
                Some(n) => (n, 3600u64),
                None => (s.as_str(), 1u64), // bare number = seconds
            },
        },
    };
    let secs = num_part.parse::<u64>().ok()?.checked_mul(mult)?;
    (secs >= SCHEDULE_MIN_INTERVAL_SECS).then_some(secs)
}

/// The ONE escrow-backed `scheduleJob` submission core, reused by the
/// `schedule_task` chat tool: sponsor rate guard → resolve the target name →
/// credit signer + embedded fee payer → sponsored approve+`scheduleJob` tx →
/// refresh the credits pill → read the new job id back from `jobsOf(caller)`
/// (its last entry; 0 if unreadable). The budget is pulled from the caller's
/// WALLET `$LH` by `transferFrom`; a wallet shortfall covered by unspent
/// chat-METER credits rides as a `withdrawCredits` call in the SAME atomic tx
/// (the escrow auto-bridge — on-chain feedback #63), so "has metered credits
/// but the escrow fails" can only mean BOTH pots together are short.
pub(crate) async fn submit_schedule_job(
    target: &str,
    task: &str,
    interval_secs: u64,
    budget_wei: u128,
    max_runs: u32,
) -> Result<u64, String> {
    super::sponsor_rate_guard()?;
    let target_id = crate::app::registry::id_of_name(target).await?;
    if target_id == 0 {
        return Err("target agent not found".to_string());
    }
    let (signer, addr) = crate::app::chat::credit_signer()
        .await
        .ok_or_else(|| "no identity".to_string())?;
    let from_hex = crate::encoding::bytes_to_hex_str(&addr);
    let bridge_wei = crate::app::chat::escrow_bridge_wei(&from_hex, budget_wei).await?;
    let fee_payer = crate::app::sponsor::signer()?;
    crate::app::registry::schedule_job_sponsored_bridged(
        &signer,
        &fee_payer,
        target_id,
        task.as_bytes(),
        interval_secs,
        budget_wei,
        max_runs,
        crate::app::registry::ALPHA_USD_ADDRESS(),
        bridge_wei,
    )
    .await?;
    // The escrow left the funder's spendable balance — reflect it.
    super::refresh_credits_pill().await;
    // New job id = the last entry in jobsOf(caller). Read it back so the
    // caller's confirmation surface reflects the freshly-mined job.
    let new_id = match crate::app::chat::credit_address_existing().await {
        Some(addr) => crate::app::registry::jobs_of(&addr)
            .await
            .ok()
            .and_then(|ids| ids.last().copied())
            .unwrap_or(0),
        None => 0,
    };
    Ok(new_id)
}

/// Cancel a scheduled job by id — sponsored `cancelJob`, which REFUNDS the
/// job's remaining escrow to the owner's WALLET. The in-chat twin of the CLI
/// `unschedule`, reused by the `cancel_task` chat tool: mirrors
/// [`submit_schedule_job`]'s sponsor-guard → credit-signer → embedded
/// fee-payer → sponsored tx → pill-refresh shape. `cancelJob` is owner-gated
/// on-chain, so cancelling a job this identity doesn't own (or an unknown id)
/// reverts — no client-side ownership pre-check needed. Returns the tx hash.
pub(crate) async fn cancel_schedule_job(job_id: u64) -> Result<String, String> {
    super::sponsor_rate_guard()?;
    let (signer, _addr) = crate::app::chat::credit_signer()
        .await
        .ok_or_else(|| "no identity".to_string())?;
    let fee_payer = crate::app::sponsor::signer()?;
    let tx = crate::app::registry::cancel_job_sponsored(
        &signer,
        &fee_payer,
        job_id,
        crate::app::registry::ALPHA_USD_ADDRESS(),
    )
    .await?;
    // The refund landed back in the wallet pot — reflect it in the credits pill.
    super::refresh_credits_pill().await;
    Ok(tx)
}
