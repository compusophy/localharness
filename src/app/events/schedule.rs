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

// The on-chain escrow submit/cancel core (`submit_schedule_job` /
// `cancel_schedule_job` over ScheduleFacet `scheduleJob`/`cancelJob`) was
// REMOVED: scheduling moved OFF-CHAIN. `scheduleJob` cost ~2.88M sponsored gas +
// locked refundable `$LH` per job — absurd for the common case (a one-shot
// reminder). The `schedule_task`/`cancel_task` chat tools now POST the proxy's
// `/api/schedule` (GitHub store, no gas/escrow) via
// `registry::{create_offchain_job, cancel_offchain_job}`. Only
// `parse_schedule_interval` (the shared cadence parser) lives on here.
