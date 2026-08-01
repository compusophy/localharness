//! `work` — a LOCAL terminal-capable agent run: the SDK's native agent loop
//! (all 8 fs builtins + `run_command`) confined to the CURRENT DIRECTORY,
//! billed through the credit proxy like every other headless turn. This is
//! "localharness as a coding agent in your terminal", and the exact surface
//! the Terminal-Bench adapter drives (`adapters/terminal-bench/`).
//!
//! Contrast with `call`: `call` embodies a REMOTE agent's persona over a
//! tool-free (or read-only) turn — a remote prompt must never touch the
//! caller's filesystem. `work` is the inverse: YOUR identity, YOUR cwd, full
//! native tools, `workspace_only` policy pinning every fs path inside it.

use crate::{ensure_meter_funded, load_signer, print_err, registry, take_as_flag, take_value_flag};

pub(crate) const WORK_USAGE: &str = "usage: localharness work [--as <me>] [--model <id>] <task…>\n  \
     run a LOCAL agent on <task> in the CURRENT DIRECTORY: native tools (read/\n  \
     write/edit/search files + run_command), workspace-confined, billed per\n  \
     model round from your meter (~1 $LH default; premium models like claude\n  \
     bill 5-20 $LH/round — fund accordingly). e.g.\n  \
     localharness work --as claude \"add a --version flag to this CLI and test it\"";

/// The lean task-mode system prompt. Deliberately persona-free: `work` is a
/// coding-agent run in a real directory, not an embodiment of a published
/// agent. Kept short — the tools carry the capability story.
const WORK_PROMPT: &str = "You are localharness running as a local coding agent \
    inside the user's current working directory. Complete the task using your \
    tools: list_directory, view_file, find_file, search_directory, create_file, \
    edit_file, delete_file, rename_file, and run_command (a real shell in this \
    directory — use it to build, test, and verify). Work autonomously: inspect \
    first, act, then VERIFY by RUNNING the result. Keep file edits minimal and \
    idiomatic. Never emit file contents as chat text — write files ONLY with \
    create_file/edit_file. Assume your deliverable is RE-EXECUTED by a grader in \
    a fresh environment, possibly with different inputs, sizes, seeds, or library \
    versions: never hardcode a value, shape, or count you observed; if the task \
    says something is unknown to you, solve the general case even when you could \
    peek at it; before finishing, test your artifact on a variant you construct \
    yourself. If you installed the runtime or libraries yourself, the grader may \
    pin different versions — prefer public APIs, wrap library internals in \
    try/except fallbacks, or hand-roll simple equivalents. KEEP WORKING until the \
    task is FULLY complete and verified — a hard task takes MANY rounds of write \
    → run → read the error → fix → re-run; do NOT stop, summarize, or hand back \
    until it actually works. When (and only when) it is done AND you have run it \
    to confirm, call the `finish` tool with a one-line summary. Never call finish \
    on an unverified or partial solution.";

/// `localharness work [--as <me>] [--model <id>] <task…>`
pub(crate) async fn work(args: &[String]) -> i32 {
    let (caller, rest) = match take_as_flag(args) {
        Ok(v) => v,
        Err(e) => {
            print_err(&e);
            return 2;
        }
    };
    let (model, rest) = match take_value_flag(&rest, "--model", WORK_USAGE) {
        Ok(v) => v,
        Err(e) => {
            print_err(&e);
            return 2;
        }
    };
    if rest.is_empty() {
        eprintln!("{WORK_USAGE}");
        return 2;
    }
    let task = rest.join(" ");

    let signer = match load_signer(caller.as_deref()) {
        Ok(s) => s,
        Err(code) => return code,
    };

    // Meter-billed (a multi-request agent loop can never ride a one-shot x402
    // nonce). Best-effort top-up from the wallet, same as `call`'s meter path.
    ensure_meter_funded(&signer).await;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let token = registry::proxy_auth_token(&signer, now, "gemini");
    // The proxy rejects signed tokens older than 5 min (FRESHNESS_WINDOW_SECS) —
    // a deep work run outlives that and every later round 401s "stale or future
    // timestamp" (TB-2.1 write-compressor died this way hours in). Re-sign per
    // request; the startup token above stays the fallback.
    let auth_signer = signer.clone();
    let auth_provider: localharness::backends::KeyProvider = std::sync::Arc::new(move || {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        registry::proxy_auth_token(&auth_signer, now, "gemini")
    });
    let base = match url::Url::parse(registry::CREDIT_PROXY_URL) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("internal: bad proxy url: {e}");
            return 1;
        }
    };

    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot resolve the current directory: {e}");
            return 1;
        }
    };
    // Coarse per-round cost hint so a meter is funded for the RIGHT model (the
    // exact table is proxy-side in _prices.ts; premium tiers are 5/20 $LH). The
    // default flash tier is 1 $LH — a flat "~1 $LH" line underfunded claude ~5x.
    let round_cost = match model.as_deref() {
        Some(m) if m.contains("opus") => "~20 $LH (premium)",
        Some(m) if m.starts_with("claude") && !m.contains("haiku") => "~5 $LH (premium)",
        _ => "~1 $LH",
    };
    eprintln!(
        "work: native agent in {} (billed {round_cost} per model round; Ctrl-C aborts)",
        cwd.display()
    );

    let fs: localharness::filesystem::SharedFilesystem =
        std::sync::Arc::new(localharness::filesystem::NativeFilesystem::new());
    // The DEFAULT capability set is read-only safety mode — a work agent needs
    // the write half (create/edit/delete/rename + run_command); containment
    // comes from the workspace policy, not tool absence.
    let mut caps = localharness::types::CapabilitiesConfig::unrestricted();
    // ENABLE auto-compaction (unrestricted() leaves it None = OFF). A `work` run
    // is a long autonomous loop that accumulates file contents + tool output
    // every round; without compaction a deep task grows the context UNBOUNDED
    // until it overflows the model window and the turn returns empty — a FALSE
    // failure — and every round re-sends the whole history (cost). Same 128K
    // ceiling as the browser session. NOT a fix for thinking-latency 504s (those
    // hit at small context); a cost + deep-task-robustness measure.
    // Same 128K threshold as the browser session. (History: this was briefly an
    // aggressive 48K stopgap because the EDGE credit proxy 504'd on slow first
    // byte, which grows with context — the proxy's Node-runtime port removed
    // that cap entirely (design/proxy-504-fix.md, 2026-08-01), and a 489KB
    // ~130K-token request now round-trips in ~3s, so deep tasks keep the full
    // context quality again.)
    const WORK_COMPACTION_THRESHOLD: u32 = 128_000;
    caps.compaction_threshold = Some(WORK_COMPACTION_THRESHOLD);
    // Every fs path is pinned inside the workspace, and a wildcard allow sits
    // BEHIND the denies: `evaluate` is default-deny once any policy exists, and
    // workspace_only only contributes deny-when-outside rules — without the
    // trailing allow, every in-workspace call died "no matching policy" (found
    // live). Deny buckets evaluate first, so containment still wins.
    let policies: Vec<localharness::policy::Policy> =
        localharness::policy::workspace_only(vec![cwd.clone()])
            .into_iter()
            .chain(std::iter::once(localharness::policy::Policy::allow("*")))
            .collect();

    // Route by model id: `claude-*` uses the Anthropic backend (needs the
    // anthropic feature), anything else Gemini. Both reach the model through
    // the credit proxy with the same signed token, so a subsidized identity
    // drives either provider with no provider key. Mirrors `call`'s routing.
    let is_claude = model.as_deref().map(|m| m.starts_with("claude")).unwrap_or(false);
    let agent = if is_claude {
        #[cfg(feature = "anthropic")]
        {
            let mut cfg = localharness::AnthropicAgentConfig::new(token)
                .with_base_url(base)
                .with_auth_provider(auth_provider)
                .with_model(model.clone().unwrap())
                .with_system_instructions(WORK_PROMPT.to_string())
                .with_filesystem(fs)
                .with_capabilities(caps)
                .with_policies(policies);
            let _ = &mut cfg;
            match localharness::Agent::start_anthropic(cfg).await {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("start failed: {e}");
                    return 1;
                }
            }
        }
        #[cfg(not(feature = "anthropic"))]
        {
            eprintln!("claude models need a build with `--features anthropic`");
            return 1;
        }
    } else {
        let mut cfg = localharness::GeminiAgentConfig::new(token)
            .with_base_url(base)
            .with_auth_provider(auth_provider)
            // Ask for a high output cap: an unset/low cap lets 3.x dynamic
            // thinking starve the answer mid-token (TB-15 regex-chess/
            // schemelike died this way). The credit proxy clamps this to its
            // LH_MAX_OUTPUT_TOKENS env server-side (raised 8192→16384
            // 2026-08-01), so the effective cap is min(this, proxy) — and the
            // truncation NUDGE in the loop is the recovery when it still hits.
            .with_max_output_tokens(65_536)
            .with_system_instructions(WORK_PROMPT.to_string())
            .with_filesystem(fs)
            .with_capabilities(caps)
            .with_policies(policies);
        if let Some(m) = &model {
            cfg = cfg.with_model(m.clone());
        }
        match localharness::Agent::start_gemini(cfg).await {
            Ok(a) => a,
            Err(e) => {
                eprintln!("start failed: {e}");
                return 1;
            }
        }
    };

    // The SDK caps ONE `agent.chat()` turn at MAX_TOOL_ROUNDS (16, turn_engine.rs)
    // then FORCES the turn to end. A hard coding task needs far more (write → run →
    // read error → fix → re-run, dozens of times), so wrap chat() in an auto-continue
    // loop: a turn cut off mid-tool-work (its last event was a tool call/result, not a
    // final answer) that did NOT call `finish` gets nudged to keep going. Mirrors the
    // browser run_send loop; without it the CLI agent was guillotined at 16 rounds and
    // never finished hard tasks (TB-2.1: every funded run stopped at ~16 calls
    // mid-work). Bounded so a stuck agent can't loop forever; compaction (set above)
    // keeps context bounded across the continuations.
    const MAX_WORK_CONTINUATIONS: u32 = 12;
    // Transcript truncation caps: big enough that a failed run is diagnosable
    // from its OWN log (TB-15 postmortem: args were cut at 160 chars and tool
    // RESULTS never logged at all — two diagnoses had to reconstruct the code
    // this agent wrote by simulation), small enough to keep logs bounded.
    const LOG_ARGS_CHARS: usize = 2048;
    const LOG_RESULT_CHARS: usize = 2048;
    use futures_util::StreamExt;
    use std::io::Write;
    // Ctrl-C: run_command children live in their OWN process groups (the
    // tree-kill fix), so the terminal's SIGINT no longer reaches them — do
    // what the terminal used to, then exit with the conventional 130.
    tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            localharness::builtins::kill_live_process_groups().await;
            std::process::exit(130);
        }
    });
    // Upstream 429s (provider RPM/TPM quota) are TRANSIENT congestion to an
    // autonomous loop. The SDK's fail-fast on rate-limit is right for an
    // interactive turn and stays; here it killed 66 of 89 full-set TB tasks
    // when 3-way concurrency blew one key's per-minute quota — litellm-based
    // agents back off and live. Ladder 15/30/45/60/60/60s ≈ ≤4.5min per
    // incident, inside harbor's 900s task budget; quota windows reset in 60s.
    const MAX_RATE_LIMIT_RETRIES: u32 = 6;
    let started = std::time::Instant::now();
    let mut input: std::borrow::Cow<str> = std::borrow::Cow::Borrowed(task.as_str());
    let mut continuations: u32 = 0;
    let mut consecutive_toolless: u32 = 0;
    let mut rate_limit_retries: u32 = 0;
    let mut failed = false;
    'run: loop {
        let reply = match agent.chat(input.as_ref()).await {
            Ok(r) => r,
            Err(e) if e.code() == localharness::error_codes::BACKEND_RATE_LIMIT
                && rate_limit_retries < MAX_RATE_LIMIT_RETRIES =>
            {
                rate_limit_retries += 1;
                let wait = 15 * rate_limit_retries.min(4) as u64;
                eprintln!(
                    "… upstream rate limit — backing off {wait}s (retry {rate_limit_retries}/{MAX_RATE_LIMIT_RETRIES})"
                );
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                continue; // re-issue the SAME input; the failed open billed nothing
            }
            Err(e) => {
                eprintln!("\nwork failed: {e}");
                failed = true;
                break;
            }
        };
        rate_limit_retries = 0;
        // Stream the turn LIVE (text → stdout; tools/results → stderr).
        let mut cursor = reply.chunks();
        let mut tool_calls_this_turn: u32 = 0;
        let mut saw_text = false;
        let mut saw_thinking = false;
        while let Some(res) = cursor.next().await {
            match res {
                Ok(localharness::types::StreamChunk::Text { text, .. }) => {
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                    saw_text = true;
                }
                Ok(localharness::types::StreamChunk::Thought { .. }) => {
                    saw_thinking = true;
                }
                Ok(localharness::types::StreamChunk::ToolCall(tc)) => {
                    let args = serde_json::to_string(&tc.args).unwrap_or_default();
                    let args: String = args.chars().take(LOG_ARGS_CHARS).collect();
                    eprintln!("→ {}({args})", tc.name);
                    tool_calls_this_turn += 1;
                }
                Ok(localharness::types::StreamChunk::ToolResult(tr)) => {
                    if let Some(err) = &tr.error {
                        eprintln!("  ✗ {}: {err}", tr.name);
                    } else if let Some(out) = &tr.result {
                        let s = serde_json::to_string(out).unwrap_or_default();
                        let s: String = s.chars().take(LOG_RESULT_CHARS).collect();
                        eprintln!("  ← {s}");
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    eprintln!("\nwork failed: {e}");
                    failed = true;
                    break 'run;
                }
            }
        }
        // Turn postmortem comes from the RESPONSE, not the chunk stream:
        // backends intercept `finish` and never emit it as a ToolCall chunk
        // (conversation.rs::finished doc) — the old chunk-sniffed `saw_finish`
        // was dead code, so finish-after-tool-work turns got NUDGED and
        // text-tail turns mid-work silently ENDED the run (TB-15: 3 of 6
        // losses were exactly that, the CLI replay of #75/#69/#67).
        let finished = reply.finished();
        // Truncation detection is turn_flow's (blocked-note precedence, the
        // "max token" wording, thinking-ate-the-budget) — the thinking arm
        // only applies when the turn produced nothing VISIBLE, else every
        // ordinary reasoning turn would read as cut off.
        let visibly_empty = tool_calls_this_turn == 0 && !saw_text;
        let truncated = matches!(
            localharness::turn_flow::classify_empty(
                reply.finish_note().as_deref(),
                saw_thinking && visibly_empty,
            ),
            localharness::turn_flow::EmptyKind::Truncated
        );
        eprintln!(
            "· turn: finished={finished} tools={tool_calls_this_turn} truncated={truncated} run_elapsed={}s",
            started.elapsed().as_secs()
        );
        let (next_toolless, nudge) =
            turn_signals(consecutive_toolless, tool_calls_this_turn, truncated, saw_text);
        consecutive_toolless = next_toolless;
        if !work_should_continue(
            finished,
            consecutive_toolless,
            continuations,
            MAX_WORK_CONTINUATIONS,
        ) {
            if let Some(summary) = reply.finish_summary() {
                eprintln!("· finish: {summary}");
            }
            break;
        }
        continuations += 1;
        eprintln!("… auto-continue {continuations}/{MAX_WORK_CONTINUATIONS}");
        input = std::borrow::Cow::Owned(
            match nudge {
                Nudge::Truncation => WORK_TRUNCATION_NUDGE,
                Nudge::TextTail => WORK_TEXT_TAIL_NUDGE,
                Nudge::ToolTail => WORK_CONTINUE_NUDGE,
            }
            .to_string(),
        );
    }
    println!();
    let _ = agent.shutdown().await;
    if failed { 1 } else { 0 }
}

/// Continue-hint when the turn ended mid-tool-work (hit the 16-round cap).
const WORK_CONTINUE_NUDGE: &str = "(continue — your last turn hit the tool-round cap \
    before finishing. Review what you've done, then take the NEXT concrete step: run \
    the result, read any error, fix it, re-run. Call `finish` only once it is actually \
    complete and verified.)";

/// Continue-hint when the turn ended on TEXT without calling `finish` — the
/// model narrated instead of acting (the #75/#69/#67 stall class).
const WORK_TEXT_TAIL_NUDGE: &str = "(you ended with analysis/summary text but did NOT \
    call `finish`. Take the next CONCRETE step with tools now — write the file, run \
    the check, read the error. If the task is genuinely complete AND you have verified \
    it by running it, call `finish` with a one-line summary instead.)";

/// Continue-hint when the turn was cut off at the output-token cap.
const WORK_TRUNCATION_NUDGE: &str = "(your last turn was TRUNCATED mid-output at the \
    token cap. Resume exactly where you were cut off. Write file contents ONLY via \
    create_file/edit_file — never as chat text — and split large files across several \
    calls.)";

/// Which continue-hint the next turn gets.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Nudge {
    /// Cut off mid-tool-work (the 16-round cap).
    ToolTail,
    /// Ended on prose without `finish` (the #75/#69/#67 stall class).
    TextTail,
    /// Output-token truncation.
    Truncation,
}

/// Pure per-turn bookkeeping (native-tested): the toolless-strike counter and
/// the nudge pick. A truncated turn is CUT OFF, not a choice to stop talking —
/// it never counts as a strike. A toolless, textless, untruncated turn (empty
/// reply / all calls failed arg-parse — those emit no ToolCall chunk) gets the
/// generic continue nudge, not the "you ended with analysis" one.
fn turn_signals(prev_toolless: u32, tool_calls: u32, truncated: bool, saw_text: bool) -> (u32, Nudge) {
    let toolless = if tool_calls > 0 || truncated { 0 } else { prev_toolless + 1 };
    let nudge = if truncated {
        Nudge::Truncation
    } else if tool_calls == 0 && saw_text {
        Nudge::TextTail
    } else {
        Nudge::ToolTail
    };
    (toolless, nudge)
}

/// Pure continue-decision (native-tested). `finish` — read from the RESPONSE
/// (`reply.finished()`), never the chunk stream — is the one true stop signal;
/// the run also stops after 2 consecutive toolless turns (a model that keeps
/// narrating isn't converging — don't burn the cap; this also bounds a purely
/// conversational invocation at two rounds) or at the continuation cap.
fn work_should_continue(
    finished: bool,
    consecutive_toolless: u32,
    continuations: u32,
    max: u32,
) -> bool {
    !finished && consecutive_toolless < 2 && continuations < max
}

#[cfg(test)]
mod tests {
    use super::{turn_signals, work_should_continue, Nudge};

    #[test]
    fn finish_is_the_only_done_signal_mid_work() {
        // Mid-work (strikes clear, under cap) → continue.
        assert!(work_should_continue(false, 0, 0, 12));
        assert!(work_should_continue(false, 0, 11, 12));
        // finish called → stop, regardless of anything else.
        assert!(!work_should_continue(true, 0, 0, 12));
        assert!(!work_should_continue(true, 1, 3, 12));
    }

    #[test]
    fn strikes_and_caps_stop() {
        // ONE toolless turn → still continues (the TB-15 killer: prose design
        // turns used to END the run silently).
        assert!(work_should_continue(false, 1, 1, 12));
        // TWO consecutive toolless turns → stop (not converging; also bounds
        // a conversational `work "what is 2+2"` at two rounds).
        assert!(!work_should_continue(false, 2, 2, 12));
        // At the continuation cap → stop (never loop forever).
        assert!(!work_should_continue(false, 0, 12, 12));
    }

    #[test]
    fn turn_signals_strikes_and_nudges() {
        // Tool work resets strikes, generic nudge.
        assert_eq!(turn_signals(1, 3, false, true), (0, Nudge::ToolTail));
        // Prose-only turn: strike + the text-tail nudge.
        assert_eq!(turn_signals(0, 0, false, true), (1, Nudge::TextTail));
        assert_eq!(turn_signals(1, 0, false, true), (2, Nudge::TextTail));
        // Truncation: never a strike, always the truncation nudge — even when
        // text flowed before the cut.
        assert_eq!(turn_signals(1, 0, true, true), (0, Nudge::Truncation));
        assert_eq!(turn_signals(0, 2, true, false), (0, Nudge::Truncation));
        // Empty/parse-error turn (no tools, no text): strike + generic nudge,
        // NOT the "you ended with analysis" text (it would be a lie).
        assert_eq!(turn_signals(0, 0, false, false), (1, Nudge::ToolTail));
    }
}
