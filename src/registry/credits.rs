use k256::ecdsa::SigningKey;

use super::*;

// --- Redeem codes -----------------------------------------------------
//
// `redeem` mints credits from a one-time code (RedeemFacet) — the `$LH`
// bootstrap. The daily-claim client (claimDaily — disabled on-chain, sybil
// hole) and the SessionFacet client (time-boxed sessions — superseded by
// per-request metering) were PURGED 2026-07-30 (reset-over-compat).

/// Encode `redeem(string)` calldata. Same dynamic-string ABI shape as
/// `encode_register`.
pub(crate) fn encode_redeem(code: &str) -> Vec<u8> {
    let sel = selector("redeem(string)");
    let bytes = code.as_bytes();
    let len = bytes.len();
    let padded_len = len.div_ceil(32) * 32;

    let mut buf = Vec::with_capacity(4 + 32 + 32 + padded_len);
    buf.extend_from_slice(&sel);
    buf.extend_from_slice(&u256_be(0x20));
    buf.extend_from_slice(&u256_be(len as u128));
    buf.extend_from_slice(bytes);
    buf.resize(4 + 32 + 32 + padded_len, 0);
    buf
}

/// Redeem a one-time code for `$LH`, via a sponsored Tempo tx so the
/// caller needs zero balance. The plaintext `code` is hashed on-chain
/// (`keccak256`) and matched against the owner-loaded set; the credits
/// are minted to `sender`.
pub async fn redeem_sponsored(
    sender: &SigningKey,
    code: &str,
) -> Result<String, String> {
    // redeem mints on the credits token (cold balanceOf + totalSupply
    // SSTOREs, AccessControl role checks, memo event) plus the claimed-flag
    // SSTORE — empirically ~1.07M inner, NOT the ~120k first assumed (a 600k
    // limit silently out-of-gassed every redeem). Plus ~275k sponsorship.
    // 2M gives headroom; sponsor is billed on gas used, not the limit.
    sponsored_diamond_call(sender, encode_redeem(code), 2_000_000).await
}

pub(crate) fn encode_deposit_credits(amount_wei: u128) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32);
    out.extend_from_slice(&selector("depositCredits(uint256)"));
    out.extend_from_slice(&u256_be(amount_wei));
    out
}

/// Read `creditOf(address)` — the user's prepaid per-request `$LH`
/// balance in the credit meter (the proxy reads this to gate a call).
pub async fn credit_balance_of(account_hex: &str) -> Result<u128, String> {
    let account = parse_eth_address(account_hex)?;
    let result = read_view(selector("creditOf(address)"), &[addr_word(&account)]).await?;
    decode_u256_as_u128(&result)
}

/// Read `withdrawableOf(address)` — the UNLOCKED portion of the meter balance
/// (`creditOf` minus any still-locked fiat-minted `$LH`). This is the only part
/// `withdrawCredits` / the meter→wallet bridge can pull out, so reading it tells
/// the caller WHY a `send_lh`/bridge would revert `InsufficientCredits` (the
/// rest is fiat-origin credit, spend-only on inference until its lock clears).
pub async fn withdrawable_credit_of(account_hex: &str) -> Result<u128, String> {
    let account = parse_eth_address(account_hex)?;
    let result = read_view(selector("withdrawableOf(address)"), &[addr_word(&account)]).await?;
    decode_u256_as_u128(&result)
}

/// Prepay `$LH` into the per-request credit meter via a sponsored Tempo
/// tx — batches `approve(diamond, amount)` + `depositCredits(amount)`
/// (same cost-gate shape as `open_session_sponsored`).
pub async fn deposit_credits_sponsored(
    sender: &SigningKey,
    amount_wei: u128,
) -> Result<String, String> {
    // approve + transferFrom (pull $LH into the diamond) + cold meter-
    // balance SSTORE + event. Like redeem, comfortably more than the old
    // 600k once cold SSTOREs are counted — 1.5M gives headroom.
    sponsored_escrow_diamond_call(
        sender,
        amount_wei,
        encode_deposit_credits(amount_wei),
        1_500_000,
    )
    .await
}

/// Pull `amount_wei` of UNSPENT metered credits back into the sender's
/// wallet `$LH` via a sponsored Tempo tx (`withdrawCredits` — the reverse
/// of [`deposit_credits_sponsored`], so the meter and the wallet are one
/// balance in practice). The auto-bridge in paid agent calls uses this to
/// cover an x402 price from chat credits when the wallet pot is short.
pub async fn withdraw_credits_sponsored(
    sender: &SigningKey,
    amount_wei: u128,
) -> Result<String, String> {
    // Ledger SSTORE + token transfer (warm balance SSTOREs) + event — well
    // under deposit's cost, but sponsorship overhead is ~275k on its own;
    // 1M keeps the same headroom policy as the other sponsored writes.
    sponsored_diamond_call(
        sender,
        encode_withdraw_credits(amount_wei),
        1_000_000,
    )
    .await
}

pub(crate) fn encode_withdraw_credits(amount_wei: u128) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32);
    out.extend_from_slice(&selector("withdrawCredits(uint256)"));
    out.extend_from_slice(&u256_be(amount_wei));
    out
}

/// `eth_call allowance(owner, spender)` on [`LOCALHARNESS_TOKEN_ADDRESS()`] —
/// how much `$LH` (18-decimal wei) `owner` has approved `spender` to pull
/// via `transferFrom`. The x402 `settle` pulls `$LH` from the payer through
/// the diamond's `transferFrom`, so the payer must have approved the diamond
/// (`REGISTRY_ADDRESS()`) for at least the payment value; this lets the client
/// check before paying and approve if short.
pub async fn lh_allowance(owner_hex: &str, spender_hex: &str) -> Result<u128, String> {
    let owner = parse_eth_address(owner_hex)?;
    let spender = parse_eth_address(spender_hex)?;
    let calldata_hex = encode_call_hex(
        selector("allowance(address,address)"),
        &[addr_word(&owner), addr_word(&spender)],
    );
    let result = eth_call(LOCALHARNESS_TOKEN_ADDRESS(), &calldata_hex).await?;
    decode_u256_as_u128(&result)
}

/// Approve `spender` to pull up to `amount_wei` `$LH` from `sender` via a
/// sponsored Tempo tx (sender holds zero gas; `fee_payer` pays AlphaUSD).
/// The x402 prerequisite: before paying an agent over `/mcp`, the payer
/// approves the diamond (`REGISTRY_ADDRESS()`) so `settle`'s `transferFrom`
/// succeeds. Pass a large/`u128::MAX` amount to approve once and reuse.
pub async fn approve_lh_sponsored(
    sender: &SigningKey,
    spender_hex: &str,
    amount_wei: u128,
) -> Result<String, String> {
    let spender = parse_eth_address(spender_hex)?;
    // approve is a single SSTORE (cold the first time) + event. 300k is
    // ample headroom on top of the AA-settlement overhead.
    sponsored_call_to(
        sender,
        LOCALHARNESS_TOKEN_ADDRESS(),
        encode_approve(&spender, amount_wei),
        300_000,
    )
    .await
}

/// Transfer `amount_wei` `$LH` from `sender` to `to_hex` as a sponsored Tempo tx
/// (sponsor pays AlphaUSD; sender holds zero native). The CLI/native twin of the
/// browser `send_lh` tool — "one agent sends another `$LH`", the same effect as a
/// redeem code (controlled funding now that the daily allowance is disabled).
pub async fn transfer_lh_sponsored(
    sender: &SigningKey,
    to_hex: &str,
    amount_wei: u128,
) -> Result<String, String> {
    let to = parse_eth_address(to_hex)?;
    sponsored_call_to(
        sender,
        LOCALHARNESS_TOKEN_ADDRESS(),
        encode_transfer(&to, amount_wei),
        300_000,
    )
    .await
}

/// Transfer `amount_units` of an arbitrary TIP-20 `token_hex` from `sender` to
/// `to_hex` as a SELF-PAID Tempo tx — the sender pays the gas itself in
/// `token_hex` (the token IS the Tempo fee token, e.g. USDC.e), so there is NO
/// relay / sponsor. The MPP on-ramp settlement leg: a brand-new agent pays its
/// quoted USDC.e to the on-ramp treasury, and the proxy mints `$LH` at parity.
/// USDC.e (and any USD TIP-20 fee token) is NOT the diamond/$LH surface, so the
/// keyless relay deliberately does not sponsor it — the caller must hold enough
/// USDC.e to cover the payment plus its own gas. Returns the mined tx hash.
pub async fn transfer_token_self_paid(
    sender: &SigningKey,
    token_hex: &str,
    to_hex: &str,
    amount_units: u128,
    gas_limit: u128,
) -> Result<String, String> {
    let token = parse_eth_address(token_hex)?;
    let to = parse_eth_address(to_hex)?;
    let call = crate::tempo_tx::TempoCall {
        to: token,
        value_wei: 0,
        input: encode_transfer(&to, amount_units),
    };
    // fee_token == the token being transferred (USDC.e), so the sender pays gas
    // in the same stablecoin it is settling with — no native gas, no sponsor.
    submit_tempo_self_paid(sender, vec![call], Some(token_hex), gas_limit).await
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deposit_credits_calldata_layout() {
        let cd = encode_deposit_credits(1_000_000_000_000_000_000);
        assert_eq!(&cd[0..4], &selector("depositCredits(uint256)"));
        assert_eq!(cd.len(), 36);
    }
}
