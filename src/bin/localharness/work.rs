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
    idiomatic. KEEP WORKING until the task is FULLY complete and verified — a hard \
    task takes MANY rounds of write → run → read the error → fix → re-run; do NOT \
    stop, summarize, or hand back until it actually works. When (and only when) it \
    is done AND you have run it to confirm, call the `finish` tool with a one-line \
    summary. Never call finish on an unverified or partial solution.";

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
    caps.compaction_threshold = Some(localharness::types::DEFAULT_COMPACTION_THRESHOLD);
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
    use futures_util::StreamExt;
    use std::io::Write;
    let mut input: std::borrow::Cow<str> = std::borrow::Cow::Borrowed(task.as_str());
    let mut continuations: u32 = 0;
    let mut failed = false;
    'run: loop {
        let reply = match agent.chat(input.as_ref()).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("\nwork failed: {e}");
                failed = true;
                break;
            }
        };
        // Stream the turn LIVE (text → stdout; tools/errors → stderr) and track HOW it
        // ended so we can tell "cut off mid-work" from "actually done".
        let mut cursor = reply.chunks();
        let mut saw_finish = false;
        let mut saw_goal_tool = false;
        let mut ended_on_tool = false;
        while let Some(res) = cursor.next().await {
            match res {
                Ok(localharness::types::StreamChunk::Text { text, .. }) => {
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                    ended_on_tool = false;
                }
                Ok(localharness::types::StreamChunk::ToolCall(tc)) => {
                    let args = serde_json::to_string(&tc.args).unwrap_or_default();
                    let args: String = args.chars().take(160).collect();
                    eprintln!("→ {}({args})", tc.name);
                    ended_on_tool = true;
                    if tc.name == "finish" {
                        saw_finish = true;
                    } else {
                        saw_goal_tool = true;
                    }
                }
                Ok(localharness::types::StreamChunk::ToolResult(tr)) => {
                    if let Some(err) = &tr.error {
                        eprintln!("  ✗ {}: {err}", tr.name);
                    }
                    ended_on_tool = true;
                }
                Ok(_) => {}
                Err(e) => {
                    eprintln!("\nwork failed: {e}");
                    failed = true;
                    break 'run;
                }
            }
        }
        if !work_should_continue(
            saw_finish,
            ended_on_tool,
            saw_goal_tool,
            continuations,
            MAX_WORK_CONTINUATIONS,
        ) {
            break;
        }
        continuations += 1;
        eprintln!("… auto-continue {continuations}/{MAX_WORK_CONTINUATIONS} (turn hit the 16-round cap mid-work)");
        input = std::borrow::Cow::Owned(WORK_CONTINUE_NUDGE.to_string());
    }
    println!();
    let _ = agent.shutdown().await;
    if failed { 1 } else { 0 }
}

/// The continue-hint fed to the agent when a turn was cut off at the round cap.
const WORK_CONTINUE_NUDGE: &str = "(continue — your last turn hit the tool-round cap \
    before finishing. Review what you've done, then take the NEXT concrete step: run \
    the result, read any error, fix it, re-run. Call `finish` only once it is actually \
    complete and verified.)";

/// Pure continue-decision (native-tested): keep going ONLY when the turn was cut off
/// mid-tool-work — its last streamed event was a tool call/result (the 16-round cap
/// guillotined an in-progress task) AND a goal tool ran AND `finish` was NOT called AND
/// we're under the continuation cap. A turn that ended on a TEXT answer, or that called
/// `finish`, is treated as done (a real conclusion or a conversational stop) — so the
/// loop never spams continuations on a completed or purely-conversational turn.
fn work_should_continue(
    saw_finish: bool,
    ended_on_tool: bool,
    saw_goal_tool: bool,
    continuations: u32,
    max: u32,
) -> bool {
    !saw_finish && ended_on_tool && saw_goal_tool && continuations < max
}

#[cfg(test)]
mod tests {
    use super::work_should_continue;

    #[test]
    fn continues_only_when_cut_off_mid_tool_work() {
        // Cut off mid-tool-work, finish not called, under cap → continue.
        assert!(work_should_continue(false, true, true, 0, 12));
        assert!(work_should_continue(false, true, true, 11, 12));
        // finish called → stop (done).
        assert!(!work_should_continue(true, true, true, 0, 12));
        // Ended on a TEXT answer (last event not a tool) → stop (done/conversational).
        assert!(!work_should_continue(false, false, true, 0, 12));
        // No goal tool ran (pure chat / ask_question only) → stop.
        assert!(!work_should_continue(false, true, false, 0, 12));
        // At the cap → stop (never loop forever).
        assert!(!work_should_continue(false, true, true, 12, 12));
    }
}
