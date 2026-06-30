//! Pure role-fit scoring core for the HR (People Ops / Recruiting) role of an
//! autonomous localharness company (`design/autonomous-business/roles/hr.md`).
//! Given a [`RoleNeed`] (the open seat: required [`work_cycle::Role`] + a minimum
//! on-chain reputation bar) and a pool of [`Candidate`] agents, it decides who is
//! ELIGIBLE and RANKS the eligible ones best-first. Pure functions over data, no
//! chain / I/O — the wiring (`reputation_of` reads, `invite_to_guild` / `set_role`
//! writes) lives in the deferred executor, same `keeper` / `lessons` /
//! `work_cycle` pattern, so the eligibility + ordering invariants run under
//! `cargo test`.
//!
//! HR ranks on **proven on-chain reputation, not tenure or self-claim** (the role
//! guardrail), so [`score_candidate`] is monotonic in reputation and exact-role.
//! This is the staffing FRONT-END of the work cycle: a [`Candidate`] mirrors a
//! [`work_cycle::WorkerState`] field-for-field (there's a [`From`] impl), and the
//! best-ranked candidates are exactly the workers
//! [`work_cycle::assign_next_task`] then allocates a posted task to — both prefer
//! the highest reputation, lowest-id on a tie, so HR's ranking and the work
//! cycle's allocation agree by construction.

use crate::work_cycle::{Role, WorkerState};

/// An open seat HR is staffing: the [`Role`] the work needs filled and the minimum
/// reputation a candidate must have proven to be eligible (0 = anyone of the role).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoleNeed {
    /// The business role the seat is for — matched EXACTLY against a candidate.
    pub role: Role,
    /// Minimum on-chain reputation to be considered (the Reviewer's attestations
    /// are the signal; HR acts on it, not a hunch).
    pub min_reputation: u32,
}

/// A hireable agent HR is considering — its identity tokenId, the role it fills,
/// its proven reputation, and whether it can take a new seat right now. Mirrors
/// [`work_cycle::WorkerState`] (see the [`From`] impl) so the same roster feeds
/// both HR ranking and work-cycle allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    /// The agent's identity tokenId (the `set_role` / `invite_to_guild` subject).
    pub id: u64,
    /// The role this agent fills.
    pub role: Role,
    /// On-chain reputation (the ranking signal).
    pub reputation: u32,
    /// Whether the agent is free to be assigned this seat.
    pub available: bool,
}

impl From<WorkerState> for Candidate {
    /// A work-cycle worker IS a hiring candidate — same fields. Lets HR rank the
    /// exact roster `work_cycle` allocates over.
    fn from(w: WorkerState) -> Self {
        Candidate { id: w.id, role: w.role, reputation: w.reputation, available: w.available }
    }
}

/// A candidate that cleared eligibility, with its fit score — the unit
/// [`rank_candidates`] returns, sorted best-first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ranked {
    /// The candidate's identity tokenId.
    pub id: u64,
    /// Fit score (higher = better). Equals the candidate's reputation, so ranking
    /// matches [`work_cycle::assign_next_task`]'s highest-reputation preference.
    pub score: u32,
}

/// Score one candidate against a need, or `None` if INELIGIBLE — the seat goes to
/// no one rather than a bad fit. Ineligible means any of: wrong [`Role`], below
/// the `min_reputation` bar, or not [`Candidate::available`]. An eligible
/// candidate scores its [`Candidate::reputation`] (HR ranks on proven reputation),
/// so a higher-rep agent always outscores a lower-rep one of the same role.
pub fn score_candidate(need: &RoleNeed, candidate: &Candidate) -> Option<u32> {
    if candidate.role != need.role
        || !candidate.available
        || candidate.reputation < need.min_reputation
    {
        return None;
    }
    Some(candidate.reputation)
}

/// Rank the ELIGIBLE candidates for a need, best-first. Ineligible candidates
/// (wrong role / below the bar / unavailable) are filtered out entirely. Ordering:
/// highest [`Ranked::score`] first, ties broken by LOWEST `id` — identical to
/// [`work_cycle::assign_next_task`], so the top of this list is the worker the
/// work cycle would assign the seat to. Empty pool ⇒ empty `Vec`.
pub fn rank_candidates(need: &RoleNeed, pool: &[Candidate]) -> Vec<Ranked> {
    let mut ranked: Vec<Ranked> = pool
        .iter()
        .filter_map(|c| score_candidate(need, c).map(|score| Ranked { id: c.id, score }))
        .collect();
    // Highest score first; on a tie the LOWEST id wins (deterministic, matches
    // assign_next_task).
    ranked.sort_by(|a, b| b.score.cmp(&a.score).then(a.id.cmp(&b.id)));
    ranked
}

/// The single best-fit candidate for a need, or `None` if no one is eligible — the
/// hire HR would make. The head of [`rank_candidates`].
pub fn best_candidate(need: &RoleNeed, pool: &[Candidate]) -> Option<Ranked> {
    rank_candidates(need, pool).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn need(role: Role, min_rep: u32) -> RoleNeed {
        RoleNeed { role, min_reputation: min_rep }
    }

    fn cand(id: u64, role: Role, rep: u32) -> Candidate {
        Candidate { id, role, reputation: rep, available: true }
    }

    // --- score_candidate eligibility --------------------------------------

    #[test]
    fn scores_an_eligible_candidate_its_reputation() {
        let n = need(Role::Coder, 2);
        assert_eq!(score_candidate(&n, &cand(7, Role::Coder, 5)), Some(5));
        // Exactly at the bar is eligible (>=, not >).
        assert_eq!(score_candidate(&n, &cand(7, Role::Coder, 2)), Some(2));
    }

    #[test]
    fn rejects_wrong_role() {
        let n = need(Role::Coder, 0);
        assert_eq!(score_candidate(&n, &cand(7, Role::Reviewer, 9)), None);
        assert_eq!(score_candidate(&n, &cand(7, Role::Accounting, 9)), None);
    }

    #[test]
    fn rejects_below_min_reputation() {
        let n = need(Role::Coder, 5);
        assert_eq!(score_candidate(&n, &cand(7, Role::Coder, 4)), None); // one short
        assert_eq!(score_candidate(&n, &cand(7, Role::Coder, 0)), None);
    }

    #[test]
    fn rejects_unavailable_even_if_qualified() {
        let n = need(Role::Coder, 0);
        let busy = Candidate { available: false, ..cand(7, Role::Coder, 9) };
        assert_eq!(score_candidate(&n, &busy), None);
    }

    // --- rank_candidates --------------------------------------------------

    #[test]
    fn ranks_eligible_high_reputation_first() {
        let n = need(Role::Coder, 0);
        let pool = vec![cand(1, Role::Coder, 3), cand(2, Role::Coder, 9), cand(3, Role::Coder, 5)];
        let ranked = rank_candidates(&n, &pool);
        assert_eq!(
            ranked,
            vec![
                Ranked { id: 2, score: 9 },
                Ranked { id: 3, score: 5 },
                Ranked { id: 1, score: 3 },
            ]
        );
    }

    #[test]
    fn rank_filters_out_every_ineligible_kind() {
        let n = need(Role::Coder, 4);
        let pool = vec![
            cand(1, Role::Reviewer, 9),                                 // wrong role
            cand(2, Role::Coder, 3),                                    // below bar
            Candidate { available: false, ..cand(3, Role::Coder, 8) },  // unavailable
            cand(4, Role::Coder, 6),                                    // eligible
            cand(5, Role::Coder, 4),                                    // eligible (at bar)
        ];
        let ranked = rank_candidates(&n, &pool);
        assert_eq!(ranked, vec![Ranked { id: 4, score: 6 }, Ranked { id: 5, score: 4 }]);
    }

    #[test]
    fn rank_tie_breaks_on_lowest_id() {
        let n = need(Role::Coder, 0);
        // Three equal-reputation candidates → ascending id order.
        let pool = vec![cand(9, Role::Coder, 5), cand(4, Role::Coder, 5), cand(7, Role::Coder, 5)];
        let ids: Vec<u64> = rank_candidates(&n, &pool).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![4, 7, 9]);
    }

    #[test]
    fn rank_empty_and_all_ineligible_pools_yield_nothing() {
        let n = need(Role::Coder, 5);
        assert!(rank_candidates(&n, &[]).is_empty());
        let none_fit =
            vec![cand(1, Role::Marketing, 9), cand(2, Role::Coder, 1)];
        assert!(rank_candidates(&n, &none_fit).is_empty());
    }

    // --- best_candidate ---------------------------------------------------

    #[test]
    fn best_candidate_is_the_top_of_the_ranking() {
        let n = need(Role::Reviewer, 1);
        let pool =
            vec![cand(1, Role::Reviewer, 4), cand(2, Role::Reviewer, 8), cand(3, Role::Coder, 9)];
        assert_eq!(best_candidate(&n, &pool), Some(Ranked { id: 2, score: 8 }));
        // No eligible candidate ⇒ no hire.
        assert_eq!(best_candidate(&need(Role::Hr, 0), &pool), None);
        assert_eq!(best_candidate(&n, &[]), None);
    }

    #[test]
    fn best_candidate_tie_breaks_lowest_id_matching_assign_next_task() {
        let n = need(Role::Coder, 0);
        let pool = vec![cand(8, Role::Coder, 5), cand(3, Role::Coder, 5)];
        // Same preference as work_cycle::assign_next_task: highest rep, lowest id.
        assert_eq!(best_candidate(&n, &pool), Some(Ranked { id: 3, score: 5 }));
    }

    // --- interop with work_cycle::WorkerState -----------------------------

    #[test]
    fn candidate_from_worker_state_preserves_fields() {
        let w = WorkerState { id: 42, role: Role::Accounting, reputation: 7, available: true };
        let c: Candidate = w.into();
        assert_eq!(c, Candidate { id: 42, role: Role::Accounting, reputation: 7, available: true });
        // A roster of workers ranks directly via the From conversion.
        let workers = vec![
            WorkerState { id: 1, role: Role::Coder, reputation: 2, available: true },
            WorkerState { id: 2, role: Role::Coder, reputation: 8, available: true },
        ];
        let pool: Vec<Candidate> = workers.into_iter().map(Candidate::from).collect();
        assert_eq!(best_candidate(&need(Role::Coder, 0), &pool), Some(Ranked { id: 2, score: 8 }));
    }
}
