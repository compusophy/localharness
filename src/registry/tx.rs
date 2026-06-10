use k256::ecdsa::SigningKey;
use sha3::{Digest, Keccak256};

use crate::wallet;

use super::*;

// --- public helpers for cross-module tx flows -------------------------
//
// The browser app's chat flow (subdomain origin) and iframe signer
// (apex origin) both need to compose native-ETH transfers — visitor
// pays the agent's TBA for a turn. These wrap the registry's RLP +
// JSON-RPC primitives so callers don't reimplement EIP-155 envelope
// encoding. Available on every target; gated only by `wallet`.

/// Pending nonce for `address_hex`. Use this as the next tx nonce so a
/// burst of payments doesn't collide with the previous tx still being
/// mined.
pub async fn next_nonce(address_hex: &str) -> Result<u128, String> {
    eth_get_transaction_count(address_hex).await
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

/// EIP-155 unsigned RLP for a native ETH transfer (zero calldata).
/// Hash this with keccak256 to get the prehash a signer commits to.
pub fn rlp_native_transfer_unsigned(
    to_hex: &str,
    value_wei: u128,
    nonce: u128,
    gas_price: u128,
    gas_limit: u128,
) -> Result<Vec<u8>, String> {
    rlp_legacy_unsigned(nonce, gas_price, gas_limit, to_hex, value_wei, &[], CHAIN_ID)
}

/// Assemble a `0x`-prefixed signed raw tx hex from a native-ETH
/// transfer's parameters plus a 65-byte signature (r||s||v, where v
/// is 27 or 28 — the format `wallet::sign_hash` produces). Lifts v
/// into the EIP-155 form (`chain_id * 2 + 35 + recovery_id`).
pub fn rlp_native_transfer_signed(
    to_hex: &str,
    value_wei: u128,
    nonce: u128,
    gas_price: u128,
    gas_limit: u128,
    sig_65: &[u8; 65],
) -> Result<String, String> {
    let rec_id = (sig_65[64] - 27) as u64;
    let v = CHAIN_ID * 2 + 35 + rec_id;
    let signed = rlp_legacy_signed(
        nonce,
        gas_price,
        gas_limit,
        to_hex,
        value_wei,
        &[],
        v,
        &sig_65[..32],
        &sig_65[32..64],
    )?;
    Ok(format!("0x{}", bytes_to_hex(&signed)))
}

/// Gas limit for a vanilla native-ETH transfer with no calldata.
/// The protocol-mandated 21_000 (EIP-2028 doesn't apply here — no data).
pub const NATIVE_TRANSFER_GAS_LIMIT: u128 = 21_000;

/// Native-ETH balance of `address_hex` in wei.
pub async fn balance_of(address_hex: &str) -> Result<u128, String> {
    let hex = rpc(
        "eth_getBalance",
        serde_json::json!([address_hex, "latest"]),
    )
    .await?;
    parse_hex_quantity(&hex)
}

/// Poll `eth_getBalance` until it reports at least `min_wei`, with
/// 1-second cadence. Returns the observed balance on success, errors
/// if no observation reached `min_wei` within `max_attempts` seconds.
/// Used by the identity-creation flow to confirm the faucet drip
/// actually landed before letting the user try a real tx.
pub async fn wait_for_min_balance(
    address_hex: &str,
    min_wei: u128,
    max_attempts: u32,
) -> Result<u128, String> {
    for _ in 0..max_attempts {
        let bal = balance_of(address_hex).await?;
        if bal >= min_wei {
            return Ok(bal);
        }
        sleep_ms(1000).await;
    }
    Err(format!(
        "balance for {address_hex} did not reach {min_wei} wei within {max_attempts}s"
    ))
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
// `claim_daily_sponsored` against the diamond instead.

/// Sign + submit `LocalharnessToken.transfer(to, amount)`. The
/// payment loop's substitute for `rlp_native_transfer` —
/// `transfer` is an ERC-20 contract call, which Tempo allows.
pub async fn token_transfer(
    signer: &SigningKey,
    to_hex: &str,
    amount_token_wei: u128,
) -> Result<String, String> {
    let to_bytes = hex_to_bytes(to_hex)?;
    if to_bytes.len() != 20 {
        return Err(format!("to must be 20 bytes, got {}", to_bytes.len()));
    }
    let selector = selector("transfer(address,uint256)");
    let mut to_padded = [0u8; 32];
    to_padded[12..].copy_from_slice(&to_bytes);
    let amount_bytes = u256_be(amount_token_wei);
    let mut calldata = Vec::with_capacity(4 + 32 + 32);
    calldata.extend_from_slice(&selector);
    calldata.extend_from_slice(&to_padded);
    calldata.extend_from_slice(&amount_bytes);
    sign_and_submit_call(signer, LOCALHARNESS_TOKEN_ADDRESS, 0, &calldata).await
}

/// Build, sign, submit, wait-for-receipt for a contract call.
/// `to_hex` is the contract, `value_wei` is the native value sent
/// with the call (usually 0 for ERC-20 ops on Tempo), `calldata` is
/// the encoded selector + args. Errors propagate from any leg.
pub(crate) async fn sign_and_submit_call(
    signer: &SigningKey,
    to_hex: &str,
    value_wei: u128,
    calldata: &[u8],
) -> Result<String, String> {
    if to_hex == zero_address() {
        return Err("target contract address is zero".into());
    }
    let from_bytes = wallet::address(signer);
    let from_hex = address_to_hex(&from_bytes);

    let nonce = eth_get_transaction_count(&from_hex).await?;
    let gas_price = eth_gas_price().await?;
    let calldata_hex = format!("0x{}", bytes_to_hex(calldata));
    let gas_limit = eth_estimate_gas(&from_hex, to_hex, &calldata_hex).await?;

    let unsigned = rlp_legacy_unsigned(
        nonce, gas_price, gas_limit, to_hex, value_wei, calldata, CHAIN_ID,
    )?;
    let mut hasher = Keccak256::new();
    hasher.update(&unsigned);
    let mut prehash = [0u8; 32];
    prehash.copy_from_slice(&hasher.finalize());

    let sig = wallet::sign_hash(signer, &prehash);
    let rec_id = (sig[64] - 27) as u64;
    let v = CHAIN_ID * 2 + 35 + rec_id;
    let signed = rlp_legacy_signed(
        nonce, gas_price, gas_limit, to_hex, value_wei, calldata,
        v, &sig[..32], &sig[32..64],
    )?;
    let raw_hex = format!("0x{}", bytes_to_hex(&signed));

    let tx_hash = eth_send_raw_transaction(&raw_hex).await?;
    wait_for_receipt(&tx_hash).await?;
    Ok(tx_hash)
}

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
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let token_addr = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS)?;
    let approve_call = crate::tempo_tx::TempoCall {
        to: token_addr,
        value_wei: 0,
        input: encode_approve(&diamond_addr, amount_wei),
    };
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input,
    };
    submit_tempo_sponsored(sender, fee_payer, vec![approve_call, call], fee_token, gas_limit).await
}


// --- legacy / EIP-155 transaction RLP --------------------------------

/// EIP-155 unsigned RLP for any legacy tx — contract call OR native
/// transfer. Pass empty `data` for native, populated `data` for a
/// contract call. Hash with keccak256 to get the prehash a signer
/// commits to. The native-transfer-specific wrapper
/// [`rlp_native_transfer_unsigned`] is built on top of this.
pub fn rlp_call_unsigned(
    to_hex: &str,
    value_wei: u128,
    data: &[u8],
    nonce: u128,
    gas_price: u128,
    gas_limit: u128,
) -> Result<Vec<u8>, String> {
    rlp_legacy_unsigned(nonce, gas_price, gas_limit, to_hex, value_wei, data, CHAIN_ID)
}

/// Assemble a `0x`-prefixed signed raw tx hex for any legacy-style
/// tx (contract call or native). General-purpose counterpart to
/// [`rlp_call_unsigned`].
pub fn rlp_call_signed(
    to_hex: &str,
    value_wei: u128,
    data: &[u8],
    nonce: u128,
    gas_price: u128,
    gas_limit: u128,
    sig_65: &[u8; 65],
) -> Result<String, String> {
    let rec_id = (sig_65[64] - 27) as u64;
    let v = CHAIN_ID * 2 + 35 + rec_id;
    let signed = rlp_legacy_signed(
        nonce, gas_price, gas_limit, to_hex, value_wei, data,
        v, &sig_65[..32], &sig_65[32..64],
    )?;
    Ok(format!("0x{}", bytes_to_hex(&signed)))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn rlp_legacy_unsigned(
    nonce: u128,
    gas_price: u128,
    gas_limit: u128,
    to_hex: &str,
    value: u128,
    data: &[u8],
    chain_id: u64,
) -> Result<Vec<u8>, String> {
    let to_bytes = hex_to_bytes(to_hex)?;
    // EIP-155: rlp([nonce, gasPrice, gasLimit, to, value, data, chainId, 0, 0])
    let items = vec![
        wallet::rlp_uint(nonce),
        wallet::rlp_uint(gas_price),
        wallet::rlp_uint(gas_limit),
        wallet::rlp_bytes(&to_bytes),
        wallet::rlp_uint(value),
        wallet::rlp_bytes(data),
        wallet::rlp_uint(chain_id as u128),
        wallet::rlp_uint(0),
        wallet::rlp_uint(0),
    ];
    Ok(wallet::rlp_list(&items))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn rlp_legacy_signed(
    nonce: u128,
    gas_price: u128,
    gas_limit: u128,
    to_hex: &str,
    value: u128,
    data: &[u8],
    v: u64,
    r: &[u8],
    s: &[u8],
) -> Result<Vec<u8>, String> {
    let to_bytes = hex_to_bytes(to_hex)?;
    // r and s are 32 bytes each; RLP wants minimal-leading-zero
    // representations. Strip leading zeros (but not all if the value is 0).
    let r_min = strip_leading_zeros(r);
    let s_min = strip_leading_zeros(s);
    let items = vec![
        wallet::rlp_uint(nonce),
        wallet::rlp_uint(gas_price),
        wallet::rlp_uint(gas_limit),
        wallet::rlp_bytes(&to_bytes),
        wallet::rlp_uint(value),
        wallet::rlp_bytes(data),
        wallet::rlp_uint(v as u128),
        wallet::rlp_bytes(r_min),
        wallet::rlp_bytes(s_min),
    ];
    Ok(wallet::rlp_list(&items))
}

pub(crate) fn strip_leading_zeros(bytes: &[u8]) -> &[u8] {
    let first_nz = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len() - 1);
    &bytes[first_nz..]
}


#[cfg(test)]
mod tests {
    use super::*;

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
