//! PURE PLANNING SHELL around the [`crate::work_cycle`] decision core — turns a
//! read-only view of the company (its backlog, its role-agents, its treasury)
//! into a [`CyclePlan`] describing WHAT THE NEXT CYCLE WOULD DO, without ever
//! executing or broadcasting anything.
//!
//! This is the preview half of the autonomous-business loop. It connects the
//! pure [`work_cycle::step`] driver to a data source via the [`Reader`] trait,
//! runs the cycle forward to quiescence, and hands back the collected
//! [`work_cycle::Action`] descriptors plus before/after [`work_cycle::State`].
//! It performs NO I/O, NO on-chain writes, NO transfers — it is a dry run.
//!
//! ## Where the deferred executor plugs in (OUT OF SCOPE THIS TICK)
//!
//! A future, GREENLIGHT-GATED executor would walk [`CyclePlan::actions`] and map
//! each descriptor onto its real sponsored `registry` edge call. The mapping is
//! already pinned on each [`work_cycle::Action`] variant's doc comment, but to
//! restate it as the executor's contract:
//!
//! - [`Action::PostBounty`] → `registry::post_bounty_sponsored(task, reward, ttl)`
//! - [`Action::AssignTask`] → `registry::claim_bounty_sponsored(bounty_id, claimant_token_id)`
//! - [`Action::AcceptResult`] → `registry::accept_result_sponsored(bounty_id)`
//! - [`Action::RejectResult`] → leave the bounty unaccepted (`cancelBounty` / reclaim on expiry)
//! - [`Action::Payout`] → a worker-TBA `$LH` transfer / `send_lh` / x402 settle
//! - [`Action::Attest`] → `registry::attest_sponsored(subject_token_id, rating, work_ref)`
//!
//! Every one of those calls is a sponsored Tempo write and so is intentionally
//! ABSENT here: this module has NO `registry`/`wallet` dependency, stays native
//! + wasm clean, and exists only to let a human (or a higher gate) inspect the
//! plan before anything is signed. The real on-chain [`Reader`] impl (diamond
//! reads of `BountyFacet` / `CreditsFacet` / persona state) is likewise deferred
//! to that executor crate — this file ships only the trait + a [`MockReader`].
//!
//! Pure functions over data, no deps — the `keeper.rs` / `lessons.rs` pattern of
//! a native-testable core, so the planning invariants run under `cargo test`.

use crate::work_cycle::{self, Action, Backlog, State, Task, WorkerState};

/// Read-only view of the company the planner needs to build a [`State`].
///
/// PURE INTERFACE — deliberately free of any `registry`/`wallet` dependency.
/// The production impl (a diamond reader pulling live `BountyFacet` tasks,
/// role-agent reputations, and the treasury `$LH` balance) belongs to the
/// deferred, greenlight-gated executor; this crate ships only the trait and the
/// in-memory [`MockReader`] used by the tests.
pub trait Reader {
    /// The current task board (each [`Task`] carries its own lifecycle
    /// [`work_cycle::Stage`], so a task already `Submitted` on-chain reads back
    /// ready for the planner to judge).
    fn tasks(&self) -> Vec<Task>;

    /// The role-agents available for allocation this cycle.
    fn workers(&self) -> Vec<WorkerState>;

    /// Treasury `$LH` (wei) available to pay out — [`work_cycle::compute_payout`]
    /// clamps each planned payout to it, so the plan never overspends what the
    /// company holds.
    fn treasury_balance(&self) -> u128;
}

/// The result of a dry run: the state read in, every [`Action`] the cycle WOULD
/// emit, the state it WOULD reach, and a human-readable [`CyclePlan::summary`].
///
/// PREVIEW ONLY. Holding a `CyclePlan` performs nothing — the executor that maps
/// `actions` onto sponsored `registry` calls is separate and greenlight-gated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CyclePlan {
    /// The [`State`] assembled from the [`Reader`] before any planning.
    pub state_before: State,
    /// The ordered [`Action`] descriptors the cycle would emit (empty = nothing
    /// actionable — quiescent board). Pure data; NOTHING is executed.
    pub actions: Vec<Action>,
    /// The [`State`] the cycle would reach once `actions` are (notionally)
    /// applied — note the treasury is debited HERE by planned payouts only as a
    /// projection, never on-chain.
    pub state_after: State,
    /// One-line description of what would happen (action counts + treasury
    /// projection). Prefixed "PLAN (preview only — nothing executed)".
    pub summary: String,
}

impl CyclePlan {
    /// Whether the cycle is quiescent — no action would be taken (empty board,
    /// only assigned-but-undelivered work, or all tasks terminal).
    pub fn is_quiescent(&self) -> bool {
        self.actions.is_empty()
    }
}

/// Build a [`State`] from a [`Reader`], run [`work_cycle::step`] up to
/// `max_steps` transitions (stopping early the moment the board goes quiescent),
/// and return the collected plan. **PREVIEW ONLY — executes NOTHING.**
///
/// Each [`work_cycle::step`] advances exactly one transition (judge a submitted
/// result, else assign a posted task, else post a planned task) and reports the
/// [`Action`]s it implies as DATA. This driver simply iterates that pure step,
/// accumulating the descriptors, until a step yields no actions (the board is
/// idle — e.g. work is assigned but no worker has delivered, which a planning
/// shell cannot fabricate) or `max_steps` is hit. The treasury debits seen in
/// [`CyclePlan::state_after`] are a PROJECTION the core computes; no `$LH`
/// moves. To actually settle the plan, a future executor maps each
/// [`CyclePlan::actions`] entry onto its sponsored `registry` call (see the
/// module-level mapping) — that path is deferred and greenlight-gated.
pub fn plan_cycle(reader: &impl Reader, max_steps: usize) -> CyclePlan {
    let state_before = State {
        backlog: Backlog { tasks: reader.tasks() },
        workers: reader.workers(),
        treasury: reader.treasury_balance(),
    };

    let mut state = state_before.clone();
    let mut actions: Vec<Action> = Vec::new();
    let mut steps = 0usize;
    for _ in 0..max_steps {
        let (next, acts) = work_cycle::step(&state);
        state = next;
        if acts.is_empty() {
            break; // quiescent — nothing more is actionable this cycle
        }
        steps += 1;
        actions.extend(acts);
    }

    let summary = summarize(&state_before, &state, &actions, steps);
    CyclePlan { state_before, actions, state_after: state, summary }
}

/// One-line, deterministic description of a plan: per-variant action counts, the
/// total `$LH` (wei) that WOULD be paid, and the projected treasury delta.
fn summarize(before: &State, after: &State, actions: &[Action], steps: usize) -> String {
    let (mut posts, mut assigns, mut accepts, mut rejects, mut payout_n, mut attests) =
        (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    let mut payout_wei: u128 = 0;
    for a in actions {
        match a {
            Action::PostBounty { .. } => posts += 1,
            Action::AssignTask { .. } => assigns += 1,
            Action::AcceptResult { .. } => accepts += 1,
            Action::RejectResult { .. } => rejects += 1,
            Action::Payout { amount, .. } => {
                payout_n += 1;
                payout_wei = payout_wei.saturating_add(*amount);
            }
            Action::Attest { .. } => attests += 1,
        }
    }
    format!(
        "PLAN (preview only — nothing executed): {steps} step(s), {} action(s) — \
         {posts} post, {assigns} assign, {accepts} accept, {rejects} reject, \
         {payout_n} payout ({payout_wei} $LH wei), {attests} attest; \
         treasury {} → {} (projected)",
        actions.len(),
        before.treasury,
        after.treasury,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_cycle::{Criteria, Role, Stage, Submission};

    /// In-memory [`Reader`] test helper — the stand-in for the deferred on-chain
    /// diamond reader. Holds the three reads as plain fields.
    struct MockReader {
        tasks: Vec<Task>,
        workers: Vec<WorkerState>,
        treasury: u128,
    }

    impl Reader for MockReader {
        fn tasks(&self) -> Vec<Task> {
            self.tasks.clone()
        }
        fn workers(&self) -> Vec<WorkerState> {
            self.workers.clone()
        }
        fn treasury_balance(&self) -> u128 {
            self.treasury
        }
    }

    fn task(id: u64, role: Role, reward: u128, min_quality: u8, stage: Stage) -> Task {
        Task {
            id,
            role,
            reward,
            min_reputation: 0,
            criteria: Criteria { min_quality },
            stage,
        }
    }

    fn worker(id: u64, role: Role, rep: u32) -> WorkerState {
        WorkerState { id, role, reputation: rep, available: true }
    }

    fn submitted(id: u64, role: Role, reward: u128, min_quality: u8, worker_id: u64, quality: u8) -> Task {
        task(
            id,
            role,
            reward,
            min_quality,
            Stage::Submitted { worker_id, submission: Submission { quality, claims_impossible: false } },
        )
    }

    #[test]
    fn empty_backlog_plans_no_actions() {
        let reader = MockReader { tasks: vec![], workers: vec![worker(1, Role::Coder, 5)], treasury: 1_000 };
        let plan = plan_cycle(&reader, 10);
        assert!(plan.actions.is_empty());
        assert!(plan.is_quiescent());
        // Read-through: the state mirrors the reader, untouched.
        assert_eq!(plan.state_before, plan.state_after);
        assert_eq!(plan.state_after.treasury, 1_000);
        assert!(plan.summary.contains("0 action(s)"));
        assert!(plan.summary.starts_with("PLAN (preview only"));
    }

    #[test]
    fn staffed_task_plans_post_then_assign_then_idles() {
        // One planned, staffable task: the plan posts it, assigns it, then goes
        // quiescent (a planning shell can't fabricate the worker's delivery).
        let reader = MockReader {
            tasks: vec![task(1, Role::Coder, 50, 3, Stage::Planned)],
            workers: vec![worker(7, Role::Coder, 2)],
            treasury: 1_000,
        };
        let plan = plan_cycle(&reader, 10);
        assert_eq!(
            plan.actions,
            vec![
                Action::PostBounty { task_id: 1, reward: 50 },
                Action::AssignTask { task_id: 1, worker_id: 7 },
            ]
        );
        // Reached quiescence: the task is now Assigned, awaiting an off-core delivery.
        assert_eq!(plan.state_after.backlog.tasks[0].stage, Stage::Assigned { worker_id: 7 });
        // Treasury is NOT debited by a post — escrow projection only happens on accept.
        assert_eq!(plan.state_after.treasury, 1_000);
        assert!(plan.summary.contains("1 post, 1 assign"));
    }

    #[test]
    fn submitted_result_plans_accept_payout_attest() {
        // A task already Submitted on-chain reads back ready to judge → the plan
        // is the full settle: accept + payout + attest.
        let reader = MockReader {
            tasks: vec![submitted(1, Role::Coder, 30, 3, 7, 5)],
            workers: vec![WorkerState { available: false, ..worker(7, Role::Coder, 2) }],
            treasury: 100,
        };
        let plan = plan_cycle(&reader, 10);
        assert_eq!(
            plan.actions,
            vec![
                Action::AcceptResult { task_id: 1, worker_id: 7 },
                Action::Payout { task_id: 1, worker_id: 7, amount: 30 },
                Action::Attest { subject_id: 7, rating: 5, work_ref: 1 },
            ]
        );
        assert_eq!(plan.state_after.backlog.tasks[0].stage, Stage::Accepted { worker_id: 7, rating: 5, paid: 30 });
        // Projected treasury debit of exactly the (clamped) payout.
        assert_eq!(plan.state_before.treasury, 100);
        assert_eq!(plan.state_after.treasury, 70);
        assert!(plan.summary.contains("1 payout (30 $LH wei)"));
        assert!(plan.summary.contains("treasury 100 → 70"));
    }

    #[test]
    fn submitted_result_payout_clamps_to_treasury() {
        // Reward (100) exceeds the treasury (40): the planned payout clamps, the
        // projection drains to zero (never underflows), and it's still accepted.
        let reader = MockReader {
            tasks: vec![submitted(1, Role::Coder, 100, 3, 7, 5)],
            workers: vec![WorkerState { available: false, ..worker(7, Role::Coder, 2) }],
            treasury: 40,
        };
        let plan = plan_cycle(&reader, 10);
        assert!(plan.actions.contains(&Action::Payout { task_id: 1, worker_id: 7, amount: 40 }));
        assert_eq!(plan.state_after.backlog.tasks[0].stage, Stage::Accepted { worker_id: 7, rating: 5, paid: 40 });
        assert_eq!(plan.state_after.treasury, 0);
        assert!(plan.summary.contains("1 payout (40 $LH wei)"));
    }

    #[test]
    fn submitted_rejection_plans_reject_and_attest_no_payout() {
        // A weak result below the bar reads back as a reject: attested low, no payout.
        let reader = MockReader {
            tasks: vec![submitted(1, Role::Coder, 30, 4, 7, 2)],
            workers: vec![WorkerState { available: false, ..worker(7, Role::Coder, 2) }],
            treasury: 100,
        };
        let plan = plan_cycle(&reader, 10);
        assert_eq!(
            plan.actions,
            vec![
                Action::RejectResult { task_id: 1, worker_id: 7 },
                Action::Attest { subject_id: 7, rating: 2, work_ref: 1 },
            ]
        );
        assert_eq!(plan.state_after.backlog.tasks[0].stage, Stage::Rejected { worker_id: 7, rating: 2 });
        // Escrow untouched in the projection — no payout was planned.
        assert_eq!(plan.state_after.treasury, 100);
        assert!(plan.summary.contains("0 payout"));
        assert!(plan.summary.contains("1 reject"));
    }

    #[test]
    fn multi_step_run_reaches_quiescence_on_mixed_board() {
        // Two planned tasks (each staffable) plus one already-submitted result:
        // the plan judges the submission, posts + assigns both planned tasks,
        // then idles. A generous max_steps proves the loop STOPS at quiescence
        // rather than spinning to the cap.
        let reader = MockReader {
            tasks: vec![
                submitted(1, Role::Coder, 30, 3, 7, 5), // judged first (accept)
                task(2, Role::Reviewer, 20, 3, Stage::Planned),
                task(3, Role::Marketing, 10, 3, Stage::Planned),
            ],
            workers: vec![
                WorkerState { available: false, ..worker(7, Role::Coder, 2) },
                worker(8, Role::Reviewer, 1),
                worker(9, Role::Marketing, 1),
            ],
            treasury: 100,
        };
        let plan = plan_cycle(&reader, 100);

        // Submission #1 settled.
        assert_eq!(plan.state_after.backlog.tasks[0].stage, Stage::Accepted { worker_id: 7, rating: 5, paid: 30 });
        // Both planned tasks posted then assigned (terminal-for-planning: Assigned).
        assert_eq!(plan.state_after.backlog.tasks[1].stage, Stage::Assigned { worker_id: 8 });
        assert_eq!(plan.state_after.backlog.tasks[2].stage, Stage::Assigned { worker_id: 9 });
        // Treasury projection debited only by the one accepted payout.
        assert_eq!(plan.state_after.treasury, 70);

        // The whole cycle showed up as data.
        assert!(plan.actions.contains(&Action::AcceptResult { task_id: 1, worker_id: 7 }));
        assert!(plan.actions.contains(&Action::Payout { task_id: 1, worker_id: 7, amount: 30 }));
        assert!(plan.actions.iter().any(|a| matches!(a, Action::PostBounty { task_id: 2, .. })));
        assert!(plan.actions.iter().any(|a| matches!(a, Action::PostBounty { task_id: 3, .. })));
        assert!(plan.actions.contains(&Action::AssignTask { task_id: 2, worker_id: 8 }));
        assert!(plan.actions.contains(&Action::AssignTask { task_id: 3, worker_id: 9 }));

        // Quiescent before the cap: the last productive step left only undelivered
        // assigned work, so re-planning the resulting board yields nothing.
        let requies = plan_cycle(
            &MockReader {
                tasks: plan.state_after.backlog.tasks.clone(),
                workers: plan.state_after.workers.clone(),
                treasury: plan.state_after.treasury,
            },
            100,
        );
        assert!(requies.is_quiescent());
    }

    #[test]
    fn max_steps_bounds_a_busy_board() {
        // Cap at 1: only the first transition (post task 1) is planned.
        let reader = MockReader {
            tasks: vec![
                task(1, Role::Coder, 50, 3, Stage::Planned),
                task(2, Role::Coder, 40, 3, Stage::Planned),
            ],
            workers: vec![worker(7, Role::Coder, 2)],
            treasury: 1_000,
        };
        let plan = plan_cycle(&reader, 1);
        assert_eq!(plan.actions, vec![Action::PostBounty { task_id: 1, reward: 50 }]);
        assert_eq!(plan.state_after.backlog.tasks[0].stage, Stage::Posted);
        assert!(plan.summary.contains("1 step(s), 1 action(s)"));
    }
}
