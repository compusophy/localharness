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
    first, act, VERIFY by running the result, and only then answer. Keep file \
    edits minimal and idiomatic. When the task is done, reply with a short \
    summary of what you changed and how you verified it.";

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

    let reply = match agent.chat(task.as_str()).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("work failed: {e}");
            let _ = agent.shutdown().await;
            return 1;
        }
    };
    // Stream the run LIVE: final text to stdout; tool activity + errors to
    // stderr so the terminal shows what the agent is doing (and a failing
    // tool is visible instead of silently burning meter rounds).
    use futures_util::StreamExt;
    let mut cursor = reply.chunks();
    let mut failed = false;
    while let Some(res) = cursor.next().await {
        match res {
            Ok(localharness::types::StreamChunk::Text { text, .. }) => {
                print!("{text}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            Ok(localharness::types::StreamChunk::ToolCall(tc)) => {
                let args = serde_json::to_string(&tc.args).unwrap_or_default();
                let args: String = args.chars().take(160).collect();
                eprintln!("→ {}({args})", tc.name);
            }
            Ok(localharness::types::StreamChunk::ToolResult(tr)) => {
                if let Some(err) = &tr.error {
                    eprintln!("  ✗ {}: {err}", tr.name);
                }
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("\nwork failed: {e}");
                failed = true;
                break;
            }
        }
    }
    println!();
    let _ = agent.shutdown().await;
    if failed { 1 } else { 0 }
}
