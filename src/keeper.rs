//! keeper.rs — the PURE decision core for a decentralized scheduler keeper.
//!
//! krafto on-chain feedback #1.5: "Scheduled jobs depend on a centralized Vercel
//! cron worker. A peer-to-peer keeper network to trigger due jobs would achieve
//! true autonomy." This module is the brain of that keeper: given the on-chain
//! [`ScheduleFacet`](crate::registry) jobs, the current time, and which keeper
//! *this* peer is in the roster, it decides **which due jobs this peer should
//! fire this tick** — fairly (no thundering herd) and with liveness (a dead
//! peer's jobs still get fired by a backup).
//!
//! It is pure control-flow + integer math, native-tested, with ZERO chain / P2P
//! dependency — exactly the project's pattern ([`crate::compose`],
//! [`crate::kv_reduce`]). The keeper *wiring* (deferred) is a thin shell:
//!   1. discover the live keeper roster over `SignalingFacet` presence (sorted by
//!      address → a consistent `my_index` / `keeper_count` on every peer),
//!   2. read the jobs (`registry::list_*`) + `now`,
//!   3. call [`jobs_to_fire`], and submit a trigger tx for each.
//! It also needs an on-chain change letting a *keeper* (not only the single
//! scheduler-role key) trigger `recordRun` — both are flagged, not done here.

/// The fields of a `ScheduleFacet` job a keeper needs to decide firing. Mirrors
/// [`crate::registry::ScheduledJob`] (status: 0 Active / 1 Paused / 2 Cancelled
/// / 3 Exhausted; `next_run` is unix seconds, 0 once terminal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeeperJob {
    /// On-chain job id (the dedup/assignment key).
    pub id: u64,
    /// Raw lifecycle byte: 0 Active, 1 Paused, 2 Cancelled, 3 Exhausted.
    pub status: u8,
    /// Unix seconds of the next due fire; 0 once terminal.
    pub next_run: u64,
    /// `$LH` (wei) still escrowed — a run can't fire without covering its cost.
    pub budget_wei: u128,
    /// Remaining runs (the hard count cap).
    pub runs_left: u32,
}

impl KeeperJob {
    /// Is this job FIREABLE at `now` for a run that needs at least `per_run_wei`
    /// of budget? Mirrors the on-chain due-condition so a keeper never wastes a
    /// trigger tx the facet would just revert: Active, not terminal, due now,
    /// runs remaining, and funded.
    pub fn is_fireable(&self, now: u64, per_run_wei: u128) -> bool {
        self.status == 0
            && self.next_run != 0
            && self.next_run <= now
            && self.runs_left > 0
            && self.budget_wei >= per_run_wei
    }

    /// Seconds this job is overdue at `now` (0 when not yet due / terminal).
    /// Drives the backup keepers' staggered backoff.
    pub fn overdue_by(&self, now: u64) -> u64 {
        if self.next_run == 0 || self.next_run > now {
            0
        } else {
            now - self.next_run
        }
    }
}

/// Deterministic 64-bit hash (FNV-1a) of `(job_id, salt)` — the same on every
/// peer, so the network agrees on assignment with NO gossip. No deps.
fn fnv1a(job_id: u64, salt: u64) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in job_id.to_le_bytes().iter().chain(salt.to_le_bytes().iter()) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The PRIMARY keeper index responsible for `job_id` during `epoch`, over a
/// roster of `keeper_count` peers. Deterministic — every peer computes the same
/// owner, so exactly one fires the job when it comes due (no thundering herd).
/// `epoch` rotates the owner over time so responsibility spreads and a churning
/// roster re-balances. Returns 0 for an empty roster (degenerate; solo path).
pub fn primary_keeper(job_id: u64, epoch: u64, keeper_count: u32) -> u32 {
    if keeper_count == 0 {
        return 0;
    }
    (fnv1a(job_id, epoch) % keeper_count as u64) as u32
}

/// Should THIS keeper (`my_index` of `keeper_count`, sorted-roster position) fire
/// `job` at `now`? Tiered so the common case is herd-free but a dead primary
/// can't strand a job:
///   1. not fireable → never.
///   2. solo keeper (`keeper_count <= 1`) → fire every fireable job.
///   3. the PRIMARY fires the instant it's due.
///   4. a BACKUP fires only once the job is overdue past `rank * backoff_secs`,
///      where `rank` is this peer's ring-distance from the primary (1, 2, …) —
///      so backups engage one at a time, deterministically, and only if the
///      primary (and nearer backups) failed to fire.
pub fn should_fire(
    job: &KeeperJob,
    now: u64,
    per_run_wei: u128,
    my_index: u32,
    keeper_count: u32,
    epoch: u64,
    backoff_secs: u64,
) -> bool {
    if !job.is_fireable(now, per_run_wei) {
        return false;
    }
    if keeper_count <= 1 {
        return true;
    }
    let primary = primary_keeper(job.id, epoch, keeper_count);
    if my_index == primary {
        return true;
    }
    // Ring distance from the primary, in 1..keeper_count.
    let rank = (my_index + keeper_count - primary) % keeper_count;
    job.overdue_by(now) >= (rank as u64).saturating_mul(backoff_secs)
}

/// The ids of the jobs THIS keeper should fire this tick — a pure filter over
/// [`should_fire`]; the wiring submits one trigger tx per returned id.
pub fn jobs_to_fire(
    jobs: &[KeeperJob],
    now: u64,
    per_run_wei: u128,
    my_index: u32,
    keeper_count: u32,
    epoch: u64,
    backoff_secs: u64,
) -> Vec<u64> {
    jobs.iter()
        .filter(|j| should_fire(j, now, per_run_wei, my_index, keeper_count, epoch, backoff_secs))
        .map(|j| j.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active(id: u64, next_run: u64) -> KeeperJob {
        KeeperJob { id, status: 0, next_run, budget_wei: 1_000, runs_left: 5 }
    }

    #[test]
    fn fireable_only_when_active_due_funded_and_with_runs() {
        let now = 100;
        assert!(active(1, 100).is_fireable(now, 100)); // due exactly now
        assert!(active(1, 50).is_fireable(now, 100)); // overdue
        assert!(!active(1, 101).is_fireable(now, 100)); // not yet due
        assert!(!active(1, 0).is_fireable(now, 100)); // terminal (next_run 0)
        // paused / cancelled / exhausted are never fireable.
        for status in [1u8, 2, 3] {
            let j = KeeperJob { status, ..active(1, 50) };
            assert!(!j.is_fireable(now, 100));
        }
        // out of runs / underfunded.
        assert!(!KeeperJob { runs_left: 0, ..active(1, 50) }.is_fireable(now, 100));
        assert!(!KeeperJob { budget_wei: 99, ..active(1, 50) }.is_fireable(now, 100));
    }

    #[test]
    fn primary_is_deterministic_and_in_range() {
        for id in 0..50u64 {
            for epoch in 0..5u64 {
                let p = primary_keeper(id, epoch, 4);
                assert!(p < 4);
                assert_eq!(p, primary_keeper(id, epoch, 4), "deterministic");
            }
        }
        // epoch rotates ownership for at least some jobs (not a fixed owner).
        let rotated = (0..20u64).any(|id| primary_keeper(id, 0, 4) != primary_keeper(id, 1, 4));
        assert!(rotated, "epoch must rotate primary ownership");
    }

    #[test]
    fn solo_keeper_fires_every_due_job() {
        let now = 100;
        let due = active(7, 100);
        assert!(should_fire(&due, now, 100, 0, 1, 0, 30));
        assert!(should_fire(&due, now, 100, 0, 0, 0, 30)); // empty roster ⇒ solo
        let not_due = active(7, 200);
        assert!(!should_fire(&not_due, now, 100, 0, 1, 0, 30));
    }

    #[test]
    fn no_thundering_herd_then_liveness_via_backoff() {
        // 3 keepers; a job due exactly now. Only the primary fires immediately;
        // each backup waits rank*backoff before stepping in.
        let count = 3u32;
        let epoch = 0;
        let backoff = 30;
        let job = active(42, 100);
        let primary = primary_keeper(job.id, epoch, count);

        // At due-time (overdue 0): exactly ONE keeper (the primary) fires.
        let firing_now: Vec<u32> = (0..count)
            .filter(|&i| should_fire(&job, 100, 100, i, count, epoch, backoff))
            .collect();
        assert_eq!(firing_now, vec![primary], "only the primary fires at due-time");

        // Primary offline + 30s overdue ⇒ the rank-1 backup now also fires.
        let firing_30: Vec<u32> = (0..count)
            .filter(|&i| should_fire(&job, 130, 100, i, count, epoch, backoff))
            .collect();
        assert!(firing_30.contains(&primary));
        let rank1 = (primary + 1) % count;
        assert!(firing_30.contains(&rank1), "rank-1 backup engages at 1*backoff overdue");

        // LIVENESS: once overdue past (count-1)*backoff, EVERY keeper would fire,
        // so the job can never be stranded by offline peers.
        for i in 0..count {
            assert!(should_fire(&job, 100 + (count as u64) * backoff, 100, i, count, epoch, backoff));
        }
    }

    #[test]
    fn jobs_to_fire_filters_to_this_keepers_due_set() {
        let now = 1_000;
        let jobs = vec![
            active(1, 900),                                    // due
            active(2, 2_000),                                  // future
            KeeperJob { status: 2, ..active(3, 900) },         // cancelled
            active(4, 1_000),                                  // due now
        ];
        // A solo keeper fires exactly the fireable ones (ids 1 and 4).
        let mut got = jobs_to_fire(&jobs, now, 100, 0, 1, 0, 30);
        got.sort_unstable();
        assert_eq!(got, vec![1, 4]);

        // Across a 3-keeper roster at due-time, every due job is fired by exactly
        // one keeper (union = all due, no duplicates) — herd-free coverage.
        let count = 3u32;
        let mut union: Vec<u64> = (0..count)
            .flat_map(|i| jobs_to_fire(&jobs, now, 100, i, count, 0, 30))
            .collect();
        union.sort_unstable();
        union.dedup();
        assert_eq!(union, vec![1, 4], "every due job covered exactly once at due-time");
    }
}
