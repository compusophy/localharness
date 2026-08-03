//! Pure chunk-partition + result-fold core for the sponsor relay's per-tx
//! call cap (native-testable, the `turn_flow` hoisting pattern; telemetry
//! #85/#88, `design/relay-allowlist-gaps.md` #2).
//!
//! The mainnet sponsor relay refuses a sponsored tx with more than
//! [`RELAY_MAX_CALLS_PER_TX`] calls (`proxy/api/sponsor.ts`:
//! `body.calls.length > 8`). The batch tools used to hard-cap N client-side;
//! this core instead PARTITIONS N items into sequential chunks (one sponsored
//! tx each, ≤8 calls) and FOLDS the per-chunk outcomes into an honest
//! aggregate: which items landed, which rode a failed chunk, which were never
//! attempted. An aux call that must ride EVERY chunk's tx (the paid-claim
//! `approve`, the meter bridge) reserves one slot per chunk — the same rule
//! the old `- 1` caps encoded. The cap is mirrored cross-language — keep it
//! in sync with the relay.

use std::ops::Range;

/// The relay's hard per-tx call cap (`proxy/api/sponsor.ts`).
pub const RELAY_MAX_CALLS_PER_TX: usize = 8;

/// Items a single chunk may carry: the relay cap, minus one when an aux call
/// (paid-claim `approve` / meter bridge) rides in every chunk's tx.
pub fn chunk_capacity(reserve_aux_slot: bool) -> usize {
    RELAY_MAX_CALLS_PER_TX - usize::from(reserve_aux_slot)
}

/// Partition `n` one-call items into in-order chunks of at most
/// [`chunk_capacity`] items each. `n == 0` → no chunks.
pub fn chunk_ranges(n: usize, reserve_aux_slot: bool) -> Vec<Range<usize>> {
    let cap = chunk_capacity(reserve_aux_slot);
    let mut out = Vec::with_capacity(n.div_ceil(cap.max(1)));
    let mut start = 0;
    while start < n {
        let end = (start + cap).min(n);
        out.push(start..end);
        start = end;
    }
    out
}

/// Weighted variant for items contributing MORE than one call each (a
/// found_company role setup: persona = 1 call, a prefund adds createTBA +
/// transfer = 2 more). Items keep their order and are NEVER split across
/// chunks — a prefund's calls must land atomically with its role. A chunk
/// closes when the next item would push its call count past the capacity. An
/// item whose own weight exceeds the capacity still gets a chunk of its own
/// (the relay rejects that tx honestly; this core never silently drops work).
pub fn chunk_ranges_weighted(weights: &[usize], reserve_aux_slot: bool) -> Vec<Range<usize>> {
    let cap = chunk_capacity(reserve_aux_slot);
    let mut out = Vec::new();
    let mut start = 0;
    let mut load = 0usize;
    for (i, &w) in weights.iter().enumerate() {
        if i > start && load + w > cap {
            out.push(start..i);
            start = i;
            load = 0;
        }
        load += w;
    }
    if start < weights.len() {
        out.push(start..weights.len());
    }
    out
}

/// Outcome of ONE chunk's sponsored tx, fed to [`fold_outcomes`] in
/// submission order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkOutcome {
    /// The chunk's tx landed. The tx hash may be EMPTY for a vacuous chunk
    /// (every item pre-filtered — e.g. all names taken — so nothing was
    /// submitted); the fold keeps such items "landed" (they were honestly
    /// handled) but omits the empty hash.
    Landed(String),
    /// The chunk's tx failed as ONE unit — none of its items landed.
    Failed(String),
}

/// The honest aggregate of a chunked batch run.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BatchFold {
    /// Item indices whose chunk landed, in order.
    pub landed: Vec<usize>,
    /// Item indices whose chunk's tx failed, in order.
    pub failed: Vec<usize>,
    /// Item indices whose chunk was never attempted (the caller stopped early).
    pub unattempted: Vec<usize>,
    /// Tx hash per landed (non-vacuous) chunk, in submission order.
    pub tx_hashes: Vec<String>,
    /// `(chunk index, error)` per failed chunk, in submission order.
    pub chunk_errors: Vec<(usize, String)>,
}

/// Fold per-chunk outcomes over the partition. `outcomes` may be SHORTER than
/// `ranges` (an early stop); the trailing chunks' items report `unattempted`.
pub fn fold_outcomes(ranges: &[Range<usize>], outcomes: &[ChunkOutcome]) -> BatchFold {
    let mut fold = BatchFold::default();
    for (i, r) in ranges.iter().enumerate() {
        match outcomes.get(i) {
            Some(ChunkOutcome::Landed(tx)) => {
                fold.landed.extend(r.clone());
                if !tx.is_empty() {
                    fold.tx_hashes.push(tx.clone());
                }
            }
            Some(ChunkOutcome::Failed(e)) => {
                fold.failed.extend(r.clone());
                fold.chunk_errors.push((i, e.clone()));
            }
            None => fold.unattempted.extend(r.clone()),
        }
    }
    fold
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(v: &[(usize, usize)]) -> Vec<Range<usize>> {
        v.iter().map(|&(a, b)| a..b).collect()
    }

    #[test]
    fn partitions_hit_exact_boundaries() {
        // (n, reserved-aux-slot, expected)
        let cases: &[(usize, bool, &[(usize, usize)])] = &[
            (0, false, &[]),
            (7, false, &[(0, 7)]),
            (8, false, &[(0, 8)]),
            (9, false, &[(0, 8), (8, 9)]),
            (15, false, &[(0, 8), (8, 15)]),
            (16, false, &[(0, 8), (8, 16)]),
            (17, false, &[(0, 8), (8, 16), (16, 17)]),
            (0, true, &[]),
            (7, true, &[(0, 7)]),
            (8, true, &[(0, 7), (7, 8)]),
            (9, true, &[(0, 7), (7, 9)]),
            (14, true, &[(0, 7), (7, 14)]),
            (15, true, &[(0, 7), (7, 14), (14, 15)]),
            (16, true, &[(0, 7), (7, 14), (14, 16)]),
        ];
        for &(n, reserved, want) in cases {
            assert_eq!(chunk_ranges(n, reserved), ranges(want), "n={n} reserved={reserved}");
        }
    }

    #[test]
    fn weighted_never_splits_an_item() {
        // found_company shapes: persona-only = 1, persona + prefund = 3.
        assert_eq!(chunk_ranges_weighted(&[], false), ranges(&[]));
        assert_eq!(chunk_ranges_weighted(&[1; 7], false), ranges(&[(0, 7)]));
        assert_eq!(chunk_ranges_weighted(&[3, 3, 3], false), ranges(&[(0, 2), (2, 3)]));
        assert_eq!(chunk_ranges_weighted(&[1, 2, 3, 1, 3], false), ranges(&[(0, 4), (4, 5)]));
        // the DEFAULT 7 roles + prefund_each = 21 calls (the #85 overrun).
        assert_eq!(
            chunk_ranges_weighted(&[3; 7], false),
            ranges(&[(0, 2), (2, 4), (4, 6), (6, 7)])
        );
        // reserved slot narrows the capacity to 7 → still 2×3 per chunk.
        assert_eq!(chunk_ranges_weighted(&[3; 4], true), ranges(&[(0, 2), (2, 4)]));
        // an oversize item gets its own (honestly doomed) chunk, never dropped.
        assert_eq!(chunk_ranges_weighted(&[9, 1], false), ranges(&[(0, 1), (1, 2)]));
    }

    #[test]
    fn fold_reports_mid_chunk_failure_honestly() {
        let r = ranges(&[(0, 3), (3, 6), (6, 8)]);
        let f = fold_outcomes(
            &r,
            &[
                ChunkOutcome::Landed("0xa".into()),
                ChunkOutcome::Failed("boom".into()),
                ChunkOutcome::Landed("0xc".into()),
            ],
        );
        assert_eq!(f.landed, vec![0, 1, 2, 6, 7]);
        assert_eq!(f.failed, vec![3, 4, 5]);
        assert!(f.unattempted.is_empty());
        assert_eq!(f.tx_hashes, vec!["0xa".to_string(), "0xc".to_string()]);
        assert_eq!(f.chunk_errors, vec![(1, "boom".to_string())]);
    }

    #[test]
    fn fold_marks_unexecuted_chunks_never_attempted() {
        let r = ranges(&[(0, 2), (2, 4), (4, 5)]);
        let f = fold_outcomes(&r, &[ChunkOutcome::Failed("signer".into())]);
        assert_eq!(f.failed, vec![0, 1]);
        assert_eq!(f.unattempted, vec![2, 3, 4]);
        assert!(f.landed.is_empty() && f.tx_hashes.is_empty());
    }

    #[test]
    fn fold_omits_a_vacuous_chunks_empty_hash() {
        let r = ranges(&[(0, 2)]);
        let f = fold_outcomes(&r, &[ChunkOutcome::Landed(String::new())]);
        assert_eq!(f.landed, vec![0, 1]);
        assert!(f.tx_hashes.is_empty());
    }
}
