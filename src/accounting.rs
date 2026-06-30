//! Pure economics decision core for the Accounting (CFO / Treasurer) role of an
//! autonomous localharness company (`design/autonomous-business/roles/accounting.md`).
//! It models the company's books as DATA so the solvency / runway / break-even
//! invariants run under `cargo test`: a [`Ledger`] is a period snapshot, every
//! judgement is a pure function over it, and NOTHING here reads the chain, the
//! meter, or a TBA balance — the wiring (`registry::query_balance` /
//! `check_balances` / the CreditMeterFacet read) lives in the deferred,
//! greenlight-gated executor, exactly like `work_cycle` / `keeper` / `lessons`.
//!
//! ## The honest constraint, encoded
//!
//! Inherited from `design/oggoel.md` and stated up front in
//! `design/autonomous-business/STRATEGY.md`: **the org is seed-capitalized, not
//! self-funding.** `$LH` enters via redeem / on-ramp / owner top-up (SEED), every
//! turn burns ~0.01–0.2 `$LH` of inference (COST), and true net-positive requires
//! *external paying callers above inference cost* (EARNED revenue). So the
//! [`Ledger`] keeps [`Ledger::seed_capital`] strictly apart from
//! [`Ledger::period_revenue`]: [`net_position`] and [`is_self_funding`] count only
//! earned revenue, so the books can never flatter the business into looking
//! self-funding when seed money is plugging the gap ([`relies_on_seed`] names that
//! exact case). Amounts are `$LH` wei (`u128`), matching `work_cycle`'s treasury /
//! reward / payout units, so this core composes with that one's
//! [`crate::work_cycle::compute_payout`].

/// A period snapshot of the company's books, in `$LH` wei. A "period" is one
/// accounting window (a heartbeat / cycle / day — the caller's choice); the
/// fields are that window's flows plus the live treasury.
///
/// **Seed is not revenue.** [`seed_capital`](Ledger::seed_capital) (redeem /
/// on-ramp / owner top-up) is held apart from
/// [`period_revenue`](Ledger::period_revenue) (earned from external paying
/// callers) so [`net_position`] and [`is_self_funding`] stay honest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ledger {
    /// `$LH` (wei) the treasury currently holds (the GuildFacet TBA balance).
    pub treasury: u128,
    /// `$LH` (wei) burned this period — inference, payroll, settled bounties.
    pub period_costs: u128,
    /// `$LH` (wei) EARNED this period from external paying callers (x402 settles,
    /// outside bounty payments). Does NOT include seed capital.
    pub period_revenue: u128,
    /// `$LH` (wei) injected as SEED this period (redeem / on-ramp / owner top-up).
    /// NOT revenue — kept separate so the books never look self-funding on
    /// borrowed money.
    pub seed_capital: u128,
}

/// Saturating signed difference `a - b` (never panics / wraps; clamps to the
/// `i128` range, which `$LH` wei never realistically reaches).
fn signed_diff(a: u128, b: u128) -> i128 {
    const CAP: u128 = i128::MAX as u128;
    if a >= b {
        (a - b).min(CAP) as i128
    } else {
        -((b - a).min(CAP) as i128)
    }
}

/// The honest bottom line for the period: **earned** revenue minus costs, SEED
/// EXCLUDED. Signed — negative is the normal early state (you burn inference
/// before outside callers pay), and that's the number Accounting reports up to
/// the Executive rather than the seed-flattered gross.
pub fn net_position(ledger: &Ledger) -> i128 {
    signed_diff(ledger.period_revenue, ledger.period_costs)
}

/// Total cash IN this period including seed (`revenue + seed_capital`, saturating)
/// — the gross figure to contrast against [`net_position`]: the gap between the
/// two is exactly how much seed propped the period up.
pub fn gross_inflow(ledger: &Ledger) -> u128 {
    ledger.period_revenue.saturating_add(ledger.seed_capital)
}

/// Can the treasury cover this period's accrued costs? `treasury >= period_costs`
/// — below that the company is running on fumes and a payout may bounce
/// (Accounting's "no surprise zero"). A zero-cost period is vacuously solvent.
pub fn is_solvent(ledger: &Ledger) -> bool {
    ledger.treasury >= ledger.period_costs
}

/// Does **earned** revenue alone cover the period's costs (no seed leaned on)?
/// This is the bar the business has to *earn* — `period_revenue >= period_costs`,
/// seed deliberately ignored. Vacuously true for a zero-cost period; pair it with
/// [`net_position`] for the magnitude and [`relies_on_seed`] for the honest
/// negative case.
pub fn is_self_funding(ledger: &Ledger) -> bool {
    ledger.period_revenue >= ledger.period_costs
}

/// The honest red flag: the period burned more than it earned AND seed capital was
/// injected to plug the gap (`costs > revenue && seed_capital > 0`). When this is
/// true the company is **not** self-funding regardless of how healthy the treasury
/// looks — say so out loud.
pub fn relies_on_seed(ledger: &Ledger) -> bool {
    ledger.period_costs > ledger.period_revenue && ledger.seed_capital > 0
}

/// How many more full cycles the treasury funds at `avg_burn_per_cycle` `$LH`/cycle
/// (floor — only complete, fundable cycles count). `None` when burn is **zero**:
/// runway is then unbounded / undefined, never a finite count. The result
/// saturates to `u64::MAX` for an astronomically over-funded treasury.
pub fn runway_cycles(treasury: u128, avg_burn_per_cycle: u128) -> Option<u64> {
    if avg_burn_per_cycle == 0 {
        return None; // zero burn ⇒ runway is effectively infinite
    }
    Some((treasury / avg_burn_per_cycle).min(u64::MAX as u128) as u64)
}

/// Total `$LH` revenue the company must take in to cover `calls` calls that each
/// cost `cost_per_call` to serve (`cost_per_call * calls`, saturating). The
/// break-even *target* for a metered service.
pub fn breakeven_revenue(cost_per_call: u128, calls: u64) -> u128 {
    cost_per_call.saturating_mul(calls as u128)
}

/// The per-call `$LH` price at which `calls` paid calls exactly recover
/// `total_cost` (the period's burn) — rounded UP so you never under-price. Charge
/// strictly above it to clear a margin (STRATEGY backlog #15: "advertise a per-call
/// price above inference cost"). `calls == 0` ⇒ `0`: no traffic to price against.
pub fn breakeven_price(total_cost: u128, calls: u64) -> u128 {
    if calls == 0 {
        return 0;
    }
    total_cost.div_ceil(calls as u128)
}

/// Signed per-call profit at `price_per_call` given `cost_per_call` (`price - cost`,
/// saturating). Positive ⇒ the call clears a margin; negative ⇒ it's served at a
/// loss. Drives the "is our advertised x402 price above inference cost?" check.
pub fn margin_per_call(price_per_call: u128, cost_per_call: u128) -> i128 {
    signed_diff(price_per_call, cost_per_call)
}

/// Total `$LH` of a payroll run (saturating sum) — what a single
/// `batch_send_lh` from the treasury would move, so Accounting can pre-check it
/// against [`is_solvent`] / the treasury before authorizing the batch.
pub fn payroll_total(amounts: &[u128]) -> u128 {
    amounts.iter().fold(0u128, |acc, &x| acc.saturating_add(x))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger(treasury: u128, costs: u128, revenue: u128, seed: u128) -> Ledger {
        Ledger { treasury, period_costs: costs, period_revenue: revenue, seed_capital: seed }
    }

    // --- net_position (signed, seed-excluded) -----------------------------

    #[test]
    fn net_position_is_earned_revenue_minus_costs() {
        assert_eq!(net_position(&ledger(0, 30, 50, 0)), 20); // earning above burn
        assert_eq!(net_position(&ledger(0, 50, 50, 0)), 0); // exactly break-even
        assert_eq!(net_position(&ledger(0, 80, 50, 0)), -30); // burning above earnings
    }

    #[test]
    fn net_position_excludes_seed_capital() {
        // A huge seed top-up does NOT turn a loss-making period positive — the
        // whole point of the seed/earned split.
        let l = ledger(1_000_000, 5, 0, 1_000_000);
        assert_eq!(net_position(&l), -5);
        // Seed never counts as earnings even when it dwarfs costs.
        let l = ledger(0, 10, 4, 999);
        assert_eq!(net_position(&l), -6);
    }

    #[test]
    fn net_position_saturates_instead_of_overflowing() {
        // Differences larger than i128::MAX clamp rather than wrap/panic.
        assert_eq!(net_position(&ledger(0, 0, u128::MAX, 0)), i128::MAX);
        assert_eq!(net_position(&ledger(0, u128::MAX, 0, 0)), i128::MIN + 1);
    }

    // --- gross_inflow -----------------------------------------------------

    #[test]
    fn gross_inflow_sums_revenue_and_seed() {
        assert_eq!(gross_inflow(&ledger(0, 0, 40, 60)), 100);
        // The gap to net_position is the seed contribution: gross 100 vs net -? .
        let l = ledger(0, 90, 40, 60);
        assert_eq!(gross_inflow(&l), 100);
        assert_eq!(net_position(&l), -50);
        assert_eq!(gross_inflow(&ledger(0, 0, u128::MAX, u128::MAX)), u128::MAX); // saturates
    }

    // --- solvency ---------------------------------------------------------

    #[test]
    fn solvency_edges_on_treasury_vs_costs() {
        assert!(is_solvent(&ledger(100, 100, 0, 0))); // exactly covers
        assert!(is_solvent(&ledger(101, 100, 0, 0))); // covers with room
        assert!(!is_solvent(&ledger(99, 100, 0, 0))); // one short → insolvent
        assert!(is_solvent(&ledger(0, 0, 0, 0))); // no costs ⇒ vacuously solvent
        assert!(!is_solvent(&ledger(0, 1, 0, 0))); // broke with a bill due
    }

    // --- self-funding vs seed reliance ------------------------------------

    #[test]
    fn self_funding_counts_only_earned_revenue() {
        assert!(is_self_funding(&ledger(0, 50, 50, 0))); // earns its keep exactly
        assert!(is_self_funding(&ledger(0, 50, 80, 0))); // net positive
        assert!(!is_self_funding(&ledger(0, 50, 49, 0))); // one short on earnings
        // Seed cannot buy self-funding: revenue still below costs.
        assert!(!is_self_funding(&ledger(1_000, 50, 10, 1_000)));
        assert!(is_self_funding(&ledger(0, 0, 0, 0))); // vacuous (no costs)
    }

    #[test]
    fn relies_on_seed_flags_the_seed_propped_period() {
        assert!(relies_on_seed(&ledger(0, 50, 10, 40))); // gap plugged by seed
        assert!(!relies_on_seed(&ledger(0, 50, 50, 40))); // earns its keep → not reliant
        assert!(!relies_on_seed(&ledger(0, 50, 10, 0))); // a loss, but no seed taken
        assert!(!relies_on_seed(&ledger(0, 10, 50, 5))); // profitable; seed irrelevant
    }

    // --- runway -----------------------------------------------------------

    #[test]
    fn runway_is_floor_division_of_treasury_by_burn() {
        assert_eq!(runway_cycles(100, 10), Some(10)); // exact
        assert_eq!(runway_cycles(105, 10), Some(10)); // floored, the partial cycle drops
        assert_eq!(runway_cycles(9, 10), Some(0)); // can't fund even one cycle
        assert_eq!(runway_cycles(0, 10), Some(0)); // broke
    }

    #[test]
    fn runway_zero_burn_is_none_not_finite() {
        assert_eq!(runway_cycles(100, 0), None); // infinite / undefined, not a count
        assert_eq!(runway_cycles(0, 0), None);
    }

    #[test]
    fn runway_saturates_to_u64_max() {
        assert_eq!(runway_cycles(u128::MAX, 1), Some(u64::MAX));
    }

    // --- break-even math --------------------------------------------------

    #[test]
    fn breakeven_revenue_is_cost_times_calls() {
        assert_eq!(breakeven_revenue(3, 10), 30);
        assert_eq!(breakeven_revenue(5, 0), 0); // no calls, nothing to recover
        assert_eq!(breakeven_revenue(u128::MAX, 2), u128::MAX); // saturates
    }

    #[test]
    fn breakeven_price_rounds_up_so_you_never_underprice() {
        assert_eq!(breakeven_price(100, 10), 10); // exact division
        assert_eq!(breakeven_price(100, 3), 34); // ceil(33.33) — round UP
        assert_eq!(breakeven_price(1, 4), 1); // any remainder ⇒ at least 1
        assert_eq!(breakeven_price(0, 4), 0); // no cost ⇒ free
        assert_eq!(breakeven_price(100, 0), 0); // no traffic to price against
    }

    #[test]
    fn margin_per_call_is_signed_price_minus_cost() {
        assert_eq!(margin_per_call(5, 3), 2); // priced above cost → profit
        assert_eq!(margin_per_call(3, 3), 0); // exactly at cost
        assert_eq!(margin_per_call(2, 3), -1); // underwater
        // A break-even-priced call clears a non-negative margin by construction.
        let cost = 7u128;
        let price = breakeven_price(cost, 1); // == ceil(cost/1) == cost
        assert!(margin_per_call(price, cost) >= 0);
    }

    // --- payroll ----------------------------------------------------------

    #[test]
    fn payroll_total_sums_a_batch() {
        assert_eq!(payroll_total(&[10, 20, 30]), 60);
        assert_eq!(payroll_total(&[]), 0); // empty run moves nothing
        assert_eq!(payroll_total(&[u128::MAX, 1]), u128::MAX); // saturates
    }

    #[test]
    fn payroll_pre_check_against_solvency() {
        // The intended composition: don't authorize a batch the treasury can't back.
        let l = ledger(50, 0, 0, 0);
        let run = [20u128, 20, 20];
        assert_eq!(payroll_total(&run), 60);
        assert!(payroll_total(&run) > l.treasury); // 60 > 50 → flag it, don't send
    }
}
