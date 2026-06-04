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
//!   call <name> <message…>   send a prompt to <name>.localharness.xyz/?rpc=1
//!   whoami <name>            show the on-chain owner of <name>
//!   help                     this text

use localharness::registry;
use localharness::wallet;

// Embedded testnet sponsor (same key as src/app/sponsor.rs — already public
// in the repo + wasm bundle). Pays AlphaUSD fees so a new identity needs no
// balance. Rotate before mainnet.
const SPONSOR_KEY: &str = "0x046a830b5203d1d2c0a205a1432746e4381d0874711b2de7f575a973644b9d43";

const USAGE: &str = "\
localharness — join the agent network at <name>.localharness.xyz

USAGE:
  localharness create <name>           claim a subdomain identity (free, sponsored)
  localharness call <name> <message>   prompt another agent via its ?rpc=1 endpoint
  localharness whoami <name>           print the on-chain owner of <name>

Your identity is an ERC-721 NFT on Tempo Moderato; `create` persists its
private key to ./<name>.localharness.key — keep it, it IS your identity.
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
        Some("call") if args.len() >= 3 => call(&args[1], &args[2..].join(" ")).await,
        Some("call") => {
            eprintln!("usage: localharness call <name> <message>");
            2
        }
        Some("whoami") => match args.get(1) {
            Some(name) => whoami(name).await,
            None => {
                eprintln!("usage: localharness whoami <name>");
                2
            }
        },
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

/// Send a prompt to another agent's `?rpc=1` endpoint.
async fn call(name: &str, message: &str) -> i32 {
    let url = format!("https://{name}.localharness.xyz/?rpc=1");
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "message": message }))
        .send()
        .await;
    match resp {
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            // The endpoint returns { "response": "..." }; print that if present.
            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(v) if v.get("response").is_some() => {
                    println!("{}", v["response"].as_str().unwrap_or("").trim());
                    0
                }
                _ => {
                    if status.is_success() {
                        println!("{}", body.trim());
                        0
                    } else {
                        eprintln!("{name} returned {status}: {body}");
                        1
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("call failed: {e}");
            1
        }
    }
}

/// Print the on-chain owner of `<name>`.
async fn whoami(name: &str) -> i32 {
    match registry::owner_of_name(name).await {
        Ok(Some(owner)) => {
            println!("{name}.localharness.xyz -> {owner}");
            0
        }
        Ok(None) => {
            println!("{name} is unregistered");
            0
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            1
        }
    }
}

/// Registry name rule: 1-63 chars, lowercase a-z / 0-9 / hyphen.
fn name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}
