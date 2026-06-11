//! Scheduled jobs — escrow-backed recurring agent runs (ScheduleFacet).

use wasm_bindgen::prelude::*;

use crate::app::{dom, templates};

/// ScheduleFacet's on-chain minimum cadence (mirrors the CLI's
/// `SCHEDULE_MIN_INTERVAL_SECS`). The facet rejects anything faster.
const SCHEDULE_MIN_INTERVAL_SECS: u64 = 60;
/// Run cap used when the optional "runs" input is left blank (mirrors the
/// CLI's `SCHEDULE_DEFAULT_RUNS`).
const SCHEDULE_DEFAULT_RUNS: u32 = 100;

/// The EXACT on-chain task marker the scheduler worker recognises as a goal
/// loop (ralph-on-chain; mirrors the CLI's `GOAL_TASK_PREFIX` and the
/// worker's `GOAL_PREFIX` in `proxy/api/scheduler.ts`): each fire re-feeds
/// the goal, the agent takes one step, and `finish_goal` self-cancels the
/// job (refunding the unspent escrow) when the goal is verifiably met.
const GOAL_TASK_PREFIX: &str = "GOAL: ";
/// Promote-to-background goal-job parameters: tight cadence (the on-chain
/// 60s minimum — the work was already mid-flight), a bounded run count, and
/// a 0.5 `$LH` escrow as the hard cost stop.
const PROMOTE_INTERVAL_SECS: u64 = 60;
const PROMOTE_MAX_RUNS: u32 = 20;
const PROMOTE_BUDGET_WEI: u128 = 500_000_000_000_000_000; // 0.5 $LH

/// Parse a human cadence (`60s` / `5m` / `1h`, bare number = seconds) into
/// seconds, enforcing the 60s floor. Mirrors the CLI's `parse_interval`
/// EXACTLY so the browser + CLI accept the same strings. `None` on garbage
/// or sub-minimum (handled by a silent no-op — no explanatory-validation).
fn parse_schedule_interval(raw: &str) -> Option<u64> {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
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
    let secs = num_part.parse::<u64>().ok()?.checked_mul(mult)?;
    (secs >= SCHEDULE_MIN_INTERVAL_SECS).then_some(secs)
}

/// Render seconds as a compact cadence (`90s` / `5m` / `2h` / `1h30m`) for
/// the jobs list. Pure mirror of the CLI's `fmt_interval`.
fn fmt_schedule_interval(secs: u64) -> String {
    if secs == 0 {
        return "0s".to_string();
    }
    if secs % 3600 == 0 {
        return format!("{}h", secs / 3600);
    }
    if secs >= 3600 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let rest_s = secs % 60;
        if rest_s == 0 {
            return format!("{h}h{m}m");
        }
    }
    if secs % 60 == 0 {
        return format!("{}m", secs / 60);
    }
    format!("{secs}s")
}

/// Schedule a recurring job from the admin panel (mirrors
/// `create_invite_pressed`). Reads the target/task/interval/budget/runs
/// inputs, resolves the target name→id, escrows the budget behind
/// `scheduleJob` in ONE sponsored tx, then swaps `#schedule-result` for the
/// success panel + refreshes the jobs list. Bad/empty input is a SILENT
/// no-op (no explanatory-validation text).
pub(super) fn schedule_job_pressed() {
    let target = dom::input_by_id("schedule-target")
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();
    let task = dom::input_by_id("schedule-task")
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();
    let interval_raw = dom::input_by_id("schedule-interval")
        .map(|i| i.value())
        .unwrap_or_default();
    let budget_raw = dom::input_by_id("schedule-budget")
        .map(|i| i.value())
        .unwrap_or_default();
    let runs_raw = dom::input_by_id("schedule-runs")
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();

    // Silent no-ops on missing/invalid fields (no explanatory text).
    if target.is_empty() || task.is_empty() {
        return;
    }
    let Some(interval_secs) = parse_schedule_interval(&interval_raw) else {
        return;
    };
    let Some(budget_wei) = crate::encoding::parse_token_amount(&budget_raw) else {
        return;
    };
    if budget_wei == 0 {
        return;
    }
    // Optional run cap: blank → default; garbage/zero → silent no-op.
    let max_runs = if runs_raw.is_empty() {
        SCHEDULE_DEFAULT_RUNS
    } else {
        match runs_raw.parse::<u32>() {
            Ok(n) if n > 0 => n,
            _ => return,
        }
    };

    dom::swap_inner(
        "schedule-result",
        "<span style=\"color:var(--muted)\">scheduling…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        match submit_schedule_job(&target, &task, interval_secs, budget_wei, max_runs).await {
            Ok(new_id) => {
                dom::swap_inner(
                    "schedule-result",
                    &templates::schedule_result_panel(new_id).into_string(),
                );
                refresh_jobs_list().await;
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("schedule job: {e}")));
                dom::swap_inner(
                    "schedule-result",
                    &dom::msg_span(
                        dom::Msg::Error,
                        "job couldn't be scheduled (need $LH to escrow)",
                    ),
                );
            }
        }
    });
}

/// The ONE escrow-backed `scheduleJob` submission core, shared by the admin
/// schedule form ([`schedule_job_pressed`]) and the in-run [⇪ background]
/// promote ([`promote_background_pressed`]): sponsor rate guard → resolve
/// the target name → credit signer + embedded fee payer → sponsored
/// approve+`scheduleJob` tx → refresh the credits pill → read the new job id
/// back from `jobsOf(caller)` (its last entry; 0 if unreadable). The budget
/// is pulled from the caller's WALLET `$LH` by `transferFrom`; a wallet
/// shortfall covered by unspent chat-METER credits rides as a
/// `withdrawCredits` call in the SAME atomic tx (the escrow auto-bridge —
/// on-chain feedback #63), so "has metered credits but the escrow fails"
/// can only mean BOTH pots together are short.
async fn submit_schedule_job(
    target: &str,
    task: &str,
    interval_secs: u64,
    budget_wei: u128,
    max_runs: u32,
) -> Result<u64, String> {
    super::sponsor_rate_guard()?;
    let target_id = crate::app::registry::id_of_name(target).await?;
    if target_id == 0 {
        return Err("target agent not found".to_string());
    }
    let (signer, addr) = crate::app::chat::credit_signer()
        .await
        .ok_or_else(|| "no identity".to_string())?;
    let from_hex = crate::encoding::bytes_to_hex_str(&addr);
    let bridge_wei = crate::app::chat::escrow_bridge_wei(&from_hex, budget_wei).await?;
    let fee_payer = crate::app::sponsor::signer()?;
    crate::app::registry::schedule_job_sponsored_bridged(
        &signer,
        &fee_payer,
        target_id,
        task.as_bytes(),
        interval_secs,
        budget_wei,
        max_runs,
        crate::app::registry::ALPHA_USD_ADDRESS,
        bridge_wei,
    )
    .await?;
    // The escrow left the funder's spendable balance — reflect it.
    super::refresh_credits_pill().await;
    // New job id = the last entry in jobsOf(caller). Read it back so the
    // caller's confirmation surface reflects the freshly-mined job.
    let new_id = match crate::app::chat::credit_address_existing().await {
        Some(addr) => crate::app::registry::jobs_of(&addr)
            .await
            .ok()
            .and_then(|ids| ids.last().copied())
            .unwrap_or(0),
        None => 0,
    };
    Ok(new_id)
}

/// CONTINUE-IN-BACKGROUND (the [⇪ background] button next to ■ while a run
/// streams): stop the in-tab turn cooperatively, then promote the run's
/// ORIGINAL user request to a headless on-chain goal job targeting this
/// tenant — the scheduler worker drives it to completion (and `finish_goal`
/// self-cancels + refunds) with the tab closed. Partial in-tab progress is
/// already durable: history persists per turn, and on-chain effects live in
/// the diamond; the goal text tells the worker run to inspect + continue.
pub(super) fn promote_background_pressed() {
    // Tenant-only (the button only renders on a tenant; belt-and-braces).
    let Some(name) = crate::app::tenant::current_name() else {
        return;
    };
    // No active run → silent no-op (the lifecycle keeps the button absent).
    let Some(prompt) = crate::app::chat::active_run_prompt() else {
        return;
    };
    // Stop the in-tab turn (same TURN_CANCEL path as ■); false = already
    // promoted this run or the run just ended — never schedule twice.
    if !crate::app::chat::request_stop_for_promote() {
        return;
    }
    // The run is over from the tab's perspective — restore the send button
    // now (also removes this button, so a second press is impossible).
    dom::swap_outer("terminal-stop", &templates::send_button().into_string());
    dom::set_status("continuing in background — scheduling…", false);
    wasm_bindgen_futures::spawn_local(async move {
        let task = format!(
            "{GOAL_TASK_PREFIX}{prompt} (promoted from an in-tab run; prior \
             partial progress may exist — inspect state and continue)"
        );
        match submit_schedule_job(
            &name,
            &task,
            PROMOTE_INTERVAL_SECS,
            PROMOTE_BUDGET_WEI,
            PROMOTE_MAX_RUNS,
        )
        .await
        {
            Ok(job_id) => {
                let job = if job_id == 0 {
                    "job scheduled".to_string()
                } else {
                    format!("job #{job_id}")
                };
                let ping = if push_ready().await {
                    "you'll get a notification when it completes"
                } else {
                    "enable notifications to get pinged"
                };
                dom::set_status(&format!("continuing in background — {job}; {ping}"), false);
                refresh_jobs_list().await;
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!(
                    "promote background: {e}"
                )));
                dom::set_status(
                    "couldn't continue in background (need 0.5 $LH to escrow)",
                    true,
                );
            }
        }
    });
}

/// Can the scheduler worker actually notify this user when the promoted job
/// completes? True only when BOTH halves hold: notification permission is
/// granted on this device (sync, free) AND a Web Push subscription is
/// published on-chain under the owner's MAIN tokenId (one `push_sub_of`
/// read; the slot rule mirrors `notifications::enable_and_publish`).
/// Best-effort — any read failure reports false, and the caller's message
/// degrades to "enable notifications" rather than promising a ping.
async fn push_ready() -> bool {
    if !matches!(
        web_sys::Notification::permission(),
        web_sys::NotificationPermission::Granted
    ) {
        return false;
    }
    let Ok((name, owner)) = crate::app::tenant::current_tenant_owner().await else {
        return false;
    };
    let token_id = match crate::registry::main_of(&owner).await {
        Ok(id) if id != 0 => id,
        _ => match crate::registry::id_of_name(&name).await {
            Ok(id) if id != 0 => id,
            _ => return false,
        },
    };
    matches!(crate::registry::push_sub_of(token_id).await, Ok(Some(_)))
}

/// Cancel a scheduled job from the admin list (ScheduleFacet `cancelJob`
/// refunds the remaining escrowed `$LH`). Then refresh the list + credits.
pub(super) fn cancel_job_pressed(job_id_raw: String) {
    let Ok(job_id) = job_id_raw.trim().parse::<u64>() else {
        return;
    };
    dom::swap_inner(
        "schedule-result",
        "<span style=\"color:var(--muted)\">cancelling…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            super::sponsor_rate_guard()?;
            let (signer, _) = crate::app::chat::credit_signer()
                .await
                .ok_or_else(|| "no identity".to_string())?;
            let fee_payer = crate::app::sponsor::signer()?;
            crate::app::registry::cancel_job_sponsored(
                &signer,
                &fee_payer,
                job_id,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
        }
        .await;
        match result {
            Ok(_) => {
                dom::swap_inner(
                    "schedule-result",
                    &dom::msg_span(dom::Msg::Muted, "cancelled — remaining $LH refunded"),
                );
                super::refresh_credits_pill().await;
                refresh_jobs_list().await;
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("cancel job: {e}")));
                dom::swap_inner(
                    "schedule-result",
                    &dom::msg_span(dom::Msg::Error, "couldn't cancel that job"),
                );
            }
        }
    });
}

/// Read the caller's `jobsOf(...)` + paint the "your jobs" list into
/// `#schedule-jobs` (per job: target name, cadence, next run, budget,
/// runs-left, status + a cancel button for Active/Paused jobs). Soft-fails
/// to a quiet line. Called on admin open + after schedule/cancel. No-op if
/// the slot isn't mounted or no identity exists yet.
pub(crate) async fn refresh_jobs_list() {
    if dom::by_id("schedule-jobs").is_none() {
        return;
    }
    let Some(addr) = crate::app::chat::credit_address_existing().await else {
        return;
    };
    let ids = match crate::app::registry::jobs_of(&addr).await {
        Ok(v) => v,
        Err(_) => {
            dom::swap_inner("schedule-jobs", "");
            return;
        }
    };
    if ids.is_empty() {
        dom::swap_inner(
            "schedule-jobs",
            &dom::msg_span(dom::Msg::Muted, "no scheduled jobs"),
        );
        return;
    }
    let now = (js_sys::Date::now() / 1000.0) as u64;
    // Resolve each job's record + target name. Sequential reads — fine at the
    // handful-of-jobs scale; the index is short.
    let mut rows: Vec<maud::Markup> = Vec::new();
    for id in ids {
        let Ok(job) = crate::app::registry::get_job(id).await else {
            continue;
        };
        let target = crate::app::registry::name_of_id(job.target_id)
            .await
            .ok()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("token#{}", job.target_id));
        let budget_whole = job.budget_wei / 1_000_000_000_000_000_000u128;
        let budget_cents =
            (job.budget_wei % 1_000_000_000_000_000_000u128) / 10_000_000_000_000_000u128;
        let cadence = fmt_schedule_interval(job.interval);
        let status = job.status_label();
        let next = if job.next_run == 0 {
            "—".to_string()
        } else if job.next_run <= now {
            "due".to_string()
        } else {
            let delta = job.next_run - now;
            format!("in {}", fmt_schedule_interval(delta.max(1)))
        };
        // Only Active(0) / Paused(1) jobs can still be cancelled for a refund.
        let cancellable = matches!(job.status, 0 | 1);
        // Inline styles (monochrome, var-driven) keep this self-contained in
        // src/app/ — the same convention `refresh_signer_list` uses for its
        // on-chain-sourced rows. maud `(…)` escapes the RPC-sourced target.
        rows.push(maud::html! {
            div style="border-top:1px solid var(--border);padding:6px 0;font-size:11px;color:var(--fg)" {
                div style="display:flex;align-items:center;gap:8px" {
                    code style="color:var(--muted)" { "#" (id) }
                    span style="flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" { (target) }
                    span style="color:var(--muted)" { (status) }
                    @if cancellable {
                        button type="button" data-action="cancel-job" data-arg=(id.to_string())
                            .ghost style="padding:0 6px" { "cancel" }
                    }
                }
                div style="display:flex;flex-wrap:wrap;gap:10px;color:var(--muted);margin-top:2px" {
                    span { "every " (cadence) }
                    span { "next " (next) }
                    span { (budget_whole) "." (format!("{budget_cents:02}")) " LH" }
                    span { (job.runs_left) " runs left" }
                }
            }
        });
    }
    let html = maud::html! {
        div style="margin-top:8px" { @for r in &rows { (r) } }
    }
    .into_string();
    dom::swap_inner("schedule-jobs", &html);
}
