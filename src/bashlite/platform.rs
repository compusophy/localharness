//! localharnesslite — read-only `lh-*` platform commands for bashlite.
//!
//! These let a bashlite script READ the platform (identity, $LH balances, name
//! resolution, advertised price) as plain commands, so an agent's intent is a
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
        "lh-resolve" => Some(lh_resolve(args).await),
        "lh-price" => Some(lh_price(args).await),
        // Not an lh-* command we own — let the host fall back to fs builtins.
        _ => None,
    }
}

/// `lh-balance [name|0xaddr]` — the `$LH` balance of an address, a name's OWNER,
/// or (no arg) the caller's own identity.
async fn lh_balance(args: &[String], identity: Option<&str>) -> Output {
    let target = match args.first() {
        Some(a) if a.starts_with("0x") => a.clone(),
        Some(name) => match crate::registry::owner_of_name(name).await {
            Ok(Some(owner)) => owner,
            Ok(None) => return Output::err(format!("lh-balance: {name}: not registered"), 1),
            Err(e) => return Output::err(format!("lh-balance: {e}"), 1),
        },
        None => match identity {
            Some(a) => a.to_string(),
            None => return Output::err("lh-balance: no identity — pass a name or 0x address", 2),
        },
    };
    match crate::registry::token_balance_of(&target).await {
        Ok(wei) => Output::ok(format!("{}\n", fmt_lh(wei))),
        Err(e) => Output::err(format!("lh-balance: {e}"), 1),
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

    #[test]
    fn fmt_lh_trims() {
        assert_eq!(fmt_lh(0), "0");
        assert_eq!(fmt_lh(2_000_000_000_000_000_000), "2");
        assert_eq!(fmt_lh(1_500_000_000_000_000_000), "1.5");
        assert_eq!(fmt_lh(10_000_000_000_000_000), "0.01");
    }
}
