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

/// Hard TOTAL-items bound for ONE batch-tool call — the spend bound the old
/// per-tx caps used to provide. Auto-chunking removed the per-tx ceiling; with
/// no total bound, one confirmed call could mint 200 names (= 200 real $LH on
/// mainnet) across ~29 sponsored txs, with only the relay's ~30-tx/h window as
/// a backstop. 28 = FOUR full chunks at the reserved-aux-slot capacity of 7
/// (`chunk_capacity(true)`): big enough for every sanctioned flow (the
/// default 7-role `found_company`, "make me 20 subdomains", a whole-fleet
/// payroll) while capping one call's worst-case real-value exposure at
/// 28 items (≤28 $LH of mainnet registrations) and ≤4 registration txs —
/// leaving relay-window room for the guild/seed/setup txs a founding also
/// submits. Enforced by EVERY chunked batch adopter (`batch_create_subdomains`,
/// `batch_send_lh`, `bulk_release_subdomains`, `found_company`).
pub const MAX_BATCH_ITEMS: usize = 28;

/// The over-limit rejection for [`MAX_BATCH_ITEMS`]: `Some(message)` when `n`
/// items exceed the bound, telling the model to SPLIT the request (each split
/// call re-rides its own typed confirmation). `None` = within bounds.
pub fn over_batch_limit(tool: &str, n: usize) -> Option<String> {
    (n > MAX_BATCH_ITEMS).then(|| {
        format!(
            "{tool}: {n} items exceed the per-call bound of {MAX_BATCH_ITEMS}. \
             Split the request into separate calls of at most {MAX_BATCH_ITEMS} \
             items each (each call is confirmed on its own)."
        )
    })
}

/// The marker `registry::rpc::wait_for_receipt` embeds in a receipt-poll
/// timeout error, followed by the tx hash. A timeout is NOT a revert — the tx
/// was accepted by the node and MAY STILL MINE; only this marker
/// distinguishes "unknown chain state" from "did not happen". The producer
/// (rpc.rs) formats with this constant so the classifier can't drift.
pub const RECEIPT_TIMEOUT_MARKER: &str = "receipt timeout for ";

/// Extract the tx hash from a receipt-timeout error (however many layers of
/// "submit: …" / "X failed: …" wrapped it). `None` = not a timeout (a revert
/// or a pre-submit failure — the tx did NOT land).
pub fn unconfirmed_tx_hash(err: &str) -> Option<String> {
    let at = err.find(RECEIPT_TIMEOUT_MARKER)?;
    let rest = &err[at + RECEIPT_TIMEOUT_MARKER.len()..];
    let hash: String = rest
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(|c: char| !c.is_ascii_alphanumeric())
        .to_string();
    (!hash.is_empty()).then_some(hash)
}

/// Classify one chunk's submit error: a receipt TIMEOUT becomes
/// [`ChunkOutcome::Unconfirmed`] (tx hash known, chain state UNKNOWN — the tx
/// may still mine), anything else [`ChunkOutcome::Failed`] (the tx did not
/// land). Every chunk loop routes its `Err` arm through this so a timed-out
/// chunk is never reported as "did NOT move/burn".
pub fn classify_failure(err: String) -> ChunkOutcome {
    match unconfirmed_tx_hash(&err) {
        Some(tx) => ChunkOutcome::Unconfirmed(tx),
        None => ChunkOutcome::Failed(err),
    }
}

/// Consecutive-failure circuit breaker: after this many failed chunks IN A
/// ROW, a batch loop stops and reports the rest via
/// [`BatchFold::unattempted`] (a systemic failure — bad signer, drained
/// wallet, relay outage — will fail every remaining chunk identically;
/// grinding on burns sponsor budget for nothing).
pub const MAX_CONSECUTIVE_CHUNK_FAILURES: usize = 2;

/// Should the chunk loop STOP before attempting the next chunk?
/// - The last outcome is [`ChunkOutcome::Unconfirmed`] → stop IMMEDIATELY:
///   chain state is unknown and continuing risks double-spending (e.g. a
///   second meter bridge in `batch_send_lh` while the first may still mine).
/// - [`MAX_CONSECUTIVE_CHUNK_FAILURES`] trailing `Failed`s → circuit-break.
pub fn should_stop(outcomes: &[ChunkOutcome]) -> bool {
    if matches!(outcomes.last(), Some(ChunkOutcome::Unconfirmed(_))) {
        return true;
    }
    outcomes.len() >= MAX_CONSECUTIVE_CHUNK_FAILURES
        && outcomes
            .iter()
            .rev()
            .take(MAX_CONSECUTIVE_CHUNK_FAILURES)
            .all(|o| matches!(o, ChunkOutcome::Failed(_)))
}

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
    /// The chunk's tx was SUBMITTED but its receipt never resolved (the
    /// [`RECEIPT_TIMEOUT_MARKER`] class). Carries the tx hash. Chain state is
    /// UNKNOWN — the tx may still mine, so its items are neither landed nor
    /// failed, and the loop must stop after it.
    Unconfirmed(String),
}

/// The honest aggregate of a chunked batch run.
#[derive(Debug, Default, PartialEq, Eq)]
#[non_exhaustive] // pub SDK surface — adding a field must not be semver-breaking
pub struct BatchFold {
    /// Item indices whose chunk landed, in order.
    pub landed: Vec<usize>,
    /// Item indices whose chunk's tx failed, in order.
    pub failed: Vec<usize>,
    /// Item indices whose chunk's tx is UNCONFIRMED (receipt timeout — it may
    /// still mine), in order. Never counted as failed.
    pub unconfirmed: Vec<usize>,
    /// Item indices whose chunk was never attempted (the caller stopped early).
    pub unattempted: Vec<usize>,
    /// Tx hash per landed (non-vacuous) chunk, in submission order.
    pub tx_hashes: Vec<String>,
    /// `(chunk index, tx hash)` per UNCONFIRMED chunk, in submission order —
    /// the hash the caller must surface so the outcome can be checked later.
    pub unconfirmed_txs: Vec<(usize, String)>,
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
            Some(ChunkOutcome::Unconfirmed(tx)) => {
                fold.unconfirmed.extend(r.clone());
                fold.unconfirmed_txs.push((i, tx.clone()));
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
        type Case = (usize, bool, &'static [(usize, usize)]);
        let cases: &[Case] = &[
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

    #[test]
    fn batch_bound_rejects_over_limit_with_a_split_instruction() {
        assert_eq!(over_batch_limit("batch_send_lh", 0), None);
        assert_eq!(over_batch_limit("batch_send_lh", MAX_BATCH_ITEMS), None);
        let msg = over_batch_limit("batch_send_lh", MAX_BATCH_ITEMS + 1).unwrap();
        assert!(msg.contains("batch_send_lh"));
        assert!(msg.contains(&MAX_BATCH_ITEMS.to_string()));
        assert!(msg.to_lowercase().contains("split"));
        // 200 names — the unbounded-mint scenario the bound exists to stop.
        assert!(over_batch_limit("batch_create_subdomains", 200).is_some());
    }

    #[test]
    fn timeout_classifies_unconfirmed_with_tx_hash_revert_stays_failed() {
        // The wrapped shape a chunk loop actually sees:
        // run_sponsored_tempo_call prepends "submit: ", tools prepend more.
        let e = format!("batch failed: submit: {RECEIPT_TIMEOUT_MARKER}0xabc123");
        assert_eq!(unconfirmed_tx_hash(&e).as_deref(), Some("0xabc123"));
        assert_eq!(
            classify_failure(e),
            ChunkOutcome::Unconfirmed("0xabc123".into())
        );
        // A trailing clause must not ride into the hash.
        let e = format!("{RECEIPT_TIMEOUT_MARKER}0xdead (check later).");
        assert_eq!(unconfirmed_tx_hash(&e).as_deref(), Some("0xdead"));
        // The bounded poll-error escape (rpc.rs wait_for_receipt: 3 dead polls
        // give up UNCONFIRMED) — the shape that used to escape marker-less and
        // read as "did NOT move".
        let e = format!("submit: {RECEIPT_TIMEOUT_MARKER}0xfeed (receipt polling unreachable)");
        assert_eq!(unconfirmed_tx_hash(&e).as_deref(), Some("0xfeed"));
        assert_eq!(
            classify_failure(e),
            ChunkOutcome::Unconfirmed("0xfeed".into())
        );
        // A revert (or any non-timeout error) stays a plain failure.
        for revert in ["tx reverted: LH2024 insufficient", "signer: no wallet"] {
            assert_eq!(unconfirmed_tx_hash(revert), None);
            assert_eq!(
                classify_failure(revert.into()),
                ChunkOutcome::Failed(revert.into())
            );
        }
    }

    #[test]
    fn model_facing_text_carries_the_live_batch_bound() {
        // MAX_BATCH_ITEMS is hand-written into tool descriptions, both prompt
        // variants, and llms.txt prose. Bumping the const must redden here
        // until every surface is re-swept — stale caps are model-facing lies.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let n = MAX_BATCH_ITEMS.to_string();
        for rel in [
            "src/app/chat/tools/platform.rs",
            "src/app/chat/tools/company.rs",
            "src/session_prompt.rs",
            "web/llms.txt",
        ] {
            let text = std::fs::read_to_string(root.join(rel)).unwrap();
            let hit = text.contains(&format!("at most {n}"))
                || text.contains(&format!("At most {n}"))
                || text.contains(&format!("\u{2264}{n}"))
                || text.contains(&format!("capped at {n}"));
            assert!(
                hit,
                "{rel}: no batch-bound phrase mentions {n} — resweep the \
                 model-facing text after changing MAX_BATCH_ITEMS"
            );
        }
    }

    #[test]
    fn breaker_stops_after_two_consecutive_failures_and_fold_reports_unattempted() {
        let fail = || ChunkOutcome::Failed("boom".into());
        let land = || ChunkOutcome::Landed("0xa".into());
        assert!(!should_stop(&[]));
        assert!(!should_stop(&[fail()]));
        assert!(should_stop(&[fail(), fail()]));
        // A landed chunk RESETS the streak.
        assert!(!should_stop(&[fail(), land(), fail()]));
        assert!(should_stop(&[land(), fail(), fail()]));
        // The broken loop's trailing chunks fold as UNATTEMPTED — the
        // reachable-`BatchFold::unattempted` proof.
        let r = ranges(&[(0, 2), (2, 4), (4, 6), (6, 8)]);
        let f = fold_outcomes(&r, &[fail(), fail()]);
        assert_eq!(f.failed, vec![0, 1, 2, 3]);
        assert_eq!(f.unattempted, vec![4, 5, 6, 7]);
    }

    #[test]
    fn unconfirmed_stops_immediately_and_folds_neither_landed_nor_failed() {
        let unconf = ChunkOutcome::Unconfirmed("0xbeef".into());
        // One unconfirmed chunk = stop NOW (chain state unknown; a further
        // chunk could double-bridge/double-spend).
        assert!(should_stop(std::slice::from_ref(&unconf)));
        assert!(should_stop(&[ChunkOutcome::Landed("0xa".into()), unconf.clone()]));
        let r = ranges(&[(0, 2), (2, 4), (4, 5)]);
        let f = fold_outcomes(&r, &[ChunkOutcome::Landed("0xa".into()), unconf]);
        assert_eq!(f.landed, vec![0, 1]);
        assert!(f.failed.is_empty() && f.chunk_errors.is_empty());
        assert_eq!(f.unconfirmed, vec![2, 3]);
        assert_eq!(f.unconfirmed_txs, vec![(1, "0xbeef".to_string())]);
        assert_eq!(f.unattempted, vec![4]);
    }
}
