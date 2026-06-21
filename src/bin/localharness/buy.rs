//! `localharness buy` / `join` — buy `$LH` with a card (the fiat on-ramp, CLI side).
//!
//! A headless command that creates a Stripe Checkout session via the credit
//! proxy and prints the hosted URL. The caller (or its operator) opens the link
//! in any browser and pays with a card or a Link-saved card — Stripe/Link holds
//! the card, we store nothing — and the proxy webhook mints the purchased `$LH`
//! on-chain to the caller's address (MintGateFacet, USD-backed). No card data
//! ever touches this CLI — PCI stays entirely with Stripe.
//!
//! `buy` with no amount (and the `join` alias) buys the $1 minimum — the
//! onboarding / sybil-guard entry amount.

use crate::{bytes_to_hex_str, fmt_lh, load_signer, registry, wallet};


/// Parse a USD amount ("5", "$5", "5.50") into integer cents. `None` on
/// empty / invalid / non-positive. Mirrors `app::events::credits::parse_usd_cents`.
pub(crate) fn parse_usd_cents(raw: &str) -> Option<u64> {
    let s = raw.trim().trim_start_matches('$').trim();
    if s.is_empty() {
        return None;
    }
    let dollars: f64 = s.parse().ok()?;
    if !dollars.is_finite() || dollars <= 0.0 {
        return None;
    }
    let cents = (dollars * 100.0).round();
    if cents < 1.0 {
        return None;
    }
    Some(cents as u64)
}

const BUY_USAGE: &str = "\
usage: localharness buy [--as <me>] [<usd>]
  buy            buy the $1 minimum of $LH (the onboarding amount)
  buy 5          buy $5 of $LH
  buy 2.50       buy $2.50 of $LH
Prints a Stripe Checkout link; pay with a card and the $LH is minted to your
wallet on-chain. `join` is an alias that buys the $1 entry amount.";

/// The proxy's MIN/MAX buy bounds (mirror `stripe-checkout.ts` so a bad amount
/// gets a clear client-side message instead of a 400 round-trip).
const MIN_USD_CENTS: u64 = 100; // $1
const MAX_USD_CENTS: u64 = 50_000; // $500

/// `localharness buy [--as <me>] [<usd>]` (and the `join` alias) — create a
/// Stripe Checkout session for `<usd>` (default $1, the onboarding amount) and
/// print the hosted URL to pay at.
pub(crate) async fn buy(caller_name: Option<&str>, rest: &[String]) -> i32 {
    // `--help` short-circuits before identity resolution.
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        println!("{BUY_USAGE}");
        return 0;
    }
    // At most one positional USD amount; bare `buy` / `join` = the $1 minimum.
    let positional: Vec<&String> = rest.iter().filter(|a| !a.starts_with('-')).collect();
    if positional.len() > 1 {
        eprintln!("{BUY_USAGE}");
        return 2;
    }
    let cents = match positional.first() {
        Some(raw) => match parse_usd_cents(raw) {
            Some(c) => c,
            None => {
                eprintln!(
                    "buy: invalid amount '{raw}' (expected dollars, e.g. 1 or 2.50)\n{BUY_USAGE}"
                );
                return 2;
            }
        },
        None => MIN_USD_CENTS,
    };
    if !(MIN_USD_CENTS..=MAX_USD_CENTS).contains(&cents) {
        eprintln!(
            "buy: amount must be between ${:.2} and ${:.2}",
            MIN_USD_CENTS as f64 / 100.0,
            MAX_USD_CENTS as f64 / 100.0
        );
        return 2;
    }

    let signer = match load_signer(caller_name) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Same `<address>:<ts>:<sig>` personal-sign token the gemini/notify proxy
    // routes use; the proxy binds lh_address from the RECOVERED signer, never a
    // client field, so the mint can only ever credit this caller.
    let token = registry::proxy_auth_token(&signer, now);
    let base = registry::CREDIT_PROXY_URL.trim_end_matches('/');
    let endpoint = format!("{base}/stripe/checkout");

    // Hosted (redirect) Checkout — a CLI/agent opens the URL in a browser. The
    // embedded path is the browser modal's; not useful from a terminal.
    let resp = match reqwest::Client::new()
        .post(&endpoint)
        .header("content-type", "application/json")
        .header("x-goog-api-key", token)
        .body(format!("{{\"usd_cents\":{cents}}}"))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("buy failed: proxy unreachable ({e})");
            return 1;
        }
    };
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    if !status.is_success() {
        let msg = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown proxy error");
        eprintln!("buy failed ({}): {msg}", status.as_u16());
        return 1;
    }
    let Some(url) = body.get("url").and_then(|v| v.as_str()) else {
        eprintln!("buy failed: proxy returned no checkout url");
        return 1;
    };
    let lh = body
        .get("lh_wei")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u128>().ok())
        .map(fmt_lh)
        .unwrap_or_else(|| "$LH".to_string());

    println!(
        "Pay ${:.2} to receive {lh} (minted to {addr}):",
        cents as f64 / 100.0
    );
    println!();
    println!("  {url}");
    println!();
    println!("Open the link in a browser and pay with a card (or a Link-saved card).");
    println!("The $LH lands in your wallet automatically — check with `localharness credits`.");
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    #[test]
    fn parse_usd_cents_accepts_dollars_and_rejects_junk() {
        assert_eq!(parse_usd_cents("1"), Some(100));
        assert_eq!(parse_usd_cents("$5"), Some(500));
        assert_eq!(parse_usd_cents("2.50"), Some(250));
        assert_eq!(parse_usd_cents(" $10.00 "), Some(1000));
        assert_eq!(parse_usd_cents("0"), None);
        assert_eq!(parse_usd_cents("-1"), None);
        assert_eq!(parse_usd_cents(""), None);
        assert_eq!(parse_usd_cents("nope"), None);
    }

    #[tokio::test]
    async fn buy_help_short_circuits_without_identity() {
        // `--help` must print usage and exit 0 without touching keys or network.
        assert_eq!(buy(None, &args(&["--help"])).await, 0);
        assert_eq!(buy(None, &args(&["-h"])).await, 0);
    }
}
