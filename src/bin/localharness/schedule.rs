use crate::{bytes_to_hex_str, collect_flags, load_signer, registry, truncate_words, wallet, SCHEDULE_DEFAULT_RUNS, SCHEDULE_MIN_INTERVAL_SECS};

// ---- schedule / goal / remind / jobs / unschedule ------------------------
//
// Durable, tab-independent jobs. NEW jobs are OFF-CHAIN (proxy GitHub store,
// fired by the cron): `schedule`/`goal` create an AGENT job (run a target each
// fire, billed per run from the caller's meter — no escrow), `remind` a free
// reminder push, all via `registry::create_offchain_job`. `jobs` lists off-chain
// + legacy on-chain; `unschedule` routes by id shape (uuid → off-chain,
// numeric → legacy on-chain ScheduleFacet `cancelJob`).

/// Parsed `schedule` arguments. `--every` is required, `--runs` defaults; a
/// `--budget` is a hard error now (off-chain jobs bill the meter, no escrow).
/// Pure (no I/O) so it is unit-testable; `Err` carries the usage / error line.
/// Leading `--as <me>` is stripped by `take_as_flag` before this.
#[derive(Debug)]
pub(crate) struct ParsedSchedule {
    target: String,
    task: String,
    interval_secs: u64,
    max_runs: u32,
}

pub(crate) const SCHEDULE_USAGE: &str = "usage: localharness schedule [--as <me>] <target> <task> \
                              --every <dur> [--runs <n>]\n  \
                              dur: 60s / 5m / 1h (min 60s).  Runs OFF-CHAIN, billed per run from \
                              your meter (no escrow).";

pub(crate) const GOAL_USAGE: &str = "usage: localharness goal [--as <me>] <target> <goal text> \
                              [--every <dur>] [--runs <n>]\n  \
                              defaults: --every 5m, --runs 100   dur: 60s / 5m / 1h (min 60s).  \
                              Off-chain, billed per run from your meter; the first \
                              fire lands one full interval after creation.";

/// The hard error when `--budget` is passed to `schedule`/`goal`: those jobs are
/// OFF-CHAIN now and bill the meter per run, so an upfront `$LH` escrow no longer
/// exists. A clean break (the user chose error-over-ignore).
pub(crate) const BUDGET_REMOVED: &str = "--budget is no longer used: scheduled agent jobs run \
    OFF-CHAIN now and bill your meter per run (~1 $LH/model call) — there is no upfront escrow. \
    Remove --budget and re-run.";

/// The EXACT task marker the scheduler worker recognises as a goal loop
/// (ralph): it wraps the run's persona with the goal-loop frame and offers the
/// `finish_goal` tool, which cancels the off-chain job when the goal is met.
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
    // `--budget` stays in the flag set so `--budget X` is CAPTURED (not silently
    // swallowed into the task positional) and we can hard-error on it.
    let ([every, budget, runs], positional) =
        collect_flags(rest, ["--every", "--budget", "--runs"], SCHEDULE_USAGE)?;
    if budget.is_some() {
        return Err(BUDGET_REMOVED.to_string());
    }
    if positional.len() < 2 {
        return Err(SCHEDULE_USAGE.to_string());
    }
    let target = positional[0].clone();
    // Everything after the target joins into the task prompt (so an unquoted
    // multi-word task still works, matching `persona`/`call`).
    let task = positional[1..].join(" ");
    let interval_secs = parse_interval(&every.ok_or(SCHEDULE_USAGE)?)?;
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
    if budget.is_some() {
        return Err(BUDGET_REMOVED.to_string());
    }
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
        max_runs,
    })
}

/// `localharness schedule [--as <me>] <target> <task> --every <dur> [--runs <n>]`
/// — run `<target>` on a fixed interval OFF-CHAIN (no tab needed), billed per run
/// from your meter (no escrow). Submits an off-chain agent job via the proxy.
pub(crate) async fn schedule(caller_name: Option<&str>, rest: &[String]) -> i32 {
    match parse_schedule_args(rest) {
        Ok(p) => submit_job(caller_name, p, false).await,
        Err(usage) => {
            eprintln!("{usage}");
            2
        }
    }
}

/// `localharness goal [--as <me>] <target> <goal text> [--every <dur>] [--runs
/// <n>]` — ralph: schedule an OFF-CHAIN agent job whose task carries the `GOAL: `
/// marker. Every fire re-feeds the SAME goal to the agent (no model memory across
/// fires); the job ends ITSELF when the agent calls `finish_goal`. `--runs` is the
/// hard stop if it never does; each fire bills your meter (no escrow/refund).
pub(crate) async fn goal(caller_name: Option<&str>, rest: &[String]) -> i32 {
    match parse_goal_args(rest) {
        Ok(p) => submit_job(caller_name, p, true).await,
        Err(usage) => {
            eprintln!("{usage}");
            2
        }
    }
}

/// Current UNIX seconds (native — the off-chain client takes `now` so it stays
/// cross-target; the browser passes `js_sys::Date::now()`).
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(crate) const REMIND_USAGE: &str = "usage: localharness remind [--as <me>] <text> --in <dur> [--runs <n>]\n  \
                              dur: 60s / 15m / 1h (min 60s);  --runs N repeats it (default 1 = one-shot).\n  \
                              Fires OFF-CHAIN (free, no $LH) and web-pushes you — enable notifications in \
                              the browser app first to receive it.";

/// `localharness remind [--as <me>] <text> --in <dur> [--runs <n>]` — schedule a
/// tab-free REMINDER that web-pushes you at the due time. OFF-CHAIN (proxy GitHub
/// store), so it's FREE — no `$LH`, no gas, no escrow (unlike `schedule`/`goal`,
/// which escrow $LH on-chain to RUN an agent). The CLI twin of the browser's
/// `schedule_task` reminder. Cancel with `unschedule <id>`.
pub(crate) async fn remind(caller_name: Option<&str>, rest: &[String]) -> i32 {
    let ([in_dur, runs_arg], positional) =
        match collect_flags(rest, ["--in", "--runs"], REMIND_USAGE) {
            Ok(v) => v,
            Err(u) => {
                eprintln!("{u}");
                return 2;
            }
        };
    if positional.is_empty() {
        eprintln!("{REMIND_USAGE}");
        return 2;
    }
    let task = positional.join(" ");
    let interval_secs = match in_dur {
        Some(d) => match parse_interval(&d) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{e}");
                return 2;
            }
        },
        None => {
            eprintln!("{REMIND_USAGE}");
            return 2;
        }
    };
    let runs = match runs_arg {
        None => 1u32,
        Some(r) => match r.parse::<u32>().ok().filter(|&n| n > 0) {
            Some(n) => n,
            None => {
                eprintln!("--runs must be a positive integer, got '{r}'");
                return 2;
            }
        },
    };
    let signer = match load_signer(caller_name) {
        Ok(s) => s,
        Err(code) => return code,
    };
    println!("scheduling a reminder in {} (×{runs}) …", fmt_interval(interval_secs));
    match registry::create_offchain_job(&signer, now_unix(), "reminder", "", &task, interval_secs, runs)
        .await
    {
        Ok(id) => {
            println!("✓ reminder scheduled — job {id} (off-chain, free; may take a few seconds to appear in `jobs`)");
            println!("  it web-pushes you at the due time (enable notifications in the browser app to receive it).");
            println!("  cancel: localharness unschedule {id}");
            0
        }
        Err(e) => {
            eprintln!("remind failed: {e}");
            1
        }
    }
}

/// Shared submission path for `schedule` + `goal`: submit an OFF-CHAIN agent job
/// via the proxy (the proxy validates the target + bills the caller's meter per
/// run), print the schedule. `goal_mode` only changes the confirmation copy (the
/// difference is entirely the task's `GOAL: ` marker, which the worker keys on).
async fn submit_job(caller_name: Option<&str>, parsed: ParsedSchedule, goal_mode: bool) -> i32 {
    let ParsedSchedule {
        target,
        task,
        interval_secs,
        max_runs,
    } = parsed;
    // An empty / whitespace-only task is a no-op job — reject it before any work
    // (same guard as call/mcp-call). A bare `GOAL: ` marker counts as blank too.
    if task_is_blank(&task) {
        let label = if goal_mode { "goal: goal text" } else { "schedule: task" };
        eprintln!("{label} is empty — nothing to send");
        return 1;
    }

    let signer = match load_signer(caller_name) {
        Ok(s) => s,
        Err(code) => return code,
    };

    let every = fmt_interval(interval_secs);
    println!("scheduling {target} every {every}, up to {max_runs} run(s) (off-chain) …");
    // OFF-CHAIN agent job: the proxy validates the target is registered (404 if
    // not), runs it each fire under its persona, and bills the CALLER's meter per
    // run — no escrow, no sponsor, no on-chain tx. The `GOAL: ` marker in `task`
    // still drives the ralph goal-loop (the worker keys on it).
    match registry::create_offchain_job(&signer, now_unix(), "agent", &target, &task, interval_secs, max_runs)
        .await
    {
        Ok(id) => {
            println!("✓ job {id}: {target} every {every}, ~{max_runs} runs (off-chain; may take a few seconds to appear in `jobs`)");
            if goal_mode {
                println!("  goal loop: each fire re-feeds the goal and the agent takes ONE step;");
                println!("  it self-ends when the agent declares the goal complete (finish_goal).");
            } else {
                println!("  runs tab-free; each fire bills your meter (~1 $LH/model call).");
            }
            println!("  cancel: localharness unschedule {id}");
            0
        }
        Err(e) => {
            eprintln!("schedule failed: {e}");
            1
        }
    }
}

/// Render one job row from the proxy's `list` JSON (reminders + agent jobs).
/// Pure-ish (no I/O).
fn format_offchain_row(j: &serde_json::Value, now: u64) -> String {
    let id = j.get("id").and_then(|v| v.as_str()).unwrap_or("?");
    let kind = j.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
    let task = j.get("task").and_then(|v| v.as_str()).unwrap_or("");
    let interval = j.get("intervalSecs").and_then(|v| v.as_u64()).unwrap_or(0);
    let runs_left = j.get("runsLeft").and_then(|v| v.as_u64()).unwrap_or(0);
    let next = j.get("nextRun").and_then(|v| v.as_u64()).unwrap_or(0);
    let target = j.get("target").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
    let next_s = if next == 0 {
        "—".to_string()
    } else if next <= now {
        "due".to_string()
    } else {
        format!("in {}", fmt_interval(next - now))
    };
    let tgt = target.map(|t| format!(" → {t}")).unwrap_or_default();
    format!(
        "  {id}  [{kind}]{tgt}  every {iv}  next {next_s}  runs-left {runs_left}\n      {snippet}",
        iv = fmt_interval(interval),
        snippet = truncate_words(task, 60),
    )
}

/// `localharness jobs [--as <me>]` — list the caller's scheduled jobs (the
/// off-chain store: reminders + agent jobs). Read-only, no `$LH`.
pub(crate) async fn list_jobs(caller_name: Option<&str>) -> i32 {
    let signer = match load_signer(caller_name) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));
    let now = now_unix();

    let mut offchain_jobs = match registry::list_offchain_jobs(&signer, now).await {
        Ok(j) => j,
        Err(e) => {
            eprintln!("(job list unavailable: {e})");
            Vec::new()
        }
    };

    // The store's directory listing lags a write by a few seconds (GitHub
    // Contents-API read-after-write consistency), so `jobs` right after
    // `schedule`/`remind` sees an empty list and looks like a silent failure
    // (telemetry #44). On empty, retry ONCE after ~2s before concluding.
    if offchain_jobs.is_empty() {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        offchain_jobs = registry::list_offchain_jobs(&signer, now).await.unwrap_or_default();
    }

    if offchain_jobs.is_empty() {
        println!("no scheduled jobs for {addr}");
        return 0;
    }
    println!("{} job(s):", offchain_jobs.len());
    for j in &offchain_jobs {
        println!("{}", format_offchain_row(j, now));
    }
    0
}

/// `localharness unschedule [--as <me>] <jobId>` — cancel a scheduled job by
/// its store id (sponsor-free; just a signed POST, owner-gated server-side).
pub(crate) async fn unschedule(caller_name: Option<&str>, job_id_arg: &str) -> i32 {
    let raw = job_id_arg.trim().trim_start_matches('#');
    if raw.is_empty() {
        eprintln!("unschedule: missing job id");
        return 2;
    }
    let signer = match load_signer(caller_name) {
        Ok(s) => s,
        Err(code) => return code,
    };
    match registry::cancel_offchain_job(&signer, now_unix(), raw).await {
        Ok(()) => {
            println!("✓ cancelled job {raw}");
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
    use crate::args;

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
            "oracle", "check", "the", "price", "--every", "5m", "--runs", "50",
        ]))
        .unwrap();
        assert_eq!(p.target, "oracle");
        assert_eq!(p.task, "check the price"); // joined multi-word task
        assert_eq!(p.interval_secs, 300);
        assert_eq!(p.max_runs, 50);

        // --runs defaults; flags may precede the task.
        let p = parse_schedule_args(&args(&["bot", "--every", "1h", "ping"])).unwrap();
        assert_eq!(p.target, "bot");
        assert_eq!(p.task, "ping");
        assert_eq!(p.interval_secs, 3600);
        assert_eq!(p.max_runs, SCHEDULE_DEFAULT_RUNS);
    }

    #[test]
    fn parse_schedule_args_rejects_bad_input() {
        // Missing --every.
        assert!(parse_schedule_args(&args(&["t", "task"])).is_err());
        // No task (only the target positional).
        assert!(parse_schedule_args(&args(&["t", "--every", "5m"])).is_err());
        // --budget is a HARD ERROR now (off-chain jobs bill the meter, no escrow).
        let e = parse_schedule_args(&args(&["t", "x", "--every", "5m", "--budget", "1"])).unwrap_err();
        assert!(e.contains("--budget"), "budget rejection message: {e}");
        // Bad --runs.
        assert!(parse_schedule_args(&args(&["t", "x", "--every", "5m", "--runs", "0"])).is_err());
        // Sub-minute interval bubbles up from parse_interval.
        assert!(parse_schedule_args(&args(&["t", "x", "--every", "10s"])).is_err());
    }

    #[test]
    fn parse_goal_args_defaults_and_marker() {
        // --every defaults to 5m, --runs to the schedule default; the task gains
        // the EXACT worker marker.
        let p = parse_goal_args(&args(&["claude", "get", "my", "TBA", "to", "1", "$LH"])).unwrap();
        assert_eq!(p.target, "claude");
        assert_eq!(p.task, "GOAL: get my TBA to 1 $LH"); // marker + joined text
        assert!(p.task.starts_with(GOAL_TASK_PREFIX));
        assert_eq!(p.interval_secs, GOAL_DEFAULT_INTERVAL_SECS); // 5m default
        assert_eq!(p.max_runs, SCHEDULE_DEFAULT_RUNS); // 100 default
    }

    #[test]
    fn parse_goal_args_explicit_flags() {
        // Explicit --every/--runs override the defaults; flags may precede the goal.
        let p = parse_goal_args(&args(&["bot", "--every", "1h", "--runs", "10", "win"])).unwrap();
        assert_eq!(p.target, "bot");
        assert_eq!(p.task, "GOAL: win");
        assert_eq!(p.interval_secs, 3600);
        assert_eq!(p.max_runs, 10);
    }

    #[test]
    fn parse_goal_args_rejects_bad_input() {
        // No goal text (only the target positional).
        assert!(parse_goal_args(&args(&["t"])).is_err());
        // --budget is a HARD ERROR now.
        assert!(parse_goal_args(&args(&["t", "x", "--budget", "1"])).is_err());
        // Bad --runs.
        assert!(parse_goal_args(&args(&["t", "x", "--runs", "0"])).is_err());
        // A sub-minute --every bubbles up from parse_interval.
        assert!(parse_goal_args(&args(&["t", "x", "--every", "10s"])).is_err());
    }

}
