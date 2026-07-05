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

/// Absolute ceiling on the per-gas price we'll authorize for a sponsored tx,
/// in wei. T7/TIP-1067 hard-caps the base fee at 12 gwei, and our builders set
/// `max_fee = 2 × spot` (≤24 gwei worst case); 50 gwei leaves ~2x margin over
/// that yet caps a hostile/MITM'd RPC from inflating `gas_limit * price` to
/// drain the embedded sponsor's fee-token float (the price is taken verbatim
/// from the node). Refuse rather than clamp — never silently authorize an
/// absurd fee. MIRRORS `proxy/api/sponsor.ts::MAX_GAS_PRICE_WEI` — keep the
/// two in lockstep.
pub const MAX_GAS_PRICE_WEI: u128 = 50_000_000_000; // 50 gwei

/// Absolute ceiling on the `gas_limit` we'll authorize for a sponsored tx. Block
/// limit is ~500M; 50M is generous headroom for the largest real write (a big
/// `setMetadata` / `submitFeedback`) yet caps `gas_limit * price` drain from a
/// hostile/MITM'd RPC. MIRRORS `proxy/api/sponsor.ts::MAX_GAS_LIMIT` (50_000_000n)
/// — keep the two in lockstep (the relay enforces it server-side; this is the
/// Rust-side single source of truth, alongside [`MAX_GAS_PRICE_WEI`]).
pub const MAX_GAS_LIMIT: u128 = 50_000_000;

/// Current `eth_gasPrice` reported by the node, in wei — REFUSING (Err) when it
/// exceeds [`MAX_GAS_PRICE_WEI`], so every sponsored builder that prices off the
/// node can't be tricked into an absurd fee by a compromised RPC.
///
/// Builders set `max_fee = 2 × this` with `priority_fee = 0` (T7/TIP-1067
/// dynamic base fee: the spot read can be outrun by up to +12.5%/block, and
/// `max_fee < base_fee` is a hard node rejection; the 2× headroom rides the
/// swing while the zero tip pays only the actual base fee).
pub async fn current_gas_price() -> Result<u128, String> {
    let price = eth_gas_price().await?;
    clamp_gas_price(price)
}

/// Reject a node-reported gas price above [`MAX_GAS_PRICE_WEI`]. Pure so the
/// guard is unit-tested without an RPC.
pub(crate) fn clamp_gas_price(price: u128) -> Result<u128, String> {
    if price > MAX_GAS_PRICE_WEI {
        return Err(format!(
            "node-reported gas price {price} wei exceeds the {MAX_GAS_PRICE_WEI} wei ceiling — \
             refusing to sign (possible hostile/MITM'd RPC)"
        ));
    }
    Ok(price)
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

/// `balanceOf(holder)` on [`LOCALHARNESS_TOKEN_ADDRESS()`]. Returns the
/// holder's $localharness balance in 18-decimal token wei. Useful for
/// confirming the faucet/transfer flows actually landed funds.
pub async fn token_balance_of(holder_hex: &str) -> Result<u128, String> {
    erc20_balance_of(LOCALHARNESS_TOKEN_ADDRESS(), holder_hex).await
}

/// `balanceOf(holder)` on the DIAMOND — the ERC-721 NAME count (how many
/// subdomains the holder owns), NOT the `$LH` balance. Mirrors exactly the
/// on-chain `LibRegistryStorage.balanceOf` the `RedeemFacet` `NoIdentity` guard
/// reads (`balanceOf == 0` ⇒ not a registered identity). Same `balanceOf(address)`
/// selector as the ERC-20 read, pointed at the diamond instead of the token.
pub async fn name_balance_of(holder_hex: &str) -> Result<u128, String> {
    erc20_balance_of(REGISTRY_ADDRESS(), holder_hex).await
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
    // Take the low 32 hex chars (16 bytes = u128) — but REJECT rather than
    // silently truncate if the dropped high bytes are non-zero. These words back
    // balance/allowance/supply reads that gate payment + escrow; returning a
    // value mod 2^128 would be a silently-wrong number used in a money decision.
    let tail = if trimmed.len() <= 32 {
        trimmed
    } else {
        let (high, low) = trimmed.split_at(trimmed.len() - 32);
        if high.bytes().any(|b| b != b'0') {
            return Err(format!(
                "value exceeds u128::MAX (high bytes set): 0x{trimmed}"
            ));
        }
        low
    };
    u128::from_str_radix(tail, 16).map_err(|e| e.to_string())
}

// --- Tempo tx submission ---------------------------------------------

/// Default USD-currency TIP-20 used as the sponsor `fee_token` (our $LH is NOT
/// eligible — its TIP-20 `currency()` is "credits", not "USD"). Sourced from
/// the active chain ([`super::chain::active`]); on Moderato this is AlphaUSD.
#[allow(non_snake_case)]
pub fn ALPHA_USD_ADDRESS() -> &'static str {
    super::chain::active().fee_token
}

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
    let fee_token_addr = fee_token.map(parse_eth_address).transpose()?;
    // See `submit_tempo_sponsored` for the retry rationale (stale nonce +
    // basefee both re-read chain state before rebuilding).
    let mut last_err = String::new();
    for attempt in 0..2 {
        let nonce = eth_get_transaction_count(&sender_hex).await?;
        let gas_price = current_gas_price().await?;
        let mut builder = TempoTxBuilder::new(CHAIN_ID())
            .max_priority_fee_per_gas(0)
            .max_fee_per_gas(gas_price * 2)
            .gas_limit(gas_limit)
            .nonce(nonce)
            .calls(calls.clone());
        if let Some(token) = fee_token_addr {
            builder = builder.fee_token(token);
        }
        let raw = sign_self_paid(builder.build(), sender);
        let raw_hex = format!("0x{}", bytes_to_hex(&raw));
        match eth_send_raw_transaction(&raw_hex).await {
            Ok(tx_hash) => {
                wait_for_receipt(&tx_hash).await?;
                return Ok(tx_hash);
            }
            Err(e) if is_retryable_submit(&e) && attempt == 0 => {
                last_err = e;
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err)
}

/// `true` when the active chain is mainnet — the published crate / web bundle
/// holds NO fee_payer key there, so the sponsored fee_payer signature comes from
/// the server relay ([`super::sponsor_relay`]) instead of a local key. Public so
/// the browser's self-assembled sponsored path (`run_sponsored_tempo_call`) can
/// branch the same way the submit chokepoints do.
pub fn is_mainnet() -> bool {
    super::chain::active().chain_id == super::chain::MAINNET.chain_id
}

/// Produce the fee_payer signature over `tx`'s fee_payer hash. Testnet/dev: sign
/// locally with the committed `local_key`. Mainnet: ask the server relay (the
/// crate embeds no mainnet money key) — fail-CLOSED if the relay refuses or is
/// down, never silently self-pay. `sender_sig` is the caller's already-computed
/// sender-half signature (the relay verifies it authorizes this exact intent).
async fn fee_payer_sig_for(
    sender: &SigningKey,
    tx: &crate::tempo_tx::TempoTx,
    sender_addr: &[u8; 20],
    local_key: &SigningKey,
    sender_sig: &[u8; 65],
) -> Result<[u8; 65], String> {
    if is_mainnet() {
        super::sponsor_relay::request_fee_payer_signature(sender, tx, sender_addr, sender_sig).await
    } else {
        Ok(wallet::sign_hash(local_key, &tx.fee_payer_hash(sender_addr)))
    }
}

/// Sign and submit a SPONSORED Tempo tx. `sender` signs the intent
/// (and needs no balance); the gas payer signs as `fee_payer` (needs
/// `fee_token` balance) on testnet, or the server RELAY signs it on mainnet
/// (the crate ships no mainnet key — `fee_payer` is unused there). The chain
/// debits the fee_payer's `fee_token` for the cost; `sender` pays nothing.
pub async fn submit_tempo_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    calls: Vec<crate::tempo_tx::TempoCall>,
    fee_token: &str,
    gas_limit: u128,
) -> Result<String, String> {
    use crate::tempo_tx::TempoTxBuilder;
    let sender_addr = wallet::address(sender);
    let sender_hex = address_to_hex(&sender_addr);
    let fee_token_addr = parse_eth_address(fee_token)?;
    // Build + sign + submit with freshly-read chain state; on a RETRYABLE
    // rejection, re-read and resubmit ONCE:
    // - STALE nonce (a "pending" read that lagged a just-submitted sibling tx —
    //   e.g. the meter→wallet bridge that runs right before the x402 approve).
    //   Without this the colliding tx either timed out (the fabricated-hash
    //   receipt poll — meter-bridge "timed out on-chain" report) or surfaced a
    //   raw node decode/nonce error (the call_agent "$LH approve: …" report).
    // - BASEFEE (T7 dynamic base fee outran the spot `eth_gasPrice` read).
    // The gas price is read INSIDE the loop so the retry reprices the rebuild.
    let mut last_err = String::new();
    for attempt in 0..2 {
        let nonce = eth_get_transaction_count(&sender_hex).await?;
        let gas_price = current_gas_price().await?;
        let tx = TempoTxBuilder::new(CHAIN_ID())
            .max_priority_fee_per_gas(0)
            .max_fee_per_gas(gas_price * 2)
            .gas_limit(gas_limit)
            .nonce(nonce)
            .calls(calls.clone())
            .fee_token(fee_token_addr)
            .sponsored()
            .build();
        let sender_sig = wallet::sign_hash(sender, &tx.sender_hash());
        let fp_sig = fee_payer_sig_for(sender, &tx, &sender_addr, fee_payer, &sender_sig).await?;
        let raw = tx.serialize_signed(&sender_sig, Some(&fp_sig));
        let raw_hex = format!("0x{}", bytes_to_hex(&raw));
        match eth_send_raw_transaction(&raw_hex).await {
            Ok(tx_hash) => {
                wait_for_receipt(&tx_hash).await?;
                return Ok(tx_hash);
            }
            Err(e) if is_retryable_submit(&e) && attempt == 0 => {
                last_err = e;
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err)
}

/// Deploy `init_code` as a NEW contract via a SPONSORED Tempo CREATE tx and
/// return the deployed contract address. `sender` is the deployer (its nonce
/// determines the address, signs the intent, needs no balance); `fee_payer`
/// pays the fees in `fee_token`. The CREATE twin of [`submit_tempo_sponsored`]:
/// same stale-nonce resubmit, but with the `create()` builder flag + an empty
/// `to`, and it returns the receipt's `contractAddress`. This is the library
/// primitive behind the SolidityLite facet-deploy path (CLI `facet deploy`).
pub async fn create_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    init_code: Vec<u8>,
    fee_token: &str,
    gas_limit: u128,
) -> Result<String, String> {
    use crate::tempo_tx::{TempoCall, TempoTxBuilder};
    let sender_addr = wallet::address(sender);
    let sender_hex = address_to_hex(&sender_addr);
    let fee_token_addr = parse_eth_address(fee_token)?;
    let mut last_err = String::new();
    for attempt in 0..2 {
        let nonce = eth_get_transaction_count(&sender_hex).await?;
        let gas_price = current_gas_price().await?;
        let tx = TempoTxBuilder::new(CHAIN_ID())
            .max_priority_fee_per_gas(0)
            .max_fee_per_gas(gas_price * 2)
            .gas_limit(gas_limit)
            .nonce(nonce)
            .call(TempoCall { to: [0u8; 20], value_wei: 0, input: init_code.clone() })
            .fee_token(fee_token_addr)
            .sponsored()
            .create()
            .build();
        let sender_sig = wallet::sign_hash(sender, &tx.sender_hash());
        let fp_sig = fee_payer_sig_for(sender, &tx, &sender_addr, fee_payer, &sender_sig).await?;
        let raw = tx.serialize_signed(&sender_sig, Some(&fp_sig));
        let raw_hex = format!("0x{}", bytes_to_hex(&raw));
        match eth_send_raw_transaction(&raw_hex).await {
            Ok(tx_hash) => {
                wait_for_receipt(&tx_hash).await?;
                return receipt_contract_address(&tx_hash).await;
            }
            Err(e) if is_retryable_submit(&e) && attempt == 0 => {
                last_err = e;
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err)
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
// per facet call: the calldata encoding and the gas budget. The fee side
// (fee_payer + fee_token) is NOT a parameter: it is resolved HERE from
// global state — `sponsor::fee_payer()` (testnet key; unused placeholder on
// mainnet, where the relay signs) + the active chain's fee_token — because
// every caller passed exactly that pair. Custom sponsors go through the
// explicit [`submit_tempo_sponsored`] / [`create_sponsored`] primitives.

/// The resolved default `(fee_payer, fee_token)` pair every sponsored
/// skeleton submits with.
pub(crate) fn default_fee() -> Result<(SigningKey, &'static str), String> {
    Ok((super::sponsor::fee_payer()?, ALPHA_USD_ADDRESS()))
}

/// Sponsored submit of a prepared multi-call batch with the default fee pair —
/// the batch-shaped sibling of [`sponsored_call_to`] (TBA execute batches,
/// remove+unlink pairs, multi-burn releases; public for the CLI's hand-built
/// setMetadata/diamondCut batches). Custom sponsors → [`submit_tempo_sponsored`].
pub async fn sponsored_batch(
    sender: &SigningKey,
    calls: Vec<crate::tempo_tx::TempoCall>,
    gas_limit: u128,
) -> Result<String, String> {
    let (fee_payer, fee_token) = default_fee()?;
    submit_tempo_sponsored(sender, &fee_payer, calls, fee_token, gas_limit).await
}

/// ONE sponsored Tempo call to `to_hex` (zero value). The shared body of
/// every single-call `*_sponsored` wrapper; non-diamond targets ($LH token
/// approve/transfer, TBA execute) pass their own address.
pub(crate) async fn sponsored_call_to(
    sender: &SigningKey,
    to_hex: &str,
    input: Vec<u8>,
    gas_limit: u128,
) -> Result<String, String> {
    let call = crate::tempo_tx::TempoCall {
        to: parse_eth_address(to_hex)?,
        value_wei: 0,
        input,
    };
    let (fee_payer, fee_token) = default_fee()?;
    submit_tempo_sponsored(sender, &fee_payer, vec![call], fee_token, gas_limit).await
}

/// ONE sponsored call to the registry diamond — the most common wrapper
/// shape (claimDaily / redeem / cancelJob / acceptInvite / attest / vote /
/// announce / submitFeedback / …).
pub(crate) async fn sponsored_diamond_call(
    sender: &SigningKey,
    input: Vec<u8>,
    gas_limit: u128,
) -> Result<String, String> {
    sponsored_call_to(sender, REGISTRY_ADDRESS(), input, gas_limit).await
}

/// `$LH.approve(diamond, amount)` + a diamond call batched in ONE sponsored
/// tx — the approve→`transferFrom`-pull ESCROW shape shared by scheduleJob /
/// createInvite / postBounty / fundGuild / depositCredits and the cost-gated
/// register/registerMain/openSession paths.
pub(crate) async fn sponsored_escrow_diamond_call(
    sender: &SigningKey,
    amount_wei: u128,
    input: Vec<u8>,
    gas_limit: u128,
) -> Result<String, String> {
    sponsored_escrow_diamond_call_bridged(sender, amount_wei, input, gas_limit, 0).await
}

/// [`sponsored_escrow_diamond_call`] with the METER AUTO-BRIDGE: when
/// `bridge_wei > 0` a `withdrawCredits(bridge_wei)` call is PREPENDED to the
/// SAME atomic Tempo tx (0x76 carries a calls array), pulling a wallet
/// shortfall back out of the caller's unspent chat-meter credits before the
/// approve→escrow pair runs — so "1057 $LH in the meter but the wallet is
/// short" no longer blocks an escrow (on-chain feedback #63). Gas is bumped
/// 150k when bridging (the same rider budget `send_lh` uses).
pub(crate) async fn sponsored_escrow_diamond_call_bridged(
    sender: &SigningKey,
    amount_wei: u128,
    input: Vec<u8>,
    gas_limit: u128,
    bridge_wei: u128,
) -> Result<String, String> {
    let calls = escrow_call_batch(amount_wei, input, bridge_wei)?;
    let gas = if bridge_wei > 0 { gas_limit + 150_000 } else { gas_limit };
    let (fee_payer, fee_token) = default_fee()?;
    submit_tempo_sponsored(sender, &fee_payer, calls, fee_token, gas).await
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
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS())?;
    let token_addr = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS())?;
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
        let diamond = parse_eth_address(REGISTRY_ADDRESS()).unwrap();
        let token = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS()).unwrap();
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

    /// The sponsored-tx gas-price guard: an in-range price passes through; a
    /// price above the ceiling (an inflated/hostile RPC report) is REFUSED, so
    /// no builder can be tricked into authorizing an absurd `gas_limit * price`.
    #[test]
    fn clamp_gas_price_passes_sane_and_refuses_inflated() {
        assert_eq!(clamp_gas_price(1_000_000_000), Ok(1_000_000_000)); // ~1 gwei
        assert_eq!(clamp_gas_price(MAX_GAS_PRICE_WEI), Ok(MAX_GAS_PRICE_WEI)); // ceiling inclusive
        assert!(clamp_gas_price(MAX_GAS_PRICE_WEI + 1).is_err());
        assert!(clamp_gas_price(u128::MAX).is_err());
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
        // High bytes set (value > u128::MAX) → ERROR, never silent truncation.
        let high = format!("{:032x}", 1u8);
        let overflow = format!("0x{}{}", high, "0".repeat(32));
        assert!(decode_u256_as_u128(&overflow).is_err());
    }
}
