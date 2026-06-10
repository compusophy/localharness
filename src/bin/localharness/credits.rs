#[allow(unused_imports)]
use crate::*;

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
    let (signer, sponsor) = match load_signer_and_sponsor(caller_name) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    match registry::redeem_sponsored(&signer, &sponsor, code, registry::ALPHA_USD_ADDRESS).await {
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
    let (signer, sponsor) = match load_signer_and_sponsor(caller_name) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    match registry::transfer_lh_sponsored(
        &signer,
        &sponsor,
        &to_hex,
        amount_wei,
        registry::ALPHA_USD_ADDRESS,
    )
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
    let (signer, sponsor) = match load_signer_and_sponsor(caller_name) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    match registry::open_session_sponsored(&signer, &sponsor, registry::ALPHA_USD_ADDRESS).await {
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

pub(crate) async fn topup(caller_name: Option<&str>) -> i32 {
    let (signer, sponsor) = match load_signer_and_sponsor(caller_name) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));
    // 1. Claim the daily allowance (mints $LH) if eligible. The allowance is
    //    DISABLED on-chain (dailyAllowance=0 — a sybil risk), so this is a
    //    no-op in practice; the dormant path stays in case it's re-enabled.
    if registry::can_claim_credits(&addr).await.unwrap_or(false) {
        match registry::claim_daily_sponsored(&signer, &sponsor, registry::ALPHA_USD_ADDRESS).await {
            Ok(tx) => println!("claimed daily $LH  tx: {tx}"),
            Err(e) => eprintln!("claim failed (continuing to deposit): {e}"),
        }
    }
    // 2. Deposit the wallet balance into the per-request meter.
    let bal = registry::token_balance_of(&addr).await.unwrap_or(0);
    if bal == 0 {
        println!("wallet has 0 $LH — nothing to deposit.");
        println!("fund it first: `localharness redeem <code>`, or have another agent `send` you $LH.");
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

