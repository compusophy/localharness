#[allow(unused_imports)]
use crate::*;

/// Prompt another agent and print its reply — HEADLESS, via the credit proxy.
///
/// No Gemini key, no live browser tab, no relay: this process runs an agent
/// turn itself, authenticating to the proxy with YOUR identity key (which
/// also spends your `$LH`) and running under the TARGET's on-chain persona so
/// it answers *as* that agent. The `?rpc=1` browser endpoint is postMessage-
/// only (tab-to-tab) and a static host can't accept an HTTP POST — so the old
/// `POST .../?rpc=1` path here always 405'd; the proxy is the real bridge.
///
///   localharness call [--as <yourname>] <target> <message…>
/// Parsed `call` arguments: the optional `--as` caller, whether `--fresh` was
/// given (start a new conversation, ignoring saved history), the target name,
/// and the joined message. Pure (no I/O) so it is unit-testable; `Err` carries
/// the usage line to print. Leading `--as`/`--fresh` flags may appear in any
/// order before the target.
pub(crate) struct ParsedCall {
    caller: Option<String>,
    fresh: bool,
    model: Option<String>,
    target: String,
    message: String,
}

pub(crate) const CALL_USAGE: &str =
    "usage: localharness call [--as <yourname>] [--fresh] [--model <id>] <target> <message>";

pub(crate) fn parse_call_args(rest: &[String]) -> Result<ParsedCall, String> {
    // `--as` is pulled from ANY position (via take_as_flag — consistent with
    // schedule/invite/send), so `call <target> "msg" --as me` works, not just
    // the leading form. --model/--fresh stay leading flags before the target.
    let (caller, rest) = take_as_flag(rest)?;
    let mut fresh = false;
    let mut model = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--model" => match rest.get(i + 1) {
                Some(m) => {
                    model = Some(m.clone());
                    i += 2;
                }
                None => return Err(CALL_USAGE.to_string()),
            },
            "--fresh" => {
                fresh = true;
                i += 1;
            }
            _ => break,
        }
    }
    match rest[i..].split_first() {
        Some((t, msg)) if !msg.is_empty() => Ok(ParsedCall {
            caller,
            fresh,
            model,
            target: t.clone(),
            message: msg.join(" "),
        }),
        _ => Err(CALL_USAGE.to_string()),
    }
}

/// The directory holding persisted `call` conversations.
pub(crate) fn history_dir() -> std::path::PathBuf {
    std::path::Path::new(".localharness").join("history")
}

/// The serialization-backend tag a `model` routes to. Conversation history is
/// serialized in a BACKEND-SPECIFIC wire shape (a Gemini thread loaded into the
/// Anthropic backend dies with `missing field 'content'` and vice-versa), so
/// the persisted thread is keyed by this tag — the two backends never share a
/// file. Mirrors the `claude*` → Anthropic routing in `run_agent_turn`.
pub(crate) fn model_backend_tag(model: Option<&str>) -> &'static str {
    if model.map(|m| m.starts_with("claude")).unwrap_or(false) {
        "anthropic"
    } else {
        "gemini"
    }
}

/// Where a `call` conversation between `caller_label` and `target` on a given
/// `backend` is persisted, so repeated calls continue the same thread. Keyed by
/// backend too so a Gemini thread and an Anthropic thread to the same target
/// never collide (their on-disk formats are incompatible). Pure path builder.
pub(crate) fn history_path(caller_label: &str, target: &str, backend: &str) -> std::path::PathBuf {
    history_dir().join(format!("{caller_label}__{target}.{backend}.bin"))
}

/// Extract the target from a history filename `<caller>__<target>.<backend>.bin`
/// (or the legacy `<caller>__<target>.bin`) for the given caller label. `None`
/// when it doesn't belong to that caller. Pure. A trailing `.gemini`/`.anthropic`
/// backend tag is stripped so `threads`/`forget` show the bare target.
pub(crate) fn thread_file_target(caller_label: &str, file_name: &str) -> Option<String> {
    let stem = file_name
        .strip_prefix(&format!("{caller_label}__"))?
        .strip_suffix(".bin")
        .filter(|t| !t.is_empty())?;
    // Drop a known backend tag if present (newer files); legacy files have none.
    let target = stem
        .strip_suffix(".gemini")
        .or_else(|| stem.strip_suffix(".anthropic"))
        .unwrap_or(stem);
    if target.is_empty() {
        return None;
    }
    Some(target.to_string())
}

/// Map a failed `call` error to an actionable hint, if recognisable. Pure —
/// covers the common proxy/auth failure modes a new agent hits, so the raw
/// transport error isn't the whole story.
pub(crate) fn hint_for_call_error(err: &str) -> Option<&'static str> {
    let e = err.to_ascii_lowercase();
    if e.contains("402")
        || e.contains("payment")
        || e.contains("no session")
        || e.contains("insufficient")
        || e.contains("credit")
    {
        return Some(
            "the credit proxy has no $LH for your identity. `call` meters \
             ~0.01 $LH per request, so a fresh identity must be funded first — \
             run `localharness redeem <code>`, or have another agent `send` you $LH.",
        );
    }
    if e.contains("401")
        || e.contains("403")
        || e.contains("unauthorized")
        || e.contains("forbidden")
        || e.contains("signature")
    {
        return Some(
            "the proxy rejected your auth signature — check that your identity \
             key is the one `whoami` shows as owner.",
        );
    }
    if e.contains("429") || e.contains("rate limit") {
        return Some("rate limited by the model backend — retry in a moment.");
    }
    None
}

/// Print an error line plus its actionable hint, if any.
pub(crate) fn report_call_error(prefix: &str, err: &str) {
    eprintln!("{prefix}: {err}");
    if let Some(hint) = hint_for_call_error(err) {
        eprintln!("  hint: {hint}");
    }
}

pub(crate) async fn call(rest: &[String]) -> i32 {
    let ParsedCall {
        caller,
        fresh,
        model,
        target,
        message,
    } = match parse_call_args(rest) {
        Ok(p) => p,
        Err(usage) => {
            eprintln!("{usage}");
            return 2;
        }
    };

    // Resolve the caller's identity key — it signs proxy auth + pays $LH.
    let (_key_file, key_hex) = match resolve_caller_key(caller.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    // Conversations persist per (caller, target, backend) so repeated calls
    // continue the same thread; `--fresh` starts over. Label by the bare
    // key-file stem (`resolve_caller_label` — basename minus KEY_SUFFIX), so a
    // cwd key and a config-home key for the same name share one history thread.
    // Keying on the backend too keeps a Gemini thread and an Anthropic thread
    // to the same target in SEPARATE files — their on-disk history formats are
    // incompatible (a Gemini thread loaded into the Anthropic backend dies with
    // `missing field 'content'`).
    let caller_label = match resolve_caller_label(caller.as_deref()) {
        Ok(l) => l,
        Err(e) => {
            // Unreachable in practice: resolve_caller_key above already resolved
            // the same identity. Mirror its exit code if the fs raced us.
            eprintln!("{e}");
            return 2;
        }
    };
    let backend = model_backend_tag(model.as_deref());
    let hist_file = history_path(&caller_label, &target, backend);
    let prior_history = if fresh {
        let _ = std::fs::remove_file(&hist_file);
        None
    } else {
        // A read failure (missing/corrupt file) is non-fatal: start fresh.
        std::fs::read(&hist_file).ok()
    };
    match run_agent_turn(&key_hex, &target, &message, prior_history, model.as_deref()).await {
        Ok((text, new_history)) => {
            println!("{}", text.trim());
            // Persist the conversation so the next `call` to this target
            // continues it. Best-effort: a save failure must not flip the code.
            if let Some(bytes) = new_history {
                if let Some(dir) = hist_file.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&hist_file, bytes);
            }
            0
        }
        Err(e) => {
            report_call_error("call failed", &e);
            1
        }
    }
}

/// Run ONE headless conversational turn as `target` — embodying its on-chain
/// persona, paid for by the identity behind `key_hex` (proxy auth + ~0.01 $LH
/// debited from its per-request meter, which this funds lazily — NOT an hourly
/// session). Returns the reply text plus the updated conversation history bytes
/// (to persist for the next turn). Shared by the CLI `call` command and the
/// `mcp` server's `call_agent` tool, so both reach an agent identically.
pub(crate) async fn run_agent_turn(
    key_hex: &str,
    target: &str,
    message: &str,
    prior_history: Option<Vec<u8>>,
    model: Option<&str>,
) -> Result<(String, Option<Vec<u8>>), String> {
    let caller =
        wallet::from_private_key_hex(key_hex).map_err(|e| format!("bad identity key: {e}"))?;

    // Embody the target's PUBLISHED persona (falls back to a generic prompt).
    let system = match registry::id_of_name(target).await {
        Ok(id) if id != 0 => match registry::persona_of(id).await {
            Ok(Some(p)) => p,
            Ok(None) => default_persona(target),
            Err(e) => return Err(format!("RPC error reading persona: {e}")),
        },
        Ok(_) => return Err(format!("{target} is not a registered agent")),
        Err(e) => return Err(format!("RPC error: {e}")),
    };

    // Pay PER REQUEST, not by the hour: fund the per-request meter so the proxy
    // debits ~CALL_COST_WEI per call. A one-shot agent call must NOT buy a
    // 10-$LH hour-long session (the old behavior). Best-effort + sponsored; an
    // unfunded wallet stays unfunded (the proxy 402s, the hint says to redeem).
    if let Ok(sponsor) = wallet::from_private_key_hex(SPONSOR_KEY) {
        let addr = bytes_to_hex_str(&wallet::address(&caller));
        if registry::credit_balance_of(&addr).await.unwrap_or(0) < CALL_COST_WEI {
            let _ = registry::deposit_credits_sponsored(
                &caller,
                &sponsor,
                CALL_METER_TOPUP_WEI,
                registry::ALPHA_USD_ADDRESS,
            )
            .await;
        }
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let token = registry::proxy_auth_token(&caller, now);
    let base = url::Url::parse(registry::CREDIT_PROXY_URL)
        .map_err(|e| format!("internal: bad proxy url: {e}"))?;

    // A pure conversational turn: no local builtins (a remote prompt must not
    // read the CALLER's filesystem), no subagents.
    let caps = localharness::types::CapabilitiesConfig {
        enabled_tools: Some(Vec::new()),
        enable_subagents: false,
        ..Default::default()
    };
    // Route by model: a `claude-*` id uses the Anthropic backend; anything else
    // (or none) uses Gemini. BOTH reach the model the same way — through the
    // credit proxy with the same signed token — so a subsidized identity calls
    // either provider with no provider key of its own. Only the config build +
    // start call differ per backend; the start-with-history-fallback and the
    // chat/harvest/shutdown tail are shared (`start_with_history_fallback` /
    // `run_turn_and_persist`).
    if model.map(|m| m.starts_with("claude")).unwrap_or(false) {
        #[cfg(feature = "anthropic")]
        {
            let model = model.unwrap().to_string();
            // Build a config, optionally seeded with prior history. Cloned inputs
            // so a failed history-seeded start can be retried from scratch.
            let build = |history: Option<Vec<u8>>| {
                let mut cfg = localharness::AnthropicAgentConfig::new(token.clone())
                    .with_base_url(base.clone())
                    .with_model(model.clone())
                    .with_system_instructions(system.clone())
                    .with_capabilities(caps.clone());
                if let Some(bytes) = history {
                    cfg = cfg.with_history_bytes(bytes);
                }
                cfg
            };
            let agent =
                start_with_history_fallback(target, prior_history, "anthropic session", |h| {
                    localharness::Agent::start_anthropic(build(h))
                })
                .await?;
            return run_turn_and_persist(agent, message).await;
        }
        #[cfg(not(feature = "anthropic"))]
        {
            return Err("Claude models require a build with `--features anthropic`".to_string());
        }
    }

    let build = |history: Option<Vec<u8>>| {
        let mut cfg = localharness::GeminiAgentConfig::new(token.clone())
            .with_base_url(base.clone())
            .with_system_instructions(system.clone())
            .with_capabilities(caps.clone());
        if let Some(bytes) = history {
            cfg = cfg.with_history_bytes(bytes);
        }
        cfg
    };
    let agent = start_with_history_fallback(target, prior_history, "agent session", |h| {
        localharness::Agent::start_gemini(build(h))
    })
    .await?;
    run_turn_and_persist(agent, message).await
}

/// Start an agent seeded with `prior_history`, falling back to a FRESH start
/// (with a warning) when the saved thread is incompatible/corrupt — rather than
/// failing the whole call. `label` names the backend in the could-not-start
/// error ("anthropic session" / "agent session"); `start` runs the backend's
/// actual start call on a given history. The shared start-half of both
/// `run_agent_turn` branches.
async fn start_with_history_fallback<F, Fut>(
    target: &str,
    prior_history: Option<Vec<u8>>,
    label: &str,
    start: F,
) -> Result<localharness::Agent, String>
where
    F: Fn(Option<Vec<u8>>) -> Fut,
    Fut: std::future::Future<Output = localharness::Result<localharness::Agent>>,
{
    match start(prior_history.clone()).await {
        Ok(a) => Ok(a),
        Err(_) if prior_history.is_some() => {
            // Incompatible/corrupt saved thread → warn + start fresh rather than
            // failing the whole call.
            eprintln!(
                "warning: could not load saved conversation with {target} \
                 (incompatible or corrupt) — starting a fresh thread"
            );
            start(None)
                .await
                .map_err(|e| format!("could not start {label}: {e}"))
        }
        Err(e) => Err(format!("could not start {label}: {e}")),
    }
}

/// Run ONE chat turn on a started agent, harvest the reply text + the updated
/// conversation-history bytes, and shut the agent down. The shared tail of both
/// `run_agent_turn` branches.
async fn run_turn_and_persist(
    agent: localharness::Agent,
    message: &str,
) -> Result<(String, Option<Vec<u8>>), String> {
    let reply = match agent.chat(message).await {
        Ok(resp) => resp.text().await.map_err(|e| format!("response error: {e}")),
        Err(e) => Err(e.to_string()),
    };
    let new_history = agent.history_bytes().ok().flatten();
    let _ = agent.shutdown().await;
    reply.map(|text| (text, new_history))
}

/// List the caller's saved conversation threads (`localharness threads`).
pub(crate) fn threads(caller_name: Option<&str>) -> i32 {
    let label = match resolve_caller_label(caller_name) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let mut found: Vec<(String, u64)> = match std::fs::read_dir(history_dir()) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let target = thread_file_target(&label, &name)?;
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                Some((target, size))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    found.sort();
    if found.is_empty() {
        println!("no saved conversations for {label}");
        return 0;
    }
    println!("conversations for {label}:");
    for (target, size) in found {
        println!("  {target}  ({size} bytes)");
    }
    0
}

/// Delete a saved conversation thread, or all of the caller's with `--all`
/// (`localharness forget [--as me] <target|--all>`). Never touches identity
/// keys or on-chain state — only local history files.
pub(crate) fn forget(caller_name: Option<&str>, target: &str) -> i32 {
    let label = match resolve_caller_label(caller_name) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    if target == "--all" {
        let mut n = 0;
        if let Ok(rd) = std::fs::read_dir(history_dir()) {
            for e in rd.flatten() {
                let Ok(name) = e.file_name().into_string() else {
                    continue;
                };
                if thread_file_target(&label, &name).is_some()
                    && std::fs::remove_file(e.path()).is_ok()
                {
                    n += 1;
                }
            }
        }
        println!("forgot {n} conversation(s) for {label}");
        return 0;
    }
    // A target can have a thread per backend (plus a legacy untagged file);
    // forget them all so `forget <target>` clears the conversation regardless
    // of which model it ran under.
    let mut removed = false;
    for candidate in [
        history_path(&label, target, "gemini"),
        history_path(&label, target, "anthropic"),
        history_dir().join(format!("{label}__{target}.bin")), // legacy untagged
    ] {
        if std::fs::remove_file(candidate).is_ok() {
            removed = true;
        }
    }
    if removed {
        println!("forgot conversation with {target}");
    } else {
        println!("no saved conversation with {target}");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_call_plain_target_and_message() {
        let p = parse_call_args(&args(&["alice", "how", "are", "you"])).unwrap();
        assert_eq!(p.caller, None);
        assert_eq!(p.target, "alice");
        assert_eq!(p.message, "how are you");
    }

    #[test]
    fn parse_call_single_word_message() {
        let p = parse_call_args(&args(&["alice", "hello"])).unwrap();
        assert_eq!(p.caller, None);
        assert_eq!(p.target, "alice");
        assert_eq!(p.message, "hello");
    }

    #[test]
    fn parse_call_with_as_flag() {
        let p = parse_call_args(&args(&["--as", "bob", "alice", "what's", "up"])).unwrap();
        assert_eq!(p.caller.as_deref(), Some("bob"));
        assert!(!p.fresh);
        assert_eq!(p.target, "alice");
        assert_eq!(p.message, "what's up");
    }

    #[test]
    fn parse_call_fresh_flag() {
        let p = parse_call_args(&args(&["--fresh", "alice", "hi"])).unwrap();
        assert!(p.fresh);
        assert_eq!(p.caller, None);
        assert_eq!(p.target, "alice");
        assert_eq!(p.message, "hi");
    }

    #[test]
    fn parse_call_flags_order_independent() {
        let a = parse_call_args(&args(&["--as", "bob", "--fresh", "alice", "hi"])).unwrap();
        let b = parse_call_args(&args(&["--fresh", "--as", "bob", "alice", "hi"])).unwrap();
        for p in [a, b] {
            assert_eq!(p.caller.as_deref(), Some("bob"));
            assert!(p.fresh);
            assert_eq!(p.target, "alice");
            assert_eq!(p.message, "hi");
        }
    }

    #[test]
    fn parse_call_accepts_model_flag_in_any_order() {
        // `--as`/`--model`/`--fresh` may appear in any order before the target.
        let perms = [
            vec!["--model", "claude-opus", "--as", "bob", "--fresh", "alice", "hi"],
            vec!["--fresh", "--model", "claude-opus", "--as", "bob", "alice", "hi"],
            vec!["--as", "bob", "--model", "claude-opus", "--fresh", "alice", "hi"],
        ];
        for parts in perms {
            let p = parse_call_args(&args(&parts)).unwrap();
            assert_eq!(p.caller.as_deref(), Some("bob"));
            assert_eq!(p.model.as_deref(), Some("claude-opus"));
            assert!(p.fresh);
            assert_eq!(p.target, "alice");
            assert_eq!(p.message, "hi");
        }
        // `--model` requires a value.
        assert!(parse_call_args(&args(&["--model"])).is_err());
    }

    #[test]
    fn parse_call_defaults_to_not_fresh() {
        let p = parse_call_args(&args(&["alice", "hi"])).unwrap();
        assert!(!p.fresh);
    }

    #[test]
    fn parse_call_message_preserves_internal_spacing_as_single_spaces() {
        // join(" ") normalises arg boundaries to single spaces — documents the
        // contract so a caller relying on exact whitespace isn't surprised.
        let p = parse_call_args(&args(&["alice", "a", "b", "c"])).unwrap();
        assert_eq!(p.message, "a b c");
    }

    #[test]
    fn parse_call_rejects_missing_message() {
        assert!(parse_call_args(&args(&["alice"])).is_err());
    }

    // ---- mcp-call (the hosted MCP-over-HTTP + x402 client) ----------------

    #[test]
    fn parse_call_rejects_empty() {
        assert!(parse_call_args(&args(&[])).is_err());
    }

    #[test]
    fn parse_call_rejects_as_without_name() {
        assert!(parse_call_args(&args(&["--as"])).is_err());
    }

    #[test]
    fn parse_call_rejects_as_name_without_target_or_message() {
        // `--as bob` alone: caller set, but no target/message → usage error.
        assert!(parse_call_args(&args(&["--as", "bob"])).is_err());
        // `--as bob alice` : target but no message → usage error.
        assert!(parse_call_args(&args(&["--as", "bob", "alice"])).is_err());
    }

    #[test]
    fn thread_file_target_parses_own_files_only() {
        // Backend-tagged files (current format): the tag is stripped.
        assert_eq!(
            thread_file_target("claude", "claude__alice.gemini.bin").as_deref(),
            Some("alice")
        );
        assert_eq!(
            thread_file_target("claude", "claude__alice.anthropic.bin").as_deref(),
            Some("alice")
        );
        // Legacy untagged files still parse (backward compatibility).
        assert_eq!(
            thread_file_target("claude", "claude__alice.bin").as_deref(),
            Some("alice")
        );
        // A target containing the separator stays intact (strip_prefix once).
        assert_eq!(
            thread_file_target("claude", "claude__a__b.gemini.bin").as_deref(),
            Some("a__b")
        );
        // Different caller → not ours.
        assert_eq!(thread_file_target("claude", "bob__alice.gemini.bin"), None);
        // Wrong extension, or empty target → rejected.
        assert_eq!(thread_file_target("claude", "claude__alice.txt"), None);
        assert_eq!(thread_file_target("claude", "claude__.bin"), None);
        assert_eq!(thread_file_target("claude", "claude__.gemini.bin"), None);
        assert_eq!(thread_file_target("claude", "unrelated.bin"), None);
    }

    #[test]
    fn thread_file_target_roundtrips_history_path() {
        // The parser must invert the filename half of history_path for both
        // backends.
        for backend in ["gemini", "anthropic"] {
            let p = history_path("claude", "alice", backend);
            let name = p.file_name().unwrap().to_str().unwrap();
            assert_eq!(thread_file_target("claude", name).as_deref(), Some("alice"));
        }
    }

    #[test]
    fn model_backend_tag_routes_claude_to_anthropic() {
        assert_eq!(model_backend_tag(Some("claude-opus-4")), "anthropic");
        assert_eq!(model_backend_tag(Some("claude")), "anthropic");
        assert_eq!(model_backend_tag(Some("gemini-3.5-flash")), "gemini");
        assert_eq!(model_backend_tag(None), "gemini");
    }

    #[test]
    fn history_path_keys_on_backend_so_formats_never_collide() {
        // The cross-backend bug: a Gemini thread and an Anthropic thread to the
        // same target must live in SEPARATE files (incompatible on-disk shapes).
        let g = history_path("claude", "alice", "gemini");
        let a = history_path("claude", "alice", "anthropic");
        assert_ne!(g, a, "backends must not share a history file");
        assert!(g.ends_with("claude__alice.gemini.bin"));
        assert!(a.ends_with("claude__alice.anthropic.bin"));
    }

    #[test]
    fn history_path_keys_on_caller_and_target() {
        let p = history_path("claude", "alice", "gemini");
        assert!(p.ends_with("claude__alice.gemini.bin"));
        // Distinct caller or target → distinct file (no cross-thread bleed).
        assert_ne!(
            history_path("claude", "alice", "gemini"),
            history_path("bob", "alice", "gemini")
        );
        assert_ne!(
            history_path("claude", "alice", "gemini"),
            history_path("claude", "bob", "gemini")
        );
        // Lives under a hidden dir so it doesn't clutter the working tree.
        assert!(p.starts_with(".localharness"));
    }

    #[test]
    fn hint_for_call_error_classifies_common_failures() {
        // Payment / session / credits → the $LH hint.
        for s in [
            "HTTP 402 Payment Required",
            "proxy: no session for 0xabc",
            "insufficient credit",
        ] {
            assert!(
                hint_for_call_error(s).unwrap().contains("$LH"),
                "expected $LH hint for {s:?}"
            );
        }
        // Auth → the signature hint.
        for s in ["401 Unauthorized", "bad signature", "403 Forbidden"] {
            assert!(
                hint_for_call_error(s).unwrap().contains("signature"),
                "expected auth hint for {s:?}"
            );
        }
        // Rate limit.
        assert!(hint_for_call_error("429 Too Many Requests")
            .unwrap()
            .contains("rate limited"));
    }

    #[test]
    fn hint_for_call_error_is_case_insensitive_and_silent_on_unknown() {
        assert!(hint_for_call_error("PAYMENT REQUIRED").is_some());
        // An unrecognised error gets no hint (caller still prints the raw text).
        assert_eq!(hint_for_call_error("connection reset by peer"), None);
        assert_eq!(hint_for_call_error("some unrelated parse error"), None);
    }
}
