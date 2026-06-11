use k256::ecdsa::SigningKey;

use super::*;

// --- Credits / daily allowance (CreditsFacet on the diamond) ---------

/// Sign + submit `CreditsFacet.claimDaily()` as a sponsored Tempo tx.
/// User holds zero of anything; sponsor pays AlphaUSD. The on-chain
/// `msg.sender` is the user (the diamond mints credits TO `msg.sender`),
/// so the sponsorship channel only covers the fee — never the issuance.
/// Reverts on-chain if the caller has already claimed this UTC day.
pub async fn claim_daily_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    fee_token: &str,
) -> Result<String, String> {
    // claimDaily inner: a single SSTORE + mint (token Transfer event +
    // memo event) — ~120k. Plus ~275k Tempo sponsorship overhead.
    sponsored_diamond_call(sender, fee_payer, selector("claimDaily()").to_vec(), fee_token, 600_000)
        .await
}

/// `eth_call canClaim(account)` — true iff `account` is eligible to
/// call `claimDaily()` right now (token configured, allowance > 0,
/// not yet claimed this UTC day).
pub async fn can_claim_credits(account_hex: &str) -> Result<bool, String> {
    let account_bytes = hex_to_bytes(account_hex)?;
    if account_bytes.len() != 20 {
        return Err(format!("account must be 20 bytes, got {}", account_bytes.len()));
    }
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(&account_bytes);
    let result_hex = read_view(selector("canClaim(address)"), &[padded]).await?;
    let trimmed = result_hex.trim().trim_start_matches("0x");
    Ok(trimmed.chars().last().map(|c| c == '1').unwrap_or(false))
}

/// `eth_call dailyAllowance()` — the current per-claim amount in
/// 18-decimal token wei.
pub async fn daily_allowance() -> Result<u128, String> {
    let result = read_view(selector("dailyAllowance()"), &[]).await?;
    decode_u256_as_u128(&result)
}

/// `eth_call lastClaimDay(account)` — the UTC day number (block.timestamp / 86400)
/// of the account's most recent claimDaily(). Returns 0 if never claimed.
pub async fn last_claim_day(account_hex: &str) -> Result<u64, String> {
    let account_bytes = hex_to_bytes(account_hex)?;
    if account_bytes.len() != 20 {
        return Err(format!("account must be 20 bytes, got {}", account_bytes.len()));
    }
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(&account_bytes);
    let result_hex = read_view(selector("lastClaimDay(address)"), &[padded]).await?;
    let val = decode_u256_as_u128(&result_hex)?;
    Ok(val as u64)
}

// --- Redeem codes + credit sessions ----------------------------------
//
// These back the `$LH` credit-proxy bootstrap: `redeem` mints credits
// from a one-time code (RedeemFacet), `open_session` spends credits to
// open a time-bounded usage session the Vercel Edge proxy reads via
// `session_expiry_of` on every request (SessionFacet). See
// `[[project-credit-proxy-monetization]]`.

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
    fee_payer: &SigningKey,
    code: &str,
    fee_token: &str,
) -> Result<String, String> {
    // redeem mints on the credits token (cold balanceOf + totalSupply
    // SSTOREs, AccessControl role checks, memo event) plus the claimed-flag
    // SSTORE — empirically ~1.07M inner, NOT the ~120k first assumed (a 600k
    // limit silently out-of-gassed every redeem). Plus ~275k sponsorship.
    // 2M gives headroom; sponsor is billed on gas used, not the limit.
    sponsored_diamond_call(sender, fee_payer, encode_redeem(code), fee_token, 2_000_000).await
}

/// Read `sessionExpiryOf(address)` — unix-seconds expiry of the
/// account's current credit session (0 / past = none). The credit
/// proxy makes this same call on every request.
pub async fn session_expiry_of(account_hex: &str) -> Result<u64, String> {
    let account = parse_eth_address(account_hex)?;
    let result = read_view(selector("sessionExpiryOf(address)"), &[addr_word(&account)]).await?;
    decode_u256_as_u64(&result)
}

/// Read `sessionPrice()` — `$LH` (wei) required to open one session.
pub async fn session_price() -> Result<u128, String> {
    let result = read_view(selector("sessionPrice()"), &[]).await?;
    decode_u256_as_u128(&result)
}

/// Open (or renew) the caller's credit session via a sponsored Tempo
/// tx. When `sessionPrice()` is non-zero, batches a
/// `LocalharnessCredits.approve(diamond, price)` call BEFORE
/// `openSession()` in the same tx — `openSession` then pulls the
/// credits via `transferFrom` inside its own body (same cost-gate
/// pattern as `register`).
pub async fn open_session_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    fee_token: &str,
) -> Result<String, String> {
    let price = session_price().await.unwrap_or(0);
    let input = selector("openSession()").to_vec();
    // approve (~46k) + openSession (transferFrom + 1 SSTORE + event,
    // ~90k) + ~275k sponsorship. 600k headroom.
    if price > 0 {
        sponsored_escrow_diamond_call(sender, fee_payer, price, input, fee_token, 600_000).await
    } else {
        sponsored_diamond_call(sender, fee_payer, input, fee_token, 600_000).await
    }
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

/// Prepay `$LH` into the per-request credit meter via a sponsored Tempo
/// tx — batches `approve(diamond, amount)` + `depositCredits(amount)`
/// (same cost-gate shape as `open_session_sponsored`).
pub async fn deposit_credits_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    amount_wei: u128,
    fee_token: &str,
) -> Result<String, String> {
    // approve + transferFrom (pull $LH into the diamond) + cold meter-
    // balance SSTORE + event. Like redeem, comfortably more than the old
    // 600k once cold SSTOREs are counted — 1.5M gives headroom.
    sponsored_escrow_diamond_call(
        sender,
        fee_payer,
        amount_wei,
        encode_deposit_credits(amount_wei),
        fee_token,
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
    fee_payer: &SigningKey,
    amount_wei: u128,
    fee_token: &str,
) -> Result<String, String> {
    // Ledger SSTORE + token transfer (warm balance SSTOREs) + event — well
    // under deposit's cost, but sponsorship overhead is ~275k on its own;
    // 1M keeps the same headroom policy as the other sponsored writes.
    sponsored_diamond_call(
        sender,
        fee_payer,
        encode_withdraw_credits(amount_wei),
        fee_token,
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

/// `eth_call allowance(owner, spender)` on [`LOCALHARNESS_TOKEN_ADDRESS`] —
/// how much `$LH` (18-decimal wei) `owner` has approved `spender` to pull
/// via `transferFrom`. The x402 `settle` pulls `$LH` from the payer through
/// the diamond's `transferFrom`, so the payer must have approved the diamond
/// (`REGISTRY_ADDRESS`) for at least the payment value; this lets the client
/// check before paying and approve if short.
pub async fn lh_allowance(owner_hex: &str, spender_hex: &str) -> Result<u128, String> {
    let owner = parse_eth_address(owner_hex)?;
    let spender = parse_eth_address(spender_hex)?;
    let calldata_hex = encode_call_hex(
        selector("allowance(address,address)"),
        &[addr_word(&owner), addr_word(&spender)],
    );
    let result = eth_call(LOCALHARNESS_TOKEN_ADDRESS, &calldata_hex).await?;
    decode_u256_as_u128(&result)
}

/// Approve `spender` to pull up to `amount_wei` `$LH` from `sender` via a
/// sponsored Tempo tx (sender holds zero gas; `fee_payer` pays AlphaUSD).
/// The x402 prerequisite: before paying an agent over `/mcp`, the payer
/// approves the diamond (`REGISTRY_ADDRESS`) so `settle`'s `transferFrom`
/// succeeds. Pass a large/`u128::MAX` amount to approve once and reuse.
pub async fn approve_lh_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    spender_hex: &str,
    amount_wei: u128,
    fee_token: &str,
) -> Result<String, String> {
    let spender = parse_eth_address(spender_hex)?;
    // approve is a single SSTORE (cold the first time) + event. 300k is
    // ample headroom on top of the AA-settlement overhead.
    sponsored_call_to(
        sender,
        fee_payer,
        LOCALHARNESS_TOKEN_ADDRESS,
        encode_approve(&spender, amount_wei),
        fee_token,
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
    fee_payer: &SigningKey,
    to_hex: &str,
    amount_wei: u128,
    fee_token: &str,
) -> Result<String, String> {
    let to = parse_eth_address(to_hex)?;
    sponsored_call_to(
        sender,
        fee_payer,
        LOCALHARNESS_TOKEN_ADDRESS,
        encode_transfer(&to, amount_wei),
        fee_token,
        300_000,
    )
    .await
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
