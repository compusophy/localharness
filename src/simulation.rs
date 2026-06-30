//! PURE MULTI-CYCLE FORECAST core for an autonomous localharness company — runs
//! the [`crate::work_cycle`] driver forward over N cycles and projects the
//! company's TRAJECTORY (treasury, throughput, runway, the honest books) so an
//! operator can SEE where the business is heading before committing real `$LH`.
//!
//! Where [`crate::work_cycle_runtime::plan_cycle`] previews ONE cycle to
//! quiescence, this projects MANY cycles: each cycle injects the off-core
//! "work" (assigned tasks get delivered), advances the pure
//! [`work_cycle::step`] driver by one transition, books the resulting revenue /
//! costs into an [`accounting::Ledger`], and records a [`CycleSnapshot`]. It
//! stops the moment the treasury can no longer afford a cycle's cost, flagging
//! the exhaustion cycle in [`Forecast::ran_out_at`].
//!
//! Like `work_cycle` / `accounting` / `keeper` / `lessons`, this is a
//! native-testable core: PURE functions over data, ZERO I/O, zero chain deps,
//! native + wasm clean. It executes / broadcasts NOTHING — a [`Forecast`] is a
//! projection, not a commitment. Composes directly with the
//! [`crate::work_cycle`] treasury / reward units (`$LH` wei, `u128`) and the
//! [`crate::accounting`] seed-vs-earned books.
//!
//! ## The modeling assumption ([`SimConfig::submit_quality`])
//!
//! The work-cycle driver judges a result only once a worker has DELIVERED it
//! (moved `Assigned` → `Submitted`); a pure forecast cannot run real agents, so
//! it must STAND IN for that off-core delivery. The assumption encoded here:
//! **every task that is `Assigned` at the start of a cycle is delivered that
//! same cycle at one fixed quality, [`SimConfig::submit_quality`], and never
//! claims an impossible capability** (`claims_impossible = false`). It is a
//! uniform best/worst-case knob — sweep it to forecast the optimistic (high
//! quality → accepts → revenue) versus pessimistic (low quality → rejects → no
//! revenue, pure burn) trajectories. Real workers vary per-task; the forecast
//! deliberately does not, so the projection stays deterministic and legible.

use crate::accounting::{self, Ledger};
use crate::work_cycle::{self, Action, Stage, State, Submission};

/// The knobs of a forecast run. Treasury / reward units are `$LH` wei (`u128`),
/// matching [`work_cycle::State::treasury`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimConfig {
    /// How many cycles to project (the forecast horizon). The run also stops
    /// early if the treasury can't afford a cycle — see [`Forecast::ran_out_at`].
    pub cycles: usize,
    /// `$LH` (wei) the company BURNS each cycle regardless of work — the inference
    /// floor (~0.01–0.2 `$LH`/turn in the live system). Debited every cycle; a
    /// cycle the treasury can't cover is the runway wall.
    pub cost_per_cycle: u128,
    /// `$LH` (wei) EARNED from an external paying caller per task the company
    /// accepts this cycle (the x402 / outside-payment income that the
    /// `Accept`-and-`Payout` of an internal task notionally fulfils). Added to the
    /// treasury and booked as [`accounting::Ledger::period_revenue`].
    pub revenue_per_accepted_task: u128,
    /// The fixed 1..=5 quality at which every `Assigned` task is assumed to be
    /// delivered each cycle (the documented modeling assumption above). A
    /// submission `>= a task's criteria.min_quality` is accepted (earns revenue),
    /// below it is rejected (pure burn). Sweep it for best/worst-case forecasts.
    pub submit_quality: u8,
}

/// One cycle's projected outcome — a single row of the forecast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CycleSnapshot {
    /// 0-based cycle index within the run.
    pub cycle: usize,
    /// Treasury `$LH` (wei) at the END of this cycle (after payouts, earned
    /// revenue, and the cycle's cost).
    pub treasury: u128,
    /// Tasks ACCEPTED this cycle (each earns [`SimConfig::revenue_per_accepted_task`]).
    /// At most one per cycle, since [`work_cycle::step`] judges one result per call.
    pub tasks_accepted_this_cycle: usize,
    /// Number of [`work_cycle::Action`] descriptors the cycle's `step` emitted
    /// (`0` = a quiescent cycle: nothing was actionable, only the cost burned).
    pub actions_count: usize,
    /// The honest cumulative bottom line through this cycle:
    /// [`accounting::net_position`] (earned revenue − costs, SEED EXCLUDED).
    /// Signed — negative is the normal early state (burn precedes earnings).
    pub net_position: i128,
}

/// The whole projection: a [`CycleSnapshot`] per run cycle plus the run totals.
/// PROJECTION ONLY — holding a `Forecast` performs nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forecast {
    /// One row per cycle actually run (length `< cfg.cycles` iff the run hit the
    /// runway wall — then `snapshots.len() == ran_out_at.unwrap()`).
    pub snapshots: Vec<CycleSnapshot>,
    /// Total tasks accepted across the whole run (the projected throughput).
    pub total_accepted: usize,
    /// 0-based index of the first cycle the treasury could NOT afford to run, or
    /// `None` if the company funded the entire horizon. When set, no snapshot
    /// exists for this cycle or any after it.
    pub ran_out_at: Option<usize>,
    /// Treasury `$LH` (wei) at the end of the run.
    pub final_treasury: u128,
    /// The cumulative books at the end of the run: `treasury` is the final
    /// balance, `period_costs` sums every cycle's inference cost + payroll
    /// payouts, `period_revenue` sums the earned revenue, and `seed_capital` is
    /// the starting treasury (the seed the company began with). Feed it to
    /// [`accounting::net_position`] / [`accounting::is_self_funding`] /
    /// [`accounting::relies_on_seed`] for the honest verdict.
    pub final_ledger: Ledger,
}

/// Project `cfg.cycles` cycles of the company forward from `initial`. **Computes
/// a forecast; executes / broadcasts NOTHING.**
///
/// Each cycle: (1) inject the off-core work — every `Assigned` task is
/// [`State::deliver`]ed at [`SimConfig::submit_quality`] (the documented
/// assumption); (2) advance [`work_cycle::step`] by one transition (judge a
/// delivered result → pay + attest, else assign a posted task, else post a
/// planned one), which already debits the treasury by any payout; (3) add
/// [`SimConfig::revenue_per_accepted_task`] per accepted task and debit
/// [`SimConfig::cost_per_cycle`]; (4) fold both into the cumulative
/// [`accounting::Ledger`] and record a [`CycleSnapshot`]. The run STOPS the
/// moment the treasury can't cover the next cycle's cost, flagging that cycle in
/// [`Forecast::ran_out_at`]. All arithmetic saturates — no panics, no underflow.
pub fn simulate(initial: State, cfg: &SimConfig) -> Forecast {
    let seed_capital = initial.treasury; // the company's starting seed
    let mut state = initial;

    let mut snapshots: Vec<CycleSnapshot> = Vec::new();
    let mut total_accepted: usize = 0;
    let mut ran_out_at: Option<usize> = None;
    // Cumulative books across the whole run.
    let mut cum_costs: u128 = 0;
    let mut cum_revenue: u128 = 0;

    for cycle in 0..cfg.cycles {
        // Runway wall: can't even afford this cycle's burn → stop here.
        if state.treasury < cfg.cost_per_cycle {
            ran_out_at = Some(cycle);
            break;
        }

        // 1. Inject the off-core "work": deliver every Assigned task at the
        //    assumed fixed quality (collect ids first — deliver mutates).
        let assigned: Vec<u64> = state
            .backlog
            .tasks
            .iter()
            .filter_map(|t| matches!(t.stage, Stage::Assigned { .. }).then_some(t.id))
            .collect();
        for id in assigned {
            state.deliver(id, Submission { quality: cfg.submit_quality, claims_impossible: false });
        }

        // 2. One work transition (step debits payouts from the treasury itself).
        let (next, actions) = work_cycle::step(&state);
        state = next;

        // 3. Tally accepts → revenue; track payroll; debit the cycle's cost.
        let accepts =
            actions.iter().filter(|a| matches!(a, Action::AcceptResult { .. })).count();
        let payouts: u128 = actions
            .iter()
            .map(|a| if let Action::Payout { amount, .. } = a { *amount } else { 0 })
            .fold(0u128, u128::saturating_add);
        let revenue = cfg.revenue_per_accepted_task.saturating_mul(accepts as u128);
        state.treasury = state.treasury.saturating_add(revenue);
        state.treasury = state.treasury.saturating_sub(cfg.cost_per_cycle);

        // 4. Evolve the cumulative ledger + snapshot.
        cum_costs = cum_costs.saturating_add(cfg.cost_per_cycle).saturating_add(payouts);
        cum_revenue = cum_revenue.saturating_add(revenue);
        total_accepted += accepts;

        let ledger = Ledger {
            treasury: state.treasury,
            period_costs: cum_costs,
            period_revenue: cum_revenue,
            seed_capital,
        };
        snapshots.push(CycleSnapshot {
            cycle,
            treasury: state.treasury,
            tasks_accepted_this_cycle: accepts,
            actions_count: actions.len(),
            net_position: accounting::net_position(&ledger),
        });
    }

    Forecast {
        snapshots,
        total_accepted,
        ran_out_at,
        final_treasury: state.treasury,
        final_ledger: Ledger {
            treasury: state.treasury,
            period_costs: cum_costs,
            period_revenue: cum_revenue,
            seed_capital,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_cycle::{Backlog, Criteria, Role, Task, WorkerState};

    fn task(id: u64, reward: u128, min_quality: u8, stage: Stage) -> Task {
        Task {
            id,
            role: Role::Coder,
            reward,
            min_reputation: 0,
            criteria: Criteria { min_quality },
            stage,
        }
    }

    fn worker(id: u64, available: bool) -> WorkerState {
        WorkerState { id, role: Role::Coder, reputation: 0, available }
    }

    fn state(tasks: Vec<Task>, workers: Vec<WorkerState>, treasury: u128) -> State {
        State { backlog: Backlog { tasks }, workers, treasury }
    }

    // --- runway exhaustion -------------------------------------------------

    #[test]
    fn high_cost_empty_backlog_runs_out_and_flags_the_cycle() {
        // 100 treasury, 30/cycle burn, nothing earning: 100→70→40→10, then cycle
        // 3 can't be afforded (10 < 30) → ran_out_at = Some(3).
        let cfg = SimConfig {
            cycles: 10,
            cost_per_cycle: 30,
            revenue_per_accepted_task: 0,
            submit_quality: 5,
        };
        let f = simulate(state(vec![], vec![], 100), &cfg);

        assert_eq!(f.ran_out_at, Some(3));
        assert_eq!(f.snapshots.len(), 3); // only cycles 0,1,2 ran
        assert_eq!(f.final_treasury, 10);
        assert_eq!(f.total_accepted, 0);
        // Each cycle was quiescent (empty board) — pure burn, no actions.
        assert!(f.snapshots.iter().all(|s| s.actions_count == 0));
        assert_eq!(f.snapshots.iter().map(|s| s.treasury).collect::<Vec<_>>(), vec![70, 40, 10]);
        // The books: 3 cycles of cost, zero revenue, seed = the starting 100.
        assert_eq!(f.final_ledger.period_costs, 90);
        assert_eq!(f.final_ledger.period_revenue, 0);
        assert_eq!(f.final_ledger.seed_capital, 100);
        // Honest verdict: burned seed, earned nothing.
        assert!(accounting::relies_on_seed(&f.final_ledger));
        assert!(!accounting::is_self_funding(&f.final_ledger));
    }

    #[test]
    fn exact_fit_funds_every_cycle_no_runout() {
        // treasury exactly covers all cycles (5 × 20 = 100) → no early stop, but
        // it ends drained at 0.
        let cfg = SimConfig {
            cycles: 5,
            cost_per_cycle: 20,
            revenue_per_accepted_task: 0,
            submit_quality: 5,
        };
        let f = simulate(state(vec![], vec![], 100), &cfg);
        assert_eq!(f.ran_out_at, None);
        assert_eq!(f.snapshots.len(), 5);
        assert_eq!(f.final_treasury, 0);
    }

    // --- self-sustaining ---------------------------------------------------

    #[test]
    fn revenue_above_cost_keeps_treasury_growing() {
        // A task pre-staged Assigned so cycle 0 delivers + accepts it; revenue
        // (100) dwarfs the reward payout (10) and the per-cycle cost (5), so the
        // treasury ends ABOVE where it started and the run never stalls.
        let cfg = SimConfig {
            cycles: 6,
            cost_per_cycle: 5,
            revenue_per_accepted_task: 100,
            submit_quality: 5,
        };
        let initial = state(
            vec![task(1, 10, 3, Stage::Assigned { worker_id: 7 })],
            vec![worker(7, false)],
            1_000,
        );
        let f = simulate(initial, &cfg);

        assert_eq!(f.ran_out_at, None);
        assert_eq!(f.total_accepted, 1);
        // Net of the one accept: +100 revenue, −10 payout, −5 cost the accept
        // cycle, −5 each of the remaining 5 idle cycles = 1000 + 100 − 10 − 30.
        assert_eq!(f.final_treasury, 1_060);
        assert!(f.final_treasury > 1_000); // self-sustaining: grew, not bled
        // Earned more than it spent → genuinely self-funding (not seed-propped).
        assert!(accounting::is_self_funding(&f.final_ledger));
        assert!(!accounting::relies_on_seed(&f.final_ledger));
        assert!(accounting::net_position(&f.final_ledger) > 0);
    }

    #[test]
    fn zero_cost_with_revenue_is_monotonic_nondecreasing() {
        // With no burn, an accept can only add (revenue 100 > reward 10) → the
        // treasury never decreases cycle-to-cycle.
        let cfg = SimConfig {
            cycles: 4,
            cost_per_cycle: 0,
            revenue_per_accepted_task: 100,
            submit_quality: 5,
        };
        let initial = state(
            vec![task(1, 10, 3, Stage::Assigned { worker_id: 7 })],
            vec![worker(7, false)],
            500,
        );
        let f = simulate(initial, &cfg);
        assert_eq!(f.ran_out_at, None);
        let mut prev = 500u128;
        for s in &f.snapshots {
            assert!(s.treasury >= prev, "treasury dipped at cycle {}", s.cycle);
            prev = s.treasury;
        }
        assert_eq!(f.final_treasury, 590); // +100 revenue − 10 payout, once
    }

    // --- empty backlog → quiescent -----------------------------------------

    #[test]
    fn empty_backlog_zero_cost_is_fully_quiescent() {
        // Nothing to do and nothing to burn: every cycle is a no-op, the treasury
        // is untouched, and the run completes the full horizon.
        let cfg = SimConfig {
            cycles: 5,
            cost_per_cycle: 0,
            revenue_per_accepted_task: 100,
            submit_quality: 5,
        };
        let f = simulate(state(vec![], vec![worker(7, true)], 250), &cfg);
        assert_eq!(f.ran_out_at, None);
        assert_eq!(f.snapshots.len(), 5);
        assert_eq!(f.total_accepted, 0);
        assert!(f.snapshots.iter().all(|s| s.actions_count == 0));
        assert!(f.snapshots.iter().all(|s| s.tasks_accepted_this_cycle == 0));
        assert!(f.snapshots.iter().all(|s| s.treasury == 250)); // untouched
        assert_eq!(f.final_treasury, 250);
        assert_eq!(f.final_ledger.period_revenue, 0);
        assert_eq!(f.final_ledger.period_costs, 0);
    }

    #[test]
    fn zero_cycles_yields_an_empty_forecast() {
        let cfg = SimConfig {
            cycles: 0,
            cost_per_cycle: 5,
            revenue_per_accepted_task: 100,
            submit_quality: 5,
        };
        let f = simulate(state(vec![], vec![], 42), &cfg);
        assert!(f.snapshots.is_empty());
        assert_eq!(f.ran_out_at, None);
        assert_eq!(f.total_accepted, 0);
        assert_eq!(f.final_treasury, 42);
        assert_eq!(f.final_ledger.seed_capital, 42);
    }

    // --- the submit-quality assumption drives accept vs reject -------------

    #[test]
    fn submit_quality_at_or_above_bar_accepts_and_earns() {
        // Same board, submit_quality 5 ≥ the min_quality-4 bar → accepted: earns
        // revenue, pays the reward, throughput counts it.
        let cfg = SimConfig {
            cycles: 3,
            cost_per_cycle: 0,
            revenue_per_accepted_task: 100,
            submit_quality: 5,
        };
        let initial = state(
            vec![task(1, 10, 4, Stage::Assigned { worker_id: 7 })],
            vec![worker(7, false)],
            1_000,
        );
        let f = simulate(initial, &cfg);
        assert_eq!(f.total_accepted, 1);
        assert_eq!(f.snapshots[0].tasks_accepted_this_cycle, 1);
        assert_eq!(f.final_treasury, 1_090); // +100 earned, −10 paid out
        assert_eq!(f.final_ledger.period_revenue, 100);
        assert_eq!(f.final_ledger.period_costs, 10); // the payout (payroll)
    }

    #[test]
    fn submit_quality_below_bar_rejects_and_earns_nothing() {
        // Identical board, the ONLY change is submit_quality 2 < the min_quality-4
        // bar → rejected: no revenue, no payout, throughput zero.
        let cfg = SimConfig {
            cycles: 3,
            cost_per_cycle: 0,
            revenue_per_accepted_task: 100,
            submit_quality: 2,
        };
        let initial = state(
            vec![task(1, 10, 4, Stage::Assigned { worker_id: 7 })],
            vec![worker(7, false)],
            1_000,
        );
        let f = simulate(initial, &cfg);
        assert_eq!(f.total_accepted, 0);
        assert_eq!(f.snapshots[0].tasks_accepted_this_cycle, 0);
        assert_eq!(f.final_treasury, 1_000); // unchanged — escrow untouched
        assert_eq!(f.final_ledger.period_revenue, 0);
        assert_eq!(f.final_ledger.period_costs, 0);
    }

    // --- multi-cycle throughput from a planned backlog ---------------------

    #[test]
    fn planned_task_takes_post_assign_deliver_cycles_to_accept() {
        // From a fresh Planned task with one worker, single-step cadence is:
        // cycle 0 post, cycle 1 assign, cycle 2 deliver+judge (accept). Proves the
        // forecast models realistic per-cycle throughput, not instant completion.
        let cfg = SimConfig {
            cycles: 5,
            cost_per_cycle: 1,
            revenue_per_accepted_task: 50,
            submit_quality: 5,
        };
        let initial =
            state(vec![task(1, 10, 3, Stage::Planned)], vec![worker(7, true)], 1_000);
        let f = simulate(initial, &cfg);

        assert_eq!(f.ran_out_at, None);
        assert_eq!(f.total_accepted, 1);
        // The accept landed on cycle 2 (post, assign, then judge).
        assert_eq!(f.snapshots[2].tasks_accepted_this_cycle, 1);
        assert_eq!(f.snapshots[0].tasks_accepted_this_cycle, 0);
        assert_eq!(f.snapshots[1].tasks_accepted_this_cycle, 0);
        // 1000 − 5 cost (5 cycles) − 10 payout + 50 revenue = 1035.
        assert_eq!(f.final_treasury, 1_035);
    }

    // --- saturation / no-panic guard ---------------------------------------

    #[test]
    fn extreme_values_saturate_without_panicking() {
        // Astronomical revenue + cost must clamp, never overflow/underflow.
        let cfg = SimConfig {
            cycles: 3,
            cost_per_cycle: u128::MAX,
            revenue_per_accepted_task: u128::MAX,
            submit_quality: 5,
        };
        let initial = state(
            vec![task(1, u128::MAX, 1, Stage::Assigned { worker_id: 7 })],
            vec![worker(7, false)],
            u128::MAX,
        );
        let f = simulate(initial, &cfg);
        // First cycle is affordable (treasury == cost); it runs, then the drained
        // treasury can't fund cycle 1.
        assert_eq!(f.ran_out_at, Some(1));
        assert_eq!(f.snapshots.len(), 1);
        // One cycle ran at cost u128::MAX, so period_costs saturated to MAX (no wrap).
        assert_eq!(f.final_ledger.period_costs, u128::MAX);
    }
}
