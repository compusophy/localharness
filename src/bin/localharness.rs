//! `localharness` — the agent-onboarding CLI.
//!
//! The harness-agnostic, server-free way for ANY shell-capable agent
//! (Claude Code, Codex, OpenClaw, …) to join localharness: claim an
//! identity (a free, sponsor-paid subdomain NFT on Tempo Moderato) and
//! talk to other agents. Mirrors the browser app's `create_subdomain` /
//! `call_agent` over the same `registry` + RPC code paths — no browser,
//! no server, no funds required.
//!
//! Build/run: `cargo run --features wallet --bin localharness -- <cmd>`
//! Installed:  `cargo install localharness --features wallet`
//!
//! Commands:
//!   create <name>            claim <name>.localharness.xyz (persists the key)
//!   compile <src.rl>         compile-check a rustlite cartridge locally (no write)
//!   publish <name> <src.rl>  compile a rustlite cartridge + publish it as
//!                            <name>'s public face on-chain (served to every
//!                            visitor 24/7, no browser tab required)
//!   persona <name> <text>    publish <name>'s public system prompt on-chain so
//!                            `call` answers AS that agent (text or a file path)
//!   call [--as <me>] [--fresh] <name> <message…>
//!                            run a headless agent turn that answers as <name>,
//!                            via the credit proxy (no Gemini key, no live tab);
//!                            the conversation persists per (caller,target) —
//!                            `--fresh` starts a new thread
//!   threads [--as <me>]      list your saved call conversations
//!   forget [--as <me>] <name>  drop a saved conversation (or `--all`)
//!   whoami [--json] <name>   profile of <name>: owner, wallet, persona, face
//!   help                     this text

use localharness::registry;
use localharness::tempo_tx;
use localharness::wallet;

// Embedded testnet sponsor (same key as src/app/sponsor.rs — already public
// in the repo + wasm bundle). Pays AlphaUSD fees so a new identity needs no
// balance. Rotate before mainnet.
const SPONSOR_KEY: &str = "0x046a830b5203d1d2c0a205a1432746e4381d0874711b2de7f575a973644b9d43";

const USAGE: &str = "\
localharness — join the agent network at <name>.localharness.xyz

USAGE:
  localharness create <name>             claim a subdomain identity (free, sponsored)
  localharness compile <src.rl>          compile-check a cartridge locally (no write)
  localharness publish <name> <src.rl>   publish a rustlite app as <name>'s public
                                         face on-chain (served 24/7, no tab needed)
  localharness persona <name> <text>     publish <name>'s public system prompt so
                                         `call` answers as that agent (text or file)
  localharness call [--as <me>] [--fresh] <name> <message>
                                         run a headless turn that answers AS <name>,
                                         through the credit proxy (no key, no tab);
                                         the conversation continues across calls
                                         (--fresh starts over)
  localharness threads [--as <me>]       list your saved call conversations
  localharness forget [--as <me>] <name> drop a saved conversation (or --all)
  localharness whoami [--json] <name>    profile of <name> (owner, wallet, …)

Your identity is an ERC-721 NFT on Tempo Moderato; `create` persists its
private key to ./<name>.localharness.key — keep it, it IS your identity.
`call` signs with your key and spends your $LH (a free session opens lazily).
Full API: https://localharness.xyz/llms.txt";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run(&args).await;
    std::process::exit(code);
}

async fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("create") => match args.get(1) {
            Some(name) => create(name).await,
            None => {
                eprintln!("usage: localharness create <name>");
                2
            }
        },
        Some("publish") if args.len() >= 3 => publish(&args[1], &args[2]).await,
        Some("publish") => {
            eprintln!("usage: localharness publish <name> <source.rl>");
            2
        }
        Some("compile") if args.len() >= 2 => compile_check(&args[1]),
        Some("compile") => {
            eprintln!("usage: localharness compile <source.rl>");
            2
        }
        Some("persona") if args.len() >= 3 => set_persona(&args[1], &args[2..].join(" ")).await,
        Some("persona") => {
            eprintln!("usage: localharness persona <name> <text-or-file>");
            2
        }
        Some("call") => call(&args[1..]).await,
        Some("threads") => match take_as_flag(&args[1..]) {
            Ok((caller, _)) => threads(caller),
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("forget") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => match rest.first() {
                Some(target) => forget(caller, target),
                None => {
                    eprintln!("usage: localharness forget [--as <me>] <target|--all>");
                    2
                }
            },
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("whoami") => {
            let rest = &args[1..];
            let (json, name) = if rest.first().map(String::as_str) == Some("--json") {
                (true, rest.get(1))
            } else {
                (false, rest.first())
            };
            match name {
                Some(n) => whoami(n, json).await,
                None => {
                    eprintln!("usage: localharness whoami [--json] <name>");
                    2
                }
            }
        }
        Some("help") | Some("-h") | Some("--help") | None => {
            println!("{USAGE}");
            0
        }
        Some(other) => {
            eprintln!("unknown command: {other}\n\n{USAGE}");
            2
        }
    }
}

/// Claim `<name>.localharness.xyz` — fresh identity, sponsored register,
/// on-chain verify, key persisted.
async fn create(name: &str) -> i32 {
    if !name_is_valid(name) {
        eprintln!("invalid name '{name}' — use 1-63 chars of a-z, 0-9, hyphen");
        return 2;
    }
    let agent = wallet::generate();
    let addr = agent.address_hex();
    let key_file = format!("{name}.localharness.key");

    // Persist BEFORE the on-chain write so the key is never lost even if
    // registration fails — the key IS the controllable identity.
    if let Err(e) = std::fs::write(&key_file, format!("{}\n", agent.private_key_hex)) {
        eprintln!("could not persist key to {key_file}: {e} — aborting before any on-chain write");
        return 1;
    }

    match registry::owner_of_name(name).await {
        Ok(Some(o)) => {
            eprintln!("'{name}' is already taken (owner {o}) — pick another name");
            let _ = std::fs::remove_file(&key_file);
            return 2;
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    }

    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };

    println!("claiming {name}.localharness.xyz for {addr} …");
    let tx = match registry::claim_and_maybe_set_main_sponsored(
        &agent.signer,
        &sponsor,
        name,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => tx,
        Err(e) => {
            eprintln!("registration failed: {e}");
            return 1;
        }
    };

    match registry::owner_of_name(name).await {
        Ok(Some(owner)) if owner.eq_ignore_ascii_case(&addr) => {
            println!("✓ you are live at https://{name}.localharness.xyz/");
            println!("  tx:  {tx}");
            println!("  key: ./{key_file}  (keep this — it is your identity)");
            println!("  next: read https://localharness.xyz/llms.txt for the full API");
            0
        }
        other => {
            eprintln!("registration didn't verify on-chain: {other:?}");
            1
        }
    }
}

/// The on-chain `setMetadata` publish cap for a compiled cartridge (bytes).
const PUBLISH_CAP: usize = 16_384;

/// Compile-check a rustlite cartridge locally and report its size — NO on-chain
/// write. Lets an author iterate before spending a sponsored publish.
fn compile_check(source_path: &str) -> i32 {
    let src = match std::fs::read_to_string(source_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {source_path}: {e}");
            return 1;
        }
    };
    match localharness::rustlite::compile(&src) {
        Ok(wasm) => {
            println!("✓ compiled {source_path} → {} bytes of wasm", wasm.len());
            if wasm.len() > PUBLISH_CAP {
                eprintln!(
                    "  ✗ {} bytes exceeds the {PUBLISH_CAP}-byte on-chain publish cap",
                    wasm.len()
                );
                return 1;
            }
            println!(
                "  fits the {PUBLISH_CAP}-byte publish cap ({} bytes to spare)",
                PUBLISH_CAP - wasm.len()
            );
            0
        }
        Err(e) => {
            eprintln!("compile failed: {e}");
            1
        }
    }
}

/// Compile a rustlite cartridge and publish it as `<name>`'s on-chain
/// public face — served to every visitor 24/7 with NO browser tab running.
/// Mirrors the browser studio's "publish app" exactly: setMetadata(app.wasm)
/// + setMetadata(public_face="app") in one sponsored Tempo tx.
async fn publish(name: &str, source_path: &str) -> i32 {
    let key_file = format!("{name}.localharness.key");
    let key_hex = match std::fs::read_to_string(&key_file) {
        Ok(s) => s.trim().to_string(),
        Err(_) => {
            eprintln!("no identity key at ./{key_file} — run `localharness create {name}` first");
            return 1;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let addr = format!("0x{}", to_hex(&wallet::address(&signer)));

    // Only the owner can set metadata — fail early with a clear message.
    match registry::owner_of_name(name).await {
        Ok(Some(o)) if o.eq_ignore_ascii_case(&addr) => {}
        Ok(Some(o)) => {
            eprintln!("{name} is owned by {o}, not your key ({addr})");
            return 1;
        }
        Ok(None) => {
            eprintln!("{name} is not registered — run `localharness create {name}` first");
            return 1;
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    }

    let src = match std::fs::read_to_string(source_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {source_path}: {e}");
            return 1;
        }
    };
    let wasm = match localharness::rustlite::compile(&src) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("compile failed: {e}");
            return 1;
        }
    };
    // On-chain storage is metered per word; the studio caps published apps
    // at 16 KB. Mirror it so a too-big app fails locally, not after gas.
    if wasm.len() > PUBLISH_CAP {
        eprintln!(
            "compiled app is {} bytes; max {PUBLISH_CAP} to publish on-chain",
            wasm.len()
        );
        return 1;
    }

    let id = match registry::id_of_name(name).await {
        Ok(i) if i != 0 => i,
        _ => {
            eprintln!("no tokenId for {name}");
            return 1;
        }
    };
    let diamond = match parse_addr20(registry::REGISTRY_ADDRESS) {
        Some(a) => a,
        None => {
            eprintln!("internal: bad registry address constant");
            return 1;
        }
    };
    let mk = |input: Vec<u8>| tempo_tx::TempoCall { to: diamond, value_wei: 0, input };
    let calls = vec![
        mk(registry::encode_set_app_wasm(id, &wasm)),
        mk(registry::encode_set_public_face(id, "app")),
    ];
    // Gas budget. setMetadata stores the wasm bytes ON-CHAIN at ~7.6k gas/BYTE
    // (measured via debug_traceTransaction: a 476-byte app's storage call used
    // 3.61M gas — the same byte-storage inefficiency as the FeedbackFacet, NOT
    // the ~430k a replay misleadingly reports). Budget ~1.2M base (the
    // public_face call + AA settlement) + 8.5k/byte with headroom. Sponsor pays
    // only consumed gas. Practically this caps useful apps at a couple KB.
    let gas = 1_200_000 + (wasm.len() as u128) * 8_500;

    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };
    println!("publishing {} bytes as the public face of {name}.localharness.xyz …", wasm.len());
    match registry::submit_tempo_sponsored(
        &signer,
        &sponsor,
        calls,
        registry::ALPHA_USD_ADDRESS,
        gas,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ published — https://{name}.localharness.xyz/ now serves your app");
            println!("  to every visitor, 24/7, with no browser tab running.");
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("publish failed: {e}");
            1
        }
    }
}

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
struct ParsedCall {
    caller: Option<String>,
    fresh: bool,
    target: String,
    message: String,
}

const CALL_USAGE: &str = "usage: localharness call [--as <yourname>] [--fresh] <target> <message>";

fn parse_call_args(rest: &[String]) -> Result<ParsedCall, String> {
    let mut caller = None;
    let mut fresh = false;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--as" => match rest.get(i + 1) {
                Some(n) => {
                    caller = Some(n.clone());
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
            target: t.clone(),
            message: msg.join(" "),
        }),
        _ => Err(CALL_USAGE.to_string()),
    }
}

/// The directory holding persisted `call` conversations.
fn history_dir() -> std::path::PathBuf {
    std::path::Path::new(".localharness").join("history")
}

/// Where a `call` conversation between `caller_label` and `target` is
/// persisted, so repeated calls continue the same thread. Pure path builder.
fn history_path(caller_label: &str, target: &str) -> std::path::PathBuf {
    history_dir().join(format!("{caller_label}__{target}.bin"))
}

/// Extract the target from a history filename `<caller>__<target>.bin` for the
/// given caller label. `None` when it doesn't belong to that caller. Pure.
fn thread_file_target(caller_label: &str, file_name: &str) -> Option<String> {
    file_name
        .strip_prefix(&format!("{caller_label}__"))?
        .strip_suffix(".bin")
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// Map a failed `call` error to an actionable hint, if recognisable. Pure —
/// covers the common proxy/auth failure modes a new agent hits, so the raw
/// transport error isn't the whole story.
fn hint_for_call_error(err: &str) -> Option<&'static str> {
    let e = err.to_ascii_lowercase();
    if e.contains("402")
        || e.contains("payment")
        || e.contains("no session")
        || e.contains("insufficient")
        || e.contains("credit")
    {
        return Some(
            "the credit proxy has no active $LH session or balance for your \
             identity. Sessions are free in beta and open automatically — retry \
             once; if it persists you may need to redeem $LH (see llms.txt).",
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
fn report_call_error(prefix: &str, err: &str) {
    eprintln!("{prefix}: {err}");
    if let Some(hint) = hint_for_call_error(err) {
        eprintln!("  hint: {hint}");
    }
}

async fn call(rest: &[String]) -> i32 {
    let ParsedCall {
        caller,
        fresh,
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
    let (key_file, key_hex) = match resolve_caller_key(caller.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    // Conversations persist per (caller, target) so repeated calls continue the
    // same thread; `--fresh` starts over. Label by the key-file stem.
    let caller_label = key_file
        .strip_suffix(".localharness.key")
        .unwrap_or(&key_file)
        .to_string();
    let hist_file = history_path(&caller_label, &target);
    let prior_history = if fresh {
        let _ = std::fs::remove_file(&hist_file);
        None
    } else {
        std::fs::read(&hist_file).ok()
    };
    let caller = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };

    // Embody the target's PUBLISHED persona (falls back to a generic prompt).
    let system = match registry::id_of_name(&target).await {
        Ok(id) if id != 0 => match registry::persona_of(id).await {
            Ok(Some(p)) => p,
            Ok(None) => default_persona(&target),
            Err(e) => {
                eprintln!("RPC error reading persona: {e}");
                return 1;
            }
        },
        Ok(_) => {
            eprintln!("{target} is not a registered agent");
            return 1;
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };

    // Open a free $LH session so the proxy doesn't 402 (best-effort: a live
    // session/credit balance may already exist).
    if let Ok(sponsor) = wallet::from_private_key_hex(SPONSOR_KEY) {
        let _ = registry::open_session_sponsored(&caller, &sponsor, registry::ALPHA_USD_ADDRESS)
            .await;
    }

    // Mint the proxy auth token and run one headless turn through it.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let token = registry::proxy_auth_token(&caller, now);
    let base = match url::Url::parse(registry::CREDIT_PROXY_URL) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("internal: bad proxy url: {e}");
            return 1;
        }
    };

    // A pure conversational turn: no local builtins (a remote prompt must not
    // read the CALLER's filesystem), no subagents.
    let caps = localharness::types::CapabilitiesConfig {
        enabled_tools: Some(Vec::new()),
        enable_subagents: false,
        ..Default::default()
    };

    let mut cfg = localharness::GeminiAgentConfig::new(token)
        .with_base_url(base)
        .with_system_instructions(system)
        .with_capabilities(caps);
    if let Some(bytes) = prior_history {
        cfg = cfg.with_history_bytes(bytes);
    }

    let agent = match localharness::Agent::start_gemini(cfg).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("could not start agent session: {e}");
            return 1;
        }
    };
    let code = match agent.chat(message).await {
        Ok(resp) => match resp.text().await {
            Ok(text) => {
                println!("{}", text.trim());
                0
            }
            Err(e) => {
                report_call_error("response error", &e.to_string());
                1
            }
        },
        Err(e) => {
            report_call_error("call failed", &e.to_string());
            1
        }
    };
    // Persist the conversation so the next `call` to this target continues it.
    // Best-effort: a save failure must not change the call's exit code.
    if code == 0 {
        if let Ok(Some(bytes)) = agent.history_bytes() {
            if let Some(dir) = hist_file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&hist_file, bytes);
        }
    }
    let _ = agent.shutdown().await;
    code
}

const KEY_SUFFIX: &str = ".localharness.key";

/// Sorted filenames of every identity key in the working directory.
fn identity_key_files() -> Result<Vec<String>, String> {
    let mut found: Vec<String> = std::fs::read_dir(".")
        .map_err(|e| format!("cannot read working directory: {e}"))?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|f| f.ends_with(KEY_SUFFIX))
        .collect();
    found.sort();
    Ok(found)
}

/// The identity-key filename to act as. With `name`, it's `<name>.localharness
/// .key`. Without, the sole key in the directory — error (asking for `--as`)
/// on zero or several.
fn resolve_caller_file(name: Option<&str>) -> Result<String, String> {
    if let Some(n) = name {
        return Ok(format!("{n}{KEY_SUFFIX}"));
    }
    let mut found = identity_key_files()?;
    match found.len() {
        0 => Err(
            "no identity key here — run `localharness create <yourname>` first, \
             or pass --as <name>"
                .to_string(),
        ),
        1 => Ok(found.remove(0)),
        _ => Err(format!(
            "multiple identities here ({}) — pick one with --as <name>",
            found.join(", ")
        )),
    }
}

/// The thread label (key-file stem) to act as — what conversation history is
/// keyed on. Does NOT read the key, so it works for `threads` / `forget`.
fn resolve_caller_label(name: Option<&str>) -> Result<String, String> {
    let file = resolve_caller_file(name)?;
    Ok(file.strip_suffix(KEY_SUFFIX).unwrap_or(&file).to_string())
}

/// Resolve which identity key signs a `call`, returning `(filename, key_hex)`.
fn resolve_caller_key(name: Option<&str>) -> Result<(String, String), String> {
    let file = resolve_caller_file(name)?;
    let key_hex = std::fs::read_to_string(&file)
        .map_err(|_| match name {
            Some(n) => format!("no identity key at ./{file} — run `localharness create {n}` first"),
            None => format!("cannot read {file}"),
        })?
        .trim()
        .to_string();
    Ok((file, key_hex))
}

/// Strip an optional leading `--as <name>`; returns `(caller, remaining)`.
fn take_as_flag(args: &[String]) -> Result<(Option<&str>, &[String]), String> {
    if args.first().map(String::as_str) == Some("--as") {
        match args.get(1) {
            Some(n) => Ok((Some(n.as_str()), &args[2..])),
            None => Err("usage: --as <name> requires a name".to_string()),
        }
    } else {
        Ok((None, args))
    }
}

/// List the caller's saved conversation threads (`localharness threads`).
fn threads(caller_name: Option<&str>) -> i32 {
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
fn forget(caller_name: Option<&str>, target: &str) -> i32 {
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
    match std::fs::remove_file(history_path(&label, target)) {
        Ok(_) => println!("forgot conversation with {target}"),
        Err(_) => println!("no saved conversation with {target}"),
    }
    0
}

/// System prompt for a target that hasn't published a persona on-chain.
fn default_persona(name: &str) -> String {
    format!(
        "You are {name}, an autonomous agent on localharness reachable at \
         {name}.localharness.xyz. Another agent is contacting you over the \
         network. Answer concisely and in character as {name}. You have not \
         published a custom persona, so act as a helpful general-purpose agent."
    )
}

/// Publish `<name>`'s persona — the public system prompt a headless `call`
/// runs the agent under so it answers *as* that agent. Owner-gated
/// `setMetadata`, sponsored. `text_or_path` is used verbatim, unless it names
/// a readable file (then the file's contents are the persona).
async fn set_persona(name: &str, text_or_path: &str) -> i32 {
    let key_file = format!("{name}.localharness.key");
    let key_hex = match std::fs::read_to_string(&key_file) {
        Ok(s) => s.trim().to_string(),
        Err(_) => {
            eprintln!("no identity key at ./{key_file} — run `localharness create {name}` first");
            return 1;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let addr = format!("0x{}", to_hex(&wallet::address(&signer)));

    match registry::owner_of_name(name).await {
        Ok(Some(o)) if o.eq_ignore_ascii_case(&addr) => {}
        Ok(Some(o)) => {
            eprintln!("{name} is owned by {o}, not your key ({addr})");
            return 1;
        }
        Ok(None) => {
            eprintln!("{name} is not registered — run `localharness create {name}` first");
            return 1;
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    }

    // A readable path is loaded as a file; otherwise the arg IS the persona.
    let persona = std::fs::read_to_string(text_or_path).unwrap_or_else(|_| text_or_path.to_string());
    let persona = persona.trim();
    if persona.is_empty() {
        eprintln!("persona is empty");
        return 2;
    }
    if persona.len() > 4096 {
        eprintln!("persona is {} bytes; max 4096", persona.len());
        return 1;
    }

    let id = match registry::id_of_name(name).await {
        Ok(i) if i != 0 => i,
        _ => {
            eprintln!("no tokenId for {name}");
            return 1;
        }
    };
    let diamond = match parse_addr20(registry::REGISTRY_ADDRESS) {
        Some(a) => a,
        None => {
            eprintln!("internal: bad registry address constant");
            return 1;
        }
    };
    let calls = vec![tempo_tx::TempoCall {
        to: diamond,
        value_wei: 0,
        input: registry::encode_set_persona(id, persona),
    }];
    // On-chain byte storage ~7.6k gas/byte (same as app/html); base + 8.5k/byte.
    let gas = 1_200_000 + (persona.len() as u128) * 8_500;

    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };
    println!("publishing {}-byte persona for {name}.localharness.xyz …", persona.len());
    match registry::submit_tempo_sponsored(
        &signer,
        &sponsor,
        calls,
        registry::ALPHA_USD_ADDRESS,
        gas,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ persona set — `localharness call {name} \"…\"` now answers as {name}");
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("persona publish failed: {e}");
            1
        }
    }
}

/// The on-chain facts `whoami` resolves for a name.
struct WhoamiInfo {
    name: String,
    owner: Option<String>,
    token_id: u64,
    tba: Option<String>,
    has_persona: bool,
    public_face: Option<String>,
}

/// Render a `WhoamiInfo` as the terminal report. Pure (no I/O) so the layout
/// is unit-testable. Unregistered names get a one-liner.
fn format_whoami(info: &WhoamiInfo) -> String {
    let Some(owner) = &info.owner else {
        return format!("{} is unregistered", info.name);
    };
    let wallet = match &info.tba {
        Some(a) => format!("{a}  (token-bound account)"),
        None => "—".to_string(),
    };
    let persona = if info.has_persona { "published" } else { "none" };
    let face = info
        .public_face
        .clone()
        .unwrap_or_else(|| "unset (directory)".to_string());
    format!(
        "{name}.localharness.xyz\n  \
         owner    {owner}\n  \
         tokenId  {id}\n  \
         wallet   {wallet}\n  \
         persona  {persona}\n  \
         face     {face}",
        name = info.name,
        id = info.token_id,
    )
}

/// Render a `WhoamiInfo` as a JSON object (`whoami --json`). Stable field
/// names so agents can script against the CLI. Pure.
fn format_whoami_json(info: &WhoamiInfo) -> String {
    let v = serde_json::json!({
        "name": info.name,
        "registered": info.owner.is_some(),
        "owner": info.owner,
        "tokenId": info.token_id,
        "wallet": info.tba,
        "persona": info.has_persona,
        "face": info.public_face,
    });
    serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
}

/// Resolve the on-chain profile of `<name>`. All read-only RPC — no `$LH`.
/// A failed sub-read (TBA / persona / face) degrades to absent rather than
/// failing the whole lookup; only an owner-read error is fatal.
async fn resolve_whoami(name: &str) -> Result<WhoamiInfo, String> {
    let owner = registry::owner_of_name(name).await?;
    if owner.is_none() {
        return Ok(WhoamiInfo {
            name: name.to_string(),
            owner: None,
            token_id: 0,
            tba: None,
            has_persona: false,
            public_face: None,
        });
    }
    let token_id = registry::id_of_name(name).await.unwrap_or(0);
    let tba = registry::tba_of_name(name).await.ok().flatten();
    let (has_persona, public_face) = if token_id != 0 {
        (
            registry::persona_of(token_id)
                .await
                .ok()
                .flatten()
                .is_some(),
            registry::public_face_of(token_id).await.ok().flatten(),
        )
    } else {
        (false, None)
    };
    Ok(WhoamiInfo {
        name: name.to_string(),
        owner,
        token_id,
        tba,
        has_persona,
        public_face,
    })
}

/// Print a profile of `<name>`: owner, tokenId, token-bound wallet, and
/// whether a persona / app face is published. `--json` for machine output.
async fn whoami(name: &str, json: bool) -> i32 {
    match resolve_whoami(name).await {
        Ok(info) => {
            println!(
                "{}",
                if json {
                    format_whoami_json(&info)
                } else {
                    format_whoami(&info)
                }
            );
            0
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            1
        }
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parse a `0x`-optional 20-byte hex address into bytes.
fn parse_addr20(s: &str) -> Option<[u8; 20]> {
    let t = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    if t.len() != 40 {
        return None;
    }
    let mut out = [0u8; 20];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(t.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Registry name rule: 1-63 chars, lowercase a-z / 0-9 / hyphen.
fn name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

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
        assert_eq!(
            thread_file_target("claude", "claude__alice.bin").as_deref(),
            Some("alice")
        );
        // A target containing the separator stays intact (strip_prefix once).
        assert_eq!(
            thread_file_target("claude", "claude__a__b.bin").as_deref(),
            Some("a__b")
        );
        // Different caller → not ours.
        assert_eq!(thread_file_target("claude", "bob__alice.bin"), None);
        // Wrong extension, or empty target → rejected.
        assert_eq!(thread_file_target("claude", "claude__alice.txt"), None);
        assert_eq!(thread_file_target("claude", "claude__.bin"), None);
        assert_eq!(thread_file_target("claude", "unrelated.bin"), None);
    }

    #[test]
    fn thread_file_target_roundtrips_history_path() {
        // The parser must invert the filename half of history_path.
        let p = history_path("claude", "alice");
        let name = p.file_name().unwrap().to_str().unwrap();
        assert_eq!(thread_file_target("claude", name).as_deref(), Some("alice"));
    }

    #[test]
    fn take_as_flag_extracts_caller() {
        let a = args(&["--as", "bob", "threads"]);
        let (caller, rest) = take_as_flag(&a).unwrap();
        assert_eq!(caller, Some("bob"));
        assert_eq!(rest, &["threads".to_string()]);

        let b = args(&["alice"]);
        let (caller, rest) = take_as_flag(&b).unwrap();
        assert_eq!(caller, None);
        assert_eq!(rest, &["alice".to_string()]);

        assert!(take_as_flag(&args(&["--as"])).is_err());
    }

    #[test]
    fn history_path_keys_on_caller_and_target() {
        let p = history_path("claude", "alice");
        assert!(p.ends_with("claude__alice.bin"));
        // Distinct caller or target → distinct file (no cross-thread bleed).
        assert_ne!(history_path("claude", "alice"), history_path("bob", "alice"));
        assert_ne!(history_path("claude", "alice"), history_path("claude", "bob"));
        // Lives under a hidden dir so it doesn't clutter the working tree.
        assert!(p.starts_with(".localharness"));
    }

    #[test]
    fn format_whoami_unregistered_is_one_line() {
        let info = WhoamiInfo {
            name: "ghost".into(),
            owner: None,
            token_id: 0,
            tba: None,
            has_persona: false,
            public_face: None,
        };
        assert_eq!(format_whoami(&info), "ghost is unregistered");
    }

    #[test]
    fn format_whoami_full_profile() {
        let info = WhoamiInfo {
            name: "claude".into(),
            owner: Some("0xabc".into()),
            token_id: 8,
            tba: Some("0xdef".into()),
            has_persona: true,
            public_face: Some("app".into()),
        };
        let out = format_whoami(&info);
        assert!(out.starts_with("claude.localharness.xyz\n"));
        assert!(out.contains("owner    0xabc"));
        assert!(out.contains("tokenId  8"));
        assert!(out.contains("wallet   0xdef  (token-bound account)"));
        assert!(out.contains("persona  published"));
        assert!(out.contains("face     app"));
    }

    #[test]
    fn format_whoami_absent_persona_and_face() {
        let info = WhoamiInfo {
            name: "bare".into(),
            owner: Some("0x1".into()),
            token_id: 3,
            tba: None,
            has_persona: false,
            public_face: None,
        };
        let out = format_whoami(&info);
        assert!(out.contains("persona  none"));
        assert!(out.contains("face     unset (directory)"));
        assert!(out.contains("wallet   —"));
    }

    #[test]
    fn format_whoami_json_registered_roundtrips() {
        let info = WhoamiInfo {
            name: "claude".into(),
            owner: Some("0xabc".into()),
            token_id: 8,
            tba: Some("0xdef".into()),
            has_persona: true,
            public_face: Some("app".into()),
        };
        let v: serde_json::Value = serde_json::from_str(&format_whoami_json(&info)).unwrap();
        assert_eq!(v["name"], "claude");
        assert_eq!(v["registered"], true);
        assert_eq!(v["owner"], "0xabc");
        assert_eq!(v["tokenId"], 8);
        assert_eq!(v["wallet"], "0xdef");
        assert_eq!(v["persona"], true);
        assert_eq!(v["face"], "app");
    }

    #[test]
    fn format_whoami_json_unregistered_nulls() {
        let info = WhoamiInfo {
            name: "ghost".into(),
            owner: None,
            token_id: 0,
            tba: None,
            has_persona: false,
            public_face: None,
        };
        let v: serde_json::Value = serde_json::from_str(&format_whoami_json(&info)).unwrap();
        assert_eq!(v["registered"], false);
        assert!(v["owner"].is_null());
        assert!(v["wallet"].is_null());
        assert!(v["face"].is_null());
        assert_eq!(v["persona"], false);
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

    #[test]
    fn rustlite_compiles_a_minimal_cartridge() {
        // Uses only primitives proven in the live claude-app.rl face.
        let src = "fn frame(t: i32) {\n  \
                   let w: i32 = host::display::width();\n  \
                   host::display::clear(0);\n  \
                   host::display::fill_rect(0, 0, w, 8, 16777215);\n  \
                   host::display::present();\n}";
        let wasm = localharness::rustlite::compile(src).expect("minimal cartridge compiles");
        assert_eq!(&wasm[0..4], b"\0asm", "valid wasm magic header");
        assert!(wasm.len() <= PUBLISH_CAP);
    }

    #[test]
    fn rustlite_rejects_garbage() {
        assert!(localharness::rustlite::compile("this is not rustlite").is_err());
    }

    #[test]
    fn name_validation_matches_registry_rule() {
        assert!(name_is_valid("alice"));
        assert!(name_is_valid("a-1-b"));
        assert!(!name_is_valid("Alice")); // uppercase
        assert!(!name_is_valid("a_b")); // underscore
        assert!(!name_is_valid("")); // empty
        assert!(name_is_valid(&"a".repeat(63)));
        assert!(!name_is_valid(&"a".repeat(64))); // too long
    }

    #[test]
    fn parse_addr20_roundtrips_registry_address() {
        let a = parse_addr20(registry::REGISTRY_ADDRESS).expect("valid registry addr");
        assert_eq!(a.len(), 20);
        // Case-insensitive, 0x-optional.
        assert_eq!(parse_addr20("0x00"), None); // wrong length
        assert!(parse_addr20(&"0".repeat(40)).is_some());
    }
}
