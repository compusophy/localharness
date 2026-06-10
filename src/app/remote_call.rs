//! Proxy-mediated paid agent call — the browser's route to a FOREIGN agent.
//!
//! The `?rpc=1` iframe path only reaches agents with state on THIS machine
//! (OPFS is per-origin but per-device), so an agent someone else owns can
//! never answer locally — it has no key and no persona here. This module is
//! the app half of the fallback `call_agent` uses instead: sign an x402
//! `PaymentAuthorization` paying the target's TBA in `$LH`, POST the
//! `ask_agent` tools/call to the hosted MCP endpoint (`<proxy>/mcp`), and
//! return the reply the proxy generated under the target's on-chain persona.
//! The caller's `$LH` pays; neither side needs a model key. Installed into
//! `x402_hook::install_remote_call` at mount.

use crate::registry;

// The auto-pay ceiling + the fallback-then-cap decision live in
// `registry::{REMOTE_CALL_MAX_AUTO_PAY_WEI, auto_pay_amount}` — pure and
// natively testable there (this module is wasm-gated). Only the error
// FORMATTING stays here.

/// How long to wait for the proxy's reply. The proxy settles on-chain and
/// then runs a full (non-streaming) model turn, so this is generous.
const REMOTE_CALL_TIMEOUT_MS: u32 = 120_000;

/// Exact-length address decode (20 bytes, optional 0x).
fn parse_addr(s: &str) -> Result<[u8; 20], String> {
    let t = s.trim().trim_start_matches("0x");
    if t.len() != 40 {
        return Err(format!("bad address length: {s}"));
    }
    crate::encoding::hex_to_bytes(t)?
        .try_into()
        .map_err(|_| format!("bad address: {s}"))
}

/// Ask `target` through the hosted x402 endpoint, paying from the local
/// credit key. Returns the agent's reply text, or a descriptive error.
pub(crate) async fn ask_via_proxy(target: &str, message: &str) -> Result<String, String> {
    let (signer, from) = super::chat::credit_signer()
        .await
        .ok_or_else(|| "no identity to pay from".to_string())?;
    let from_hex = crate::encoding::bytes_to_hex_str(&from);

    // The payee is the target's on-chain TBA — resolved here AND re-checked
    // by the proxy against the registry, so a bogus name fails fast.
    let to_hex = registry::tba_of_name(target)
        .await
        .map_err(|e| format!("payee lookup: {e}"))?
        .ok_or_else(|| format!("'{target}' is not a registered agent"))?;
    let to = parse_addr(&to_hex)?;

    // Pay the target's effective price (advertised on-chain, else the
    // platform default) — the proxy enforces it as a floor, so paying the
    // old flat tip would just 402. Capped by `registry::auto_pay_amount`.
    let token_id = registry::id_of_name(target)
        .await
        .map_err(|e| format!("price lookup: {e}"))?;
    let advertised = registry::x402_price_of(token_id)
        .await
        .map_err(|e| format!("price lookup: {e}"))?;
    let pay_wei = registry::auto_pay_amount(
        advertised,
        registry::REMOTE_CALL_MAX_AUTO_PAY_WEI,
    )
    .map_err(|over_cap_wei| {
        format!(
            "'{target}' charges {} $LH per call — above the {} $LH auto-pay cap; \
             call it yourself if you accept the price",
            crate::app::format_wei_as_test_eth(over_cap_wei),
            crate::app::format_wei_as_test_eth(registry::REMOTE_CALL_MAX_AUTO_PAY_WEI),
        )
    })?;

    // `settle` pulls the $LH from the payer via `transferFrom`, so the payer
    // must have approved the diamond once. Sponsored, so a fresh identity
    // with zero gas can still approve. A read failure shouldn't hard-block —
    // settle is the authoritative gate.
    match registry::lh_allowance(&from_hex, registry::REGISTRY_ADDRESS).await {
        Ok(allowance) if allowance >= pay_wei => {}
        Ok(_) => {
            let sponsor = super::sponsor::signer()?;
            registry::approve_lh_sponsored(
                &signer,
                &sponsor,
                registry::REGISTRY_ADDRESS,
                u128::MAX,
                registry::ALPHA_USD_ADDRESS,
            )
            .await
            .map_err(|e| format!("$LH approve: {e}"))?;
        }
        Err(_) => {}
    }

    let now = (js_sys::Date::now() / 1000.0) as u64;
    let valid_before = now + 3600;
    let nonce = registry::random_x402_nonce();
    let signature = registry::sign_x402(
        &signer,
        &from,
        &to,
        pay_wei,
        0,
        valid_before,
        &nonce,
    )?;
    let header = registry::x402_authorization_json(
        &from_hex,
        &to_hex,
        pay_wei,
        0,
        valid_before,
        &nonce,
        &signature,
    );
    let body = registry::x402_ask_agent_body(target, message);

    // Browser fetch has no timeout (and `reqwest::Client::timeout` is a no-op
    // on wasm) — race against a timer like `registry::rpc` does.
    let json = super::net::with_timeout(REMOTE_CALL_TIMEOUT_MS, async {
        let resp = reqwest::Client::new()
            .post(registry::mcp_endpoint_url())
            .header("content-type", "application/json")
            .header("x-x402-authorization", header.to_string())
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("proxy request: {e}"))?;
        resp.json::<serde_json::Value>()
            .await
            .map_err(|e| format!("proxy response decode: {e}"))
    })
    .await
    .map_err(|_| format!("proxy call timed out after {}s", REMOTE_CALL_TIMEOUT_MS / 1000))??;

    registry::parse_mcp_tool_reply(&json)
}
