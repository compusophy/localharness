use k256::ecdsa::SigningKey;

use crate::wallet;

use super::*;

// --- public helpers for cross-module tx flows -------------------------
//
// Nonce / gas-price / submit primitives the sponsored Tempo flows are
// assembled from (the browser's `run_sponsored_tempo_call` composes them
// around an iframe-signed tx). Available on every target; gated only by
// `wallet`. The legacy EIP-155/native-transfer lineage that used to live
// here (claim_name / token_transfer / rlp_native_transfer_* / rlp_call_* /
// the rlp_legacy_* encoder / the BootstrapFaucet path) was removed as dead
// code — every live write is a Tempo 0x76 tx via `tempo_tx`.

/// Pending nonce for `address_hex`. Use this as the next tx nonce so a
/// burst of payments doesn't collide with the previous tx still being
/// mined.
pub async fn next_nonce(address_hex: &str) -> Result<u128, String> {
    eth_get_transaction_count(address_hex).await
}

/// Gas limit for a sponsored Tempo tx whose dominant cost is ONE
/// `setMetadata(uint256,bytes32,bytes)` write of `byte_len` payload bytes
/// (app.wasm / public.html / persona / x402_price / sealed Gemini key).
///
/// THE canonical formula — every setMetadata budget (browser app and CLI)
/// must call this, not hand-roll a copy. `1.2M base + 8_500/byte`: storing
/// bytes on-chain costs ~7.6k gas/BYTE (measured via
/// `debug_traceTransaction`, 2026-06-03 — same byte-storage cost as the
/// FeedbackFacet), plus the ~275k Tempo sponsorship overhead and base call,
/// with margin. The old `~1.3M + words*40k` shape (~1.25k gas/byte) was ~6x
/// below the measured cost and silently OOG-reverted large writes. Over-budget
/// is FREE — the sponsor is billed on gas USED, not the limit — so headroom
/// is correct (see CLAUDE.md "On-chain writes that store data are gas-HUNGRY").
///
/// Batches that add a second tiny `setMetadata` (the `public_face` choice
/// string) fit inside the base headroom; genuinely different writes
/// (mints, burns, TBA executes) budget separately at their call sites.
pub fn set_metadata_gas(byte_len: usize) -> u128 {
    1_200_000 + byte_len as u128 * 8_500
}

/// Current `eth_gasPrice` reported by the node, in wei.
pub async fn current_gas_price() -> Result<u128, String> {
    eth_gas_price().await
}

/// Submit a signed raw tx hex and block until the receipt is mined.
/// Returns the tx hash. Errors if the receipt status is `0x0` (revert)
/// or if no receipt lands within the polling window.
pub async fn submit_and_wait_receipt(raw_hex: &str) -> Result<String, String> {
    let tx_hash = eth_send_raw_transaction(raw_hex).await?;
    wait_for_receipt(&tx_hash).await?;
    Ok(tx_hash)
}

// --- $localharness ERC-20 helpers ------------------------------------

/// `balanceOf(holder)` on [`LOCALHARNESS_TOKEN_ADDRESS`]. Returns the
/// holder's $localharness balance in 18-decimal token wei. Useful for
/// confirming the faucet/transfer flows actually landed funds.
pub async fn token_balance_of(holder_hex: &str) -> Result<u128, String> {
    erc20_balance_of(LOCALHARNESS_TOKEN_ADDRESS, holder_hex).await
}

/// `balanceOf(holder)` on an arbitrary ERC-20/TIP-20 token. Used by the
/// sponsor balance monitor to read the sponsor's fee-token (AlphaUSD)
/// balance and warn when it runs low.
pub async fn erc20_balance_of(token_hex: &str, holder_hex: &str) -> Result<u128, String> {
    let holder_bytes = hex_to_bytes(holder_hex)?;
    if holder_bytes.len() != 20 {
        return Err(format!("holder must be 20 bytes, got {}", holder_bytes.len()));
    }
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(&holder_bytes);
    let calldata_hex = encode_call_hex(selector("balanceOf(address)"), &[padded]);
    let result = eth_call(token_hex, &calldata_hex).await?;
    decode_u256_as_u128(&result)
}

// `token_faucet_self` removed in 2026-05-26 token migration — the
// new credit token has no `faucet(address)` method. Use
// `claim_daily_sponsored` against the diamond instead. `token_transfer`
// (the legacy SELF-PAID `transfer` + its `sign_and_submit_call` EIP-155
// plumbing) was removed as dead code — use `transfer_lh_sponsored`.

pub(crate) fn decode_u256_as_u128(hex: &str) -> Result<u128, String> {
    let trimmed = hex.trim_start_matches("0x");
    if trimmed.is_empty() {
        return Ok(0);
    }
    // Strip leading zeros so we fit in u128 (last 32 hex chars).
    let tail = if trimmed.len() <= 32 {
        trimmed
    } else {
        &trimmed[trimmed.len() - 32..]
    };
    u128::from_str_radix(tail, 16).map_err(|e| e.to_string())
}

// --- Tempo tx submission ---------------------------------------------

/// Native TIP-20 stablecoins on Tempo Moderato. These ARE eligible as
/// `fee_token` on a Tempo Transaction; our $LH is not (TIP-20-compliance
/// check fails). Pick one as the default fee_token for user-facing txs.
pub const ALPHA_USD_ADDRESS: &str = "0x20c0000000000000000000000000000000000001";

/// Sign and submit a SELF-PAID Tempo tx. Sender pays fees in
/// `fee_token` (`None` = native). Returns the tx hash once mined.
pub async fn submit_tempo_self_paid(
    sender: &SigningKey,
    calls: Vec<crate::tempo_tx::TempoCall>,
    fee_token: Option<&str>,
    gas_limit: u128,
) -> Result<String, String> {
    use crate::tempo_tx::{sign_self_paid, TempoTxBuilder};
    let sender_addr = wallet::address(sender);
    let sender_hex = address_to_hex(&sender_addr);
    let nonce = eth_get_transaction_count(&sender_hex).await?;
    let gas_price = eth_gas_price().await?;
    let mut builder = TempoTxBuilder::new(CHAIN_ID)
        .max_priority_fee_per_gas(gas_price)
        .max_fee_per_gas(gas_price)
        .gas_limit(gas_limit)
        .nonce(nonce)
        .calls(calls);
    if let Some(token) = fee_token {
        builder = builder.fee_token(parse_eth_address(token)?);
    }
    let tx = builder.build();
    let raw = sign_self_paid(tx, sender);
    let raw_hex = format!("0x{}", bytes_to_hex(&raw));
    let tx_hash = eth_send_raw_transaction(&raw_hex).await?;
    wait_for_receipt(&tx_hash).await?;
    Ok(tx_hash)
}

/// Sign and submit a SPONSORED Tempo tx. `sender` signs the intent
/// (and needs no balance); `fee_payer` signs as the gas payer (needs
/// `fee_token` balance). The chain debits `fee_payer`'s `fee_token`
/// balance for the cost; `sender` pays nothing.
pub async fn submit_tempo_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    calls: Vec<crate::tempo_tx::TempoCall>,
    fee_token: &str,
    gas_limit: u128,
) -> Result<String, String> {
    use crate::tempo_tx::{sign_sponsored, TempoTxBuilder};
    let sender_addr = wallet::address(sender);
    let sender_hex = address_to_hex(&sender_addr);
    let nonce = eth_get_transaction_count(&sender_hex).await?;
    let gas_price = eth_gas_price().await?;
    let tx = TempoTxBuilder::new(CHAIN_ID)
        .max_priority_fee_per_gas(gas_price)
        .max_fee_per_gas(gas_price)
        .gas_limit(gas_limit)
        .nonce(nonce)
        .calls(calls)
        .fee_token(parse_eth_address(fee_token)?)
        .sponsored()
        .build();
    let raw = sign_sponsored(tx, sender, fee_payer);
    let raw_hex = format!("0x{}", bytes_to_hex(&raw));
    let tx_hash = eth_send_raw_transaction(&raw_hex).await?;
    wait_for_receipt(&tx_hash).await?;
    Ok(tx_hash)
}

pub(crate) fn parse_eth_address(hex_str: &str) -> Result<[u8; 20], String> {
    let bytes = hex_to_bytes(hex_str)?;
    if bytes.len() != 20 {
        return Err(format!("address must be 20 bytes, got {}", bytes.len()));
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(out)
}

// --- sponsored-write skeletons ----------------------------------------
//
// Every `*_sponsored` wrapper repeated the same parse-address → TempoCall →
// submit_tempo_sponsored skeleton; the wrappers now keep ONLY what differs
// per facet call: the calldata encoding and the gas budget.

/// ONE sponsored Tempo call to `to_hex` (zero value). The shared body of
/// every single-call `*_sponsored` wrapper; non-diamond targets ($LH token
/// approve/transfer, TBA execute) pass their own address.
pub(crate) async fn sponsored_call_to(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    to_hex: &str,
    input: Vec<u8>,
    fee_token: &str,
    gas_limit: u128,
) -> Result<String, String> {
    let call = crate::tempo_tx::TempoCall {
        to: parse_eth_address(to_hex)?,
        value_wei: 0,
        input,
    };
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, gas_limit).await
}

/// ONE sponsored call to the registry diamond — the most common wrapper
/// shape (claimDaily / redeem / cancelJob / acceptInvite / attest / vote /
/// announce / submitFeedback / …).
pub(crate) async fn sponsored_diamond_call(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    input: Vec<u8>,
    fee_token: &str,
    gas_limit: u128,
) -> Result<String, String> {
    sponsored_call_to(sender, fee_payer, REGISTRY_ADDRESS, input, fee_token, gas_limit).await
}

/// `$LH.approve(diamond, amount)` + a diamond call batched in ONE sponsored
/// tx — the approve→`transferFrom`-pull ESCROW shape shared by scheduleJob /
/// createInvite / postBounty / fundGuild / depositCredits and the cost-gated
/// register/registerMain/openSession paths.
pub(crate) async fn sponsored_escrow_diamond_call(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    amount_wei: u128,
    input: Vec<u8>,
    fee_token: &str,
    gas_limit: u128,
) -> Result<String, String> {
    sponsored_escrow_diamond_call_bridged(sender, fee_payer, amount_wei, input, fee_token, gas_limit, 0)
        .await
}

/// [`sponsored_escrow_diamond_call`] with the METER AUTO-BRIDGE: when
/// `bridge_wei > 0` a `withdrawCredits(bridge_wei)` call is PREPENDED to the
/// SAME atomic Tempo tx (0x76 carries a calls array), pulling a wallet
/// shortfall back out of the caller's unspent chat-meter credits before the
/// approve→escrow pair runs — so "1057 $LH in the meter but the wallet is
/// short" no longer blocks an escrow (on-chain feedback #63). Gas is bumped
/// 150k when bridging (the same rider budget `send_lh` uses).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn sponsored_escrow_diamond_call_bridged(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    amount_wei: u128,
    input: Vec<u8>,
    fee_token: &str,
    gas_limit: u128,
    bridge_wei: u128,
) -> Result<String, String> {
    let calls = escrow_call_batch(amount_wei, input, bridge_wei)?;
    let gas = if bridge_wei > 0 { gas_limit + 150_000 } else { gas_limit };
    submit_tempo_sponsored(sender, fee_payer, calls, fee_token, gas).await
}

/// Build the calls array for a (possibly meter-bridged) escrow tx, in EXACT
/// execution order: `withdrawCredits(bridge_wei)` on the diamond (only when
/// `bridge_wei > 0`) → `$LH.approve(diamond, amount_wei)` → the escrow diamond
/// call itself. Pure (no I/O) so the layout is natively testable.
pub(crate) fn escrow_call_batch(
    amount_wei: u128,
    input: Vec<u8>,
    bridge_wei: u128,
) -> Result<Vec<crate::tempo_tx::TempoCall>, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let token_addr = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS)?;
    let mut calls = Vec::with_capacity(3);
    if bridge_wei > 0 {
        calls.push(crate::tempo_tx::TempoCall {
            to: diamond_addr,
            value_wei: 0,
            input: encode_withdraw_credits(bridge_wei),
        });
    }
    calls.push(crate::tempo_tx::TempoCall {
        to: token_addr,
        value_wei: 0,
        input: encode_approve(&diamond_addr, amount_wei),
    });
    calls.push(crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input,
    });
    Ok(calls)
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Bridged escrow batch: call ORDER must be withdrawCredits → approve →
    /// escrow, with the bridge + escrow aimed at the DIAMOND and the approve
    /// at the $LH token. A reordered batch would approve before the bridged
    /// funds exist (revert) or escrow before the approve (revert).
    #[test]
    fn escrow_call_batch_bridged_order_and_targets() {
        let escrow_input = vec![0xAA, 0xBB, 0xCC, 0xDD];
        let calls = escrow_call_batch(700, escrow_input.clone(), 250).unwrap();
        assert_eq!(calls.len(), 3);
        let diamond = parse_eth_address(REGISTRY_ADDRESS).unwrap();
        let token = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS).unwrap();
        // 1. withdrawCredits(bridge_wei) on the diamond.
        assert_eq!(calls[0].to, diamond);
        assert_eq!(calls[0].input, encode_withdraw_credits(250));
        // 2. $LH.approve(diamond, amount_wei) on the token.
        assert_eq!(calls[1].to, token);
        assert_eq!(calls[1].input, encode_approve(&diamond, 700));
        // 3. The escrow call itself, untouched, on the diamond.
        assert_eq!(calls[2].to, diamond);
        assert_eq!(calls[2].input, escrow_input);
        // Every call is zero-value (the $LH moves via calldata, never `value`).
        assert!(calls.iter().all(|c| c.value_wei == 0));
    }

    /// `bridge_wei == 0` must be BYTE-IDENTICAL to the original two-call
    /// approve+escrow shape — the bridged fn is a strict superset, so every
    /// existing caller (CLI included) keeps its exact wire bytes.
    #[test]
    fn escrow_call_batch_zero_bridge_matches_original_shape() {
        let escrow_input = vec![0x01, 0x02, 0x03];
        let unbridged = escrow_call_batch(42, escrow_input.clone(), 0).unwrap();
        let bridged = escrow_call_batch(42, escrow_input, 7).unwrap();
        // No bridge call rides along when bridge_wei is 0.
        assert_eq!(unbridged.len(), 2);
        // The trailing approve+escrow pair is byte-identical either way.
        for (a, b) in unbridged.iter().zip(&bridged[1..]) {
            assert_eq!(a.to, b.to);
            assert_eq!(a.value_wei, b.value_wei);
            assert_eq!(a.input, b.input);
        }
    }

    #[test]
    fn decode_u256_as_u128_truncation_and_empty() {
        // Empty → 0.
        assert_eq!(decode_u256_as_u128("0x").unwrap(), 0);
        // Normal small value.
        assert_eq!(decode_u256_as_u128(&format!("0x{}", word_usize(42))).unwrap(), 42);
        // Exactly u128::MAX in the low 16 bytes.
        let max = format!("0x{}{}", "0".repeat(32), "f".repeat(32));
        assert_eq!(decode_u256_as_u128(&max).unwrap(), u128::MAX);
    }
}
