//! Pure work-cycle decision core for an autonomous localharness company — the
//! logic by which a founded org actually DOES work: allocate a funded task to
//! the best-fit role-agent, judge the delivered result, pay the worker, and
//! attest the outcome to reputation. It models ONE
//! claim → work → judge → pay → attest cycle (the oggoel / agent-coordination
//! flywheel) as DATA: every decision is a pure function and every side effect is
//! an [`Action`] descriptor the caller later maps onto a real edge call
//! (`registry::post_bounty_sponsored` / `claim_bounty_sponsored` /
//! `accept_result_sponsored` / `attest_sponsored` / a TBA `$LH` transfer).
//!
//! Zero I/O, zero chain deps, native + wasm clean — the `keeper.rs` /
//! `lessons.rs` / `confirm.rs` pattern of a native-testable core hoisted out of
//! the wiring so the allocation / judgement / payout invariants run under
//! `cargo test`. Grounded in `design/autonomous-business/STRATEGY.md` (the
//! role→primitive map), `design/shipped/agent-coordination.md` (the rungs), and
//! `design/oggoel.md` (the live token-governed-company prior). The on-chain
//! shapes the [`Action`]s map onto are real (`BountyFacet` status 0 Open / 1
//! Claimed / 2 Submitted / 3 Paid; `attest(subject, rating 1..=5, workRef)`).

/// Reputation a worker gains when its result is accepted — the proof-of-work
/// signal that ranks future claims (mirrors a `+` Reviewer attestation; the
/// Coder role's "reputation climbs from accepted work").
pub const REP_GAIN_ON_ACCEPT: u32 = 1;

/// A business role an agent fills — each is an identity NFT + TBA with an
/// on-chain persona (`design/autonomous-business/roles/*.md`). The work cycle
/// matches a task's required role to a worker that fills it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Executive,
    ProductManager,
    Coder,
    Reviewer,
    Accounting,
    Hr,
    Marketing,
}

/// The acceptance bar a Reviewer scores a deliverable against. A submission
/// rated `>= min_quality` (on the 1..=5 scale) clears it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Criteria {
    /// Minimum 1..=5 quality the Reviewer must score for an accept.
    pub min_quality: u8,
}

/// A worker's delivered result, as the Reviewer observes it. The prose artifact
/// lives off-core; this is the judged summary the decision core needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    /// Observed 1..=5 quality / task-fit of the deliverable.
    pub quality: u8,
    /// The deliverable claims something impossible on the serverless platform
    /// (binds a port, runs a daemon, …) — a hallucination the Reviewer scores
    /// low and auto-rejects regardless of `quality`.
    pub claims_impossible: bool,
}

/// A task's position in the claim → work → judge → pay → attest lifecycle (the
/// `BountyFacet` status line modeled as data). [`Stage::Accepted`] and
/// [`Stage::Rejected`] are terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// In the backlog, reward not yet escrowed.
    Planned,
    /// Reward escrowed, open for a worker to claim (BountyFacet Open).
    Posted,
    /// Claimed by `worker_id`; work in progress (Claimed).
    Assigned { worker_id: u64 },
    /// `worker_id` delivered `submission`; awaiting the Reviewer (Submitted).
    Submitted { worker_id: u64, submission: Submission },
    /// Judged accept: `paid` `$LH` settled to the worker, `rating` attested (Paid).
    Accepted { worker_id: u64, rating: u8, paid: u128 },
    /// Judged reject: escrow stays reclaimable; the low `rating` is still attested.
    Rejected { worker_id: u64, rating: u8 },
}

/// A unit of fundable work. The reward is escrowed when the task is posted and
/// settled (clamped to the treasury) to the worker's TBA on accept; `role` +
/// `min_reputation` gate who may be assigned it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// Stable id — doubles as the bounty id and the attestation `work_ref`.
    pub id: u64,
    /// The role best suited to the work (matched against [`WorkerState::role`]).
    pub role: Role,
    /// `$LH` (wei) reward — escrowed at post, paid (clamped) on accept.
    pub reward: u128,
    /// Minimum worker reputation eligible to be assigned (0 = anyone).
    pub min_reputation: u32,
    /// The Reviewer's acceptance bar.
    pub criteria: Criteria,
    /// Lifecycle position.
    pub stage: Stage,
}

/// A role-agent's live state for allocation: its id (the tokenId that is both
/// the `claimBounty` claimant and the `attest` subject), the role it fills, its
/// reputation (the ranking signal Reviewer attestations build), and whether it
/// can take a new task this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerState {
    pub id: u64,
    pub role: Role,
    pub reputation: u32,
    pub available: bool,
}

/// The company's task board.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Backlog {
    pub tasks: Vec<Task>,
}

/// A decided allocation: give `task_id` to `worker_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assignment {
    pub task_id: u64,
    pub worker_id: u64,
}

/// The Reviewer's verdict on a submission — both arms carry the 1..=5 `rating`
/// to attest (reputation moves on accepts AND rejects, per the Reviewer role).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptDecision {
    /// Meets the bar — settle the reward, attest `rating`.
    Accept { rating: u8 },
    /// Below the bar — escrow stays locked, attest the low `rating`.
    Reject { rating: u8 },
}

impl AcceptDecision {
    /// Whether the result was accepted (reward settles).
    pub fn is_accept(self) -> bool {
        matches!(self, AcceptDecision::Accept { .. })
    }
    /// The 1..=5 rating attested for this verdict (set in both arms).
    pub fn rating(self) -> u8 {
        match self {
            AcceptDecision::Accept { rating } | AcceptDecision::Reject { rating } => rating,
        }
    }
}

/// A side-effect descriptor the [`step`] driver emits — pure DATA the caller
/// maps onto a real sponsored edge call (named per variant). The core itself
/// never performs I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Escrow `reward` behind the task spec and open it for claims.
    /// → `registry::post_bounty_sponsored(task, reward_wei, ttl)`.
    PostBounty { task_id: u64, reward: u128 },
    /// Assign the task to the worker (its own tokenId is the claimant).
    /// → `registry::claim_bounty_sponsored(bounty_id, claimant_token_id)`.
    AssignTask { task_id: u64, worker_id: u64 },
    /// Mark the delivered result accepted (releases the bounty escrow).
    /// → `registry::accept_result_sponsored(bounty_id)`.
    AcceptResult { task_id: u64, worker_id: u64 },
    /// Reject the delivered result — escrow stays locked / reclaimable.
    /// → leave the bounty unaccepted (or `cancelBounty` / `reclaimExpired`).
    RejectResult { task_id: u64, worker_id: u64 },
    /// Pay `amount` `$LH` to the worker's TBA (treasury payroll / settle).
    /// → a TBA `registry::…transfer` / `send_lh` / x402 settle.
    Payout { task_id: u64, worker_id: u64, amount: u128 },
    /// Write the 1..=5 reputation attestation keyed to the work.
    /// → `registry::attest_sponsored(subject_token_id, rating, work_ref)`.
    Attest { subject_id: u64, rating: u8, work_ref: u64 },
}

/// The whole company work-cycle state the [`step`] driver advances.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub backlog: Backlog,
    pub workers: Vec<WorkerState>,
    /// Treasury `$LH` (wei) available to pay out — debited at payout, never below
    /// zero ([`compute_payout`] clamps a payout to it). The on-chain bounty
    /// escrow debits at post; a caller using escrow-pay reconciles the two.
    pub treasury: u128,
}

/// Pick the best `(task, worker)` allocation right now, or `None` if no posted
/// task has an eligible available worker. Tasks are weighed highest-reward first
/// (FIFO id tie-break); a worker is eligible when it is `available`, fills the
/// task's `role`, and meets `min_reputation`; among eligible workers the
/// highest reputation wins (lowest id tie-break). Only [`Stage::Posted`] tasks
/// are assignable — a high-reward task with no eligible worker is skipped so a
/// lower-priority staffable task still gets allocated.
pub fn assign_next_task(backlog: &Backlog, workers: &[WorkerState]) -> Option<Assignment> {
    let mut posted: Vec<&Task> =
        backlog.tasks.iter().filter(|t| t.stage == Stage::Posted).collect();
    posted.sort_by(|a, b| b.reward.cmp(&a.reward).then(a.id.cmp(&b.id)));
    for task in posted {
        let best = workers
            .iter()
            .filter(|w| w.available && w.role == task.role && w.reputation >= task.min_reputation)
            // Highest reputation; on a tie the LOWEST id (so `b.id.cmp(&a.id)`).
            .max_by(|a, b| a.reputation.cmp(&b.reputation).then(b.id.cmp(&a.id)));
        if let Some(w) = best {
            return Some(Assignment { task_id: task.id, worker_id: w.id });
        }
    }
    None
}

/// Judge a submission against a task's criteria. A deliverable that claims an
/// impossible (serverless-violating) capability is auto-rejected at rating 1;
/// otherwise the rating is the observed quality clamped to 1..=5 and the result
/// is accepted iff it meets `min_quality`.
pub fn evaluate_result(submission: &Submission, criteria: &Criteria) -> AcceptDecision {
    if submission.claims_impossible {
        return AcceptDecision::Reject { rating: 1 };
    }
    let rating = submission.quality.clamp(1, 5);
    if rating >= criteria.min_quality {
        AcceptDecision::Accept { rating }
    } else {
        AcceptDecision::Reject { rating }
    }
}

/// The payout for an accepted task: its reward, clamped to the treasury balance
/// (you can't pay what you don't hold — Accounting flags the shortfall). The
/// caller debits the treasury by exactly this amount.
pub fn compute_payout(task: &Task, treasury_balance: u128) -> u128 {
    task.reward.min(treasury_balance)
}

impl State {
    /// Record a worker's delivery: move an [`Stage::Assigned`] task to
    /// [`Stage::Submitted`] so the next [`step`] judges it. Returns `false` if
    /// `task_id` isn't currently assigned (a worker can only submit what it
    /// claimed). This models the off-core "work" half the driver never invents.
    pub fn deliver(&mut self, task_id: u64, submission: Submission) -> bool {
        for t in &mut self.backlog.tasks {
            if t.id == task_id {
                if let Stage::Assigned { worker_id } = t.stage {
                    t.stage = Stage::Submitted { worker_id, submission };
                    return true;
                }
                return false;
            }
        }
        false
    }
}

/// Advance the cycle by exactly ONE transition and return the new state plus the
/// [`Action`]s it implies. Priority: judge a delivered result first (finish work
/// in flight), then assign the best posted task to a worker, then post the next
/// planned task. The action list is empty when nothing is actionable — every
/// task is terminal, OR the only work in flight is assigned-but-not-yet-delivered
/// (the driver is idle, waiting on a worker; call [`State::deliver`] to proceed).
pub fn step(state: &State) -> (State, Vec<Action>) {
    let mut next = state.clone();

    // 1. Judge the lowest-id submitted result → pay + attest, or reject + attest.
    let submitted_idx = next
        .backlog
        .tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| matches!(t.stage, Stage::Submitted { .. }))
        .min_by_key(|(_, t)| t.id)
        .map(|(i, _)| i);
    if let Some(i) = submitted_idx {
        let task = next.backlog.tasks[i].clone();
        let Stage::Submitted { worker_id: w, submission } = &task.stage else {
            return (next, Vec::new()); // unreachable by the filter above
        };
        let worker_id = *w;
        let mut actions = Vec::new();
        match evaluate_result(submission, &task.criteria) {
            AcceptDecision::Accept { rating } => {
                let amount = compute_payout(&task, next.treasury);
                next.treasury -= amount;
                actions.push(Action::AcceptResult { task_id: task.id, worker_id });
                actions.push(Action::Payout { task_id: task.id, worker_id, amount });
                actions.push(Action::Attest { subject_id: worker_id, rating, work_ref: task.id });
                next.backlog.tasks[i].stage = Stage::Accepted { worker_id, rating, paid: amount };
                free_worker(&mut next.workers, worker_id, true);
            }
            AcceptDecision::Reject { rating } => {
                actions.push(Action::RejectResult { task_id: task.id, worker_id });
                actions.push(Action::Attest { subject_id: worker_id, rating, work_ref: task.id });
                next.backlog.tasks[i].stage = Stage::Rejected { worker_id, rating };
                free_worker(&mut next.workers, worker_id, false);
            }
        }
        return (next, actions);
    }

    // 2. Assign the best posted task to its best-fit available worker.
    if let Some(a) = assign_next_task(&next.backlog, &next.workers) {
        if let Some(ti) = next.backlog.tasks.iter().position(|t| t.id == a.task_id) {
            next.backlog.tasks[ti].stage = Stage::Assigned { worker_id: a.worker_id };
        }
        if let Some(wi) = next.workers.iter().position(|w| w.id == a.worker_id) {
            next.workers[wi].available = false;
        }
        return (next, vec![Action::AssignTask { task_id: a.task_id, worker_id: a.worker_id }]);
    }

    // 3. Post the next planned task (escrow its reward, open it for claims).
    if let Some(i) = min_id_idx(&next.backlog.tasks, |t| t.stage == Stage::Planned) {
        let (id, reward) = (next.backlog.tasks[i].id, next.backlog.tasks[i].reward);
        next.backlog.tasks[i].stage = Stage::Posted;
        return (next, vec![Action::PostBounty { task_id: id, reward }]);
    }

    (next, Vec::new())
}

/// Index of the lowest-`id` task matching `pred`, if any (a deterministic
/// frontier pick independent of `Vec` order).
fn min_id_idx(tasks: &[Task], pred: impl Fn(&Task) -> bool) -> Option<usize> {
    tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| pred(t))
        .min_by_key(|(_, t)| t.id)
        .map(|(i, _)| i)
}

/// Free a worker after judgement: mark it available again, and on an accept
/// bump its reputation by [`REP_GAIN_ON_ACCEPT`] (proven work ranks it higher).
fn free_worker(workers: &mut [WorkerState], id: u64, accepted: bool) {
    if let Some(w) = workers.iter_mut().find(|w| w.id == id) {
        w.available = true;
        if accepted {
            w.reputation = w.reputation.saturating_add(REP_GAIN_ON_ACCEPT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: u64, role: Role, reward: u128, min_rep: u32, min_quality: u8) -> Task {
        Task {
            id,
            role,
            reward,
            min_reputation: min_rep,
            criteria: Criteria { min_quality },
            stage: Stage::Planned,
        }
    }

    fn posted(id: u64, role: Role, reward: u128, min_rep: u32, min_quality: u8) -> Task {
        Task { stage: Stage::Posted, ..task(id, role, reward, min_rep, min_quality) }
    }

    fn worker(id: u64, role: Role, rep: u32) -> WorkerState {
        WorkerState { id, role, reputation: rep, available: true }
    }

    fn sub(quality: u8) -> Submission {
        Submission { quality, claims_impossible: false }
    }

    // --- assign_next_task -------------------------------------------------

    #[test]
    fn assign_prefers_high_reward_then_high_reputation() {
        let backlog = Backlog {
            tasks: vec![posted(1, Role::Coder, 10, 0, 3), posted(2, Role::Coder, 50, 0, 3)],
        };
        let workers = vec![worker(7, Role::Coder, 2), worker(8, Role::Coder, 9)];
        // Highest-reward task (#2) to the highest-reputation eligible worker (#8).
        assert_eq!(
            assign_next_task(&backlog, &workers),
            Some(Assignment { task_id: 2, worker_id: 8 })
        );
    }

    #[test]
    fn assign_respects_role_and_min_reputation_and_skips_unstaffable() {
        let backlog = Backlog {
            tasks: vec![
                posted(1, Role::Reviewer, 100, 5, 3), // top reward, needs a rep>=5 Reviewer
                posted(2, Role::Coder, 20, 0, 3),
            ],
        };
        let workers = vec![
            worker(3, Role::Reviewer, 4), // right role, reputation too low for #1
            worker(4, Role::Coder, 1),    // staffs #2
        ];
        // #1 has no eligible worker → fall through to the staffable #2.
        assert_eq!(
            assign_next_task(&backlog, &workers),
            Some(Assignment { task_id: 2, worker_id: 4 })
        );
    }

    #[test]
    fn assign_tie_breaks_lowest_worker_id() {
        let backlog = Backlog { tasks: vec![posted(1, Role::Coder, 10, 0, 3)] };
        let workers = vec![worker(9, Role::Coder, 5), worker(4, Role::Coder, 5)];
        assert_eq!(assign_next_task(&backlog, &workers).unwrap().worker_id, 4);
    }

    #[test]
    fn assign_none_when_no_available_matching_worker() {
        let backlog = Backlog { tasks: vec![posted(1, Role::Coder, 10, 0, 3)] };
        // Wrong role.
        assert_eq!(assign_next_task(&backlog, &[worker(1, Role::Reviewer, 9)]), None);
        // Right role but unavailable.
        let busy = WorkerState { available: false, ..worker(1, Role::Coder, 9) };
        assert_eq!(assign_next_task(&backlog, &[busy]), None);
        // No workers at all.
        assert_eq!(assign_next_task(&backlog, &[]), None);
        // A Planned (not yet Posted) task is not assignable.
        let planned = Backlog { tasks: vec![task(1, Role::Coder, 10, 0, 3)] };
        assert_eq!(assign_next_task(&planned, &[worker(1, Role::Coder, 9)]), None);
    }

    // --- evaluate_result --------------------------------------------------

    #[test]
    fn evaluate_accepts_at_or_above_bar_rejects_below() {
        let crit = Criteria { min_quality: 3 };
        assert_eq!(evaluate_result(&sub(3), &crit), AcceptDecision::Accept { rating: 3 });
        assert_eq!(evaluate_result(&sub(5), &crit), AcceptDecision::Accept { rating: 5 });
        assert_eq!(evaluate_result(&sub(2), &crit), AcceptDecision::Reject { rating: 2 });
        assert!(evaluate_result(&sub(3), &crit).is_accept());
        assert_eq!(evaluate_result(&sub(2), &crit).rating(), 2);
    }

    #[test]
    fn evaluate_clamps_rating_and_auto_rejects_hallucination() {
        let crit = Criteria { min_quality: 3 };
        assert_eq!(evaluate_result(&sub(9), &crit), AcceptDecision::Accept { rating: 5 });
        assert_eq!(evaluate_result(&sub(0), &crit), AcceptDecision::Reject { rating: 1 });
        // A serverless-impossible claim is rejected at 1 even with high quality.
        let halluc = Submission { quality: 5, claims_impossible: true };
        assert_eq!(evaluate_result(&halluc, &crit), AcceptDecision::Reject { rating: 1 });
    }

    // --- compute_payout ---------------------------------------------------

    #[test]
    fn payout_clamps_to_treasury() {
        let t = task(1, Role::Coder, 100, 0, 3);
        assert_eq!(compute_payout(&t, 250), 100); // funded → full reward
        assert_eq!(compute_payout(&t, 100), 100); // exactly funded
        assert_eq!(compute_payout(&t, 40), 40); // short → clamped to balance
        assert_eq!(compute_payout(&t, 0), 0); // broke → nothing
    }

    // --- deliver ----------------------------------------------------------

    #[test]
    fn deliver_only_transitions_assigned_tasks() {
        let mut state = State {
            backlog: Backlog {
                tasks: vec![Task {
                    stage: Stage::Assigned { worker_id: 7 },
                    ..task(1, Role::Coder, 50, 0, 3)
                }],
            },
            workers: vec![],
            treasury: 0,
        };
        assert!(state.deliver(1, sub(4)));
        assert_eq!(
            state.backlog.tasks[0].stage,
            Stage::Submitted { worker_id: 7, submission: sub(4) }
        );
        // Re-delivering (now Submitted) fails; an unknown id fails.
        assert!(!state.deliver(1, sub(5)));
        assert!(!state.deliver(99, sub(5)));
    }

    // --- step driver ------------------------------------------------------

    #[test]
    fn step_posts_then_assigns_then_idles_on_undelivered_work() {
        let state = State {
            backlog: Backlog { tasks: vec![task(1, Role::Coder, 50, 0, 3)] },
            workers: vec![worker(7, Role::Coder, 2)],
            treasury: 1_000,
        };
        let (s1, a1) = step(&state);
        assert_eq!(a1, vec![Action::PostBounty { task_id: 1, reward: 50 }]);
        assert_eq!(s1.backlog.tasks[0].stage, Stage::Posted);

        let (s2, a2) = step(&s1);
        assert_eq!(a2, vec![Action::AssignTask { task_id: 1, worker_id: 7 }]);
        assert_eq!(s2.backlog.tasks[0].stage, Stage::Assigned { worker_id: 7 });
        assert!(!s2.workers[0].available);

        // Assigned but not yet delivered → idle (the driver waits on the worker).
        let (s3, a3) = step(&s2);
        assert!(a3.is_empty());
        assert_eq!(s3.backlog.tasks[0].stage, Stage::Assigned { worker_id: 7 });
    }

    #[test]
    fn full_accept_cycle_pays_and_attests() {
        let mut state = State {
            backlog: Backlog { tasks: vec![task(1, Role::Coder, 30, 0, 3)] },
            workers: vec![worker(7, Role::Coder, 2)],
            treasury: 100,
        };
        let (s, _) = step(&state);
        state = s; // post
        let (s, _) = step(&state);
        state = s; // assign
        assert!(state.deliver(1, sub(5))); // worker delivers a 5★ result
        let (s, acts) = step(&state);
        state = s; // judge → pay + attest
        assert_eq!(
            acts,
            vec![
                Action::AcceptResult { task_id: 1, worker_id: 7 },
                Action::Payout { task_id: 1, worker_id: 7, amount: 30 },
                Action::Attest { subject_id: 7, rating: 5, work_ref: 1 },
            ]
        );
        assert_eq!(state.backlog.tasks[0].stage, Stage::Accepted { worker_id: 7, rating: 5, paid: 30 });
        assert_eq!(state.treasury, 70);
        assert!(state.workers[0].available);
        assert_eq!(state.workers[0].reputation, 3); // 2 + REP_GAIN_ON_ACCEPT
        // Nothing left to do.
        assert!(step(&state).1.is_empty());
    }

    #[test]
    fn reject_cycle_attests_low_and_pays_nothing() {
        let mut state = State {
            backlog: Backlog { tasks: vec![task(1, Role::Coder, 30, 0, 4)] },
            workers: vec![worker(7, Role::Coder, 2)],
            treasury: 100,
        };
        let (s, _) = step(&state);
        state = s; // post
        let (s, _) = step(&state);
        state = s; // assign
        assert!(state.deliver(1, sub(2))); // weak result, below the min_quality=4 bar
        let (s, acts) = step(&state);
        state = s;
        assert_eq!(
            acts,
            vec![
                Action::RejectResult { task_id: 1, worker_id: 7 },
                Action::Attest { subject_id: 7, rating: 2, work_ref: 1 },
            ]
        );
        assert_eq!(state.backlog.tasks[0].stage, Stage::Rejected { worker_id: 7, rating: 2 });
        assert_eq!(state.treasury, 100); // escrow untouched — no payout
        assert!(state.workers[0].available); // freed to take other work
        assert_eq!(state.workers[0].reputation, 2); // no gain on a reject
    }

    #[test]
    fn accept_cycle_clamps_payout_to_treasury() {
        let mut state = State {
            backlog: Backlog { tasks: vec![task(1, Role::Coder, 100, 0, 3)] },
            workers: vec![worker(7, Role::Coder, 2)],
            treasury: 40, // less than the 100 reward
        };
        let (s, _) = step(&state);
        state = s;
        let (s, _) = step(&state);
        state = s;
        assert!(state.deliver(1, sub(5)));
        let (s, acts) = step(&state);
        state = s;
        assert!(acts.contains(&Action::Payout { task_id: 1, worker_id: 7, amount: 40 }));
        assert_eq!(state.backlog.tasks[0].stage, Stage::Accepted { worker_id: 7, rating: 5, paid: 40 });
        assert_eq!(state.treasury, 0); // drained, not underflowed
    }

    #[test]
    fn multi_step_run_drives_two_tasks_to_terminal() {
        let mut state = State {
            backlog: Backlog {
                tasks: vec![
                    task(1, Role::Coder, 30, 0, 3),    // will be accepted (5★)
                    task(2, Role::Reviewer, 20, 0, 3), // will be rejected (1★)
                ],
            },
            workers: vec![worker(7, Role::Coder, 1), worker(8, Role::Reviewer, 1)],
            treasury: 100,
        };
        let mut log: Vec<Action> = Vec::new();
        for _ in 0..50 {
            // A worker delivers as soon as its task is assigned (the off-core work).
            let assigned: Vec<u64> = state
                .backlog
                .tasks
                .iter()
                .filter(|t| matches!(t.stage, Stage::Assigned { .. }))
                .map(|t| t.id)
                .collect();
            for id in assigned {
                state.deliver(id, sub(if id == 1 { 5 } else { 1 }));
            }
            let (s, acts) = step(&state);
            state = s;
            if acts.is_empty() {
                break;
            }
            log.extend(acts);
        }

        // Both tasks reached a terminal stage.
        assert!(matches!(state.backlog.tasks[0].stage, Stage::Accepted { paid: 30, .. }));
        assert!(matches!(state.backlog.tasks[1].stage, Stage::Rejected { .. }));
        // Treasury debited only by the accepted task's clamped payout.
        assert_eq!(state.treasury, 70);
        // The whole cycle showed up as data: both posted, one paid, both attested.
        assert!(log.iter().any(|a| matches!(a, Action::PostBounty { task_id: 1, .. })));
        assert!(log.iter().any(|a| matches!(a, Action::PostBounty { task_id: 2, .. })));
        assert!(log.contains(&Action::Payout { task_id: 1, worker_id: 7, amount: 30 }));
        assert!(log.iter().any(|a| matches!(a, Action::Attest { subject_id: 7, rating: 5, work_ref: 1 })));
        assert!(log.iter().any(|a| matches!(a, Action::Attest { subject_id: 8, work_ref: 2, .. })));
        // Both workers freed; the accepted worker's reputation climbed, the other's didn't.
        assert!(state.workers.iter().all(|w| w.available));
        assert_eq!(state.workers[0].reputation, 2); // 1 + gain
        assert_eq!(state.workers[1].reputation, 1); // unchanged on reject
    }
}
