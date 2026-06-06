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
//!   create <name> [--persona <text|file>]
//!                            claim <name>.localharness.xyz (persists the key);
//!                            --persona ships its on-chain system prompt too
//!   face <name> <directory|app|html>
//!                            set the subdomain's public face (visitor view)
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
//!   list [--as <me>]         list the subdomains you own (`--json` for machine output)
//!   feedback [--as <me>] [text]
//!                            submit on-chain feedback (text), or read the log (no text)
//!   probe [--as <fleet>]     autonomous QA self-checks; report failures on-chain
//!   triage                   dedup + recurrence-rank the on-chain feedback log
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
  localharness create <name> [--persona <text|file>]
                                         claim a subdomain identity (free, sponsored);
                                         --persona publishes its system prompt too,
                                         so the name ships configured in one command
  localharness face <name> <directory|app|html>
                                         set what visitors see (publish sets 'app')
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
  localharness mcp                       run an MCP (stdio) server exposing a
                                         `call_agent` tool, so any MCP client
                                         (Claude Code, …) can call localharness
                                         agents; pays as the local identity
  localharness list [--as <me>]          list the subdomains you own (+ --json)
  localharness credits [--as <me>]       show your $LH wallet + per-call meter + session
  localharness topup [--as <me>]         claim daily $LH + fund the per-call meter
                                         (the billing self-test: topup -> call -> credits)
  localharness feedback [--as <me>] [text]   submit on-chain feedback, or read all
  localharness probe [--as <fleet>]      run QA self-checks; report failures on-chain
  localharness triage                    dedup + rank the on-chain feedback log
  localharness threads [--as <me>]       list your saved call conversations
  localharness forget [--as <me>] <name> drop a saved conversation (or --all)
  localharness whoami [--json] <name>    profile of <name> (owner, wallet, …; alias: lookup)

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
        Some("create") => match parse_create_args(&args[1..]) {
            Ok((name, persona)) => create(&name, persona.as_deref()).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("publish") if args.len() >= 3 => publish(&args[1], &args[2]).await,
        Some("publish") => {
            eprintln!("usage: localharness publish <name> <source.rl>");
            2
        }
        Some("face") if args.len() >= 3 => set_face(&args[1], &args[2]).await,
        Some("face") => {
            eprintln!("usage: localharness face <name> <directory|app|html>");
            2
        }
        Some("compile") if args.len() >= 2 => compile_check(&args[1], args.get(2).map(String::as_str)),
        Some("compile") => {
            eprintln!("usage: localharness compile <source.rl> [out.wasm]");
            2
        }
        Some("persona") if args.len() >= 3 => set_persona(&args[1], &args[2..].join(" ")).await,
        Some("persona") => {
            eprintln!("usage: localharness persona <name> <text-or-file>");
            2
        }
        Some("call") => call(&args[1..]).await,
        Some("mcp") => mcp_serve(&args[1..]).await,
        Some("list") | Some("mine") => match parse_list_flags(&args[1..]) {
            Ok((caller, json)) => list_mine(caller.as_deref(), json).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("feedback") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) if rest.is_empty() => {
                let _ = caller;
                feedback_read().await
            }
            Ok((caller, rest)) => feedback_submit(caller.as_deref(), &rest.join(" ")).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("topup") => match take_as_flag(&args[1..]) {
            Ok((caller, _)) => topup(caller.as_deref()).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("credits") => match take_as_flag(&args[1..]) {
            Ok((caller, _)) => credits_show(caller.as_deref()).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("probe") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) if rest.iter().any(|a| a == "--deep") => {
                probe_agent(caller.as_deref()).await
            }
            Ok((caller, _)) => probe(caller.as_deref()).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("triage") => triage().await,
        Some("threads") => match take_as_flag(&args[1..]) {
            Ok((caller, _)) => threads(caller.as_deref()),
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("forget") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => match rest.first() {
                Some(target) => forget(caller.as_deref(), target),
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
        Some("whoami") | Some("lookup") => {
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
        Some("version") | Some("--version") | Some("-V") => {
            println!("localharness {}", env!("CARGO_PKG_VERSION"));
            0
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

/// Parse `create <name> [--persona <text|file>]`. Pure/testable. One-shot
/// actor creation: a name plus, optionally, its on-chain system prompt.
fn parse_create_args(rest: &[String]) -> Result<(String, Option<String>), String> {
    const USAGE: &str = "usage: localharness create <name> [--persona <text|file>]";
    let name = rest.first().ok_or(USAGE)?.clone();
    let persona = match rest.get(1).map(String::as_str) {
        None => None,
        Some("--persona") => Some(
            rest.get(2..)
                .filter(|s| !s.is_empty())
                .map(|s| s.join(" "))
                .ok_or(USAGE)?,
        ),
        Some(other) => return Err(format!("unexpected argument '{other}' ({USAGE})")),
    };
    Ok((name, persona))
}

/// Claim `<name>.localharness.xyz` — fresh identity, sponsored register,
/// on-chain verify, key persisted. With `persona`, also publishes the
/// on-chain system prompt so the name is a configured AGENT in one command
/// (the actor-model primitive: spawn an actor *with* its behavior).
async fn create(name: &str, persona: Option<&str>) -> i32 {
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
            // One-shot actor: publish the persona right after the claim so the
            // name ships with its behavior, no separate edit step.
            if let Some(p) = persona {
                println!("  publishing persona …");
                let code = set_persona(name, p).await;
                if code != 0 {
                    return code;
                }
            }
            println!("  next: read https://localharness.xyz/llms.txt for the full API");
            0
        }
        other => {
            eprintln!("registration didn't verify on-chain: {other:?}");
            1
        }
    }
}

/// Set `<name>`'s on-chain public face choice: `directory`, `app`, or `html`.
/// What visitors see. Owner-gated `setMetadata`, sponsored. (`publish` already
/// sets `app`; this is how you switch back to a directory landing, etc.)
async fn set_face(name: &str, choice: &str) -> i32 {
    if !matches!(choice, "directory" | "app" | "html") {
        eprintln!("face must be one of: directory, app, html (got '{choice}')");
        return 2;
    }
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
    let id = match registry::id_of_name(name).await {
        Ok(i) if i != 0 => i,
        Ok(_) => {
            eprintln!("{name} is not registered");
            return 1;
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    match registry::owner_of_name(name).await {
        Ok(Some(o)) if o.eq_ignore_ascii_case(&addr) => {}
        Ok(Some(o)) => {
            eprintln!("{name} is owned by {o}, not your key ({addr})");
            return 1;
        }
        _ => {
            eprintln!("{name} is not registered");
            return 1;
        }
    }
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
        input: registry::encode_set_public_face(id, choice),
    }];
    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };
    match registry::submit_tempo_sponsored(
        &signer,
        &sponsor,
        calls,
        registry::ALPHA_USD_ADDRESS,
        1_200_000,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ {name}.localharness.xyz public face → {choice}");
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("set-face failed: {e}");
            1
        }
    }
}

/// The on-chain `setMetadata` publish cap for a compiled cartridge (bytes).
const PUBLISH_CAP: usize = 16_384;

/// Map a filesystem IO error to a clean, OS-agnostic message. `verb` is the
/// attempted action ("read"/"write"). Addresses on-chain QA feedback: raw
/// `std::fs` errors leaked "(os error 2)" to users instead of a readable
/// "file not found".
fn clean_io_error(verb: &str, path: &str, e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => format!("file not found: {path}"),
        std::io::ErrorKind::PermissionDenied => format!("permission denied: {path}"),
        _ => format!("cannot {verb} {path}: {e}"),
    }
}

/// Read a file, mapping common IO errors to clean, OS-agnostic messages.
fn read_file_clean(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| clean_io_error("read", path, &e))
}

/// True when `arg` looks like it was MEANT as a file path (a path separator or a
/// known text/source extension) rather than literal persona text. Used so the
/// `persona` command can give a clean "file not found" error when the user
/// clearly intended a file, instead of silently using the path string as the
/// persona OR leaking a raw "(os error 2)".
fn looks_like_path(arg: &str) -> bool {
    arg.contains('/')
        || arg.contains('\\')
        || [".txt", ".md", ".rl", ".json", ".toml", ".prompt"]
            .iter()
            .any(|ext| arg.to_ascii_lowercase().ends_with(ext))
}

/// Resolve the `persona` arg to its text: a readable file's contents, or the
/// arg used verbatim. Returns a clean error (never a raw OS error) when the arg
/// is path-shaped but unreadable. A non-path-shaped string is always literal
/// text — so a one-line persona never trips the filesystem.
fn resolve_persona_arg(text_or_path: &str) -> Result<String, String> {
    match std::fs::read_to_string(text_or_path) {
        Ok(s) => Ok(s),
        // Path-shaped + unreadable → the user meant a file; surface it cleanly.
        Err(e) if looks_like_path(text_or_path) => Err(clean_io_error("read", text_or_path, &e)),
        // Otherwise the arg IS the persona text.
        Err(_) => Ok(text_or_path.to_string()),
    }
}

/// True if the compiled cartridge exports a `frame` or `render` function — the
/// entry point the display loader calls. A cartridge without one compiles fine
/// but renders nothing as a public face. Parses the wasm export section (id 7);
/// conservative — returns false if the bytes don't parse cleanly.
fn cartridge_has_entry(wasm: &[u8]) -> bool {
    fn leb(b: &[u8], i: &mut usize) -> Option<u64> {
        let (mut result, mut shift) = (0u64, 0u32);
        loop {
            let byte = *b.get(*i)?;
            *i += 1;
            result |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Some(result);
            }
            shift += 7;
            if shift >= 64 {
                return None;
            }
        }
    }
    if wasm.len() < 8 || &wasm[0..4] != b"\0asm" {
        return false;
    }
    let mut i = 8; // skip magic + version
    while i < wasm.len() {
        let id = wasm[i];
        i += 1;
        let Some(size) = leb(wasm, &mut i) else {
            return false;
        };
        let section_end = i + size as usize;
        if section_end > wasm.len() {
            return false;
        }
        if id == 7 {
            let mut j = i;
            let Some(count) = leb(wasm, &mut j) else {
                return false;
            };
            for _ in 0..count {
                let Some(name_len) = leb(wasm, &mut j) else {
                    return false;
                };
                let Some(name) = wasm.get(j..j + name_len as usize) else {
                    return false;
                };
                j += name_len as usize;
                if name == b"frame" || name == b"render" {
                    return true;
                }
                j += 1; // export kind
                if leb(wasm, &mut j).is_none() {
                    return false;
                }
            }
        }
        i = section_end;
    }
    false
}

/// Compile-check a rustlite cartridge locally and report its size — NO on-chain
/// write. Lets an author iterate before spending a sponsored publish. With
/// `out_path`, also writes the compiled `.wasm` (handy for local validation).
fn compile_check(source_path: &str, out_path: Option<&str>) -> i32 {
    let src = match read_file_clean(source_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    match localharness::rustlite::compile(&src) {
        Ok(wasm) => {
            println!("✓ compiled {source_path} → {} bytes of wasm", wasm.len());
            if let Some(out) = out_path {
                if let Err(e) = std::fs::write(out, &wasm) {
                    eprintln!("  {}", clean_io_error("write", out, &e));
                    return 1;
                }
                println!("  wrote {out}");
            }
            if !cartridge_has_entry(&wasm) {
                eprintln!(
                    "  ✗ no `frame` or `render` export — the loader has no entry to \
                     call, so this would render nothing as a face"
                );
                return 1;
            }
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

    let src = match read_file_clean(source_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
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
    // A cartridge with no entry point compiles but renders nothing — refuse to
    // publish a dead face (the visitor would see a blank canvas forever).
    if !cartridge_has_entry(&wasm) {
        eprintln!(
            "compiled cartridge has no `frame`/`render` export — it would render \
             nothing as a face; aborting before the on-chain write"
        );
        return 1;
    }
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
    model: Option<String>,
    target: String,
    message: String,
}

const CALL_USAGE: &str =
    "usage: localharness call [--as <yourname>] [--fresh] [--model <id>] <target> <message>";

fn parse_call_args(rest: &[String]) -> Result<ParsedCall, String> {
    let mut caller = None;
    let mut fresh = false;
    let mut model = None;
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
fn history_dir() -> std::path::PathBuf {
    std::path::Path::new(".localharness").join("history")
}

/// The serialization-backend tag a `model` routes to. Conversation history is
/// serialized in a BACKEND-SPECIFIC wire shape (a Gemini thread loaded into the
/// Anthropic backend dies with `missing field 'content'` and vice-versa), so
/// the persisted thread is keyed by this tag — the two backends never share a
/// file. Mirrors the `claude*` → Anthropic routing in `run_agent_turn`.
fn model_backend_tag(model: Option<&str>) -> &'static str {
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
fn history_path(caller_label: &str, target: &str, backend: &str) -> std::path::PathBuf {
    history_dir().join(format!("{caller_label}__{target}.{backend}.bin"))
}

/// Extract the target from a history filename `<caller>__<target>.<backend>.bin`
/// (or the legacy `<caller>__<target>.bin`) for the given caller label. `None`
/// when it doesn't belong to that caller. Pure. A trailing `.gemini`/`.anthropic`
/// backend tag is stripped so `threads`/`forget` show the bare target.
fn thread_file_target(caller_label: &str, file_name: &str) -> Option<String> {
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
    let (key_file, key_hex) = match resolve_caller_key(caller.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    // Conversations persist per (caller, target, backend) so repeated calls
    // continue the same thread; `--fresh` starts over. Label by the key-file
    // stem. Keying on the backend too keeps a Gemini thread and an Anthropic
    // thread to the same target in SEPARATE files — their on-disk history
    // formats are incompatible (a Gemini thread loaded into the Anthropic
    // backend dies with `missing field 'content'`).
    let caller_label = key_file
        .strip_suffix(".localharness.key")
        .unwrap_or(&key_file)
        .to_string();
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
/// persona, paid for by the identity behind `key_hex` (proxy auth + a free $LH
/// session). Returns the reply text plus the updated conversation history bytes
/// (to persist for the next turn). Shared by the CLI `call` command and the
/// `mcp` server's `call_agent` tool, so both reach an agent identically.
async fn run_agent_turn(
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

    // Open a free $LH session so the proxy doesn't 402 (best-effort).
    if let Ok(sponsor) = wallet::from_private_key_hex(SPONSOR_KEY) {
        let _ = registry::open_session_sponsored(&caller, &sponsor, registry::ALPHA_USD_ADDRESS)
            .await;
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
    // either provider with no provider key of its own.
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
            let agent = match localharness::Agent::start_anthropic(build(prior_history.clone())).await
            {
                Ok(a) => a,
                Err(_) if prior_history.is_some() => {
                    // Incompatible/corrupt saved thread → warn + start fresh
                    // rather than failing the whole call.
                    eprintln!(
                        "warning: could not load saved conversation with {target} \
                         (incompatible or corrupt) — starting a fresh thread"
                    );
                    localharness::Agent::start_anthropic(build(None))
                        .await
                        .map_err(|e| format!("could not start anthropic session: {e}"))?
                }
                Err(e) => return Err(format!("could not start anthropic session: {e}")),
            };
            let reply = match agent.chat(message).await {
                Ok(resp) => resp.text().await.map_err(|e| format!("response error: {e}")),
                Err(e) => Err(e.to_string()),
            };
            let new_history = agent.history_bytes().ok().flatten();
            let _ = agent.shutdown().await;
            return reply.map(|text| (text, new_history));
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
    let agent = match localharness::Agent::start_gemini(build(prior_history.clone())).await {
        Ok(a) => a,
        Err(_) if prior_history.is_some() => {
            // Incompatible/corrupt saved thread → warn + start fresh rather than
            // failing the whole call.
            eprintln!(
                "warning: could not load saved conversation with {target} \
                 (incompatible or corrupt) — starting a fresh thread"
            );
            localharness::Agent::start_gemini(build(None))
                .await
                .map_err(|e| format!("could not start agent session: {e}"))?
        }
        Err(e) => return Err(format!("could not start agent session: {e}")),
    };
    let reply = match agent.chat(message).await {
        Ok(resp) => resp.text().await.map_err(|e| format!("response error: {e}")),
        Err(e) => Err(e.to_string()),
    };
    let new_history = agent.history_bytes().ok().flatten();
    let _ = agent.shutdown().await;
    reply.map(|text| (text, new_history))
}

// ---- MCP server ----------------------------------------------------------
//
// `localharness mcp` speaks the Model Context Protocol over stdio (newline-
// delimited JSON-RPC 2.0), exposing localharness agents as a TOOL any MCP client
// (Claude Code, Codex, …) can call. The headline tool `call_agent` lets an
// external agent invoke a sovereign `<name>.localharness.xyz` agent under its
// on-chain persona — the demand-side experiment: will anyone actually call these
// agents? The server acts AS the sole identity key in the working directory (it
// signs proxy auth and pays the $LH).

async fn mcp_serve(args: &[String]) -> i32 {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    // The identity that signs proxy auth + pays for outbound calls. `--as <name>`
    // picks it; with a single key in the dir it's inferred.
    let caller = match take_as_flag(args) {
        Ok((caller, _rest)) => caller,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let key_hex = match resolve_caller_key(caller.as_deref()) {
        Ok((_file, hex)) => hex,
        Err(e) => {
            eprintln!("mcp: no usable identity ({e}). Pass --as <name> or run `localharness create <name>` first.");
            return 2;
        }
    };

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    eprintln!("localharness mcp: ready on stdio (acting as the local identity).");

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let req: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // ignore malformed frames
        };
        // Notifications (no `id`, e.g. notifications/initialized) get no reply.
        let Some(id) = req.get("id").cloned() else { continue };
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");

        let envelope = match mcp_handle(method, &req, &key_hex).await {
            Ok(result) => serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, msg)) => {
                serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
            }
        };
        if out.write_all(format!("{envelope}\n").as_bytes()).await.is_err() {
            break;
        }
        let _ = out.flush().await;
    }
    0
}

async fn mcp_handle(
    method: &str,
    req: &serde_json::Value,
    key_hex: &str,
) -> Result<serde_json::Value, (i64, String)> {
    match method {
        "initialize" => Ok(serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "localharness", "version": env!("CARGO_PKG_VERSION") }
        })),
        "tools/list" => Ok(serde_json::json!({ "tools": mcp_tool_list() })),
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or_default();
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_default();
            mcp_tool_call(name, &args, key_hex).await
        }
        "ping" => Ok(serde_json::json!({})),
        other => Err((-32601, format!("method not found: {other}"))),
    }
}

fn mcp_tool_list() -> serde_json::Value {
    serde_json::json!([
        {
            "name": "call_agent",
            "description": "Send a message to a sovereign localharness agent (a <name>.localharness.xyz NFT) and get its reply. The agent answers under its published on-chain persona; this server's configured identity pays in $LH credits.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "the agent's registered name / subdomain, e.g. \"claude\"" },
                    "message": { "type": "string", "description": "the message to send the agent" }
                },
                "required": ["name", "message"]
            }
        }
    ])
}

async fn mcp_tool_call(
    name: &str,
    args: &serde_json::Value,
    key_hex: &str,
) -> Result<serde_json::Value, (i64, String)> {
    match name {
        "call_agent" => {
            let target = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            let message = args.get("message").and_then(|v| v.as_str()).unwrap_or("");
            if target.is_empty() || message.trim().is_empty() {
                return Ok(mcp_text_result("call_agent requires both 'name' and 'message'", true));
            }
            // Stateless per MCP request for v1 (no persisted thread).
            match run_agent_turn(key_hex, target, message, None, None).await {
                Ok((text, _hist)) => Ok(mcp_text_result(text.trim(), false)),
                Err(e) => Ok(mcp_text_result(&format!("call_agent failed: {e}"), true)),
            }
        }
        other => Err((-32602, format!("unknown tool: {other}"))),
    }
}

fn mcp_text_result(text: &str, is_error: bool) -> serde_json::Value {
    serde_json::json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": is_error
    })
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

/// Extract a `--as <name>` flag from ANYWHERE in the arg list (not just the
/// first position) and return `(caller, remaining_args_without_the_flag)`. The
/// remainder is owned so the flag can be removed from the middle. Position-
/// fragile parsing was a real bug: `probe --deep --as fleet` failed because
/// `--as` wasn't first, so the fleet name was never resolved and the call
/// errored with "multiple identities". A second `--as` is an error.
fn take_as_flag(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
    let mut caller: Option<String> = None;
    let mut rest: Vec<String> = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--as" {
            if caller.is_some() {
                return Err("--as given more than once".to_string());
            }
            match args.get(i + 1) {
                Some(n) => {
                    caller = Some(n.clone());
                    i += 2;
                }
                None => return Err("usage: --as <name> requires a name".to_string()),
            }
        } else {
            rest.push(args[i].clone());
            i += 1;
        }
    }
    Ok((caller, rest))
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
    // A path-shaped-but-unreadable arg gets a CLEAN error, not a raw OS error.
    let persona = match resolve_persona_arg(text_or_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
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

/// Parse `list`'s optional `--as <name>` / `--json` flags (order-independent).
/// `list` takes no positional args — anything else is an error.
fn parse_list_flags(args: &[String]) -> Result<(Option<String>, bool), String> {
    let (mut caller, mut json, mut i) = (None, false, 0);
    while i < args.len() {
        match args[i].as_str() {
            "--as" => {
                caller = Some(
                    args.get(i + 1)
                        .ok_or("usage: localharness list [--as <me>] [--json]")?
                        .clone(),
                );
                i += 2;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok((caller, json))
}

/// Render the caller's owned subdomains. Pure (no I/O) so it's unit-testable.
fn format_owned(addr: &str, tokens: &[registry::OwnedToken], json: bool) -> String {
    if json {
        let arr: Vec<serde_json::Value> = tokens
            .iter()
            .map(|t| {
                serde_json::json!({ "name": t.name, "tokenId": t.token_id, "wallet": t.tba })
            })
            .collect();
        return serde_json::to_string_pretty(&serde_json::json!({
            "owner": addr,
            "count": tokens.len(),
            "subdomains": arr,
        }))
        .unwrap_or_else(|_| "{}".to_string());
    }
    if tokens.is_empty() {
        return format!("no subdomains owned by {addr}\n");
    }
    let mut out = format!("{} subdomain(s) owned by {addr}:\n", tokens.len());
    for t in tokens {
        let wallet = t.tba.as_deref().unwrap_or("—");
        out.push_str(&format!("  {}  (tokenId {})  {wallet}\n", t.name, t.token_id));
    }
    out
}

/// List the subdomains the caller's identity owns (read-only — no `$LH`).
/// Mirrors the browser `list_subdomains` tool.
async fn list_mine(caller_name: Option<&str>, json: bool) -> i32 {
    let (key_file, key_hex) = match resolve_caller_key(caller_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
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
    match registry::list_owned_tokens(&addr).await {
        Ok(tokens) => {
            print!("{}", format_owned(&addr, &tokens, json));
            0
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            1
        }
    }
}

/// A parsed `qa/v1` autonomous-fleet feedback envelope. `version`/`body` are
/// consumed by the triage agent (roadmap Phase 4); `source` tags the listing.
#[allow(dead_code)]
struct QaEnvelope {
    source: String,
    version: String,
    body: String,
}

/// Parse a `qa/v1 source=<s> v<ver>: <body>` envelope. `None` unless it is a
/// well-formed qa/v1 envelope — the triage path must NOT consume a body (e.g.
/// a repro string) from a malformed or non-fleet entry, since the feedback log
/// is permissionless and an attacker can plant crafted text (a critique gate).
fn parse_qa_envelope(text: &str) -> Option<QaEnvelope> {
    let (header, body) = text.strip_prefix("qa/v1 ")?.split_once(": ")?;
    let source = header.split_whitespace().find_map(|t| t.strip_prefix("source="))?;
    let version = header.split_whitespace().find_map(|t| {
        t.strip_prefix('v')
            .filter(|v| v.starts_with(|c: char| c.is_ascii_digit()))
    })?;
    if source.is_empty() || body.trim().is_empty() {
        return None;
    }
    Some(QaEnvelope {
        source: source.to_string(),
        version: version.to_string(),
        body: body.to_string(),
    })
}

/// Render the on-chain feedback log (newest first). Pure for testing. Entries
/// the autonomous fleet authored (valid `qa/v1` envelopes) are tagged so the
/// maintainer can tell agent-filed bugs from human ones at a glance.
fn format_feedback(entries: &[registry::FeedbackEntry]) -> String {
    if entries.is_empty() {
        return "no on-chain feedback yet\n".to_string();
    }
    let mut out = format!("{} on-chain feedback entr(ies), newest first:\n", entries.len());
    for e in entries {
        let tag = match parse_qa_envelope(&e.text) {
            Some(env) => format!(" [fleet:{}]", env.source),
            None => String::new(),
        };
        out.push_str(&format!(
            "  [{}] {}{}\n    {}\n",
            e.timestamp,
            e.sender,
            tag,
            e.text.replace('\n', " ")
        ));
    }
    out
}

/// Collapse feedback bodies into a deduplicated, recurrence-ranked work-list:
/// the same bug filed across many probe runs becomes ONE item, ranked by how
/// often it recurred (most-reported first). Dedup BEFORE ranking, else the
/// log's natural repetition drowns the signal. Ties break by first-seen order
/// for stable output. The triage agent's deterministic core (roadmap Phase 4).
fn triage_findings(bodies: &[String]) -> Vec<(String, usize)> {
    use std::collections::HashMap;
    // key -> (representative text, count, first-seen index)
    let mut counts: HashMap<String, (String, usize, usize)> = HashMap::new();
    for (i, body) in bodies.iter().enumerate() {
        let key = body.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        if key.is_empty() {
            continue;
        }
        let e = counts.entry(key).or_insert_with(|| (body.trim().to_string(), 0, i));
        e.1 += 1;
    }
    let mut v: Vec<(String, usize, usize)> = counts.into_values().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
    v.into_iter().map(|(rep, count, _)| (rep, count)).collect()
}

/// `localharness triage` — read the on-chain feedback log and print a
/// deduplicated, recurrence-ranked work-list. Read-only, no `$LH`. Prefers the
/// `qa/v1` body when an entry is a fleet envelope.
async fn triage() -> i32 {
    let entries = match registry::list_feedback().await {
        Ok(e) => e,
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    let bodies: Vec<String> = entries
        .iter()
        .map(|e| parse_qa_envelope(&e.text).map(|env| env.body).unwrap_or_else(|| e.text.clone()))
        .collect();
    let ranked = triage_findings(&bodies);
    if ranked.is_empty() {
        println!("no feedback to triage");
        return 0;
    }
    println!("{} distinct item(s), most-recurring first:", ranked.len());
    for (i, (rep, count)) in ranked.iter().enumerate() {
        println!("  {}. (x{count}) {}", i + 1, rep.replace('\n', " "));
    }
    0
}

/// Read the on-chain feedback log (`localharness feedback`, no text).
async fn feedback_read() -> i32 {
    match registry::list_feedback().await {
        Ok(entries) => {
            print!("{}", format_feedback(&entries));
            0
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            1
        }
    }
}

/// Submit on-chain feedback as the caller's identity (sponsored). This is the
/// agent-to-platform leg of the feedback loop: a test agent reports bugs / UX
/// friction / errors here, and `feedback` (no text) reads them back.
async fn feedback_submit(caller_name: Option<&str>, text: &str) -> i32 {
    let text = text.trim();
    if text.is_empty() {
        eprintln!("feedback text is empty");
        return 2;
    }
    if text.len() > 2048 {
        eprintln!("feedback too long: {} bytes (max 2048)", text.len());
        return 1;
    }
    let (key_file, key_hex) = match resolve_caller_key(caller_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };
    println!("submitting {}-byte feedback on-chain …", text.len());
    match registry::submit_feedback_sponsored(&signer, &sponsor, text, registry::ALPHA_USD_ADDRESS)
        .await
    {
        Ok(tx) => {
            println!("✓ feedback submitted\n  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("feedback failed: {e}");
            1
        }
    }
}

/// Lowercase 0x string of a 20-byte address (the credit identity the proxy
/// authenticates + meters).
fn addr_to_hex(a: [u8; 20]) -> String {
    let mut s = String::from("0x");
    for b in a {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Format `$LH` wei as a 2-decimal LH string.
fn fmt_lh(wei: u128) -> String {
    let whole = wei / 1_000_000_000_000_000_000u128;
    let cents = (wei % 1_000_000_000_000_000_000u128) / 10_000_000_000_000_000u128;
    format!("{whole}.{cents:02} LH")
}

/// `localharness credits [--as <me>]` — show the caller's billing state: wallet
/// `$LH`, the per-request meter (`creditOf`, what per-call billing debits), and
/// any session window. Read-only; these are the exact numbers the proxy gates on.
async fn credits_show(caller_name: Option<&str>) -> i32 {
    let (key_file, key_hex) = match resolve_caller_key(caller_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let addr = addr_to_hex(wallet::address(&signer));
    let token = registry::token_balance_of(&addr).await.unwrap_or(0);
    let meter = registry::credit_balance_of(&addr).await.unwrap_or(0);
    let expiry = registry::session_expiry_of(&addr).await.unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("{addr}");
    println!("  wallet   {}", fmt_lh(token));
    println!("  meter    {}   <- per-call billing debits this", fmt_lh(meter));
    if expiry > now {
        println!(
            "  session  active ~{}min left (free; a funded meter now overrides it)",
            (expiry - now) / 60
        );
    } else {
        println!("  session  none");
    }
    0
}

/// `localharness topup [--as <me>]` — fund the caller for PER-CALL billing:
/// claim the daily `$LH` allowance (if eligible) then deposit the whole wallet
/// balance into the per-request meter, so the proxy debits real `$LH` each
/// `call`. Sponsored — needs no gas. The end-to-end billing self-test:
/// `topup` -> `call` -> `credits` (watch the meter drop).
async fn topup(caller_name: Option<&str>) -> i32 {
    let (key_file, key_hex) = match resolve_caller_key(caller_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };
    let addr = addr_to_hex(wallet::address(&signer));
    // 1. Claim the daily allowance (mints $LH to the wallet) if eligible.
    if registry::can_claim_credits(&addr).await.unwrap_or(false) {
        match registry::claim_daily_sponsored(&signer, &sponsor, registry::ALPHA_USD_ADDRESS).await {
            Ok(tx) => println!("claimed daily $LH  tx: {tx}"),
            Err(e) => eprintln!("claim failed (continuing to deposit): {e}"),
        }
    } else {
        println!("daily allowance already claimed today (or none) - skipping claim");
    }
    // 2. Deposit the wallet balance into the per-request meter.
    let bal = registry::token_balance_of(&addr).await.unwrap_or(0);
    if bal == 0 {
        println!("wallet has 0 $LH - nothing to deposit");
        return 0;
    }
    match registry::deposit_credits_sponsored(&signer, &sponsor, bal, registry::ALPHA_USD_ADDRESS)
        .await
    {
        Ok(tx) => {
            println!("deposited {} into the meter  tx: {tx}", fmt_lh(bal));
            0
        }
        Err(e) => {
            eprintln!("deposit failed: {e}");
            1
        }
    }
}

/// Deterministic, network-free QA checks the `probe` runs against the platform.
/// Each pushes a failure description on an UNEXPECTED result (a real bug); an
/// empty result means every invariant held. Pure + testable — the core of the
/// autonomous loop's read-only observe pass (roadmap Track B / Phase 2).
fn run_qa_checks() -> Vec<String> {
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
/// and files concrete findings on-chain. Needs a live run (proxy + Gemini).
async fn probe_agent(caller_name: Option<&str>) -> i32 {
    let (key_file, key_hex) = match resolve_caller_key(caller_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let caller = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    if let Ok(sponsor) = wallet::from_private_key_hex(SPONSOR_KEY) {
        let _ = registry::open_session_sponsored(&caller, &sponsor, registry::ALPHA_USD_ADDRESS)
            .await;
    }
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
/// live chain read; on any failure it REPORTS on-chain as a `qa/v1` feedback
/// envelope (no human bridge — the agent files its own bug). One-shot and
/// synchronous (no daemon). The checks are deterministic; network is touched
/// only for the chain read and the feedback submit (no `$LH` for the read).
async fn probe(caller_name: Option<&str>) -> i32 {
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
    // Report on-chain as the fleet identity (best-effort). The qa/v1 envelope
    // marks fleet-authored feedback so a future triage pass can filter it.
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
        eprintln!("  → reported on-chain");
    }
    1
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
    fn parse_create_args_name_only_and_with_persona() {
        let (n, p) = parse_create_args(&args(&["alice"])).unwrap();
        assert_eq!(n, "alice");
        assert_eq!(p, None);

        let (n, p) = parse_create_args(&args(&["alice", "--persona", "you", "are", "alice"]))
            .unwrap();
        assert_eq!(n, "alice");
        assert_eq!(p.as_deref(), Some("you are alice"));
    }

    #[test]
    fn parse_create_args_rejects_bad_forms() {
        assert!(parse_create_args(&args(&[])).is_err()); // no name
        assert!(parse_create_args(&args(&["alice", "--persona"])).is_err()); // empty persona
        assert!(parse_create_args(&args(&["alice", "bob"])).is_err()); // stray positional
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
    fn take_as_flag_extracts_caller() {
        let a = args(&["--as", "bob", "threads"]);
        let (caller, rest) = take_as_flag(&a).unwrap();
        assert_eq!(caller.as_deref(), Some("bob"));
        assert_eq!(rest, vec!["threads".to_string()]);

        let b = args(&["alice"]);
        let (caller, rest) = take_as_flag(&b).unwrap();
        assert_eq!(caller, None);
        assert_eq!(rest, vec!["alice".to_string()]);

        assert!(take_as_flag(&args(&["--as"])).is_err());
    }

    #[test]
    fn take_as_flag_scans_any_position() {
        // The real bug: `probe --deep --as fleet` — `--as` is NOT first, so the
        // old first-arg-only parser missed it and the fleet name never resolved.
        let (caller, rest) = take_as_flag(&args(&["--deep", "--as", "fleet"])).unwrap();
        assert_eq!(caller.as_deref(), Some("fleet"));
        assert_eq!(rest, vec!["--deep".to_string()]);

        // Trailing flag is still consumed; surrounding args preserved in order.
        let (caller, rest) = take_as_flag(&args(&["a", "b", "--as", "me", "c"])).unwrap();
        assert_eq!(caller.as_deref(), Some("me"));
        assert_eq!(rest, vec!["a".to_string(), "b".to_string(), "c".to_string()]);

        // `--as` requiring a value, even mid-list.
        assert!(take_as_flag(&args(&["--deep", "--as"])).is_err());
        // A duplicated `--as` is an error, not a silent last-wins.
        assert!(take_as_flag(&args(&["--as", "a", "--as", "b"])).is_err());
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
    fn cartridge_entry_detection() {
        // A real frame() cartridge exports the entry the loader calls.
        let with =
            localharness::rustlite::compile("fn frame(t: i32) { host::display::present(); }")
                .unwrap();
        assert!(cartridge_has_entry(&with), "frame() must be detected");

        // Compiles, but only a helper — no entry → would render nothing.
        let without = localharness::rustlite::compile("fn helper(n: i32) -> i32 { n + 1 }").unwrap();
        assert!(!cartridge_has_entry(&without), "no entry must be rejected");

        // The shipped bitmask cartridge has an entry.
        let bitmask = localharness::rustlite::compile(include_str!("../../bitmask.rl")).unwrap();
        assert!(cartridge_has_entry(&bitmask));

        // Malformed / truncated bytes never panic and report no entry.
        assert!(!cartridge_has_entry(b""));
        assert!(!cartridge_has_entry(b"\0asm")); // header only
        assert!(!cartridge_has_entry(b"\0asm\x01\0\0\0\x07\xff")); // bogus section size
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
    fn usage_documents_every_command() {
        // Every dispatchable subcommand must appear in the help text, so a new
        // command can't ship undocumented for beta testers reading `help`.
        for cmd in [
            "create", "compile", "publish", "face", "persona", "call", "list",
            "feedback", "probe", "triage", "threads", "forget", "whoami",
        ] {
            assert!(
                USAGE.contains(cmd),
                "`{cmd}` is dispatchable but missing from the help/USAGE text"
            );
        }
    }

    #[test]
    fn parse_list_flags_handles_as_and_json_any_order() {
        assert_eq!(parse_list_flags(&args(&[])).unwrap(), (None, false));
        assert_eq!(parse_list_flags(&args(&["--json"])).unwrap(), (None, true));
        let (c, j) = parse_list_flags(&args(&["--as", "bob", "--json"])).unwrap();
        assert_eq!((c.as_deref(), j), (Some("bob"), true));
        let (c, j) = parse_list_flags(&args(&["--json", "--as", "bob"])).unwrap();
        assert_eq!((c.as_deref(), j), (Some("bob"), true));
        assert!(parse_list_flags(&args(&["--as"])).is_err()); // dangling --as
        assert!(parse_list_flags(&args(&["alice"])).is_err()); // no positionals
    }

    #[test]
    fn format_owned_text_and_json() {
        let toks = vec![
            registry::OwnedToken { token_id: 8, name: "claude".into(), tba: Some("0xabc".into()) },
            registry::OwnedToken { token_id: 3, name: "alice".into(), tba: None },
        ];
        let text = format_owned("0xowner", &toks, false);
        assert!(text.contains("2 subdomain"));
        assert!(text.contains("claude  (tokenId 8)  0xabc"));
        assert!(text.contains("alice  (tokenId 3)  —"));

        let v: serde_json::Value =
            serde_json::from_str(&format_owned("0xowner", &toks, true)).unwrap();
        assert_eq!(v["count"], 2);
        assert_eq!(v["owner"], "0xowner");
        assert_eq!(v["subdomains"][0]["name"], "claude");
        assert_eq!(v["subdomains"][0]["tokenId"], 8);
        assert!(v["subdomains"][1]["wallet"].is_null());
    }

    #[test]
    fn read_file_clean_maps_not_found_without_leaking_os_error() {
        // Closes on-chain QA finding #1: "os error 2" must not reach the user.
        let err = read_file_clean("definitely-nonexistent-file-xyz123.rl").unwrap_err();
        assert!(err.contains("file not found"), "got: {err}");
        assert!(err.contains("definitely-nonexistent-file-xyz123.rl"), "got: {err}");
        assert!(!err.contains("os error"), "must not leak raw OS error: {err}");
    }

    #[test]
    fn looks_like_path_distinguishes_files_from_prose() {
        // Path-shaped: separators or known source/text extensions.
        assert!(looks_like_path("persona.txt"));
        assert!(looks_like_path("prompts/agent.md"));
        assert!(looks_like_path("C:\\agents\\bob.prompt"));
        assert!(looks_like_path("./x.rl"));
        // Plain prose persona text is NOT a path.
        assert!(!looks_like_path("You are bob, a helpful agent"));
        assert!(!looks_like_path("bob"));
    }

    #[test]
    fn resolve_persona_arg_literal_text_passthrough() {
        // A non-path-shaped, unreadable string is the persona text verbatim —
        // it must NOT touch the filesystem error path.
        let p = resolve_persona_arg("You are bob, answer tersely").unwrap();
        assert_eq!(p, "You are bob, answer tersely");
    }

    #[test]
    fn resolve_persona_arg_missing_file_is_clean_error() {
        // A path-shaped arg that doesn't exist → clean error, no raw OS error,
        // and NOT silently used as literal text.
        let err = resolve_persona_arg("definitely-nonexistent-xyz123.txt").unwrap_err();
        assert!(err.contains("file not found"), "got: {err}");
        assert!(!err.contains("os error"), "must not leak raw OS error: {err}");
    }

    #[test]
    fn qa_checks_pass_on_a_healthy_platform() {
        // The probe's deterministic invariants must hold against the shipped
        // rustlite + entry detector. If this fails, the probe would (correctly)
        // file an on-chain bug — so it doubles as a platform-health assertion.
        let fails = run_qa_checks();
        assert!(fails.is_empty(), "probe found issues on a healthy build: {fails:?}");
    }

    #[test]
    fn triage_dedups_and_ranks_by_recurrence() {
        let bodies = vec![
            "Compile leaks OS error".to_string(),
            "compile leaks os error".to_string(),       // same modulo case
            "  Compile   leaks OS error ".to_string(),  // same modulo whitespace
            "whoami is slow".to_string(),
        ];
        let ranked = triage_findings(&bodies);
        assert_eq!(ranked.len(), 2, "two distinct issues after dedup");
        assert_eq!(ranked[0].1, 3, "the recurring one ranks first with count 3");
        assert!(ranked[0].0.to_lowercase().contains("compile leaks"));
        assert_eq!(ranked[1].1, 1);
    }

    #[test]
    fn triage_skips_empty_bodies() {
        let bodies = vec!["".to_string(), "   ".to_string(), "real bug".to_string()];
        let ranked = triage_findings(&bodies);
        assert_eq!(ranked, vec![("real bug".to_string(), 1)]);
    }

    #[test]
    fn parse_qa_envelope_accepts_valid_rejects_others() {
        let env =
            parse_qa_envelope("qa/v1 source=qa-probe v0.20.0: compile leaked os error").unwrap();
        assert_eq!(env.source, "qa-probe");
        assert_eq!(env.version, "0.20.0");
        assert!(env.body.contains("compile leaked"));
        // Not a fleet envelope → rejected (triage won't consume its body).
        assert!(parse_qa_envelope("just some human feedback").is_none());
        assert!(parse_qa_envelope("qa/v1 source=x v1.0.0:   ").is_none()); // empty body
        assert!(parse_qa_envelope("qa/v1 no source or colon").is_none());
        assert!(parse_qa_envelope("qa/v1 source=x vNOTVERSION: body").is_none());
    }

    #[test]
    fn format_feedback_tags_fleet_envelopes_only() {
        let entries = vec![
            registry::FeedbackEntry {
                sender: "0x1".into(),
                timestamp: 1,
                text: "qa/v1 source=qa-probe v0.20.0: a real bug".into(),
            },
            registry::FeedbackEntry {
                sender: "0x2".into(),
                timestamp: 2,
                text: "a human note".into(),
            },
        ];
        let out = format_feedback(&entries);
        assert!(out.contains("[fleet:qa-probe]"));
        assert!(
            out.lines().any(|l| l.contains("0x2") && !l.contains("[fleet")),
            "human feedback must not be tagged as fleet"
        );
    }

    #[test]
    fn format_feedback_empty_and_entries() {
        assert!(format_feedback(&[]).contains("no on-chain feedback"));
        let entries = vec![
            registry::FeedbackEntry {
                sender: "0xabc".into(),
                timestamp: 1700000000,
                text: "create flow worked\nbut whoami was slow".into(),
            },
        ];
        let out = format_feedback(&entries);
        assert!(out.contains("1 on-chain feedback"));
        assert!(out.contains("0xabc"));
        // Newlines collapsed so one entry stays one block.
        assert!(out.contains("create flow worked but whoami was slow"));
    }

    #[test]
    fn format_owned_empty() {
        assert!(format_owned("0xo", &[], false).contains("no subdomains"));
        let v: serde_json::Value = serde_json::from_str(&format_owned("0xo", &[], true)).unwrap();
        assert_eq!(v["count"], 0);
        assert!(v["subdomains"].as_array().unwrap().is_empty());
    }

    #[test]
    fn sponsor_key_is_valid_and_derives_documented_address() {
        // The embedded SPONSOR_KEY pays fees for EVERY sponsored CLI op
        // (create/publish/persona). If it's stale or mistyped, all onboarding
        // silently fails. Guard that it parses and derives the documented
        // sponsor address (the dedicated low-budget key, rotated 2026-05-25) —
        // so a future rotation that forgets the bin won't ship broken.
        let signer = wallet::from_private_key_hex(SPONSOR_KEY).expect("SPONSOR_KEY must parse");
        let addr = format!("0x{}", to_hex(&wallet::address(&signer)));
        assert_eq!(
            addr.to_ascii_lowercase(),
            "0x0aff88ad13ef24cac5befd0f9dc3a05df79a922c",
            "SPONSOR_KEY no longer derives the documented sponsor address"
        );
    }

    #[test]
    fn llms_txt_publishes_canonical_onchain_constants() {
        // The agent-facing spec is read by agents to orient on-chain. It must
        // not drift from the code's source of truth — stale addresses would
        // send an agent to the wrong chain/contract. Automates the manual
        // "audit llms.txt vs registry.rs" pass.
        let spec = include_str!("../../web/llms.txt");
        assert!(
            spec.contains(registry::REGISTRY_ADDRESS),
            "llms.txt missing canonical diamond address {}",
            registry::REGISTRY_ADDRESS
        );
        assert!(
            spec.contains(registry::LOCALHARNESS_TOKEN_ADDRESS),
            "llms.txt missing the $LH token address {}",
            registry::LOCALHARNESS_TOKEN_ADDRESS
        );
        assert!(
            spec.contains(registry::RPC_URL),
            "llms.txt missing the RPC URL {}",
            registry::RPC_URL
        );
        assert!(
            spec.contains(&registry::CHAIN_ID.to_string()),
            "llms.txt missing chain id {}",
            registry::CHAIN_ID
        );
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
