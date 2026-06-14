//! Last-Writer-Wins key/value CRDT for SessionRoom shared state (GitHub #22).
//!
//! Pure — no I/O, no chain, no crypto. A SessionRoom is an append-only on-chain
//! log of *encrypted* ops ([`crate::kv_room`] seals/opens them); this module is
//! the deterministic fold that turns the decrypted op set into a converged map.
//! Because the merge is a total order, every replica that has seen the same set
//! of ops — in any order, with any duplicates — computes the SAME map. That
//! convergence is the load-bearing correctness property and is what the tests
//! below pin.

use std::collections::BTreeMap;

/// A single key/value write (or delete) appended to a room.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KvOp {
    /// The key being written.
    pub key: String,
    /// The value, or `None` for a tombstone (delete).
    pub value: Option<Vec<u8>>,
    /// Logical clock — higher wins. A writer stamps `next_lamport(seen)`.
    pub lamport: u64,
    /// Writer address (20 bytes) — the deterministic tiebreak when two ops for
    /// the same key share a `lamport`.
    pub writer: [u8; 20],
    /// Wall-clock seconds when written. Used ONLY for optional TTL filtering,
    /// never for ordering (clocks are not trustworthy across writers).
    pub ts: u64,
}

/// Does `candidate` beat `current` for the same key? Higher `lamport` wins; on a
/// tie the lexicographically greater `writer` wins. This is a strict total order
/// over `(lamport, writer)`, so the fold is order-independent and symmetric —
/// the same tiebreak discipline as `sharedfs_reconcile`'s hash tiebreak.
fn op_wins(candidate: &KvOp, current: &KvOp) -> bool {
    (candidate.lamport, candidate.writer) > (current.lamport, current.writer)
}

/// Fold `ops` into the converged map. A tombstone that wins suppresses its key.
///
/// `ttl_secs == 0` disables TTL filtering; otherwise an op is ignored once
/// `op.ts + ttl_secs <= now` (ephemeral rooms age their state out). TTL is
/// applied BEFORE the LWW merge, so an expired winner does not mask a still-live
/// older write for the same key.
pub fn reduce(ops: &[KvOp], now: u64, ttl_secs: u64) -> BTreeMap<String, Vec<u8>> {
    let mut winners: BTreeMap<&str, &KvOp> = BTreeMap::new();
    for op in ops {
        if ttl_secs != 0 && op.ts.saturating_add(ttl_secs) <= now {
            continue;
        }
        match winners.get(op.key.as_str()) {
            Some(current) if !op_wins(op, current) => {}
            _ => {
                winners.insert(op.key.as_str(), op);
            }
        }
    }
    winners
        .into_iter()
        .filter_map(|(k, op)| op.value.as_ref().map(|v| (k.to_string(), v.clone())))
        .collect()
}

/// The lamport a writer should stamp on its next op, given every op it has
/// observed: `max(lamport) + 1`, or `0` for an empty log.
pub fn next_lamport(ops: &[KvOp]) -> u64 {
    ops.iter().map(|o| o.lamport).max().map_or(0, |m| m + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(key: &str, val: Option<&[u8]>, lamport: u64, writer: u8, ts: u64) -> KvOp {
        KvOp {
            key: key.to_string(),
            value: val.map(|v| v.to_vec()),
            lamport,
            writer: [writer; 20],
            ts,
        }
    }

    fn map_of(pairs: &[(&str, &[u8])]) -> BTreeMap<String, Vec<u8>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_vec()))
            .collect()
    }

    #[test]
    fn empty_log_is_empty_map() {
        assert!(reduce(&[], 0, 0).is_empty());
        assert_eq!(next_lamport(&[]), 0);
    }

    #[test]
    fn higher_lamport_wins() {
        let ops = [
            op("a", Some(b"old"), 1, 1, 100),
            op("a", Some(b"new"), 2, 1, 100),
        ];
        assert_eq!(reduce(&ops, 0, 0), map_of(&[("a", b"new")]));
    }

    #[test]
    fn writer_tiebreak_on_equal_lamport() {
        // Same lamport — the greater writer address wins, regardless of order.
        let lo = op("a", Some(b"lo"), 5, 1, 100);
        let hi = op("a", Some(b"hi"), 5, 9, 100);
        assert_eq!(reduce(&[lo.clone(), hi.clone()], 0, 0), map_of(&[("a", b"hi")]));
        assert_eq!(reduce(&[hi, lo], 0, 0), map_of(&[("a", b"hi")]));
    }

    #[test]
    fn tombstone_wins_then_loses_by_clock() {
        // A newer tombstone deletes the key.
        let del = [
            op("a", Some(b"v"), 1, 1, 100),
            op("a", None, 2, 1, 100),
        ];
        assert!(reduce(&del, 0, 0).is_empty());
        // A newer write resurrects it.
        let revive = [
            op("a", None, 2, 1, 100),
            op("a", Some(b"v2"), 3, 1, 100),
        ];
        assert_eq!(reduce(&revive, 0, 0), map_of(&[("a", b"v2")]));
    }

    #[test]
    fn converges_under_any_permutation() {
        let base = [
            op("x", Some(b"1"), 1, 1, 10),
            op("y", Some(b"2"), 1, 2, 10),
            op("x", Some(b"3"), 3, 2, 20),
            op("y", None, 2, 1, 30),
            op("z", Some(b"9"), 7, 3, 40),
            op("x", Some(b"3"), 3, 2, 20), // exact duplicate (idempotence)
        ];
        let expected = reduce(&base, 0, 0);
        // Every rotation must produce the identical map.
        for shift in 0..base.len() {
            let mut perm = base.to_vec();
            perm.rotate_left(shift);
            assert_eq!(reduce(&perm, 0, 0), expected, "rotation {shift} diverged");
        }
        // y was tombstoned (lamport 2) after its only write (lamport 1) → gone.
        assert_eq!(expected, map_of(&[("x", b"3"), ("z", b"9")]));
    }

    #[test]
    fn idempotent_apply_twice_equals_once() {
        let ops = [
            op("a", Some(b"1"), 1, 1, 10),
            op("b", Some(b"2"), 2, 2, 10),
        ];
        let once = reduce(&ops, 0, 0);
        let doubled: Vec<KvOp> = ops.iter().chain(ops.iter()).cloned().collect();
        assert_eq!(reduce(&doubled, 0, 0), once);
    }

    #[test]
    fn ttl_filters_expired_ops() {
        let ops = [
            op("a", Some(b"stale"), 1, 1, 100),
            op("b", Some(b"fresh"), 1, 1, 900),
        ];
        // now=1000, ttl=300 → 'a' expired (100+300<=1000), 'b' kept (900+300>1000).
        assert_eq!(reduce(&ops, 1000, 300), map_of(&[("b", b"fresh")]));
        // ttl=0 disables the filter — both survive.
        assert_eq!(
            reduce(&ops, 1000, 0),
            map_of(&[("a", b"stale"), ("b", b"fresh")])
        );
    }

    #[test]
    fn ttl_expired_winner_does_not_mask_live_older_write() {
        // An expired high-lamport write must not delete a still-live lower one.
        let ops = [
            op("a", Some(b"live"), 1, 1, 950),
            op("a", Some(b"expired"), 5, 1, 100),
        ];
        assert_eq!(reduce(&ops, 1000, 300), map_of(&[("a", b"live")]));
    }

    #[test]
    fn next_lamport_is_max_plus_one() {
        let ops = [op("a", Some(b"v"), 4, 1, 0), op("b", Some(b"v"), 9, 1, 0)];
        assert_eq!(next_lamport(&ops), 10);
    }
}
