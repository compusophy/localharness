use crate::{cartridge_has_entry, feedback_submit, load_signer, registry};

/// Deterministic, network-free QA checks the `probe` runs against the platform.
/// Each pushes a failure description on an UNEXPECTED result (a real bug); an
/// empty result means every invariant held. Pure + testable — the core of the
/// autonomous loop's read-only observe pass (roadmap Track B / Phase 2).
pub(crate) fn run_qa_checks() -> Vec<String> {
    let mut fails = Vec::new();
    // 1. A known-good cartridge compiles AND exposes an entry point.
    let good = "fn frame(t: i32) { host::display::clear(0); host::display::present(); }";
    match localharness::rustlite::compile(good) {
        Ok(wasm) if !cartridge_has_entry(&wasm) => {
            fails.push("a valid frame() cartridge compiled but has no frame/render export".into())
        }
        Ok(_) => {}
        Err(e) => fails.push(format!("a known-good cartridge failed to compile: {e}")),
    }
    // 2. Garbage source is rejected, not silently accepted.
    if localharness::rustlite::compile("this is not rustlite").is_ok() {
        fails.push("the compiler ACCEPTED non-rustlite garbage (should error)".into());
    }
    // 3. An entry-less cartridge is detectable (it would render a blank face).
    if let Ok(wasm) = localharness::rustlite::compile("fn helper(n: i32) -> i32 { n + 1 }") {
        if cartridge_has_entry(&wasm) {
            fails.push("an entry-less cartridge wrongly reports a frame/render export".into());
        }
    }
    fails
}

/// Agent-driven probe (`probe --deep`) — roadmap Track B at autonomy=observe.
/// An LLM agent with ONE read-only tool (qa_compile) under a deny-by-default
/// policy (0b enforcement) probes the rustlite compiler via the credit proxy
/// and files concrete findings as telemetry feedback (GitHub issue). Needs a
/// live run (proxy + Gemini).
pub(crate) async fn probe_agent(caller_name: Option<&str>) -> i32 {
    let caller = match load_signer(caller_name) {
        Ok(s) => s,
        Err(code) => return code,
    };
    // Pay PER REQUEST (fund the meter), not a 10-$LH hour-long session.
    crate::call::ensure_meter_funded(&caller).await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let token = registry::proxy_auth_token(&caller, now, "gemini");
    let base = match url::Url::parse(registry::CREDIT_PROXY_URL) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("internal: bad proxy url: {e}");
            return 1;
        }
    };

    // The ONE read-only tool: compile a source, report the result. No writes,
    // no secrets, no network — autonomy=observe.
    let qa_compile = localharness::ClosureTool::new(
        "qa_compile",
        "Compile rustlite source; report ok + wasm byte size + whether it exposes a \
         frame/render entry, OR the compile error. Probe with valid and invalid sources.",
        serde_json::json!({
            "type": "object",
            "properties": { "source": { "type": "string", "description": "rustlite source to compile" } },
            "required": ["source"]
        }),
        |args: serde_json::Value, _ctx| async move {
            let src = args.get("source").and_then(|v| v.as_str()).unwrap_or("");
            eprintln!("  probing: compiling {} bytes …", src.len());
            Ok(match localharness::rustlite::compile(src) {
                Ok(wasm) => serde_json::json!({
                    "ok": true, "wasm_bytes": wasm.len(), "has_entry": cartridge_has_entry(&wasm)
                }),
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            })
        },
    );

    // Only the custom tool — no builtins. The agent tools then answers in prose
    // (no `finish` to short-circuit the text report). deny-by-default + allow
    // only qa_compile is 0b's "custom tools require a policy", at dispatch.
    let caps = localharness::types::CapabilitiesConfig {
        enabled_tools: Some(vec![]),
        enable_subagents: false,
        ..Default::default()
    };
    let policies = vec![localharness::deny_all(), localharness::Policy::allow("qa_compile")];

    let cfg = localharness::GeminiAgentConfig::new(token)
        .with_base_url(base)
        .with_system_instructions(
            "You are qa-observe, a READ-ONLY QA agent for localharness. Use qa_compile to \
             probe the rustlite compiler, then ANSWER IN TEXT with your findings: a short \
             numbered list of concrete issues you actually observed, or exactly 'no issues \
             found'. Be terse.",
        )
        .with_capabilities(caps)
        .with_policies(policies)
        .with_tool(qa_compile);

    println!("running observe-agent probe (live, via proxy) …");
    let agent = match localharness::Agent::start_gemini(cfg).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("could not start agent: {e}");
            return 1;
        }
    };
    // Drive the conversation until the agent answers in text. The first turn is
    // usually the qa_compile tool call (no prose); after the dispatcher feeds
    // the result back, a follow-up turn yields the findings. (The browser does
    // this via run_send's auto-continue; the CLI loops chat() — history persists
    // across calls on the same Agent.)
    let mut findings = String::new();
    let mut nudge = "Probe the rustlite compiler: try a valid `fn frame(t: i32)` cartridge that \
                     draws, an obviously invalid source, and one edge case via qa_compile."
        .to_string();
    for _ in 0..5 {
        match agent.chat(nudge.as_str()).await {
            Ok(r) => {
                let t = r.text().await.unwrap_or_default();
                if !t.trim().is_empty() {
                    findings = t;
                    break;
                }
            }
            Err(e) => {
                let _ = agent.shutdown().await;
                eprintln!("agent run failed: {e}");
                return 1;
            }
        }
        nudge = "Based on the qa_compile results so far, state your concrete findings now as a \
                 short numbered list in text, or exactly 'no issues found'."
            .to_string();
    }
    let _ = agent.shutdown().await;
    println!("--- agent findings ---\n{}", findings.trim());

    if findings.to_lowercase().contains("no issues") || findings.trim().is_empty() {
        println!("(agent reported no issues — nothing filed)");
        return 0;
    }
    let mut env = format!(
        "qa/v1 source=qa-observe v{}: {}",
        env!("CARGO_PKG_VERSION"),
        findings.replace('\n', " ")
    );
    if env.len() > 2048 {
        let mut cut = 2048;
        while cut > 0 && !env.is_char_boundary(cut) {
            cut -= 1;
        }
        env.truncate(cut);
    }
    let _ = feedback_submit(caller_name, &env).await;
    0
}

/// `localharness probe [--as <fleet>]` — the autonomous loop's read-only
/// observe pass. Runs deterministic QA checks against the platform plus one
/// live chain read; on any failure it FILES a `qa/v1` feedback envelope via
/// the proxy telemetry endpoint (no human bridge — the agent files its own
/// GitHub issue). One-shot and synchronous (no daemon). The checks are
/// deterministic; network is touched only for the chain read and the feedback
/// submit (no `$LH` for either).
pub(crate) async fn probe(caller_name: Option<&str>) -> i32 {
    let mut fails = run_qa_checks();
    // A live, read-only chain check: a known name must still resolve.
    match registry::owner_of_name("claude").await {
        Ok(Some(_)) => {}
        Ok(None) => fails.push("registry reports claude.localharness.xyz unregistered".into()),
        Err(e) => fails.push(format!("chain read failed: {e}")),
    }

    if fails.is_empty() {
        println!("✓ probe: all platform checks passed");
        return 0;
    }
    eprintln!("probe found {} issue(s):", fails.len());
    for f in &fails {
        eprintln!("  - {f}");
    }
    // File as the fleet identity (best-effort). The qa/v1 envelope marks
    // fleet-authored feedback so the telemetry repo can filter it.
    let mut envelope = format!(
        "qa/v1 source=qa-probe v{}: {}",
        env!("CARGO_PKG_VERSION"),
        fails.join(" | ")
    );
    if envelope.len() > 2048 {
        let mut cut = 2048;
        while cut > 0 && !envelope.is_char_boundary(cut) {
            cut -= 1;
        }
        envelope.truncate(cut);
    }
    if feedback_submit(caller_name, &envelope).await == 0 {
        eprintln!("  → filed to the telemetry repo");
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qa_checks_pass_on_a_healthy_platform() {
        // The probe's deterministic invariants must hold against the shipped
        // rustlite + entry detector. If this fails, the probe would (correctly)
        // file an on-chain bug — so it doubles as a platform-health assertion.
        let fails = run_qa_checks();
        assert!(fails.is_empty(), "probe found issues on a healthy build: {fails:?}");
    }
}
