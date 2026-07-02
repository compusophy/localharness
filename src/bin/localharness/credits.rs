use crate::{bytes_to_hex_str, fmt_lh, load_signer, registry, wallet};

/// `localharness credits [--as <me>]` — show the caller's billing state: wallet
/// `$LH`, the per-request meter (`creditOf`, what per-call billing debits), and
/// any session window. Read-only; these are the exact numbers the proxy gates on.
pub(crate) async fn credits_show(caller_name: Option<&str>) -> i32 {
    let signer = match load_signer(caller_name) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));
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
            "  session  active ~{}min left (proxy access without per-call metering)",
            (expiry - now) / 60
        );
    } else {
        println!("  session  none  (open one with `localharness session`, or just `topup` for per-call billing)");
    }
    0
}

/// `localharness topup [--as <me>]` — fund the caller for PER-CALL billing:
/// deposit the whole wallet `$LH` balance into the per-request meter, so the
/// proxy debits real `$LH` each `call`. (Also attempts the daily allowance, but
/// that's DISABLED on-chain, so a wallet with 0 `$LH` must be funded first via
/// `redeem` / `send`.) Sponsored — needs no gas. The end-to-end billing
/// self-test: `topup` -> `call` -> `credits` (watch the meter drop).
/// `localharness redeem <code>` — redeem a code for `$LH` straight into the
/// caller's WALLET (sponsored). Redeem codes are the controlled funding path
/// now that the daily allowance is disabled (it was a sybil risk: free accounts
/// × free daily mint = infinite credits). A redeemed wallet can `topup` (deposit
/// to the per-request meter), pay agents via `mcp-call` / x402, or `send_lh` to
/// fund another agent (same effect as a code).
pub(crate) async fn redeem(caller_name: Option<&str>, code: &str) -> i32 {
    let code = code.trim();
    if code.is_empty() {
        eprintln!("redeem: empty code");
        return 2;
    }
    let signer = match load_signer(caller_name) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    // Redeem is a TOP-UP for EXISTING identities (the on-chain RedeemFacet guard
    // rejects a caller with no registered name). Pre-check so a fresh address
    // gets an actionable message instead of a raw `NoIdentity` revert. Gate on
    // the SAME predicate as the contract — the ERC-721 NAME count
    // (`name_balance_of`), NOT `main_of` (which can be 0 for a real name-holder
    // whose MAIN-set tx failed or who released their MAIN). Block only on a
    // CONFIRMED zero; on an RPC error fall through and let the contract decide.
    let addr = bytes_to_hex_str(&wallet::address(&signer));
    if let Ok(0) = registry::name_balance_of(&addr).await {
        eprintln!("redeem tops up an EXISTING identity, but {addr} owns no subdomain yet.");
        eprintln!("claim one first (`localharness create <name>` — costs 1 $LH; fund via `localharness buy 2` or an invite), then redeem to top it up.");
        return 2;
    }
    match registry::redeem_sponsored(&signer, code).await {
        Ok(tx) => {
            println!("redeemed — $LH minted to your wallet  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("redeem failed: {e}");
            1
        }
    }
}

/// `localharness send <recipient> <amount>` — transfer `$LH` from your wallet to
/// a `0x…` address or a subdomain name's on-chain OWNER (sponsored). The CLI twin
/// of the browser `send_lh` tool — fund another agent (the same effect as a
/// redeem code; "one agent sends another `$LH`").
pub(crate) async fn send_lh(caller_name: Option<&str>, recipient: &str, amount: &str) -> i32 {
    use localharness::encoding::{classify_recipient, Recipient};
    let to_hex = match classify_recipient(recipient) {
        Ok(Recipient::Address(a)) => a,
        Ok(Recipient::Name(n)) => match registry::owner_of_name(&n).await {
            Ok(Some(o)) => o,
            Ok(None) => {
                eprintln!("send: '{n}' is not registered");
                return 1;
            }
            Err(e) => {
                eprintln!("send: RPC error resolving '{n}': {e}");
                return 1;
            }
        },
        Err(e) => {
            eprintln!("send: {e}");
            return 2;
        }
    };
    let amount_wei = match localharness::encoding::parse_token_amount(amount) {
        Some(w) if w > 0 => w,
        _ => {
            eprintln!("send: invalid amount '{amount}' (expected a positive number of $LH)");
            return 2;
        }
    };
    let signer = match load_signer(caller_name) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    match registry::transfer_lh_sponsored(&signer, &to_hex, amount_wei)
    .await
    {
        Ok(tx) => {
            println!("sent {amount} $LH to {to_hex}  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("send failed: {e}");
            1
        }
    }
}

/// `localharness session` — open a time-boxed proxy session by spending
/// `sessionPrice()` `$LH` (sponsored gas). Grants `sessionDuration()` of proxy
/// access without per-request metering. Needs `$LH` in your WALLET (redeem a code
/// or receive `send`).
pub(crate) async fn open_session(caller_name: Option<&str>) -> i32 {
    let signer = match load_signer(caller_name) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    match registry::open_session_sponsored(&signer).await {
        Ok(tx) => {
            println!("session opened  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("open session failed: {e}");
            1
        }
    }
}

pub(crate) const TOPUP_USAGE: &str = "\
usage: localharness topup [--as <me>] [<amount>|--all]
  topup <amount>   deposit that much wallet $LH into the per-call meter (e.g. 0.5)
  topup --all      deposit the ENTIRE wallet balance
  topup            show the wallet balance and what --all would move (deposits nothing)";

/// Parsed `topup` arguments. Pure (no I/O) so it is unit-testable — and so
/// `--help` short-circuits BEFORE identity resolution (the old code resolved
/// the caller key first, so `topup --help` with several local keys died with
/// "multiple identities — pick one with --as" instead of printing usage).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TopupArgs {
    /// `--help` / `-h` — print usage, touch nothing.
    Help,
    /// Explicit positional amount to deposit, in wei (parsed like `send`).
    Amount(u128),
    /// `--all` — deposit the entire wallet balance (the explicit opt-in).
    All,
    /// No amount and no `--all` — print the balances, deposit NOTHING.
    Inspect,
}

pub(crate) fn parse_topup_args(rest: &[String]) -> Result<TopupArgs, String> {
    let mut all = false;
    let mut amount: Option<u128> = None;
    for arg in rest {
        match arg.as_str() {
            "--help" | "-h" => return Ok(TopupArgs::Help),
            "--all" => all = true,
            raw => {
                if amount.is_some() {
                    return Err(TOPUP_USAGE.to_string());
                }
                match localharness::encoding::parse_token_amount(raw) {
                    Some(w) if w > 0 => amount = Some(w),
                    _ => {
                        return Err(format!(
                            "topup: invalid amount '{raw}' (expected a positive number of $LH)\n{TOPUP_USAGE}"
                        ))
                    }
                }
            }
        }
    }
    match (all, amount) {
        (true, Some(_)) => Err(format!("topup: pass an amount OR --all, not both\n{TOPUP_USAGE}")),
        (true, None) => Ok(TopupArgs::All),
        (false, Some(w)) => Ok(TopupArgs::Amount(w)),
        (false, None) => Ok(TopupArgs::Inspect),
    }
}

/// `localharness topup [--as <me>] [<amount>|--all]` — fund the caller for
/// PER-CALL billing by depositing wallet `$LH` into the per-request meter
/// (the pot the proxy debits each `call`). An explicit `<amount>` (decimal
/// `$LH`, parsed like `send`) moves exactly that much; `--all` moves the
/// whole wallet. With NEITHER it deposits NOTHING — it prints the wallet
/// balance and what `--all` would move. (Sweeping the entire wallet by
/// default cost real users real `$LH`; the full sweep is now an explicit
/// opt-in.) Sponsored — needs no gas. (Also attempts the daily allowance,
/// but that's DISABLED on-chain, so a 0-`$LH` wallet must be funded first
/// via `redeem` / `send`.)
pub(crate) async fn topup(caller_name: Option<&str>, parsed: TopupArgs) -> i32 {
    // `--help` short-circuits BEFORE any identity resolution.
    if parsed == TopupArgs::Help {
        println!("{TOPUP_USAGE}");
        return 0;
    }
    let signer = match load_signer(caller_name) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));
    // 1. Claim the daily allowance (mints $LH) if eligible. The allowance is
    //    DISABLED on-chain (dailyAllowance=0 — a sybil risk), so this is a
    //    no-op in practice; the dormant path stays in case it's re-enabled.
    if registry::can_claim_credits(&addr).await.unwrap_or(false) {
        match registry::claim_daily_sponsored(&signer).await {
            Ok(tx) => println!("claimed daily $LH  tx: {tx}"),
            Err(e) => eprintln!("claim failed (continuing to deposit): {e}"),
        }
    }
    // 2. Resolve what to deposit into the per-request meter.
    let bal = registry::token_balance_of(&addr).await.unwrap_or(0);
    if bal == 0 {
        println!("wallet has 0 $LH — nothing to deposit.");
        println!("fund it first: `localharness redeem <code>`, or have another agent `send` you $LH.");
        return 0;
    }
    let deposit_wei = match parsed {
        TopupArgs::Help => return 0, // handled above
        TopupArgs::All => bal,
        TopupArgs::Amount(w) if w > bal => {
            eprintln!(
                "topup: wallet holds only {} — can't deposit {}",
                fmt_lh(bal),
                fmt_lh(w)
            );
            return 1;
        }
        TopupArgs::Amount(w) => w,
        TopupArgs::Inspect => {
            // No amount, no --all: deposit NOTHING. The old default moved the
            // ENTIRE wallet with no confirmation (a real user lost 5 $LH);
            // the full sweep now requires the explicit --all.
            println!("wallet holds {} — nothing was deposited.", fmt_lh(bal));
            println!("  localharness topup <amount>   deposit that much into the per-call meter");
            println!("  localharness topup --all      deposit the entire {}", fmt_lh(bal));
            return 2;
        }
    };
    match registry::deposit_credits_sponsored(&signer, deposit_wei)
    .await
    {
        Ok(tx) => {
            println!("deposited {} into the meter  tx: {tx}", fmt_lh(deposit_wei));
            0
        }
        Err(e) => {
            eprintln!("deposit failed: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    #[test]
    fn parse_topup_args_amount_all_and_inspect() {
        // Bare topup = inspect-only (the old behavior swept the WHOLE wallet
        // into the meter with no confirmation — a real user lost 5 $LH).
        assert_eq!(parse_topup_args(&args(&[])), Ok(TopupArgs::Inspect));
        // The full sweep is the explicit --all opt-in.
        assert_eq!(parse_topup_args(&args(&["--all"])), Ok(TopupArgs::All));
        // A positional amount parses like `send` (decimal $LH → wei).
        assert_eq!(
            parse_topup_args(&args(&["0.5"])),
            Ok(TopupArgs::Amount(500_000_000_000_000_000))
        );
        assert_eq!(
            parse_topup_args(&args(&["2"])),
            Ok(TopupArgs::Amount(2_000_000_000_000_000_000))
        );
        // Conflicts and junk are rejected, never a tx.
        assert!(parse_topup_args(&args(&["0.5", "--all"])).is_err()); // both
        assert!(parse_topup_args(&args(&["--all", "0.5"])).is_err()); // both, either order
        assert!(parse_topup_args(&args(&["0"])).is_err()); // zero
        assert!(parse_topup_args(&args(&["nope"])).is_err()); // non-numeric
        assert!(parse_topup_args(&args(&["-1"])).is_err()); // negative / unknown flag
        assert!(parse_topup_args(&args(&["1", "2"])).is_err()); // two amounts
    }

    #[test]
    fn parse_topup_args_help_short_circuits_before_identity() {
        // `topup --help` used to die with "multiple identities — pick one with
        // --as" because the caller key was resolved BEFORE the args were read.
        // Help must parse purely (no identity, no RPC) wherever it appears.
        assert_eq!(parse_topup_args(&args(&["--help"])), Ok(TopupArgs::Help));
        assert_eq!(parse_topup_args(&args(&["-h"])), Ok(TopupArgs::Help));
        assert_eq!(parse_topup_args(&args(&["--all", "--help"])), Ok(TopupArgs::Help));
    }
}

