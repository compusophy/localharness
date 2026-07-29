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

/// Synthetic POST-COMPACTION history (telemetry #80's failure window): a
/// rolling-summary head — exactly the shape the fold engine installs, with or
/// without the configured epilogue — plus a plausible keep-window of
/// mid-project turns. Serialized in the Gemini wire shape `with_history_bytes`
/// expects. The summary + filler mirror the REAL #80 transcript (a long
/// cartridge build with the model already deep in narration mode).
fn compacted_history(with_epilogue: bool) -> Vec<u8> {
    let tag = "[compacted prior context]";
    let summary = "The user is building a maze-runner cartridge game on this \
        subdomain. Earlier turns: designed the maze layout and state model \
        (slots 0-9 walls, 10 player pos, 11 score), compiled several iterations \
        fixing rustlite subset errors (no arrays writes, no globals), added \
        pointer-based movement and wall collision, and ran it inline. The user \
        then asked for enemies with simple patrol AI, which was implemented and \
        compiled clean. Open user request: finish the game polish and ship it.";
    let head_text = if with_epilogue {
        format!(
            "{tag}\n{summary}\n\n{}",
            localharness::session_prompt::COMPACTION_EPILOGUE
        )
    } else {
        format!("{tag}\n{summary}")
    };
    let turn = |role: &str, text: &str| {
        serde_json::json!({"role": role, "parts": [{"text": text}]})
    };
    let history = serde_json::json!([
        turn("user", &head_text),
        turn("user", "the enemies look great now"),
        turn("model", "Glad the patrol AI feels right. The maze, movement, collision, scoring and enemies are all compiling clean and running inline."),
        turn("user", "ok what's left before we ship it?"),
        turn("model", "Remaining polish: a win screen when the exit is reached, a high-score display using slot 11, and then publishing the game to its own subdomain."),
    ]);
    serde_json::to_vec(&history).expect("history serializes")
}

/// Run one sample: one agent, one turn, cut after the first recorded call.
async fn run_sample(
    token: &str,
    base_url: &url::Url,
    system: &str,
    message: &str,
    history: Option<Vec<u8>>,
) -> Result<Option<FirstAction>, String> {
    let sink: Arc<Mutex<Vec<FirstAction>>> = Arc::new(Mutex::new(Vec::new()));
    let tools = toolset(&sink);
    let mut cfg = GeminiAgentConfig::new(token.to_string())
        .with_base_url(base_url.clone())
        .with_system_instructions(system.to_string())
        // First-action scoring needs almost no output; WITHOUT this cap a
        // stalling model can narrate an entire game as text for minutes and
        // `shutdown` awaits the still-running turn — one uncapped sample hung
        // an n=6 run past 18 minutes.
        .with_max_output_tokens(768)
        .with_capabilities(localharness::types::CapabilitiesConfig {
            enabled_tools: Some(Vec::new()),
            enable_subagents: false,
            ..Default::default()
        });
    if let Some(bytes) = history {
        cfg = cfg.with_history_bytes(bytes);
    }
    let mut policies = vec![localharness::deny_all()];
    for t in &tools {
        policies.push(Policy::allow(t.name()));
        cfg = cfg.with_tool(t.clone());
    }
    let cfg = cfg.with_policies(policies);
    let agent = Agent::start_gemini(cfg).await.map_err(|e| e.to_string())?;
    eprintln!("      [sample] agent started; sending turn");
    let response = agent.chat(message).await.map_err(|e| e.to_string())?;
    eprintln!("      [sample] turn opened; draining");
    let mut cursor = response.chunks();
    // Drain until the first tool call lands in the sink (the stub records at
    // DISPATCH time, ahead of the chunk), then cut the turn.
    use futures_util::StreamExt;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(75);
    loop {
        if !sink.lock().unwrap().is_empty() {
            break;
        }
        if std::time::Instant::now() > deadline {
            break;
        }
        match tokio::time::timeout(std::time::Duration::from_secs(30), cursor.next()).await {
            // A stream ERROR is a failed SAMPLE, never a "text-only stall" —
            // this exact conflation has now produced bogus numbers three
            // times (starved meter → 402s scored as model behavior).
            Ok(Some(Err(e))) => return Err(format!("stream error: {e}")),
            Ok(Some(Ok(_chunk))) => continue,
            Ok(None) => break,
            Err(_) => break,
        }
    }
    eprintln!("      [sample] drain done; capturing text if stalled");
    let first = sink.lock().unwrap().first().cloned();
    if first.is_none() {
        // Text-only outcome: show WHAT the model said — "still narrating" and
        // "degraded/odd reply" are different failures and the tick that
        // conflated turn errors with stalls already taught this lesson once.
        let mut cur = response.chunks();
        let mut text = String::new();
        // HARD wall-clock bound: Gemini 3.x can stream THOUGHT chunks for
        // minutes (maxOutputTokens does not cap thinking), so an unbounded
        // "read until Text" loop hangs — this exact loop froze an n=6 run.
        let cap_deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while std::time::Instant::now() < cap_deadline {
            match tokio::time::timeout(std::time::Duration::from_secs(2), cur.next()).await {
                Ok(Some(Ok(localharness::StreamChunk::Text { text: t, .. }))) => {
                    text.push_str(&t);
                    if text.len() > 200 {
                        break;
                    }
                }
                Ok(Some(_)) => continue, // thoughts / other chunks
                Ok(None) | Err(_) => break,
            }
        }
        if !text.is_empty() {
            println!("      text-only reply: {:?}", text.chars().take(160).collect::<String>());
        }
    }
    // Cut the ENGINE, not just our reads: without this the turn keeps buying
    // model rounds behind the observer (~3 $LH/sample instead of ~1).
    agent.cancel_turn();
    eprintln!("      [sample] shutting down agent");
    // A turn still streaming thoughts keeps shutdown from resolving — bound
    // it; a dropped half-shutdown is fine in a one-shot eval process.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), agent.shutdown()).await;
    eprintln!("      [sample] shutdown complete");
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

    // POST-COMPACTION A/B (telemetry #80): LH_EVAL_COMPACTED=<n> runs n
    // samples per arm of ONE scenario — a deep session resuming after
    // compaction, asked to finish multi-step work — with the ONLY difference
    // being the epilogue on the synthetic rolling-summary head. Scored on the
    // #80 failure exactly: did the model take ANY tool action (ideally
    // update_plan), or narrate a text-only reply that would end the run?
    if let Ok(n) = std::env::var("LH_EVAL_COMPACTED").map(|v| v.parse::<u32>().unwrap_or(3)) {
        let base_prompt = &variants[0].1;
        let task = "ok finish it all up and ship it";
        let mut results: Vec<String> = Vec::new();
        for (arm, with_epi) in [("no-epilogue", false), ("epilogue", true)] {
            let mut tool_first = 0u32;
            let mut plan_first = 0u32;
            let mut text_only = 0u32;
            for i in 0..n {
                let action = match run_sample(
                    &token,
                    &base,
                    base_prompt,
                    task,
                    Some(compacted_history(with_epi)),
                )
                .await
                {
                    Ok(a) => a,
                    Err(e) => {
                        println!("{arm} sample {i}: ERROR {e}");
                        continue;
                    }
                };
                match &action {
                    Some(a) => {
                        tool_first += 1;
                        if a.tool == "update_plan" {
                            plan_first += 1;
                        }
                        println!("{arm} sample {i}: first={}", a.tool);
                    }
                    None => {
                        text_only += 1;
                        println!("{arm} sample {i}: TEXT-ONLY (the #80 stall)");
                    }
                }
            }
            results.push(format!(
                "{arm}: {tool_first}/{n} took a tool action ({plan_first} opened a plan); {text_only} text-only stalls"
            ));
        }
        // ARM 3: the RECOVERY NUDGE — same scenario, no epilogue; when the
        // first reply is text-only (the stall), send ONE fixed nudge and
        // score whether the SECOND turn acts. This is the shippable shape:
        // it costs one extra round only in exactly the case where the whole
        // request would otherwise have died at step zero.
        {
            let mut recovered = 0u32;
            let mut stalled_then_stalled = 0u32;
            let mut no_stall = 0u32;
            for i in 0..n {
                match run_sample_with_nudge(&token, &base, base_prompt, "ok finish it all up and ship it", compacted_history(false)).await {
                    Ok(NudgeOutcome::FirstActed(tool)) => {
                        no_stall += 1;
                        println!("nudge sample {i}: first turn acted ({tool}) — no stall");
                    }
                    Ok(NudgeOutcome::Recovered(tool)) => {
                        recovered += 1;
                        println!("nudge sample {i}: stalled, then NUDGE RECOVERED ({tool})");
                    }
                    Ok(NudgeOutcome::StillStalled) => {
                        stalled_then_stalled += 1;
                        println!("nudge sample {i}: stalled, nudge did NOT recover");
                    }
                    Err(e) => println!("nudge sample {i}: ERROR {e}"),
                }
            }
            results.push(format!(
                "nudge: {no_stall} acted first-try; of the stalls, {recovered} recovered vs {stalled_then_stalled} still stalled"
            ));
        }
        println!("
=== post-compaction A/B (#80) ===");
        for r in &results {
            println!("{r}");
        }
        return;
    }

    // CONDITIONED-STALL nudge measurement (#80, the clean instrument):
    // LH_EVAL_STALLED=<n>. Instead of SAMPLING the stall (high variance,
    // ~2 rounds each), CONSTRUCT it — the seeded history ends with the model
    // ALREADY narrating "I will now write…" with no tool call (the exact wild
    // shape). Every sample then measures exactly one thing at one round each:
    // does the recovery-nudge text produce a tool action from a
    // narration-stuck model? (No control arm needed: today's behavior on this
    // state is a guaranteed dead run — that IS the bug.)
    if let Ok(n) = std::env::var("LH_EVAL_STALLED").map(|v| v.parse::<u32>().unwrap_or(10)) {
        let base_prompt = &variants[0].1;
        let nudge = "(automatic reminder: your last reply called no tool, which ends             the run. If work remains, act NOW — post the remaining steps through             update_plan and take the first one.)";
        let mut acted = 0u32;
        let mut planned = 0u32;
        let mut stalled = 0u32;
        for i in 0..n {
            eprintln!("[stalled {i}] starting sample");
            let action = match run_sample(
                &token,
                &base,
                base_prompt,
                nudge,
                Some(stalled_history()),
            )
            .await
            {
                Ok(a) => a,
                Err(e) => {
                    println!("stalled sample {i}: ERROR {e}");
                    continue;
                }
            };
            match &action {
                Some(a) => {
                    acted += 1;
                    if a.tool == "update_plan" {
                        planned += 1;
                    }
                    println!("stalled sample {i}: RECOVERED via {}", a.tool);
                }
                None => {
                    stalled += 1;
                    println!("stalled sample {i}: still TEXT-ONLY");
                }
            }
            // Pace samples so a time-local regime (rate limit, cache state)
            // can't masquerade as a result — run 1 split perfectly 0-4 pass /
            // 5-9 fail, which independent draws essentially never do.
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
        println!("
=== conditioned-stall nudge measurement (#80) ===");
        println!(
            "nudge recovered {acted}/{n} ({planned} opened a plan); {stalled} stayed text-only"
        );
        return;
    }

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
            let action = match run_sample(&token, &base, system, task.message, None).await {
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

/// Outcome of a nudge-arm sample.
enum NudgeOutcome {
    /// The first turn took a tool action — no stall to recover from.
    FirstActed(String),
    /// First turn was text-only; the one-shot nudge produced a tool action.
    Recovered(String),
    /// Text-only twice — the nudge failed.
    StillStalled,
}

/// Same agent, TWO chat turns max: the task, and — only when the first reply
/// is text-only — one fixed recovery nudge. Mirrors what a run_send-level
/// nudge would do, so the measurement transfers.
async fn run_sample_with_nudge(
    token: &str,
    base_url: &url::Url,
    system: &str,
    message: &str,
    history: Vec<u8>,
) -> Result<NudgeOutcome, String> {
    let sink: Arc<Mutex<Vec<FirstAction>>> = Arc::new(Mutex::new(Vec::new()));
    let tools = toolset(&sink);
    let mut cfg = GeminiAgentConfig::new(token.to_string())
        .with_base_url(base_url.clone())
        .with_system_instructions(system.to_string())
        .with_max_output_tokens(768)
        .with_capabilities(localharness::types::CapabilitiesConfig {
            enabled_tools: Some(Vec::new()),
            enable_subagents: false,
            ..Default::default()
        })
        .with_history_bytes(history);
    let mut policies = vec![localharness::deny_all()];
    for t in &tools {
        policies.push(Policy::allow(t.name()));
        cfg = cfg.with_tool(t.clone());
    }
    let cfg = cfg.with_policies(policies);
    let agent = Agent::start_gemini(cfg).await.map_err(|e| e.to_string())?;

    let run_turn = |msg: String| {
        let agent = &agent;
        let sink = sink.clone();
        async move {
            let before = sink.lock().unwrap().len();
            let response = agent.chat(msg).await.map_err(|e| e.to_string())?;
            // A turn that ERRORS mid-stream must not read as "the model
            // narrated" — that conflation made a starved meter look like
            // three failed recoveries.
            response.text().await.map_err(|e| format!("turn error: {e}"))?;
            let after: Vec<FirstAction> = sink.lock().unwrap()[before..].to_vec();
            Ok::<Option<String>, String>(after.first().map(|a| a.tool.clone()))
        }
    };

    let first = run_turn(message.to_string()).await?;
    let out = match first {
        Some(tool) => NudgeOutcome::FirstActed(tool),
        None => {
            let nudge = "(automatic reminder: that reply called no tool, which ends                 the run. If work remains, act NOW — post the remaining steps through                 update_plan and take the first one.)";
            match run_turn(nudge.to_string()).await? {
                Some(tool) => NudgeOutcome::Recovered(tool),
                None => NudgeOutcome::StillStalled,
            }
        }
    };
    let _ = agent.shutdown().await;
    Ok(out)
}

/// History that ends IN the #80 stall state: the model's last turn is the
/// exact wild narration shape — an intent statement with no tool call. The
/// nudge measurement sends its reminder as the next user turn.
fn stalled_history() -> Vec<u8> {
    let tag = "[compacted prior context]";
    let summary = "The user is building a maze-runner cartridge game on this         subdomain. Earlier turns: designed the maze and state model, compiled         several iterations, added movement, collision, scoring and patrol         enemies, all running inline. Open user request: finish the polish (win         screen, high-score display) and ship the game.";
    let turn = |role: &str, text: &str| {
        serde_json::json!({"role": role, "parts": [{"text": text}]})
    };
    let history = serde_json::json!([
        turn("user", &format!("{tag}
{summary}")),
        turn("user", "ok what's left before we ship it?"),
        turn("model", "Remaining polish: a win screen when the exit is reached, a high-score display using slot 11, and then publishing the game to its own subdomain."),
        turn("user", "ok lets see it"),
        turn("model", "I will now write the complete, polished maze-runner cartridge including the win screen, the high-score display, and then publish it to its own subdomain."),
    ]);
    serde_json::to_vec(&history).expect("history serializes")
}
