//! localharnesslite — read-only `lh-*` platform commands for bashlite.
//!
//! These let a bashlite script READ the platform (identity, wallet + meter $LH
//! balances, name resolution, advertised price, owned agents) as plain commands
//! — `lh-whoami`, `lh-balance`, `lh-meter`, `lh-resolve`, `lh-price`, `lh-list`,
//! `lh-discover`, `lh-bounties` — so an agent's intent is a
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
        "lh-price" => Some(lh_price(args).await),
        "lh-list" => Some(lh_list(args, identity).await),
        "lh-discover" => Some(lh_discover(args).await),
        "lh-bounties" => Some(lh_bounties(args).await),
        // Not an lh-* command we own — let the host fall back to fs builtins.
        _ => None,
    }
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

/// What a value-moving command needs from its host: the caller's identity key,
/// the `fee_payer` sponsor, and the chain fee token.
pub struct WriteEnv<'a> {
    pub signer: &'a SigningKey,
    pub sponsor: &'a SigningKey,
    pub fee_token: &'a str,
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
        _ => None,
    }
}

/// Whether `cmd` is a value-MOVING command (handled by [`dispatch_write`], gated
/// by the dry-run-manifest confirm flow) — so a host can require an identity +
/// route it through the gate before the read/fs fallbacks.
pub fn is_write_command(cmd: &str) -> bool {
    matches!(cmd, "lh-send")
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
    match crate::registry::transfer_lh_sponsored(
        env.signer,
        env.sponsor,
        &to_hex,
        amount_wei,
        env.fee_token,
    )
    .await
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
            sponsor: &k.signer,
            fee_token: "0x20c0000000000000000000000000000000000001",
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
