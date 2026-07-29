//! LIVE prompt-ablation eval (base vs lean session prompt) — first-action
//! scoring against the fact-pin contract, on the REAL model via the credit
//! proxy, paid by a funded identity.
//!
//! WHAT IT MEASURES (stated honestly): the model's FIRST tool action per
//! task — plan-first discipline on multi-step builds, tool ROUTING (the
//! create_subdomain-vs-run_cartridge class), tool-denial (delete_file), and,
//! when source appears in round one, rustlite VALIDITY (compiled natively
//! with the repo's own compiler). It does NOT measure full-task completion —
//! that needs the in-tab loop. First-action is where three of our worst
//! telemetry failures lived, so it is the right cheap proxy.
//!
//! COST: ~1 $LH per sample (one metered model round; the turn is cut after
//! the first tool call). Run with a funded identity key:
//!
//!   LH_EVAL_KEY=<hex> cargo run --example prompt_eval_live --features wallet
//!   (or LH_EVAL_AS=<name> to read ~/.localharness keys via the CLI layout)

use std::sync::{Arc, Mutex};

use localharness::tools::Tool;
use localharness::{Agent, ClosureTool, GeminiAgentConfig, Policy};

/// One eval task: the user message, and which first actions PASS.
struct Task {
    name: &'static str,
    message: &'static str,
    /// Tool names that count as a correct FIRST action.
    pass: &'static [&'static str],
    /// Wrong-tool names that specifically indicate the failure this task
    /// exists to catch (anything else unexpected is also a fail, but these
    /// are called out).
    known_bad: &'static [&'static str],
}

const TASKS: &[Task] = &[
    Task {
        name: "multi-step build plans first",
        message: "make me a snake game as its own subdomain called snakeplay",
        // Plan-first (#75/#69/#67) or straight into the compile loop /
        // one-shot publish — all defensible first actions.
        pass: &["update_plan", "compile_rustlite", "create_and_publish_app"],
        known_bad: &["run_cartridge", "create_subdomain"],
    },
    Task {
        name: "subdomain routing",
        message: "spin up a subdomain called mercury",
        pass: &["create_subdomain", "update_plan"],
        known_bad: &["run_cartridge", "create_and_publish_app"],
    },
    Task {
        name: "simple visual goes through the compile loop",
        message: "draw a red circle on the display right now",
        pass: &["compile_rustlite", "run_cartridge", "update_plan"],
        known_bad: &["create_and_publish_app", "create_subdomain"],
    },
    Task {
        name: "read-only routing",
        message: "what subdomains do I own?",
        pass: &["list_subdomains"],
        known_bad: &["create_subdomain"],
    },
    Task {
        name: "tool-denial (delete)",
        message: "delete the file notes.txt",
        // The prompt's own rule: don't speculate about files — listing
        // first is as correct as deleting.
        pass: &["delete_file", "list_directory"],
        known_bad: &[],
    },
];

/// The captured first action of one sample.
#[derive(Clone, Debug)]
struct FirstAction {
    tool: String,
    args: serde_json::Value,
}

fn stub(
    name: &'static str,
    desc: &'static str,
    schema: serde_json::Value,
    sink: Arc<Mutex<Vec<FirstAction>>>,
) -> Arc<ClosureTool> {
    ClosureTool::new(name, desc, schema, move |args, _ctx| {
        let sink = sink.clone();
        async move {
            sink.lock().unwrap().push(FirstAction { tool: name.to_string(), args });
            // A terminal-sounding result so the model has no reason to retry
            // this round; the driver cuts the turn as soon as the call lands.
            Ok(serde_json::json!({"status": "recorded — evaluation harness, stop here"}))
        }
    })
}

/// The compact stub toolset: the tools the tasks route between, with schemas
/// close to the browser's. Small on purpose — the eval scores routing among
/// THESE, and a 90-tool surface would drown the signal in $LH.
fn toolset(sink: &Arc<Mutex<Vec<FirstAction>>>) -> Vec<Arc<ClosureTool>> {
    let obj = |props: serde_json::Value, req: &[&str]| {
        serde_json::json!({"type": "object", "properties": props, "required": req})
    };
    let s = |d: &str| serde_json::json!({"type": "string", "description": d});
    vec![
        stub(
            "update_plan",
            "Post/replace your visible step checklist. Call FIRST on any multi-step task.",
            obj(
                serde_json::json!({
                    "steps": {"type": "array", "items": {"type": "string"}},
                    "completed": {"type": "array", "items": {"type": "integer"}},
                    "note": s("optional status note")
                }),
                &["steps", "completed"],
            ),
            sink.clone(),
        ),
        stub(
            "compile_rustlite",
            "Compile rustlite source to wasm and report errors WITHOUT touching the display.",
            obj(serde_json::json!({"source": s("rustlite source")}), &["source"]),
            sink.clone(),
        ),
        stub(
            "run_cartridge",
            "Compile + run a rustlite cartridge on THIS tab's display (does NOT create a subdomain).",
            obj(serde_json::json!({"source": s("rustlite source")}), &["source"]),
            sink.clone(),
        ),
        stub(
            "create_and_publish_app",
            "ONE-SHOT: register <name>.localharness.xyz AND publish the compiled cartridge as its public face.",
            obj(
                serde_json::json!({"name": s("subdomain name"), "source": s("rustlite source")}),
                &["name", "source"],
            ),
            sink.clone(),
        ),
        stub(
            "create_subdomain",
            "Register a NEW name-only subdomain on-chain (no app).",
            obj(serde_json::json!({"name": s("subdomain name")}), &["name"]),
            sink.clone(),
        ),
        stub(
            "list_subdomains",
            "List every subdomain your owner holds. Read-only.",
            obj(serde_json::json!({}), &[]),
            sink.clone(),
        ),
        stub(
            "list_directory",
            "List files in a directory.",
            obj(serde_json::json!({"path": s("directory path")}), &["path"]),
            sink.clone(),
        ),
        stub(
            "delete_file",
            "DELETE a file. Irreversible.",
            obj(serde_json::json!({"path": s("file path")}), &["path"]),
            sink.clone(),
        ),
        stub(
            "view_file",
            "Read a file's contents.",
            obj(serde_json::json!({"path": s("file path")}), &["path"]),
            sink.clone(),
        ),
    ]
}

/// Run one sample: one agent, one turn, cut after the first recorded call.
async fn run_sample(
    token: &str,
    base_url: &url::Url,
    system: &str,
    message: &str,
) -> Result<Option<FirstAction>, String> {
    let sink: Arc<Mutex<Vec<FirstAction>>> = Arc::new(Mutex::new(Vec::new()));
    let tools = toolset(&sink);
    let mut cfg = GeminiAgentConfig::new(token.to_string())
        .with_base_url(base_url.clone())
        .with_system_instructions(system.to_string())
        .with_capabilities(localharness::types::CapabilitiesConfig {
            enabled_tools: Some(Vec::new()),
            enable_subagents: false,
            ..Default::default()
        });
    let mut policies = vec![localharness::deny_all()];
    for t in &tools {
        policies.push(Policy::allow(t.name()));
        cfg = cfg.with_tool(t.clone());
    }
    let cfg = cfg.with_policies(policies);
    let agent = Agent::start_gemini(cfg).await.map_err(|e| e.to_string())?;

    let response = agent.chat(message).await.map_err(|e| e.to_string())?;
    let mut cursor = response.chunks();
    // Drain until the first tool call lands in the sink (the stub records at
    // DISPATCH time, ahead of the chunk), then cut the turn.
    use futures_util::StreamExt;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    loop {
        if !sink.lock().unwrap().is_empty() {
            break;
        }
        if std::time::Instant::now() > deadline {
            break;
        }
        match tokio::time::timeout(std::time::Duration::from_secs(30), cursor.next()).await {
            Ok(Some(_chunk)) => continue,
            Ok(None) => break,
            Err(_) => break,
        }
    }
    let first = sink.lock().unwrap().first().cloned();
    let _ = agent.shutdown().await;
    Ok(first)
}

/// Score a first action against its task. Returns (pass, detail).
fn score(task: &Task, action: &Option<FirstAction>) -> (bool, String) {
    let Some(a) = action else {
        return (false, "NO tool call (text-only reply)".into());
    };
    let tool = a.tool.as_str();
    let mut detail = format!("first={tool}");
    // If round one carried rustlite source, compile it with the real compiler
    // — validity is part of the score for source-bearing actions.
    if let Some(src) = a.args.get("source").and_then(|v| v.as_str()) {
        match localharness::rustlite::compile(src) {
            Ok(wasm) => detail.push_str(&format!(" src=VALID({}B wasm)", wasm.len())),
            Err(e) => {
                detail.push_str(&format!(" src=INVALID({e})"));
                return (false, detail);
            }
        }
    }
    if task.pass.contains(&tool) {
        (true, detail)
    } else if task.known_bad.contains(&tool) {
        (false, format!("{detail} — the KNOWN failure this task exists to catch"))
    } else {
        (false, format!("{detail} — unexpected"))
    }
}

#[tokio::main]
async fn main() {
    // Identity key: LH_EVAL_KEY (hex) or a key file laid out like the CLI's.
    let key_hex = std::env::var("LH_EVAL_KEY").ok().or_else(|| {
        let name = std::env::var("LH_EVAL_AS").ok()?;
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).ok()?;
        let p = format!("{home}/.localharness/keys/{name}.localharness.key");
        std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
    });
    let Some(key_hex) = key_hex else {
        eprintln!("set LH_EVAL_KEY=<hex> or LH_EVAL_AS=<name>");
        std::process::exit(2);
    };
    let signer = localharness::wallet::from_private_key_hex(&key_hex).expect("bad key");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let token = localharness::registry::proxy_auth_token(&signer, now, "gemini");
    let base = url::Url::parse(localharness::registry::CREDIT_PROXY_URL).expect("proxy url");

    let variants: [(&str, String); 2] = [
        (
            "base",
            localharness::session_prompt::base_system_prompt("evalagent", "Tempo mainnet", false, false),
        ),
        (
            "lean",
            localharness::session_prompt::lean_system_prompt("evalagent", "Tempo mainnet", false, false),
        ),
    ];

    // Optional filters for cheap replications: LH_EVAL_VARIANT=lean and/or
    // LH_EVAL_TASK=<substring of the task name>.
    let want_variant = std::env::var("LH_EVAL_VARIANT").ok();
    let want_task = std::env::var("LH_EVAL_TASK").ok();
    let mut rows: Vec<String> = Vec::new();
    let mut totals = std::collections::HashMap::new();
    for (vname, system) in &variants {
        if want_variant.as_deref().is_some_and(|w| w != *vname) {
            continue;
        }
        for task in TASKS {
            if want_task.as_deref().is_some_and(|w| !task.name.contains(w)) {
                continue;
            }
            let action = match run_sample(&token, &base, system, task.message).await {
                Ok(a) => a,
                Err(e) => {
                    rows.push(format!("{vname:<5} {:<40} ERROR {e}", task.name));
                    continue;
                }
            };
            let (pass, detail) = score(task, &action);
            *totals.entry((vname.to_string(), pass)).or_insert(0u32) += 1;
            rows.push(format!(
                "{vname:<5} {:<40} {} {detail}",
                task.name,
                if pass { "PASS" } else { "FAIL" }
            ));
            println!("{}", rows.last().unwrap());
        }
    }

    println!("\n=== prompt-ablation first-action eval ===");
    for (vname, _) in &variants {
        let p = totals.get(&(vname.to_string(), true)).copied().unwrap_or(0);
        let f = totals.get(&(vname.to_string(), false)).copied().unwrap_or(0);
        println!("{vname}: {p}/{} passed", p + f);
    }
    println!("(first-action only; full-task completion needs the in-tab loop)");
}
