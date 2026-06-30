//! Pure PREVIEW of localharness's autonomous-business decision cores — the logic
//! a self-sovereign agent "company" uses to staff a role, run one
//! claim → work → judge → pay → attest work cycle, and keep honest books.
//!
//! Unlike `basic_agent` / `tempo_tx_live` / `create_subagent_live`, this example
//! touches NO chain, NO network, NO wallet, and needs NO API key. Every core it
//! exercises — `work_cycle`, `work_cycle_runtime`, `accounting`, `hiring` — is a
//! pure function over data in the DEFAULT build, so the whole demo is a
//! deterministic dry run: it DECIDES and PRINTS what the company *would* do, and
//! executes / signs / transfers absolutely nothing. The on-chain executor that
//! maps each planned action onto its sponsored `registry` edge call is deferred
//! and greenlight-gated; this file is the inspectable preview that comes first.
//!
//! Run:
//!
//! ```sh
//! cargo run --example autonomous_company
//! ```

use localharness::accounting::{self, Ledger};
use localharness::hiring::{best_candidate, rank_candidates, Candidate, RoleNeed};
use localharness::work_cycle::{Backlog, Criteria, Role, Stage, Submission, Task, WorkerState};
use localharness::work_cycle_runtime::{plan_cycle, Reader};

/// In-memory [`Reader`] for the demo — the pure stand-in for the deferred
/// on-chain diamond reader (live `BountyFacet` tasks + role-agent reputations +
/// the treasury `$LH` balance). Holds the three reads as plain fields; the real
/// impl belongs to the greenlight-gated executor, never this preview.
struct DemoReader {
    tasks: Vec<Task>,
    workers: Vec<WorkerState>,
    treasury: u128,
}

impl Reader for DemoReader {
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

/// A short label for a role (Debug also works, but this keeps columns tidy).
fn role_name(r: Role) -> &'static str {
    match r {
        Role::Executive => "Executive",
        Role::ProductManager => "ProductManager",
        Role::Coder => "Coder",
        Role::Reviewer => "Reviewer",
        Role::Accounting => "Accounting",
        Role::Hr => "HR",
        Role::Marketing => "Marketing",
    }
}

/// A fresh, fundable backlog task (`Planned` — reward not yet escrowed).
fn planned(id: u64, role: Role, reward: u128, min_rep: u32, min_quality: u8) -> Task {
    Task {
        id,
        role,
        reward,
        min_reputation: min_rep,
        criteria: Criteria { min_quality },
        stage: Stage::Planned,
    }
}

fn main() {
    println!("=== localharness · autonomous-company decision-core PREVIEW ===");
    println!("(pure dry run — no chain, no network, no keys; nothing is executed)\n");

    // --- 1. The company: a roster of role-agents + a funded backlog ---------
    // Each worker is an identity tokenId filling a business role, with proven
    // on-chain reputation. They feed BOTH the HR ranking and the work cycle.
    let workers = vec![
        WorkerState { id: 7, role: Role::Coder, reputation: 2, available: true },
        WorkerState { id: 8, role: Role::Coder, reputation: 9, available: true },
        WorkerState { id: 11, role: Role::Reviewer, reputation: 4, available: true },
        WorkerState { id: 5, role: Role::Marketing, reputation: 1, available: true },
    ];

    let backlog = Backlog {
        tasks: vec![
            // Already delivered on-chain — reads back ready for the Reviewer to
            // judge: a 5-star Coder result against a 3-star bar.
            Task {
                id: 1,
                role: Role::Coder,
                reward: 30,
                min_reputation: 0,
                criteria: Criteria { min_quality: 3 },
                stage: Stage::Submitted {
                    worker_id: 7,
                    submission: Submission { quality: 5, claims_impossible: false },
                },
            },
            // Fresh work waiting in the backlog.
            planned(2, Role::Coder, 50, 0, 3),
            planned(3, Role::Reviewer, 20, 0, 3),
        ],
    };
    let treasury: u128 = 100;

    println!("--- 1. The company (roster + funded backlog) ---");
    println!("Roster ({} role-agents):", workers.len());
    for w in &workers {
        println!(
            "  #{:<2} {:<10} rep {}  {}",
            w.id,
            role_name(w.role),
            w.reputation,
            if w.available { "free" } else { "busy" }
        );
    }
    println!("\nBacklog ({} tasks), treasury {} $LH wei:", backlog.tasks.len(), treasury);
    for t in &backlog.tasks {
        println!(
            "  task {} · {} · reward {} · min_rep {} · bar {}*  [{:?}]",
            t.id,
            role_name(t.role),
            t.reward,
            t.min_reputation,
            t.criteria.min_quality,
            t.stage,
        );
    }

    // --- 2. HR · who staffs an open seat? -----------------------------------
    // A WorkerState IS a hiring Candidate (From impl), so the same roster ranks
    // directly. HR ranks on proven reputation, lowest-id on a tie — identical to
    // the work cycle's own allocation preference.
    println!("\n--- 2. HR · who staffs the open Coder seat? ---");
    let pool: Vec<Candidate> = workers.iter().copied().map(Candidate::from).collect();
    let need = RoleNeed { role: Role::Coder, min_reputation: 0 };
    let ranking = rank_candidates(&need, &pool);
    println!("RoleNeed {{ role: Coder, min_reputation: 0 }} — eligible, best-first:");
    for (i, r) in ranking.iter().enumerate() {
        println!("  {}. agent #{}  (fit score {})", i + 1, r.id, r.score);
    }
    match best_candidate(&need, &pool) {
        Some(best) => println!("=> HR would assign the seat to agent #{} (highest proven rep).", best.id),
        None => println!("=> no eligible candidate."),
    }
    // Raising the bar narrows the field — only proven agents qualify.
    let senior = RoleNeed { role: Role::Coder, min_reputation: 5 };
    let coders = pool.iter().filter(|c| c.role == Role::Coder).count();
    println!(
        "Raise the bar to min_reputation 5: {} of {} coders still eligible.",
        rank_candidates(&senior, &pool).len(),
        coders,
    );

    // --- 3. Work cycle · PREVIEW the next cycle's actions --------------------
    // plan_cycle reads the company through the Reader, runs work_cycle::step to
    // quiescence, and returns the Action descriptors it WOULD emit — pure data,
    // nothing signed or broadcast.
    println!("\n--- 3. Work cycle · preview the next cycle's actions ---");
    let reader = DemoReader { tasks: backlog.tasks.clone(), workers: workers.clone(), treasury };
    let plan = plan_cycle(&reader, 100);
    println!("{}", plan.summary);
    println!("Actions the cycle WOULD emit ({}) — descriptors only, nothing executed:", plan.actions.len());
    for (i, a) in plan.actions.iter().enumerate() {
        println!("  {:>2}. {:?}", i + 1, a);
    }
    println!(
        "Projected treasury: {} -> {} $LH wei (debited only by the accepted payout)",
        plan.state_before.treasury, plan.state_after.treasury,
    );
    // Re-planning the resulting board proves the loop terminates (only
    // undelivered, assigned work remains — a preview can't fabricate delivery).
    let settled = DemoReader {
        tasks: plan.state_after.backlog.tasks.clone(),
        workers: plan.state_after.workers.clone(),
        treasury: plan.state_after.treasury,
    };
    println!("Re-planning the resulting board is quiescent (loop terminates): {}", plan_cycle(&settled, 100).is_quiescent());

    // --- 4. Accounting · the honest books (seed vs earned) ------------------
    // Seed capital is kept strictly apart from earned revenue so the books can
    // never flatter a loss-making period into looking self-funding.
    println!("\n--- 4. Accounting · honest books (seed vs earned) ---");
    let ledger = Ledger {
        treasury: plan.state_after.treasury, // carry the projected balance through
        period_costs: 35,                    // inference burn + the 30 $LH payout
        period_revenue: 12,                  // EARNED from external paying callers
        seed_capital: 100,                   // redeem / on-ramp / owner top-up (NOT revenue)
    };
    println!(
        "Ledger: treasury {} · costs {} · earned {} · seed {} ($LH wei)",
        ledger.treasury, ledger.period_costs, ledger.period_revenue, ledger.seed_capital,
    );
    println!("net_position (earned - costs, seed EXCLUDED): {}", accounting::net_position(&ledger));
    println!("gross_inflow (earned + seed):                 {}", accounting::gross_inflow(&ledger));
    println!("solvent?      (treasury >= costs):            {}", accounting::is_solvent(&ledger));
    println!("self-funding? (earned   >= costs):            {}", accounting::is_self_funding(&ledger));
    println!("relies on seed? (loss plugged by seed):       {}", accounting::relies_on_seed(&ledger));

    let burn_per_cycle = ledger.period_costs;
    match accounting::runway_cycles(ledger.treasury, burn_per_cycle) {
        Some(n) => println!("runway: {} more full cycle(s) at {} $LH/cycle", n, burn_per_cycle),
        None => println!("runway: unbounded (zero burn)"),
    }

    let calls = 12u64;
    let cost_per_call = 3u128;
    println!(
        "break-even over {} paid calls: need {} $LH revenue; price >= {} $LH/call to recover the period",
        calls,
        accounting::breakeven_revenue(cost_per_call, calls),
        accounting::breakeven_price(ledger.period_costs, calls),
    );
    let price = 5u128;
    println!(
        "at {} $LH/call vs {} $LH inference cost: margin {} $LH/call",
        price,
        cost_per_call,
        accounting::margin_per_call(price, cost_per_call),
    );

    // The honest read, stated out loud (STRATEGY: never flatter the books).
    if accounting::relies_on_seed(&ledger) {
        println!("\nHONEST READ: this period burned more than it EARNED; seed capital plugged");
        println!("the gap. The treasury looks healthy, but the company is NOT yet self-funding");
        println!("— it must land paying callers priced above inference cost.");
    } else if accounting::is_self_funding(&ledger) {
        println!("\nHONEST READ: earned revenue covered costs — self-funding this period.");
    }

    println!("\n=== preview complete · nothing was signed, sent, or charged ===");
}
