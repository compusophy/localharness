#[allow(unused_imports)]
use crate::*;

pub(crate) const ONBOARD_USAGE: &str = "\
usage: localharness onboard --invite <code> [--as <name>]
  Take a brand-new terminal identity from zero to FUNDED so it can transact on
  mainnet — the terminal mirror of the web's new-user $LH grant.
  --invite <code>   accept an onboarding invite: an operator/parent agent runs
                    `invite create` once (escrowing ~2 $LH, supply-neutral) and
                    hands you the code; accepting it funds your wallet.
  --as <name>       name the local key file for this identity (CREATED if absent,
                    NOT claimed on-chain — claim a name later with `create <name>`).
  With no --invite, prints how to get your first $LH and exits non-zero (it never
  invents value — a free faucet would be a sybil hole; funding is operator-paid).";

/// `localharness onboard --invite <code> [--as <name>]` — Phase 1B of
/// `design/cli-mainnet-onboarding.md`. Walks a brand-new terminal identity from
/// zero to FUNDED: ensure a local key exists (created if absent, NO on-chain
/// claim — the key IS the identity; a name is a separate step), then accept an
/// onboarding invite so the keypair holds its first `$LH`. Built entirely on
/// existing primitives (key gen + headless `invite accept`, relay-sponsored gas)
/// — the terminal-native equivalent of the web's ~2 `$LH` new-user gift.
pub(crate) async fn onboard(args: &[String]) -> i32 {
    let mut invite_code: Option<String> = None;
    let mut as_name: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--invite" => match args.get(i + 1) {
                Some(c) => {
                    invite_code = Some(c.clone());
                    i += 2;
                }
                None => {
                    eprintln!("--invite needs a code\n{ONBOARD_USAGE}");
                    return 2;
                }
            },
            "--as" => match args.get(i + 1) {
                Some(n) => {
                    as_name = Some(n.clone());
                    i += 2;
                }
                None => {
                    eprintln!("--as needs a name\n{ONBOARD_USAGE}");
                    return 2;
                }
            },
            other => {
                eprintln!("unexpected argument '{other}'\n{ONBOARD_USAGE}");
                return 2;
            }
        }
    }

    // 1. Ensure a local identity key exists (created locally if `--as <name>` is
    //    given and none exists — NO on-chain claim).
    if let Err(code) = ensure_identity_key(as_name.as_deref()) {
        return code;
    }
    // Resolve the signer + address; `load_signer` errors cleanly (asking for
    // `--as <name>`) when there's no key or several to pick from.
    let signer = match load_signer(as_name.as_deref()) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));
    println!("identity: {addr}");

    // 2. Fund it. Invites are the supply-neutral, sybil-safe path: an operator or
    //    parent agent escrows the gift via `invite create`, and accepting it pays
    //    that `$LH` into this keypair's wallet (relay-sponsored gas — zero funds
    //    needed to accept). No `--invite` → show the funding options, never invent value.
    let Some(code) = invite_code else {
        eprintln!();
        eprintln!("this identity holds no $LH yet. to get your first $LH:");
        eprintln!("  - accept an invite from an operator/parent agent:");
        eprintln!("      localharness onboard --invite <code>   (alias: invite accept <code>)");
        eprintln!("  - or fund it yourself with a card:  localharness buy");
        eprintln!("  - or have a funded agent send to you:  localharness send {addr} 2");
        return 1;
    };
    let rc = invite_accept(as_name.as_deref(), &code).await;
    if rc != 0 {
        return rc;
    }

    // 3. Report the funded balance + the next step.
    let bal = registry::token_balance_of(&addr).await.unwrap_or(0);
    if bal > 0 {
        println!("✓ onboarded — wallet balance: {}", fmt_lh(bal));
    } else {
        println!("✓ onboarded — your first $LH is in your wallet");
    }
    println!("  next: claim a subdomain identity with `localharness create <name>`");
    println!("        (a name costs ~1 $LH from this balance; gas stays sponsored)");
    0
}

/// Ensure a local identity key FILE exists for `--as <name>`, GENERATING +
/// persisting a fresh one (config home, perms-locked) if absent — the same secure
/// key write as `create`, but with NO on-chain claim. A no-op when the key already
/// exists or no name is given (then [`load_signer`] resolves the sole key, or
/// errors asking for `--as`). Returns a process exit code on a hard failure.
fn ensure_identity_key(as_name: Option<&str>) -> Result<(), i32> {
    let Some(name) = as_name else { return Ok(()) };
    if !name_is_valid(name) {
        eprintln!("invalid name '{name}' — use 1-63 chars of a-z, 0-9, hyphen");
        return Err(2);
    }
    if resolve_key_read_path(name).is_some() {
        return Ok(()); // reuse the existing key — the key IS the identity, never overwrite it.
    }
    let agent = wallet::generate();
    let key_file = key_write_path(name);
    // Persist before anything else so the key is never lost.
    if let Err(e) = std::fs::write(&key_file, format!("{}\n", agent.private_key_hex)) {
        eprintln!("could not persist key to {key_file}: {e}");
        return Err(1);
    }
    secure_key_file(&key_file); // 0600 (unix) + keep a cwd-fallback key out of git.
    println!("created a new identity key for '{name}' ({key_file})");
    Ok(())
}
