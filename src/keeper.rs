//! Pure decision core for a decentralized scheduler keeper (krafto #1.5): given
//! the on-chain ScheduleFacet jobs + this peer's roster position, decide which due
//! jobs to fire this tick — herd-free (one primary per job) with backup liveness.
//! No chain/P2P deps; the wiring is a thin shell (`localharness keeper` + the
//! proxy `?poke`).

/// Job fields a keeper needs (mirrors `registry::ScheduledJob`; status 0 Active /
/// 1 Paused / 2 Cancelled / 3 Exhausted; `next_run` unix secs, 0 = terminal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeeperJob {
    pub id: u64,
    pub status: u8,
    pub next_run: u64,
    pub budget_wei: u128,
    pub runs_left: u32,
}

impl KeeperJob {
    /// The on-chain due-condition (Active, due, funded, runs left), so a keeper
    /// never wastes a tx the facet would revert.
    pub fn is_fireable(&self, now: u64, per_run_wei: u128) -> bool {
        self.status == 0
            && self.next_run != 0
            && self.next_run <= now
            && self.runs_left > 0
            && self.budget_wei >= per_run_wei
    }

    /// Seconds overdue at `now` (0 if not due/terminal); drives backup backoff.
    pub fn overdue_by(&self, now: u64) -> u64 {
        if self.next_run == 0 || self.next_run > now { 0 } else { now - self.next_run }
    }
}

/// Deterministic FNV-1a of `(job_id, salt)` — identical on every peer, so keeper
/// assignment needs no gossip.
fn fnv1a(job_id: u64, salt: u64) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in job_id.to_le_bytes().iter().chain(salt.to_le_bytes().iter()) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The primary keeper for `job_id` this `epoch` over `keeper_count` peers (fires
/// it at due-time). `epoch` rotates ownership; 0 for an empty roster.
pub fn primary_keeper(job_id: u64, epoch: u64, keeper_count: u32) -> u32 {
    if keeper_count == 0 {
        return 0;
    }
    (fnv1a(job_id, epoch) % keeper_count as u64) as u32
}

/// Should THIS peer fire `job` at `now`? Primary fires immediately; each backup
/// waits `rank * backoff_secs` overdue (rank = ring distance from the primary),
/// so the steady state is herd-free yet a dead primary can't strand a job. Solo
/// (`keeper_count <= 1`) fires everything fireable.
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
    let rank = (my_index + keeper_count - primary) % keeper_count;
    job.overdue_by(now) >= (rank as u64).saturating_mul(backoff_secs)
}

/// The job ids this peer should fire this tick (filter over `should_fire`).
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

/// A keeper presence entry from SignalingFacet (addr + unix expiry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RosterEntry {
    pub addr: [u8; 20],
    pub expiry: u64,
}

/// This peer's `(index, count)` in the roster: live peers (expiry > now), deduped
/// and sorted, so every peer agrees on `primary_keeper` with no gossip. None if
/// `me` isn't a live keeper.
pub fn roster_position(entries: &[RosterEntry], now: u64, me: &[u8; 20]) -> Option<(u32, u32)> {
    let mut live: Vec<[u8; 20]> =
        entries.iter().filter(|e| e.expiry > now).map(|e| e.addr).collect();
    live.sort_unstable();
    live.dedup();
    let idx = live.iter().position(|a| a == me)? as u32;
    Some((idx, live.len() as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active(id: u64, next_run: u64) -> KeeperJob {
        KeeperJob { id, status: 0, next_run, budget_wei: 1_000, runs_left: 5 }
    }

    #[test]
    fn fireable_requires_active_due_funded_with_runs() {
        assert!(active(1, 100).is_fireable(100, 100)); // due now
        assert!(active(1, 50).is_fireable(100, 100)); // overdue
        assert!(!active(1, 101).is_fireable(100, 100)); // not due
        assert!(!active(1, 0).is_fireable(100, 100)); // terminal
        for status in [1u8, 2, 3] {
            assert!(!KeeperJob { status, ..active(1, 50) }.is_fireable(100, 100));
        }
        assert!(!KeeperJob { runs_left: 0, ..active(1, 50) }.is_fireable(100, 100));
        assert!(!KeeperJob { budget_wei: 99, ..active(1, 50) }.is_fireable(100, 100));
    }

    #[test]
    fn primary_is_deterministic_in_range_and_rotates() {
        for id in 0..50u64 {
            for epoch in 0..5u64 {
                assert!(primary_keeper(id, epoch, 4) < 4);
            }
        }
        assert!((0..20u64).any(|id| primary_keeper(id, 0, 4) != primary_keeper(id, 1, 4)));
    }

    #[test]
    fn solo_keeper_fires_every_due_job() {
        assert!(should_fire(&active(7, 100), 100, 100, 0, 1, 0, 30));
        assert!(should_fire(&active(7, 100), 100, 100, 0, 0, 0, 30)); // empty roster = solo
        assert!(!should_fire(&active(7, 200), 100, 100, 0, 1, 0, 30)); // not due
    }

    #[test]
    fn no_herd_at_due_time_then_backup_liveness() {
        let (count, backoff) = (3u32, 30u64);
        let job = active(42, 100);
        let primary = primary_keeper(job.id, 0, count);
        // Due-time: only the primary fires.
        let now: Vec<u32> = (0..count).filter(|&i| should_fire(&job, 100, 100, i, count, 0, backoff)).collect();
        assert_eq!(now, vec![primary]);
        // +backoff overdue: the rank-1 backup joins.
        assert!(should_fire(&job, 130, 100, (primary + 1) % count, count, 0, backoff));
        // Past (count-1)*backoff: every peer fires, so an offline set can't strand it.
        for i in 0..count {
            assert!(should_fire(&job, 100 + (count as u64) * backoff, 100, i, count, 0, backoff));
        }
    }

    #[test]
    fn jobs_to_fire_covers_due_set_once() {
        let jobs = vec![
            active(1, 900),
            active(2, 2_000),                          // future
            KeeperJob { status: 2, ..active(3, 900) }, // cancelled
            active(4, 1_000),
        ];
        let mut solo = jobs_to_fire(&jobs, 1_000, 100, 0, 1, 0, 30);
        solo.sort_unstable();
        assert_eq!(solo, vec![1, 4]);
        // Across a 3-keeper roster: union = the due set, no duplicates.
        let mut union: Vec<u64> =
            (0..3u32).flat_map(|i| jobs_to_fire(&jobs, 1_000, 100, i, 3, 0, 30)).collect();
        union.sort_unstable();
        union.dedup();
        assert_eq!(union, vec![1, 4]);
    }

    #[test]
    fn roster_is_live_sorted_deterministic() {
        let (a, b, c, d) = ([1u8; 20], [2u8; 20], [3u8; 20], [4u8; 20]);
        let entries = vec![
            RosterEntry { addr: c, expiry: 200 },
            RosterEntry { addr: a, expiry: 200 },
            RosterEntry { addr: d, expiry: 50 }, // expired
            RosterEntry { addr: b, expiry: 200 },
        ];
        assert_eq!(roster_position(&entries, 100, &a), Some((0, 3)));
        assert_eq!(roster_position(&entries, 100, &c), Some((2, 3)));
        assert_eq!(roster_position(&entries, 100, &d), None); // expired
        assert_eq!(roster_position(&entries, 100, &[9u8; 20]), None); // stranger
    }

    #[test]
    fn roster_dedups_and_drops_expired() {
        let (a, b) = ([1u8; 20], [2u8; 20]);
        let entries = vec![
            RosterEntry { addr: a, expiry: 200 },
            RosterEntry { addr: a, expiry: 300 }, // dup
            RosterEntry { addr: b, expiry: 200 },
            RosterEntry { addr: [7u8; 20], expiry: 1 }, // expired
        ];
        assert_eq!(roster_position(&entries, 100, &a), Some((0, 2)));
        assert_eq!(roster_position(&entries, 100, &b), Some((1, 2)));
        assert_eq!(roster_position(&[], 100, &a), None);
    }

    #[test]
    fn roster_yields_exactly_one_primary() {
        let (a, b, c) = ([10u8; 20], [20u8; 20], [30u8; 20]);
        let entries = vec![
            RosterEntry { addr: b, expiry: 9 },
            RosterEntry { addr: a, expiry: 9 },
            RosterEntry { addr: c, expiry: 9 },
        ];
        let (ia, n) = roster_position(&entries, 0, &a).unwrap();
        let (ib, _) = roster_position(&entries, 0, &b).unwrap();
        let (ic, _) = roster_position(&entries, 0, &c).unwrap();
        assert_eq!((ia, ib, ic, n), (0, 1, 2, 3));
        let primary = primary_keeper(7, 0, n);
        assert_eq!([ia, ib, ic].iter().filter(|&&i| i == primary).count(), 1);
    }
}
