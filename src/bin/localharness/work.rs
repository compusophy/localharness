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
     model round from your meter (~1 $LH each). e.g.\n  \
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
    eprintln!(
        "work: native agent in {} (billed ~1 $LH per model round; Ctrl-C aborts)",
        cwd.display()
    );

    let fs: localharness::filesystem::SharedFilesystem =
        std::sync::Arc::new(localharness::filesystem::NativeFilesystem::new());
    let mut cfg = localharness::GeminiAgentConfig::new(token)
        .with_base_url(base)
        .with_system_instructions(WORK_PROMPT.to_string())
        .with_filesystem(fs)
        // The DEFAULT capability set is read-only safety mode — a work agent
        // needs the write half (create/edit/delete/rename + run_command);
        // containment comes from the workspace policy below, not tool absence.
        .with_capabilities(localharness::types::CapabilitiesConfig::unrestricted())
        // Every fs path is pinned inside the workspace, and a wildcard allow
        // sits BEHIND the denies: `evaluate` is default-deny once any policy
        // exists, and workspace_only only contributes deny-when-outside rules
        // — without the trailing allow, every in-workspace call died with
        // "no matching policy" (found live). Deny buckets evaluate first, so
        // containment still wins.
        .with_policies(
            localharness::policy::workspace_only(vec![cwd.clone()])
                .into_iter()
                .chain(std::iter::once(localharness::policy::Policy::allow("*")))
                .collect::<Vec<_>>(),
        );
    if let Some(m) = &model {
        cfg = cfg.with_model(m.clone());
    }

    let agent = match localharness::Agent::start_gemini(cfg).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("start failed: {e}");
            return 1;
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
