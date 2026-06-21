use crate::{bytes_to_hex_str, fmt_lh, load_signer, registry, report_call_error, wallet};

use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const ONRAMP_USAGE: &str = "\
usage: localharness onramp --pay <usdce> [--as <name>]
  The crypto-native first-$LH rail (design/cli-mainnet-onboarding.md C-2 / Phase
  2): pay USDC.e on Tempo and the proxy GROSS-mints $LH into your meter at parity
  (1 USDC.e = 100 $LH). No human, no card, no parent agent.
  --pay <usdce>   how much USDC.e to pay (e.g. 1 or 2.5). USDC.e is the Tempo fee
                  token, so this is SELF-PAID — the relay does NOT sponsor it; you
                  must hold enough USDC.e for the payment PLUS its gas.
  --as <name>     act as this local identity (its key signs the auth + the tx).
  The 402<->200 dance: POST without a credential -> parse the 402 Payment
  challenge (USDC.e, treasury recipient, quote) -> transfer USDC.e to the treasury
  -> retry with the settlement tx -> the proxy verifies on-chain and mints $LH.";

/// USDC.e is a 6-decimal TIP-20; `--pay` is parsed at this precision.
const USDCE_DECIMALS: u32 = 6;

/// Gas for ONE USDC.e `transfer(address,uint256)` (a TIP-20 transfer is ~50-65k;
/// budget headroom). Self-paid, so the SENDER is billed USDC.e for gas used (not
/// the limit) — over-budget is harmless, under-budget OOG-reverts.
const USDCE_TRANSFER_GAS: u128 = 150_000;

/// The MPP charge terms parsed from a 402 `WWW-Authenticate: Payment` challenge:
/// who to pay (`pay_to` treasury), in what asset (`asset` USDC.e), and how much
/// (`max_amount_required`, USDC.e base units). The proxy binds the eventual mint
/// to the ON-CHAIN transfer, so these are the payment instructions, not a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MppChallenge {
    pub asset: String,
    pub pay_to: String,
    pub max_amount_required: u128,
}

/// Parse the MPP charge terms out of a 402's `WWW-Authenticate: Payment …`
/// header. The header carries a base64url-encoded `request="…"` JSON blob
/// (`_mpp.ts::challengeHeader`); we decode it and read `asset` / `payTo` /
/// `maxAmountRequired`. Pure (no I/O) so the wire contract is unit-tested.
pub(crate) fn parse_payment_challenge(header: &str) -> Result<MppChallenge, String> {
    // The header is `Payment id="…", method="tempo", …, request="<b64url>", …`.
    let request_b64 = extract_quoted_param(header, "request")
        .ok_or_else(|| "402 challenge has no `request` parameter".to_string())?;
    let json_bytes = base64url_decode(&request_b64)
        .ok_or_else(|| "402 challenge `request` is not valid base64url".to_string())?;
    let json = String::from_utf8(json_bytes)
        .map_err(|_| "402 challenge `request` is not valid UTF-8".to_string())?;
    parse_challenge_json(&json)
}

/// Parse the decoded MPP `request` JSON into the charge terms. Split from
/// [`parse_payment_challenge`] so the JSON shape is testable without the
/// header/base64 envelope.
pub(crate) fn parse_challenge_json(json: &str) -> Result<MppChallenge, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|_| "402 challenge request is not JSON".to_string())?;
    let asset = v
        .get("asset")
        .and_then(|a| a.as_str())
        .ok_or_else(|| "402 challenge missing `asset`".to_string())?
        .to_string();
    let pay_to = v
        .get("payTo")
        .and_then(|p| p.as_str())
        .ok_or_else(|| "402 challenge missing `payTo`".to_string())?
        .to_string();
    // maxAmountRequired is a decimal STRING of USDC.e base units (it can exceed
    // what a JSON number safely holds), so parse it from the string form.
    let max_amount_required = v
        .get("maxAmountRequired")
        .and_then(|m| m.as_str())
        .ok_or_else(|| "402 challenge missing `maxAmountRequired`".to_string())?
        .parse::<u128>()
        .map_err(|_| "402 challenge `maxAmountRequired` is not an integer".to_string())?;
    if !localharness::encoding::is_address_hex(&pay_to) {
        return Err(format!("402 challenge `payTo` is not a 20-byte address: {pay_to}"));
    }
    if !localharness::encoding::is_address_hex(&asset) {
        return Err(format!("402 challenge `asset` is not a 20-byte address: {asset}"));
    }
    Ok(MppChallenge { asset, pay_to, max_amount_required })
}

/// Pull a `name="value"` quoted parameter out of an auth-style header. Returns
/// the inner value (without quotes), or None when the param is absent. Tolerant
/// of surrounding spaces; matches the first occurrence. Pure.
pub(crate) fn extract_quoted_param(header: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = header.find(&needle)? + needle.len();
    let rest = &header[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// The `Authorization: Payment` credential value carrying the settlement proof —
/// the base64url-encoded `{settlementTx, payTo}` JSON the proxy's
/// `parseCredential` reads (`payload="<b64url>"` form). `settlement_tx` is the
/// USDC.e transfer's hash; `pay_to` is the caller address to credit the $LH to.
/// Pure so the exact shape is unit-tested.
pub(crate) fn build_payment_credential(settlement_tx: &str, pay_to: &str) -> String {
    // serde_json emits the object with sorted-by-insertion keys; the proxy reads
    // by key name, so field order is irrelevant — only the names matter.
    let payload = serde_json::json!({
        "settlementTx": settlement_tx,
        "payTo": pay_to,
    })
    .to_string();
    let b64 = base64url_encode(payload.as_bytes());
    format!("Payment payload=\"{b64}\"")
}

// --- base64url (no external dep; the proxy uses URL-safe, padless) -----------

const B64URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn base64url_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64URL_ALPHABET[(n >> 18) as usize & 0x3f] as char);
        out.push(B64URL_ALPHABET[(n >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            out.push(B64URL_ALPHABET[(n >> 6) as usize & 0x3f] as char);
        }
        if chunk.len() > 2 {
            out.push(B64URL_ALPHABET[n as usize & 0x3f] as char);
        }
    }
    out
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let decode_char = |c: u8| -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    };
    // Strip any padding the encoder might have emitted (the proxy is padless).
    let bytes: Vec<u8> = input.bytes().filter(|&b| b != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        if chunk.len() < 2 {
            return None; // a lone trailing char encodes nothing valid
        }
        let mut n = 0u32;
        for &c in chunk {
            n = (n << 6) | decode_char(c)?;
        }
        // Left-align the partial group so the high bytes come out first.
        n <<= 6 * (4 - chunk.len());
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// `localharness onramp --pay <usdce> [--as <name>]` — the Tempo MPP USDC.e -> $LH
/// on-ramp (design/cli-mainnet-onboarding.md C-2 / Phase 2). Drives the MPP
/// 402<->200 dance against the proxy's `/mpp/onramp`, pays USDC.e SELF-PAID
/// (USDC.e is the Tempo fee token; the relay does NOT sponsor it), and prints the
/// minted $LH + the Payment-Receipt.
pub(crate) async fn onramp(args: &[String]) -> i32 {
    let mut pay: Option<String> = None;
    let mut as_name: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--pay" => match args.get(i + 1) {
                Some(v) => {
                    pay = Some(v.clone());
                    i += 2;
                }
                None => {
                    eprintln!("--pay needs a USDC.e amount\n{ONRAMP_USAGE}");
                    return 2;
                }
            },
            "--as" => match args.get(i + 1) {
                Some(n) => {
                    as_name = Some(n.clone());
                    i += 2;
                }
                None => {
                    eprintln!("--as needs a name\n{ONRAMP_USAGE}");
                    return 2;
                }
            },
            other => {
                eprintln!("unexpected argument '{other}'\n{ONRAMP_USAGE}");
                return 2;
            }
        }
    }

    let Some(pay_raw) = pay else {
        eprintln!("onramp: --pay <usdce> is required\n{ONRAMP_USAGE}");
        return 2;
    };
    let Some(usdce_units) = localharness::encoding::parse_token_amount_decimals(&pay_raw, USDCE_DECIMALS)
    else {
        eprintln!("--pay must be a positive USDC.e amount (e.g. 1 or 2.5), got '{pay_raw}'");
        return 2;
    };
    if usdce_units == 0 {
        eprintln!("--pay must be greater than zero");
        return 2;
    }

    let signer = match load_signer(as_name.as_deref()) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));
    println!("identity: {addr}");

    let base = registry::CREDIT_PROXY_URL.trim_end_matches('/');
    let endpoint = format!("{base}/mpp/onramp");
    let client = reqwest::Client::new();

    // --- step 1: POST without a credential -> 402 + Payment challenge ---------
    let challenge = match fetch_challenge(&client, &endpoint, &signer, usdce_units).await {
        Ok(c) => c,
        Err(e) => {
            report_call_error("onramp: challenge request failed", &e);
            return 1;
        }
    };
    // Pay what the caller asked (`--pay`), but never UNDER the proxy's quoted
    // minimum (`maxAmountRequired` is its band floor for the requested $LH). The
    // proxy mints from the ON-CHAIN USDC.e amount at parity (it does not cap the
    // mint to the quote), so paying our own `--pay` mints exactly that, while the
    // floor keeps a too-small `--pay` from landing below the mintable minimum.
    let to_pay = usdce_units.max(challenge.max_amount_required);
    println!(
        "402 challenge: treasury {} ({}); quoted minimum {} USDC.e",
        challenge.pay_to,
        challenge.asset,
        fmt_usdce(challenge.max_amount_required)
    );

    // The asset the proxy named MUST be the USDC.e fee token we will self-pay in.
    let fee_token = registry::ALPHA_USD_ADDRESS();
    if challenge.asset.to_lowercase() != fee_token.to_lowercase() {
        eprintln!(
            "onramp: the 402 names asset {} but this chain's USDC.e is {} — refusing to pay \
             (wrong chain? use `--dev` for testnet, or check the proxy config)",
            challenge.asset, fee_token
        );
        return 1;
    }

    // --- step 2: SELF-PAY the quoted USDC.e to the treasury -------------------
    // USDC.e is the Tempo fee token, NOT the diamond/$LH surface, so the keyless
    // relay does not sponsor this — the caller signs both halves and pays its own
    // gas in USDC.e. Must hold to_pay + gas.
    println!("paying {} USDC.e to the treasury (self-paid, no sponsor) …", fmt_usdce(to_pay));
    let settlement_tx = match registry::transfer_token_self_paid(
        &signer,
        fee_token,
        &challenge.pay_to,
        to_pay,
        USDCE_TRANSFER_GAS,
    )
    .await
    {
        Ok(tx) => tx,
        Err(e) => {
            report_call_error("onramp: USDC.e payment failed", &e);
            eprintln!(
                "  hint: this is SELF-PAID — your wallet must hold {} USDC.e plus gas. \
                 Check your USDC.e balance.",
                fmt_usdce(to_pay)
            );
            return 1;
        }
    };
    println!("  settled on-chain (tx {settlement_tx})");

    // --- step 3: retry with the Payment credential -> verify + mint -----------
    println!("claiming the mint …");
    match claim_mint(&client, &endpoint, &signer, &settlement_tx, &addr, usdce_units).await {
        Ok(out) => {
            print_mint_result(&out);
            0
        }
        Err(e) => {
            report_call_error("onramp: mint claim failed", &e);
            eprintln!(
                "  your USDC.e payment landed on-chain (tx {settlement_tx}); the mint is bound to \
                 that tx and is idempotent, so you can safely retry the claim once it confirms."
            );
            1
        }
    }
}

/// POST without a credential and parse the 402 Payment challenge. A non-402
/// response is surfaced as an error (a 200 here would mean the proxy minted with
/// no payment, which it never should; a 4xx/5xx carries its own message).
async fn fetch_challenge(
    client: &reqwest::Client,
    endpoint: &str,
    signer: &k256::ecdsa::SigningKey,
    usdce_units: u128,
) -> Result<MppChallenge, String> {
    let token = registry::proxy_auth_token(signer, now_secs());
    // Request the parity $LH for the USDC.e we intend to pay (whole $LH; the
    // proxy re-derives the actual mint from the ON-CHAIN amount, this only sizes
    // the quote). 1 USDC.e = 100 $LH at parity.
    let lh_amount = usdce_units_to_whole_lh(usdce_units);
    let body = serde_json::json!({ "lh_amount": lh_amount.to_string(), "pay_to": "" });
    let resp = client
        .post(endpoint)
        .header("x-goog-api-key", token)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    if status != 402 {
        // Drain the body for a useful message (the proxy returns problem+json).
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("expected a 402 Payment challenge, got HTTP {status}: {}", text.trim()));
    }
    let header = resp
        .headers()
        .get("www-authenticate")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| "402 response has no WWW-Authenticate header".to_string())?
        .to_string();
    parse_payment_challenge(&header)
}

/// Retry with the `Authorization: Payment` credential; on 200 parse the mint
/// result, else surface the body's error (402 = not yet confirmed; retryable).
async fn claim_mint(
    client: &reqwest::Client,
    endpoint: &str,
    signer: &k256::ecdsa::SigningKey,
    settlement_tx: &str,
    pay_to: &str,
    usdce_units: u128,
) -> Result<serde_json::Value, String> {
    let token = registry::proxy_auth_token(signer, now_secs());
    let credential = build_payment_credential(settlement_tx, pay_to);
    let lh_amount = usdce_units_to_whole_lh(usdce_units);
    let body = serde_json::json!({ "lh_amount": lh_amount.to_string(), "pay_to": pay_to });
    let resp = client
        .post(endpoint)
        .header("x-goog-api-key", token)
        .header("authorization", credential)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status == 200 {
        return serde_json::from_str(&text).map_err(|e| format!("bad 200 body: {e} ({text})"));
    }
    // The proxy returns {minted:false, error:"…"} with a 402 (retry) or 5xx.
    let reason = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| text.trim().to_string());
    Err(format!("HTTP {status}: {reason}"))
}

/// Print the proxy's 200 mint result: the minted $LH + the settlement tx.
fn print_mint_result(out: &serde_json::Value) {
    let lh_wei = out
        .get("lh_wei")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u128>().ok());
    match lh_wei {
        Some(wei) => println!("minted {} into your meter", fmt_lh(wei)),
        None => println!("mint confirmed"),
    }
    if out.get("idempotent").and_then(|v| v.as_bool()).unwrap_or(false) {
        println!("  (this settlement was already minted — idempotent no-op)");
    }
    if let Some(tx) = out.get("mint_tx").and_then(|v| v.as_str()) {
        println!("  mint tx: {tx}");
    }
    println!("  next: your $LH is in the meter — `localharness credits` to see it");
}

/// Whole-$LH quote for `usdce_units` (6-decimal) USDC.e at parity (1 USDC.e =
/// 100 $LH). Floors sub-$LH dust — the proxy re-derives the exact mint from the
/// on-chain amount, so this only sizes the challenge quote.
fn usdce_units_to_whole_lh(usdce_units: u128) -> u128 {
    // usdce_units / 10^6 = whole USDC.e; * 100 = whole $LH.
    (usdce_units / 1_000_000) * 100
}

/// Format USDC.e base units (6-decimal) as a human decimal string.
fn fmt_usdce(units: u128) -> String {
    let whole = units / 1_000_000;
    let frac = units % 1_000_000;
    if frac == 0 {
        format!("{whole}")
    } else {
        // Trim trailing zeros from the 6-decimal fractional part.
        let frac_str = format!("{frac:06}");
        format!("{whole}.{}", frac_str.trim_end_matches('0'))
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_challenge_json_extracts_terms() {
        // The exact shape _mpp.ts::buildChallenge emits inside `request`.
        let json = r#"{
            "scheme":"mpp","intent":"charge","network":"tempo","chainId":4217,
            "asset":"0x20c000000000000000000000b9537d11c60e8b50",
            "payTo":"0x0aff88ad13ef24cac5befd0f9dc3a05df79a922c",
            "maxAmountRequired":"2500000","maxTimeoutSeconds":600,
            "resource":"https://x/mpp/onramp","description":"…"
        }"#;
        let ch = parse_challenge_json(json).unwrap();
        assert_eq!(ch.asset, "0x20c000000000000000000000b9537d11c60e8b50");
        assert_eq!(ch.pay_to, "0x0aff88ad13ef24cac5befd0f9dc3a05df79a922c");
        assert_eq!(ch.max_amount_required, 2_500_000); // 2.5 USDC.e in base units
    }

    #[test]
    fn parse_challenge_json_rejects_malformed() {
        // Missing fields.
        assert!(parse_challenge_json(r#"{"asset":"0x..","payTo":"0x.."}"#).is_err());
        assert!(parse_challenge_json("not json").is_err());
        // maxAmountRequired must be a STRING integer, not a JSON number.
        let n = r#"{"asset":"0x20c000000000000000000000b9537d11c60e8b50",
            "payTo":"0x0aff88ad13ef24cac5befd0f9dc3a05df79a922c","maxAmountRequired":1}"#;
        assert!(parse_challenge_json(n).is_err());
        // A non-address payTo is rejected (we would otherwise pay garbage).
        let bad_to = r#"{"asset":"0x20c000000000000000000000b9537d11c60e8b50",
            "payTo":"nope","maxAmountRequired":"100"}"#;
        assert!(parse_challenge_json(bad_to).is_err());
    }

    #[test]
    fn parse_payment_challenge_decodes_header() {
        // Build the b64url request the proxy puts in `request="…"` and assert the
        // header parser round-trips it.
        let req = r#"{"asset":"0x20c000000000000000000000b9537d11c60e8b50",
            "payTo":"0x0aff88ad13ef24cac5befd0f9dc3a05df79a922c","maxAmountRequired":"1000000"}"#;
        let b64 = base64url_encode(req.as_bytes());
        let header = format!(
            "Payment id=\"abc123\", method=\"tempo\", intent=\"charge\", request=\"{b64}\", expires=\"123\""
        );
        let ch = parse_payment_challenge(&header).unwrap();
        assert_eq!(ch.max_amount_required, 1_000_000); // 1 USDC.e
        assert_eq!(ch.asset, "0x20c000000000000000000000b9537d11c60e8b50");
    }

    #[test]
    fn parse_payment_challenge_errors_without_request_param() {
        let header = "Payment id=\"abc\", method=\"tempo\"";
        assert!(parse_payment_challenge(header).is_err());
    }

    #[test]
    fn extract_quoted_param_pulls_named_value() {
        let h = "Payment id=\"x\", request=\"abc\", expires=\"9\"";
        assert_eq!(extract_quoted_param(h, "request").as_deref(), Some("abc"));
        assert_eq!(extract_quoted_param(h, "id").as_deref(), Some("x"));
        assert_eq!(extract_quoted_param(h, "missing"), None);
    }

    #[test]
    fn build_payment_credential_matches_proxy_shape() {
        // The proxy's parseCredential reads `Payment payload="<b64url-json>"`
        // and decodes {settlementTx, payTo}. Round-trip through our own decoder.
        let tx = "0x".to_string() + &"ab".repeat(32);
        let to = "0x0aff88ad13ef24cac5befd0f9dc3a05df79a922c";
        let cred = build_payment_credential(&tx, to);
        let b64 = extract_quoted_param(&cred, "payload").expect("payload param");
        assert!(cred.starts_with("Payment payload=\""));
        let json = String::from_utf8(base64url_decode(&b64).unwrap()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v.get("settlementTx").and_then(|x| x.as_str()), Some(tx.as_str()));
        assert_eq!(v.get("payTo").and_then(|x| x.as_str()), Some(to));
    }

    #[test]
    fn base64url_roundtrips_all_lengths() {
        // Padless URL-safe base64 must round-trip at every residue mod 3 (the
        // 1- and 2-byte tail groups are where an off-by-one would corrupt).
        for n in 0..40usize {
            let data: Vec<u8> = (0..n).map(|i| (i * 7 + 3) as u8).collect();
            let enc = base64url_encode(&data);
            assert!(!enc.contains('='), "must be padless: {enc}");
            assert!(!enc.contains('+') && !enc.contains('/'), "must be URL-safe: {enc}");
            assert_eq!(base64url_decode(&enc).unwrap(), data, "roundtrip failed at len {n}");
        }
    }

    #[test]
    fn usdce_units_to_whole_lh_is_parity() {
        // 1 USDC.e (1_000_000 base units) = 100 $LH at parity.
        assert_eq!(usdce_units_to_whole_lh(1_000_000), 100);
        assert_eq!(usdce_units_to_whole_lh(3_000_000), 300); // 3 USDC.e
        // This sizes a WHOLE-$LH quote, so it floors to whole USDC.e first: 2.5
        // USDC.e quotes 200 $LH (= 2 whole USDC.e). The proxy re-derives the real
        // mint from the on-chain amount, so the quote only needs to be in-band.
        assert_eq!(usdce_units_to_whole_lh(2_500_000), 200);
        // Sub-USDC.e dust floors to 0 whole $LH (the on-chain amount is authoritative).
        assert_eq!(usdce_units_to_whole_lh(500_000), 0);
    }

    #[test]
    fn fmt_usdce_renders_decimal() {
        assert_eq!(fmt_usdce(1_000_000), "1");
        assert_eq!(fmt_usdce(2_500_000), "2.5");
        assert_eq!(fmt_usdce(1_230_000), "1.23");
        assert_eq!(fmt_usdce(1), "0.000001");
        assert_eq!(fmt_usdce(0), "0");
    }
}
