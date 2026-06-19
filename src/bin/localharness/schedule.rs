#[allow(unused_imports)]
use crate::*;

// ---- schedule / jobs / unschedule (ScheduleFacet) ------------------------
//
// Durable, tab-independent recurring jobs: ESCROW `$LH` to back an agent that
// runs on a fixed interval, on-chain, so the job + its budget survive any tab
// or process dying. `schedule` creates one (approve + scheduleJob in one
// sponsored tx), `jobs` lists the caller's, `unschedule` cancels one (refunds
// the remaining budget). Mirrors `registry::schedule_job_sponsored` etc.

/// Parsed `schedule` arguments. `--every`/`--budget` are required, `--runs`
/// defaults. Pure (no I/O) so it is unit-testable; `Err` carries the usage
/// line. Leading `--as <me>` is stripped by `take_as_flag` before this.
pub(crate) struct ParsedSchedule {
    target: String,
    task: String,
    interval_secs: u64,
    budget_wei: u128,
    max_runs: u32,
}

pub(crate) const SCHEDULE_USAGE: &str = "usage: localharness schedule [--as <me>] <target> <task> \
                              --every <dur> --budget <amount> [--runs <n>]\n  \
                              dur: 60s / 5m / 1h (min 60s)   amount: $LH (e.g. 1 or 0.5)";

pub(crate) const GOAL_USAGE: &str = "usage: localharness goal [--as <me>] <target> <goal text> \
                              --budget <amount> [--every <dur>] [--runs <n>]\n  \
                              defaults: --every 5m, --runs 100   dur: 60s / 5m / 1h (min 60s)";

/// The EXACT on-chain task marker the scheduler worker recognises as a goal
/// loop (ralph-on-chain): it wraps the run's persona with the goal-loop frame
/// and offers the `finish_goal` tool, which ends the job via the facet's
/// `completeJob` (refunding the unspent escrow) when the goal is met.
pub(crate) const GOAL_TASK_PREFIX: &str = "GOAL: ";

/// Default `--every` for `goal` — 5 minutes, the worker cron's MVP cadence
/// (a tighter loop than the typical standing job; the budget is the leash).
pub(crate) const GOAL_DEFAULT_INTERVAL_SECS: u64 = 300;

/// Whether a schedule/goal task is effectively empty: whitespace-only, or a
/// bare `GOAL: ` marker with no goal text behind it. An empty task escrows
/// real `$LH` behind a job that does nothing — rejected before any identity
/// or escrow work. Pure + testable.
pub(crate) fn task_is_blank(task: &str) -> bool {
    let t = task.trim();
    t.is_empty() || t == GOAL_TASK_PREFIX.trim()
}

/// Parse an interval like `60s` / `5m` / `1h` / `90` (bare = seconds) into
/// seconds, enforcing the facet's 60s floor. Pure + testable. A unit suffix of
/// `s`/`m`/`h` (case-insensitive) scales; anything else (or a sub-60s result,
/// or zero, or non-numeric) is an error so a bad cadence never reaches a tx.
pub(crate) fn parse_interval(raw: &str) -> Result<u64, String> {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        return Err("interval is empty".to_string());
    }
    let (num_part, mult) = match s.strip_suffix('s') {
        Some(n) => (n, 1u64),
        None => match s.strip_suffix('m') {
            Some(n) => (n, 60u64),
            None => match s.strip_suffix('h') {
                Some(n) => (n, 3600u64),
                None => (s.as_str(), 1u64), // bare number = seconds
            },
        },
    };
    let n: u64 = num_part
        .parse()
        .map_err(|_| format!("invalid interval '{raw}' (use 60s / 5m / 1h)"))?;
    let secs = n
        .checked_mul(mult)
        .ok_or_else(|| format!("interval '{raw}' overflows"))?;
    if secs < SCHEDULE_MIN_INTERVAL_SECS {
        return Err(format!(
            "interval '{raw}' is below the {SCHEDULE_MIN_INTERVAL_SECS}s minimum"
        ));
    }
    Ok(secs)
}

/// Render seconds back as a compact human duration (`90s`/`5m`/`2h`/`1h30m`).
/// Pure — used in the schedule confirmation + the `jobs` listing.
pub(crate) fn fmt_interval(secs: u64) -> String {
    if secs == 0 {
        return "0s".to_string();
    }
    if secs % 3600 == 0 {
        return format!("{}h", secs / 3600);
    }
    // An exact-minute span ≥ 1h reads better split into h+m than as raw minutes
    // (5400s → "1h30m", not "90m"); plain minutes for under an hour.
    if secs % 60 == 0 {
        return if secs > 3600 {
            format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
        } else {
            format!("{}m", secs / 60)
        };
    }
    format!("{secs}s")
}

pub(crate) fn parse_schedule_args(rest: &[String]) -> Result<ParsedSchedule, String> {
    let ([every, budget, runs], positional) =
        collect_flags(rest, ["--every", "--budget", "--runs"], SCHEDULE_USAGE)?;
    if positional.len() < 2 {
        return Err(SCHEDULE_USAGE.to_string());
    }
    let target = positional[0].clone();
    // Everything after the target joins into the task prompt (so an unquoted
    // multi-word task still works, matching `persona`/`call`).
    let task = positional[1..].join(" ");
    let interval_secs = parse_interval(&every.ok_or(SCHEDULE_USAGE)?)?;
    let budget_raw = budget.ok_or(SCHEDULE_USAGE)?;
    let budget_wei = match localharness::encoding::parse_token_amount(&budget_raw) {
        Some(w) if w > 0 => w,
        _ => return Err(format!("--budget must be a positive $LH amount, got '{budget_raw}'")),
    };
    let max_runs = match runs {
        None => SCHEDULE_DEFAULT_RUNS,
        Some(r) => r
            .parse::<u32>()
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| format!("--runs must be a positive integer, got '{r}'"))?,
    };
    Ok(ParsedSchedule {
        target,
        task,
        interval_secs,
        budget_wei,
        max_runs,
    })
}

/// Parsed `goal` arguments — `schedule` sugar with goal-loop ergonomics:
/// only `--budget` is required (`--every` defaults to 5m, `--runs` to the
/// schedule default), and the task is the goal text behind the exact
/// `GOAL: ` marker the worker keys the ralph frame + `finish_goal` tool on.
/// Pure (no I/O) so it is unit-testable; `Err` carries the usage line.
pub(crate) fn parse_goal_args(rest: &[String]) -> Result<ParsedSchedule, String> {
    let ([every, budget, runs], positional) =
        collect_flags(rest, ["--every", "--budget", "--runs"], GOAL_USAGE)?;
    if positional.len() < 2 {
        return Err(GOAL_USAGE.to_string());
    }
    let target = positional[0].clone();
    // Everything after the target joins into the goal text (unquoted
    // multi-word goals work, matching `schedule`/`call`).
    let goal_text = positional[1..].join(" ");
    let interval_secs = match every {
        None => GOAL_DEFAULT_INTERVAL_SECS,
        Some(e) => parse_interval(&e)?,
    };
    let budget_raw = budget.ok_or(GOAL_USAGE)?;
    let budget_wei = match localharness::encoding::parse_token_amount(&budget_raw) {
        Some(w) if w > 0 => w,
        _ => return Err(format!("--budget must be a positive $LH amount, got '{budget_raw}'")),
    };
    let max_runs = match runs {
        None => SCHEDULE_DEFAULT_RUNS,
        Some(r) => r
            .parse::<u32>()
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| format!("--runs must be a positive integer, got '{r}'"))?,
    };
    Ok(ParsedSchedule {
        target,
        task: format!("{GOAL_TASK_PREFIX}{goal_text}"),
        interval_secs,
        budget_wei,
        max_runs,
    })
}

/// `localharness schedule [--as <me>] <target> <task> --every <dur> --budget
/// <amount> [--runs <n>]` — escrow `$LH` to run `<target>` on a fixed interval,
/// on-chain (no tab needed). Resolves the target name → tokenId, escrows the
/// budget (approve + scheduleJob in one sponsored tx), and prints the schedule.
pub(crate) async fn schedule(caller_name: Option<&str>, rest: &[String]) -> i32 {
    match parse_schedule_args(rest) {
        Ok(p) => submit_job(caller_name, p, false).await,
        Err(usage) => {
            eprintln!("{usage}");
            2
        }
    }
}

/// `localharness goal [--as <me>] <target> <goal text> --budget <amount>
/// [--every <dur>] [--runs <n>]` — ralph-on-chain: schedule a recurring job
/// whose task carries the `GOAL: ` marker. Every fire re-feeds the SAME goal
/// to the agent (progress lives on-chain, not in model memory); the job ends
/// ITSELF — `finish_goal` → the facet's `completeJob`, refunding the unspent
/// escrow to you — once the agent verifies the goal is complete. The budget
/// and `--runs` remain the hard stops if it never is.
pub(crate) async fn goal(caller_name: Option<&str>, rest: &[String]) -> i32 {
    match parse_goal_args(rest) {
        Ok(p) => submit_job(caller_name, p, true).await,
        Err(usage) => {
            eprintln!("{usage}");
            2
        }
    }
}

/// Shared submission path for `schedule` + `goal`: resolve the target name →
/// tokenId, escrow the budget (approve + scheduleJob in one sponsored tx),
/// print the schedule. `goal_mode` only changes the confirmation copy (the
/// on-chain difference is entirely the task's `GOAL: ` marker).
async fn submit_job(caller_name: Option<&str>, parsed: ParsedSchedule, goal_mode: bool) -> i32 {
    let ParsedSchedule {
        target,
        task,
        interval_secs,
        budget_wei,
        max_runs,
    } = parsed;
    // An empty / whitespace-only task escrowed real $LH behind a no-op job —
    // reject it BEFORE any identity/escrow work (same guard as call/mcp-call).
    if task_is_blank(&task) {
        let label = if goal_mode { "goal: goal text" } else { "schedule: task" };
        eprintln!("{label} is empty — nothing to send");
        return 1;
    }

    let (signer, sponsor) = match load_signer_and_sponsor(caller_name) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    // The escrow pulls the budget from the WALLET pot — auto-bridge any
    // shortfall out of the chat meter first (on-chain feedback #63).
    let from_hex = bytes_to_hex_str(&wallet::address(&signer));
    if let Err(code) = ensure_wallet_covers(&signer, &from_hex, budget_wei).await {
        return code;
    }

    // Resolve the target agent's tokenId (the facet rejects an unregistered
    // target with `UnregisteredTarget`, so fail early with a clear message).
    let target_id = match registry::id_of_name(&target).await {
        Ok(id) if id != 0 => id,
        Ok(_) => {
            eprintln!("schedule: '{target}' is not a registered agent");
            return 1;
        }
        Err(e) => {
            eprintln!("schedule: RPC error resolving '{target}': {e}");
            return 1;
        }
    };

    let every = fmt_interval(interval_secs);
    println!(
        "scheduling {target} every {every}, budget {}, up to {max_runs} run(s) …",
        fmt_lh(budget_wei)
    );
    match registry::schedule_job_sponsored(
        &signer,
        &sponsor,
        target_id,
        task.as_bytes(),
        interval_secs,
        budget_wei,
        max_runs,
        registry::ALPHA_USD_ADDRESS(),
    )
    .await
    {
        Ok(tx) => {
            // The new job id is the last entry in the owner's jobsOf index.
            let addr = bytes_to_hex_str(&wallet::address(&signer));
            let id_note = match registry::jobs_of(&addr).await {
                Ok(ids) if !ids.is_empty() => format!("job #{}", ids[ids.len() - 1]),
                _ => "scheduled".to_string(),
            };
            println!("✓ {id_note}: {target} every {every}, budget {}, ~{max_runs} runs", fmt_lh(budget_wei));
            if goal_mode {
                println!("  goal loop: each fire re-feeds the goal and the agent takes ONE step;");
                println!("  the job SELF-CANCELS (refunding the unspent budget to your wallet) when");
                println!("  the agent declares the goal complete — budget/runs are the hard stops.");
            } else {
                println!("  the escrowed $LH backs it 24/7 — it fires with no browser tab open.");
            }
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("schedule failed: {e}");
            1
        }
    }
}

/// True for a TERMINAL job status (Cancelled / Exhausted): no further fire is
/// scheduled, so the row must not advertise a "next due" time. Pure + testable.
pub(crate) fn job_is_terminal(status: u8) -> bool {
    matches!(status, 2 | 3)
}

/// Render one job row for the `jobs` listing. Pure (no I/O) so the layout is
/// unit-testable: id, target name, cadence, next run, budget remaining, runs
/// left, status. A TERMINAL job (cancelled / exhausted) prints no "next" time —
/// the old row showed "next due now" for a cancelled job that will never fire
/// again, and the runs-left/budget of a dead job is noise, so both collapse to
/// "—" (on-chain feedback #82).
pub(crate) fn format_job_row(id: u64, target: &str, job: &registry::ScheduledJob, task: &str, now: u64) -> String {
    let terminal = job_is_terminal(job.status);
    let next = if terminal || job.next_run == 0 {
        "—".to_string()
    } else if job.next_run <= now {
        "due now".to_string()
    } else {
        format!("in {}", fmt_interval(job.next_run - now))
    };
    // A live job shows its remaining runs + escrow; a terminal one shows neither
    // (the budget refunded on cancel, the runs spent on exhaust).
    let runs = if terminal { "—".to_string() } else { job.runs_left.to_string() };
    let budget = if terminal { "—".to_string() } else { fmt_lh(job.budget_wei) };
    let snippet = truncate_words(task, 60);
    format!(
        "  #{id}  {target}  every {interval}  next {next}  budget {budget}  runs-left {runs}  [{status}]\n      {snippet}",
        interval = fmt_interval(job.interval),
        status = job.status_label(),
    )
}

/// `localharness jobs [--as <me>]` — list the caller's scheduled jobs
/// (`jobsOf` + a `getJob`/`taskOf` per id). Read-only, no `$LH`.
pub(crate) async fn list_jobs(caller_name: Option<&str>) -> i32 {
    let signer = match load_signer(caller_name) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));
    let ids = match registry::jobs_of(&addr).await {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    if ids.is_empty() {
        println!("no scheduled jobs for {addr}");
        return 0;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("{} scheduled job(s) for {addr}:", ids.len());
    for id in ids {
        let job = match registry::get_job(id).await {
            Ok(j) => j,
            Err(e) => {
                println!("  #{id}  (could not read: {e})");
                continue;
            }
        };
        // Resolve the target's name for readability; fall back to the id.
        let target = registry::name_of_id(job.target_id)
            .await
            .ok()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("token#{}", job.target_id));
        let task = registry::task_of(id).await.unwrap_or_default();
        println!("{}", format_job_row(id, &target, &job, &task, now));
        // #52: surface the LAST run so the owner can tell a fired job from a
        // silently-skipped one (nextRun in the past + no last-run = never fired).
        if let Ok((ts, status)) = registry::last_run_of(id).await {
            if ts == 0 {
                println!("    last run: — (not yet run)");
            } else {
                let ago = now.saturating_sub(ts);
                let post = match status {
                    0 => "active",
                    3 => "exhausted",
                    _ => "ran",
                };
                println!("    last run: {ago}s ago [{post}]");
            }
        }
    }
    0
}

/// `localharness unschedule [--as <me>] <jobId>` — cancel a scheduled job;
/// the facet refunds the remaining escrowed `$LH` to the owner.
pub(crate) async fn unschedule(caller_name: Option<&str>, job_id_arg: &str) -> i32 {
    let job_id: u64 = match job_id_arg.trim().trim_start_matches('#').parse() {
        Ok(n) => n,
        Err(_) => {
            eprintln!("unschedule: '{job_id_arg}' is not a job id (a number, e.g. 3)");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller_name) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    match registry::cancel_job_sponsored(&signer, &sponsor, job_id, registry::ALPHA_USD_ADDRESS()).await
    {
        Ok(tx) => {
            println!("✓ cancelled job #{job_id} — remaining budget refunded to your wallet");
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("unschedule failed: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_is_blank_catches_empty_and_bare_goal_marker() {
        // Whitespace-only tasks must never reach an escrow tx.
        assert!(task_is_blank(""));
        assert!(task_is_blank("   \t"));
        // A goal with no text behind the marker is blank too: `goal t ""`
        // parses to exactly "GOAL: ".
        assert!(task_is_blank(GOAL_TASK_PREFIX));
        assert!(task_is_blank("  GOAL:  "));
        // Real tasks pass.
        assert!(!task_is_blank("check the price"));
        assert!(!task_is_blank("GOAL: win"));
    }

    #[test]
    fn parse_interval_units_and_floor() {
        // Suffix units scale to seconds.
        assert_eq!(parse_interval("60s"), Ok(60));
        assert_eq!(parse_interval("5m"), Ok(300));
        assert_eq!(parse_interval("1h"), Ok(3600));
        assert_eq!(parse_interval("2h"), Ok(7200));
        // Bare number = seconds; case + whitespace tolerant.
        assert_eq!(parse_interval(" 90 "), Ok(90));
        assert_eq!(parse_interval("5M"), Ok(300));
        // Below the 60s minimum is rejected (the facet reverts on it).
        assert!(parse_interval("59s").is_err());
        assert!(parse_interval("0m").is_err());
        assert!(parse_interval("30").is_err());
        // Non-numeric / empty / overflow are errors, never a tx.
        assert!(parse_interval("abc").is_err());
        assert!(parse_interval("").is_err());
        assert!(parse_interval("m").is_err());
        assert!(parse_interval(&format!("{}h", u64::MAX)).is_err());
    }

    #[test]
    fn fmt_interval_compact() {
        assert_eq!(fmt_interval(60), "1m");
        assert_eq!(fmt_interval(300), "5m");
        assert_eq!(fmt_interval(3600), "1h");
        assert_eq!(fmt_interval(90), "90s");
        assert_eq!(fmt_interval(5400), "1h30m");
        assert_eq!(fmt_interval(0), "0s");
    }

    #[test]
    fn parse_schedule_args_full_and_defaults() {
        let p = parse_schedule_args(&args(&[
            "oracle", "check", "the", "price", "--every", "5m", "--budget", "1", "--runs", "50",
        ]))
        .unwrap();
        assert_eq!(p.target, "oracle");
        assert_eq!(p.task, "check the price"); // joined multi-word task
        assert_eq!(p.interval_secs, 300);
        assert_eq!(p.budget_wei, 1_000_000_000_000_000_000); // 1 $LH in wei
        assert_eq!(p.max_runs, 50);

        // --runs defaults; flags may precede the task; fractional budget.
        let p = parse_schedule_args(&args(&[
            "bot", "--every", "1h", "--budget", "0.5", "ping",
        ]))
        .unwrap();
        assert_eq!(p.target, "bot");
        assert_eq!(p.task, "ping");
        assert_eq!(p.interval_secs, 3600);
        assert_eq!(p.budget_wei, 500_000_000_000_000_000); // 0.5 $LH
        assert_eq!(p.max_runs, SCHEDULE_DEFAULT_RUNS);
    }

    #[test]
    fn parse_schedule_args_rejects_bad_input() {
        // Missing required flags.
        assert!(parse_schedule_args(&args(&["t", "task"])).is_err());
        assert!(parse_schedule_args(&args(&["t", "task", "--every", "5m"])).is_err());
        // No task (only the target positional).
        assert!(parse_schedule_args(&args(&["t", "--every", "5m", "--budget", "1"])).is_err());
        // Zero / non-numeric budget + bad runs.
        assert!(parse_schedule_args(&args(&["t", "x", "--every", "5m", "--budget", "0"])).is_err());
        assert!(parse_schedule_args(&args(&["t", "x", "--every", "5m", "--budget", "nope"])).is_err());
        assert!(
            parse_schedule_args(&args(&["t", "x", "--every", "5m", "--budget", "1", "--runs", "0"]))
                .is_err()
        );
        // Sub-minute interval bubbles up from parse_interval.
        assert!(parse_schedule_args(&args(&["t", "x", "--every", "10s", "--budget", "1"])).is_err());
    }

    #[test]
    fn parse_goal_args_defaults_and_marker() {
        // Only --budget is required: --every defaults to 5m, --runs to the
        // schedule default, and the task gains the EXACT worker marker.
        let p = parse_goal_args(&args(&[
            "claude", "get", "my", "TBA", "to", "1", "$LH", "--budget", "0.5",
        ]))
        .unwrap();
        assert_eq!(p.target, "claude");
        assert_eq!(p.task, "GOAL: get my TBA to 1 $LH"); // marker + joined text
        assert!(p.task.starts_with(GOAL_TASK_PREFIX));
        assert_eq!(p.interval_secs, GOAL_DEFAULT_INTERVAL_SECS); // 5m default
        assert_eq!(p.budget_wei, 500_000_000_000_000_000); // 0.5 $LH
        assert_eq!(p.max_runs, SCHEDULE_DEFAULT_RUNS); // 100 default
    }

    #[test]
    fn parse_goal_args_explicit_flags() {
        // Explicit --every/--runs override the defaults; flags may precede
        // the goal text (collect_flags order-independence, like schedule).
        let p = parse_goal_args(&args(&[
            "bot", "--every", "1h", "--budget", "2", "--runs", "10", "win",
        ]))
        .unwrap();
        assert_eq!(p.target, "bot");
        assert_eq!(p.task, "GOAL: win");
        assert_eq!(p.interval_secs, 3600);
        assert_eq!(p.budget_wei, 2_000_000_000_000_000_000);
        assert_eq!(p.max_runs, 10);
    }

    #[test]
    fn parse_goal_args_rejects_bad_input() {
        // --budget is required.
        assert!(parse_goal_args(&args(&["t", "goal"])).is_err());
        // No goal text (only the target positional).
        assert!(parse_goal_args(&args(&["t", "--budget", "1"])).is_err());
        // Zero / non-numeric budget + bad runs.
        assert!(parse_goal_args(&args(&["t", "x", "--budget", "0"])).is_err());
        assert!(parse_goal_args(&args(&["t", "x", "--budget", "nope"])).is_err());
        assert!(parse_goal_args(&args(&["t", "x", "--budget", "1", "--runs", "0"])).is_err());
        // A sub-minute --every bubbles up from parse_interval.
        assert!(parse_goal_args(&args(&["t", "x", "--budget", "1", "--every", "10s"])).is_err());
    }

    #[test]
    fn format_job_row_contains_key_fields() {
        let job = registry::ScheduledJob {
            owner: "0xowner".into(),
            interval: 300,
            status: 0,
            next_run: 1_000 + 120, // 2m out from `now`
            budget_wei: 1_000_000_000_000_000_000,
            runs_left: 42,
            target_id: 7,
        };
        let row = format_job_row(3, "oracle", &job, "check\nthe price", 1_000);
        assert!(row.contains("#3"));
        assert!(row.contains("oracle"));
        assert!(row.contains("every 5m"));
        assert!(row.contains("next in 2m"));
        assert!(row.contains("runs-left 42"));
        assert!(row.contains("[active]"));
        assert!(row.contains("check the price")); // newline flattened
    }

    #[test]
    fn job_is_terminal_flags_cancelled_and_exhausted() {
        assert!(!job_is_terminal(0)); // active
        assert!(!job_is_terminal(1)); // paused
        assert!(job_is_terminal(2)); // cancelled
        assert!(job_is_terminal(3)); // exhausted
    }

    #[test]
    fn format_job_row_cancelled_does_not_advertise_next_due() {
        // The bug: a cancelled job whose next_run is a stale past timestamp
        // printed "next due now" — it will NEVER fire again. Terminal jobs show
        // "next —" + collapse budget/runs to "—".
        let job = registry::ScheduledJob {
            owner: "0x0".into(),
            interval: 300,
            status: 2,       // cancelled
            next_run: 100,   // stale past timestamp (not zeroed)
            budget_wei: 1_000_000_000_000_000_000,
            runs_left: 5,
            target_id: 1,
        };
        let row = format_job_row(7, "bot", &job, "", 5_000);
        assert!(row.contains("next —"), "cancelled job must not say due now: {row}");
        assert!(!row.contains("due now"));
        assert!(row.contains("[cancelled]"));
        assert!(row.contains("runs-left —"), "terminal runs collapse to —: {row}");
        assert!(row.contains("budget —"));
    }

    #[test]
    fn format_job_row_terminal_and_due() {
        // next_run == 0 (terminal) → em-dash; status label flows through.
        let job = registry::ScheduledJob {
            owner: "0x0".into(),
            interval: 60,
            status: 3,
            next_run: 0,
            budget_wei: 0,
            runs_left: 0,
            target_id: 1,
        };
        let row = format_job_row(1, "bot", &job, "", 5_000);
        assert!(row.contains("next —"));
        assert!(row.contains("[exhausted]"));
        // Due-now: next_run in the past.
        let mut due = job.clone();
        due.status = 0;
        due.next_run = 100;
        let row = format_job_row(2, "bot", &due, "", 5_000);
        assert!(row.contains("next due now"));
    }
}
