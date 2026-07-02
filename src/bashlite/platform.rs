//! localharnesslite — read-only `lh-*` platform commands for bashlite.
//!
//! These let a bashlite script READ the platform (identity, wallet + meter $LH
//! balances, name resolution, advertised price, owned agents) as plain commands
//! — `lh-whoami`, `lh-balance`, `lh-meter`, `lh-resolve`, `lh-tba`, `lh-price`,
//! `lh-list`, `lh-discover`, `lh-bounties`, `lh-help` — so an agent's intent is a
//! PROGRAM over the platform — not a stutter of tool round-trips, and not an
//! agent loop at all. A surface (CLI / browser) wires [`dispatch`] into its
//! `BashHost::run_builtin` override, falling back to the fs builtins for
//! everything else.
//!
//! READ-ONLY by design: every command here is a registry view (no signer, no
//! gas, no confirm). Value-MOVING `lh-*` (`lh-send`, `lh-create`, …) are v2 and
//! go behind the dry-run-manifest confirm gate (`design/bashlite.md`). Like the
//! fs builtins, these are TOTAL: bad args / RPC failures return a nonzero
//! [`Output`], never a panic.
//!
//! Feature-gated on `wallet` (the `registry` layer); the bashlite core itself
//! stays dependency-free.

use super::Output;

/// Dispatch a read-only `lh-*` command. Returns `None` when `cmd` is not an
/// `lh-` command this core owns (so the host falls through to the fs builtins);
/// `Some(output)` — possibly a nonzero error output — when it is.
///
/// `identity` is the caller's `0x` address (the host's logged-in agent), used as
/// the default subject for `lh-whoami` / `lh-balance`.
pub async fn dispatch(cmd: &str, args: &[String], identity: Option<&str>) -> Option<Output> {
    match cmd {
        "lh-whoami" => Some(match identity {
            Some(a) => Output::ok(format!("{a}\n")),
            None => Output::err("lh-whoami: no identity on this host", 1),
        }),
        "lh-balance" => Some(lh_balance(args, identity).await),
        "lh-meter" => Some(lh_meter(args, identity).await),
        "lh-resolve" => Some(lh_resolve(args).await),
        "lh-tba" => Some(lh_tba(args).await),
        "lh-price" => Some(lh_price(args).await),
        "lh-list" => Some(lh_list(args, identity).await),
        "lh-list-mine" => Some(lh_list_mine(identity).await),
        "lh-discover" => Some(lh_discover(args).await),
        "lh-bounties" => Some(lh_bounties(args).await),
        "lh-help" => Some(lh_help()),
        // Not an lh-* command we own — let the host fall back to fs builtins.
        _ => None,
    }
}

/// `lh-help` — list every localharnesslite command, one per line, so an agent
/// dropped into a bashlite shell can DISCOVER the platform surface without
/// leaving it. Pure, static, read-only; the text doubles as the spec. Keep it in
/// sync as `lh-*` commands are added.
fn lh_help() -> Output {
    Output::ok(
        "localharnesslite — platform commands for bashlite\n\
         \n\
         reads (no signer, no gas):\n\
         \x20 lh-whoami                 this host's identity address\n\
         \x20 lh-balance [name|0xaddr]  wallet $LH balance (default: self)\n\
         \x20 lh-meter   [name|0xaddr]  metered $LH balance (default: self)\n\
         \x20 lh-resolve <name>         name -> owner address\n\
         \x20 lh-tba     <name>         name -> token-bound account (payment target)\n\
         \x20 lh-price   <name>         agent's advertised per-call $LH price\n\
         \x20 lh-list    [name|0xaddr]  agents owned (default: self)\n\
         \x20 lh-list-mine               YOUR owned subdomains, one per line\n\
         \x20 lh-discover <query...>    find agents by relevance\n\
         \x20 lh-bounties [query...]    open paid work\n\
         \x20 lh-help                   this list\n\
         \n\
         writes (confirm-gated):\n\
         \x20 lh-send <name|0xaddr> <amount>   transfer $LH\n\
         \x20 lh-publish <name> <source.rl>    compile + publish/update an owned app\n",
    )
}

/// `lh-bounties [query…]` — open bounties (paid work) one per line:
/// `#<id> <reward> $LH: <task>`. No query lists ALL open bounties; a query ranks
/// by task relevance. The counterpart to `lh-discover` (find agents) — find WORK;
/// claim it with the CLI `bounty claim`.
async fn lh_bounties(args: &[String]) -> Output {
    const SCAN: u64 = 100;
    let query = args.join(" "); // empty = all open bounties (newest-first)
    match crate::registry::discover_bounties(&query, SCAN).await {
        Ok(bounties) => {
            let mut out = String::new();
            for (id, task, reward) in bounties {
                // One bounty = one line (so `wc -l` counts them); flatten the task.
                let task = task.replace(['\n', '\r'], " ");
                out.push_str(&format!("#{id} {} $LH: {}\n", fmt_lh(reward), task.trim()));
            }
            Output::ok(out)
        }
        Err(e) => Output::err(format!("lh-bounties: {e}"), 1),
    }
}

/// `lh-discover <query…>` — find agents by capability (the Agent Yellow Pages),
/// ONE name per line so `for a in $(lh-discover coding); do …; done` fans out
/// over them. The query may be several words (ORed). Empty output = no matches.
async fn lh_discover(args: &[String]) -> Output {
    if args.is_empty() {
        return Output::err("lh-discover: usage: lh-discover <query…>", 2);
    }
    // Scan the most recent agents; matches the CLI `discover` default.
    const SCAN: u64 = 100;
    let query = args.join(" ");
    match crate::registry::discover_agents(&query, SCAN).await {
        Ok(matches) => {
            let mut out = String::new();
            for (name, _persona) in matches {
                out.push_str(&name);
                out.push('\n');
            }
            Output::ok(out)
        }
        Err(e) => Output::err(format!("lh-discover: {e}"), 1),
    }
}

/// Resolve the SUBJECT address of a read command: a `0x…` address verbatim, a
/// name's OWNER, or (no arg) the caller's identity. `Err(output)` is a ready
/// nonzero result. Shared by the address-keyed reads (balance/meter/list).
async fn subject_address(
    args: &[String],
    identity: Option<&str>,
    cmd: &str,
) -> Result<String, Output> {
    match args.first() {
        Some(a) if a.starts_with("0x") => Ok(a.clone()),
        Some(name) => match crate::registry::owner_of_name(name).await {
            Ok(Some(owner)) => Ok(owner),
            Ok(None) => Err(Output::err(format!("{cmd}: {name}: not registered"), 1)),
            Err(e) => Err(Output::err(format!("{cmd}: {e}"), 1)),
        },
        None => match identity {
            Some(a) => Ok(a.to_string()),
            None => Err(Output::err(format!("{cmd}: no identity — pass a name or 0x address"), 2)),
        },
    }
}

/// `lh-balance [name|0xaddr]` — the WALLET `$LH` balance of an address, a name's
/// OWNER, or (no arg) the caller's own identity.
async fn lh_balance(args: &[String], identity: Option<&str>) -> Output {
    let target = match subject_address(args, identity, "lh-balance").await {
        Ok(a) => a,
        Err(out) => return out,
    };
    match crate::registry::token_balance_of(&target).await {
        Ok(wei) => Output::ok(format!("{}\n", fmt_lh(wei))),
        Err(e) => Output::err(format!("lh-balance: {e}"), 1),
    }
}

/// `lh-meter [name|0xaddr]` — the per-call METER `$LH` balance (what the proxy
/// debits per request), distinct from the spendable wallet balance.
async fn lh_meter(args: &[String], identity: Option<&str>) -> Output {
    let target = match subject_address(args, identity, "lh-meter").await {
        Ok(a) => a,
        Err(out) => return out,
    };
    match crate::registry::credit_balance_of(&target).await {
        Ok(wei) => Output::ok(format!("{}\n", fmt_lh(wei))),
        Err(e) => Output::err(format!("lh-meter: {e}"), 1),
    }
}

/// `lh-list [name|0xaddr]` — the agent names an identity owns, ONE per line (so
/// `for a in $(lh-list); do …; done` fans out over them). Empty output = none.
async fn lh_list(args: &[String], identity: Option<&str>) -> Output {
    let target = match subject_address(args, identity, "lh-list").await {
        Ok(a) => a,
        Err(out) => return out,
    };
    match crate::registry::list_owned_tokens(&target).await {
        Ok(tokens) => {
            let mut out = String::new();
            for t in tokens {
                out.push_str(&t.name);
                out.push('\n');
            }
            Output::ok(out)
        }
        Err(e) => Output::err(format!("lh-list: {e}"), 1),
    }
}

/// `lh-list-mine` — the CALLER's own owned subdomain names, ONE per line. The
/// no-argument, identity-pinned form of `lh-list` (it ignores args so it can
/// never accidentally list someone else's holdings), purpose-built for the
/// fan-out `for s in $(lh-list-mine); do lh-publish $s app.rl; done`. Needs an
/// identity on the host (run with `--as <name>`); no identity = a usage error.
async fn lh_list_mine(identity: Option<&str>) -> Output {
    let Some(addr) = identity else {
        return Output::err("lh-list-mine: no identity on this host — run with --as <name>", 2);
    };
    match crate::registry::list_owned_tokens(addr).await {
        Ok(tokens) => {
            let mut out = String::new();
            for t in tokens {
                out.push_str(&t.name);
                out.push('\n');
            }
            Output::ok(out)
        }
        Err(e) => Output::err(format!("lh-list-mine: {e}"), 1),
    }
}

/// `lh-resolve <name>` — the token id, owner, and TBA of a registered name.
async fn lh_resolve(args: &[String]) -> Output {
    let Some(name) = args.first() else {
        return Output::err("lh-resolve: usage: lh-resolve <name>", 2);
    };
    let id = match crate::registry::id_of_name(name).await {
        Ok(0) => return Output::err(format!("lh-resolve: {name}: not registered"), 1),
        Ok(id) => id,
        Err(e) => return Output::err(format!("lh-resolve: {e}"), 1),
    };
    let owner = crate::registry::owner_of_name(name).await.ok().flatten().unwrap_or_default();
    let tba = crate::registry::tba_of_name(name).await.ok().flatten().unwrap_or_default();
    Output::ok(format!("token_id {id}\nowner {owner}\ntba {tba}\n"))
}

/// `lh-tba <name>` — JUST the agent's token-bound account address, one line, no
/// label, so it COMPOSES: `lh-send $(lh-tba alice) 5` funds alice's treasury
/// (x402 / bounty payments settle to the TBA, not the owner). `lh-resolve` prints
/// the same TBA among other fields for humans; this is the pipeable form, like
/// `lh-whoami` for an identity. Errors distinguish unregistered from
/// not-yet-deployed.
async fn lh_tba(args: &[String]) -> Output {
    let Some(name) = args.first() else {
        return Output::err("lh-tba: usage: lh-tba <name>", 2);
    };
    match crate::registry::id_of_name(name).await {
        Ok(0) => return Output::err(format!("lh-tba: {name}: not registered"), 1),
        Ok(_) => {}
        Err(e) => return Output::err(format!("lh-tba: {e}"), 1),
    }
    match crate::registry::tba_of_name(name).await {
        Ok(Some(tba)) => Output::ok(format!("{tba}\n")),
        Ok(None) => Output::err(format!("lh-tba: {name}: no token-bound account deployed yet"), 1),
        Err(e) => Output::err(format!("lh-tba: {e}"), 1),
    }
}

/// `lh-price <name>` — the agent's advertised per-call `$LH` price.
async fn lh_price(args: &[String]) -> Output {
    let Some(name) = args.first() else {
        return Output::err("lh-price: usage: lh-price <name>", 2);
    };
    let id = match crate::registry::id_of_name(name).await {
        Ok(0) => return Output::err(format!("lh-price: {name}: not registered"), 1),
        Ok(id) => id,
        Err(e) => return Output::err(format!("lh-price: {e}"), 1),
    };
    match crate::registry::x402_ask_price_of(id).await {
        Ok(wei) => Output::ok(format!("{} $LH\n", fmt_lh(wei))),
        Err(e) => Output::err(format!("lh-price: {e}"), 1),
    }
}

// --- value-MOVING lh-* (writes) + the dry-run manifest ----------------------
//
// Writes are NOT dispatched by the read [`dispatch`] above — they need a signer,
// a fee_payer, and the dry-run-manifest confirm flow (`design/bashlite.md`). A
// host that holds an identity calls [`dispatch_write`]: in DRY-RUN the command
// records a one-line plan and moves nothing; the host collects every plan into a
// manifest, confirms the WHOLE manifest once, then re-runs LIVE. Same total
// contract — bad args / RPC errors are a nonzero [`Output`], never a panic.

use k256::ecdsa::SigningKey;

/// What a value-moving command needs from its host: the caller's identity key.
/// (The `fee_payer` sponsor + fee token are resolved inside `registry::` now —
/// testnet key / mainnet relay + the active chain's fee_token.)
pub struct WriteEnv<'a> {
    pub signer: &'a SigningKey,
}

/// Dispatch a VALUE-MOVING `lh-*` command. Returns `None` when `cmd` is not a
/// value-moving command this core owns. `Some((output, plan))`: `plan` is a
/// one-line manifest description when the command WOULD move value (empty when
/// it failed before committing to a move, e.g. bad args). In `dry_run` the
/// `output` is the plan and NOTHING is sent; live, the `output` carries the
/// result (tx hash).
pub async fn dispatch_write(
    cmd: &str,
    args: &[String],
    env: &WriteEnv<'_>,
    dry_run: bool,
) -> Option<(Output, String)> {
    match cmd {
        "lh-send" => Some(lh_send(args, env, dry_run).await),
        // `lh-publish <name> <source>` — the host has ALREADY read the source
        // FILE and replaced args[1] with its CONTENT (the publish core is
        // dependency-free and fs-agnostic; the host owns the sandbox fs).
        "lh-publish" => Some(lh_publish(args, env, dry_run).await),
        _ => None,
    }
}

/// Whether `cmd` is a value-MOVING / state-CHANGING command (handled by
/// [`dispatch_write`], gated by the dry-run-manifest confirm flow) — so a host
/// can require an identity + route it through the gate before the read/fs
/// fallbacks.
pub fn is_write_command(cmd: &str) -> bool {
    matches!(cmd, "lh-send" | "lh-publish")
}

/// Whether `cmd`'s SECOND argument is a SOURCE FILE PATH the host must read into
/// memory (and substitute) before [`dispatch_write`] — the publish core takes
/// source CONTENT, keeping it fs-agnostic + dependency-free. Today only
/// `lh-publish`.
pub fn write_reads_source_file(cmd: &str) -> bool {
    matches!(cmd, "lh-publish")
}

/// `lh-publish <name> <source>` — compile a rustlite cartridge and publish (or
/// UPDATE) it as `<name>`'s on-chain public face, for ANY subdomain the caller
/// OWNS. `source` is the cartridge SOURCE TEXT (the host reads the file). The
/// owner's seed holds every subdomain NFT, so it signs `setMetadata` for the
/// target's tokenId — no re-register, no actor model. Refuses unregistered
/// names and names owned by someone else. Sponsored (caller pays no gas).
/// Rides the dry-run-manifest gate: in `dry_run` it compiles + ownership-checks
/// and emits a one-line plan, writing NOTHING.
async fn lh_publish(args: &[String], env: &WriteEnv<'_>, dry_run: bool) -> (Output, String) {
    let none = String::new();
    let (Some(name), Some(source)) = (args.first(), args.get(1)) else {
        return (
            Output::err("lh-publish: usage: lh-publish <name> <source.rl>", 2),
            none,
        );
    };
    if source.trim().is_empty() {
        return (Output::err(format!("lh-publish: {name}: source is empty"), 2), none);
    }
    // Compile FIRST — a bad cartridge fails before any ownership read / write.
    let wasm = match crate::rustlite::compile(source) {
        Ok(w) => w,
        Err(e) => return (Output::err(format!("lh-publish: compile failed: {}", e.render(source)), 1), none),
    };
    let cap = crate::registry::APP_STORE_MAX_WASM_BYTES;
    if wasm.len() > cap {
        return (
            Output::err(
                format!("lh-publish: {name}: {} bytes exceeds the {cap}-byte cap", wasm.len()),
                1,
            ),
            none,
        );
    }
    // DRY-RUN: a compiled cartridge is enough to record the manifest plan —
    // emit it and write NOTHING (no RPC). The ownership gate + write run only
    // on the LIVE pass (matching the dry-run-manifest contract: dry collects,
    // confirm executes).
    let plan = format!("publish {} bytes -> {name} (app cartridge)", wasm.len());
    if dry_run {
        return (Output::ok(format!("[plan] {plan}\n")), plan);
    }
    // Ownership gate (LIVE only): the target must be registered AND owned by
    // THIS signer (the seed holding all the caller's names). Refuse otherwise.
    let signer_addr = crate::encoding::bytes_to_hex_str(&crate::wallet::address(env.signer));
    let owner = match crate::registry::owner_of_name(name).await {
        Ok(Some(o)) => o,
        Ok(None) => {
            return (
                Output::err(
                    format!("lh-publish: {name}: not registered — claim it first (localharness create {name})"),
                    1,
                ),
                plan,
            )
        }
        Err(e) => return (Output::err(format!("lh-publish: {e}"), 1), plan),
    };
    if !owner.eq_ignore_ascii_case(&signer_addr) {
        return (
            Output::err(
                format!("lh-publish: {name} is owned by {owner}, not you ({signer_addr})"),
                1,
            ),
            plan,
        );
    }
    // Publish OFF-CHAIN to the app store (free, no gas). The ownership gate above
    // already proved THIS signer (an EOA) owns the name — exactly what the proxy
    // re-checks server-side (ownerOf(name) == token signer). The blockchain keeps
    // only ownership; the cartridge bytes live in GitHub. No tokenId lookup needed
    // (the proxy resolves it).
    let token = crate::registry::proxy_auth_token(env.signer, crate::runtime::now_unix_secs(), "publish");
    match crate::registry::publish_app_to_store(name, &token, &wasm, source).await {
        Ok(()) => (
            Output::ok(format!(
                "published {name}.localharness.xyz (off-chain, no gas)\n"
            )),
            plan,
        ),
        Err(e) => (Output::err(format!("lh-publish: {e}"), 1), plan),
    }
}

/// `lh-send <name|0xaddr> <amount>` — transfer `$LH` to an address or a name's
/// owner (sponsored; the caller pays no gas).
async fn lh_send(args: &[String], env: &WriteEnv<'_>, dry_run: bool) -> (Output, String) {
    use crate::encoding::{classify_recipient, parse_token_amount, Recipient};
    let none = String::new();
    let (Some(recipient), Some(amount)) = (args.first(), args.get(1)) else {
        return (Output::err("lh-send: usage: lh-send <name|0xaddr> <amount>", 2), none);
    };
    let to_hex = match classify_recipient(recipient) {
        Ok(Recipient::Address(a)) => a,
        Ok(Recipient::Name(n)) => match crate::registry::owner_of_name(&n).await {
            Ok(Some(o)) => o,
            Ok(None) => return (Output::err(format!("lh-send: {n}: not registered"), 1), none),
            Err(e) => return (Output::err(format!("lh-send: {e}"), 1), none),
        },
        Err(e) => return (Output::err(format!("lh-send: {e}"), 2), none),
    };
    let amount_wei = match parse_token_amount(amount) {
        Some(w) if w > 0 => w,
        _ => return (Output::err(format!("lh-send: invalid amount '{amount}'"), 2), none),
    };
    let plan = format!("send {amount} $LH -> {recipient} ({to_hex})");
    if dry_run {
        return (Output::ok(format!("[plan] {plan}\n")), plan);
    }
    match crate::registry::transfer_lh_sponsored(env.signer, &to_hex, amount_wei).await
    {
        Ok(tx) => (Output::ok(format!("sent {amount} $LH -> {to_hex}  tx {tx}\n")), plan),
        Err(e) => (Output::err(format!("lh-send: {e}"), 1), plan),
    }
}

/// Format `$LH` wei (18-dec) as a trimmed decimal string: `1500000000000000000`
/// → `1.5`, `2000000000000000000` → `2`, `0` → `0`.
fn fmt_lh(wei: u128) -> String {
    const UNIT: u128 = 1_000_000_000_000_000_000;
    let whole = wei / UNIT;
    let frac = wei % UNIT;
    if frac == 0 {
        return whole.to_string();
    }
    let mut f = format!("{frac:018}");
    while f.ends_with('0') {
        f.pop();
    }
    format!("{whole}.{f}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dispatch_passes_non_lh_commands_through() {
        // Anything not `lh-*` → None, so the host falls back to fs builtins.
        assert!(dispatch("echo", &["hi".into()], None).await.is_none());
        assert!(dispatch("ls", &[], Some("0xabc")).await.is_none());
    }

    #[tokio::test]
    async fn help_lists_every_command_offline() {
        // Pure, no RPC, exit 0 — and it must mention EVERY lh-* command so the
        // listing can't silently drift as commands are added/renamed.
        let r = dispatch("lh-help", &[], None).await.unwrap();
        assert_eq!(r.code, 0);
        for cmd in [
            "lh-whoami", "lh-balance", "lh-meter", "lh-resolve", "lh-tba", "lh-price",
            "lh-list", "lh-list-mine", "lh-discover", "lh-bounties", "lh-help", "lh-send",
            "lh-publish",
        ] {
            assert!(r.stdout.contains(cmd), "lh-help is missing `{cmd}`");
        }
    }

    #[tokio::test]
    async fn whoami_prints_identity_or_errors() {
        let r = dispatch("lh-whoami", &[], Some("0xF00")).await.unwrap();
        assert_eq!(r.stdout, "0xF00\n");
        assert_eq!(r.code, 0);
        let r = dispatch("lh-whoami", &[], None).await.unwrap();
        assert_ne!(r.code, 0); // no identity → nonzero, not a panic
    }

    #[tokio::test]
    async fn balance_without_identity_or_arg_is_a_usage_error() {
        // No arg + no identity → a clean usage error BEFORE any RPC call.
        let r = dispatch("lh-balance", &[], None).await.unwrap();
        assert_eq!(r.code, 2);
        assert!(r.stderr.contains("identity"));
    }

    #[tokio::test]
    async fn resolve_and_price_require_a_name() {
        assert_eq!(dispatch("lh-resolve", &[], None).await.unwrap().code, 2);
        assert_eq!(dispatch("lh-price", &[], None).await.unwrap().code, 2);
        // lh-tba is dispatched and also a name-required usage error before any RPC.
        assert_eq!(dispatch("lh-tba", &[], None).await.unwrap().code, 2);
    }

    #[tokio::test]
    async fn discover_is_dispatched_and_requires_a_query() {
        // Owned by the dispatcher; no query → usage error (exit 2) before any RPC.
        let r = dispatch("lh-discover", &[], None).await;
        assert!(r.is_some());
        assert_eq!(r.unwrap().code, 2);
    }

    #[tokio::test]
    async fn meter_and_list_are_dispatched_and_subject_gated() {
        // The new reads are owned by the dispatcher (Some), and with no arg + no
        // identity they fail cleanly (usage exit 2) BEFORE any RPC call.
        for cmd in ["lh-meter", "lh-list"] {
            let r = dispatch(cmd, &[], None).await;
            assert!(r.is_some(), "{cmd} should be dispatched");
            let r = r.unwrap();
            assert_eq!(r.code, 2, "{cmd} no-arg+no-identity should be a usage error");
            assert!(r.stderr.contains("identity"), "{cmd}: {:?}", r.stderr);
        }
    }

    #[tokio::test]
    async fn list_mine_is_dispatched_and_identity_gated() {
        // Owned by the read dispatcher; no identity → usage error (exit 2)
        // BEFORE any RPC, and it IGNORES any args (always lists self).
        let r = dispatch("lh-list-mine", &[], None).await;
        assert!(r.is_some(), "lh-list-mine should be dispatched");
        let r = r.unwrap();
        assert_eq!(r.code, 2, "no-identity should be a usage error");
        assert!(r.stderr.contains("identity"), "{:?}", r.stderr);
        // Even with a stray arg, no identity is still the (pre-RPC) failure.
        let r = dispatch("lh-list-mine", &["someone".into()], None).await.unwrap();
        assert_eq!(r.code, 2);
    }

    #[tokio::test]
    async fn publish_is_a_write_command_and_reads_a_source_file() {
        assert!(is_write_command("lh-publish"));
        assert!(write_reads_source_file("lh-publish"));
        // lh-send moves value but takes no source FILE.
        assert!(is_write_command("lh-send"));
        assert!(!write_reads_source_file("lh-send"));
        // A read command is neither.
        assert!(!is_write_command("lh-list-mine"));
    }

    #[tokio::test]
    async fn publish_dry_run_plans_a_valid_cartridge_without_writing() {
        let k = crate::wallet::generate();
        let env = WriteEnv {
            signer: &k.signer,
        };
        // A minimal valid cartridge → dry-run compiles + plans, NOTHING sent
        // (the ownership/RPC checks come only on the LIVE pass).
        let src = "fn frame(t: i32) { host::display::present(); }".to_string();
        let (out, plan) =
            dispatch_write("lh-publish", &["mine".into(), src], &env, true).await.unwrap();
        assert_eq!(out.code, 0, "{:?}", out.stderr);
        assert!(out.stdout.contains("[plan] publish"), "{:?}", out.stdout);
        assert!(plan.contains("publish") && plan.contains("mine"), "{plan}");
    }

    #[tokio::test]
    async fn publish_rejects_bad_args_and_garbage_before_any_rpc() {
        let k = crate::wallet::generate();
        let env = WriteEnv {
            signer: &k.signer,
        };
        // Missing source → usage error, empty plan (no write recorded).
        let (out, plan) = dispatch_write("lh-publish", &["mine".into()], &env, true).await.unwrap();
        assert_eq!(out.code, 2);
        assert!(plan.is_empty());
        // Garbage source → compile failure (exit 1), empty plan, BEFORE any RPC.
        let (out, plan) = dispatch_write(
            "lh-publish",
            &["mine".into(), "this is not rustlite".into()],
            &env,
            true,
        )
        .await
        .unwrap();
        assert_eq!(out.code, 1);
        assert!(out.stderr.contains("compile failed"), "{:?}", out.stderr);
        assert!(plan.is_empty());
        // Not our command.
        assert!(dispatch_write("echo", &[], &env, true).await.is_none());
    }

    #[test]
    fn fmt_lh_trims() {
        assert_eq!(fmt_lh(0), "0");
        assert_eq!(fmt_lh(2_000_000_000_000_000_000), "2");
        assert_eq!(fmt_lh(1_500_000_000_000_000_000), "1.5");
        assert_eq!(fmt_lh(10_000_000_000_000_000), "0.01");
    }

    #[tokio::test]
    async fn dispatch_write_dry_run_plans_without_sending() {
        let k = crate::wallet::generate();
        let env = WriteEnv {
            signer: &k.signer,
        };
        // Non-value-moving commands are not ours.
        assert!(dispatch_write("echo", &[], &env, true).await.is_none());
        assert!(dispatch_write("lh-resolve", &["x".into()], &env, true).await.is_none());

        // lh-send to a 0x ADDRESS (no network) in dry-run → a plan, NOTHING sent.
        let addr = "0x00000000000000000000000000000000000000aa".to_string();
        let (out, plan) =
            dispatch_write("lh-send", &[addr.clone(), "2.5".into()], &env, true).await.unwrap();
        assert_eq!(out.code, 0);
        assert!(out.stdout.contains("[plan] send 2.5 $LH"), "{:?}", out.stdout);
        assert!(plan.contains("send 2.5 $LH"));

        // Bad args / amount → nonzero with an EMPTY plan (no value move recorded).
        let (out, plan) = dispatch_write("lh-send", &[], &env, true).await.unwrap();
        assert_eq!(out.code, 2);
        assert!(plan.is_empty());
        let (out, plan) =
            dispatch_write("lh-send", &[addr, "-5".into()], &env, true).await.unwrap();
        assert_eq!(out.code, 2);
        assert!(plan.is_empty());
    }
}
