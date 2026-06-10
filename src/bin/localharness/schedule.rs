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
    let mut positional: Vec<String> = Vec::new();
    let mut every: Option<String> = None;
    let mut budget: Option<String> = None;
    let mut runs: Option<String> = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--every" => {
                every = Some(rest.get(i + 1).ok_or(SCHEDULE_USAGE)?.clone());
                i += 2;
            }
            "--budget" => {
                budget = Some(rest.get(i + 1).ok_or(SCHEDULE_USAGE)?.clone());
                i += 2;
            }
            "--runs" => {
                runs = Some(rest.get(i + 1).ok_or(SCHEDULE_USAGE)?.clone());
                i += 2;
            }
            _ => {
                positional.push(rest[i].clone());
                i += 1;
            }
        }
    }
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

/// `localharness schedule [--as <me>] <target> <task> --every <dur> --budget
/// <amount> [--runs <n>]` — escrow `$LH` to run `<target>` on a fixed interval,
/// on-chain (no tab needed). Resolves the target name → tokenId, escrows the
/// budget (approve + scheduleJob in one sponsored tx), and prints the schedule.
pub(crate) async fn schedule(caller_name: Option<&str>, rest: &[String]) -> i32 {
    let ParsedSchedule {
        target,
        task,
        interval_secs,
        budget_wei,
        max_runs,
    } = match parse_schedule_args(rest) {
        Ok(p) => p,
        Err(usage) => {
            eprintln!("{usage}");
            return 2;
        }
    };
    if task.trim().is_empty() {
        eprintln!("schedule: task is empty");
        return 2;
    }

    let (signer, sponsor) = match load_signer_and_sponsor(caller_name) {
        Ok(pair) => pair,
        Err(code) => return code,
    };

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
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            // The new job id is the last entry in the owner's jobsOf index.
            let addr = addr_to_hex(wallet::address(&signer));
            let id_note = match registry::jobs_of(&addr).await {
                Ok(ids) if !ids.is_empty() => format!("job #{}", ids[ids.len() - 1]),
                _ => "scheduled".to_string(),
            };
            println!("✓ {id_note}: {target} every {every}, budget {}, ~{max_runs} runs", fmt_lh(budget_wei));
            println!("  the escrowed $LH backs it 24/7 — it fires with no browser tab open.");
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("schedule failed: {e}");
            1
        }
    }
}

/// Render one job row for the `jobs` listing. Pure (no I/O) so the layout is
/// unit-testable: id, target name, cadence, next run, budget remaining, runs
/// left, status.
pub(crate) fn format_job_row(id: u64, target: &str, job: &registry::ScheduledJob, task: &str, now: u64) -> String {
    let next = if job.next_run == 0 {
        "—".to_string()
    } else if job.next_run <= now {
        "due now".to_string()
    } else {
        format!("in {}", fmt_interval(job.next_run - now))
    };
    let snippet: String = task.replace('\n', " ").chars().take(60).collect();
    format!(
        "  #{id}  {target}  every {interval}  next {next}  budget {budget}  runs-left {runs}  [{status}]\n      {snippet}",
        interval = fmt_interval(job.interval),
        budget = fmt_lh(job.budget_wei),
        runs = job.runs_left,
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
    let addr = addr_to_hex(wallet::address(&signer));
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
    match registry::cancel_job_sponsored(&signer, &sponsor, job_id, registry::ALPHA_USD_ADDRESS).await
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
