use k256::ecdsa::SigningKey;

use crate::wallet;

use super::*;

// --- MAIN identity helpers -------------------------------------------

/// `eth_call mainOf(holder)` — returns the tokenId the holder has
/// registered as their MAIN, or 0 if none. Used by the bundle to
/// decide whether to auto-register on first claim and to badge the
/// MAIN entry in the apex agents list.
pub async fn main_of(holder_hex: &str) -> Result<u64, String> {
    let holder_bytes = hex_to_bytes(holder_hex)?;
    if holder_bytes.len() != 20 {
        return Err(format!("holder must be 20 bytes, got {}", holder_bytes.len()));
    }
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(&holder_bytes);
    let result = read_view(selector("mainOf(address)"), &[padded]).await?;
    decode_u256_as_u64(&result)
}

// `register_main` (the legacy SELF-PAID variant) was removed as dead code —
// the sponsored counterpart below is the only live MAIN-registration path.

/// Sponsored `MainIdentityFacet.registerMain(tokenId)`. `sender` (the holder
/// authorizing the MAIN change) signs the intent and needs zero balance;
/// `fee_payer` pays the gas in `fee_token` (typically AlphaUSD). Use this
/// from bundle paths where the user shouldn't need to hold native gas
/// to update their MAIN.
///
/// When `main_cost()` is non-zero on-chain, prepends a
/// `credits.approve(diamond, cost)` call so `registerMain`'s internal
/// `transferFrom` has the allowance it needs. User pays the cost in
/// LH from their balance; the credits land at the diamond's treasury.
pub async fn register_main_sponsored(
    sender: &SigningKey,
    token_id: u64,
) -> Result<String, String> {
    let cost = main_cost().await.unwrap_or(0);
    let input = encode_register_main(token_id);
    // registerMain inner: storage write + event (~50k). +approve
    // (~50k) + transferFrom (~30k) when cost > 0. + ~275k Tempo
    // sponsorship. 700k gives headroom either way.
    if cost > 0 {
        sponsored_escrow_diamond_call(sender, cost, input, 700_000).await
    } else {
        sponsored_diamond_call(sender, input, 700_000).await
    }
}

pub(crate) fn encode_register_main(token_id: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector("registerMain(uint256)"));
    data.extend_from_slice(&u256_be(token_id as u128));
    data
}

// --- MultiSignerAccount (TBA execute) ---------------------------------

/// Execute a batch of calls AS the TBA (the asset owner), signed by a
/// local key authorized on that TBA — the consolidation owner-action
/// path. Batches `createTokenBoundAccount(token_id)` (idempotent) + one
/// `TBA.execute(target, 0, data)` per entry. Sponsored.
pub async fn tba_execute_batch_sponsored(
    signer: &SigningKey,
    token_id: u64,
    tba_hex: &str,
    targets: &[([u8; 20], Vec<u8>)],
    gas_limit: u128,
) -> Result<String, String> {
    let diamond = parse_eth_address(REGISTRY_ADDRESS())?;
    let tba = parse_eth_address(tba_hex)?;
    let mut calls = Vec::with_capacity(targets.len() + 1);
    calls.push(crate::tempo_tx::TempoCall {
        to: diamond,
        value_wei: 0,
        input: encode_create_tba(token_id),
    });
    for (target, data) in targets {
        calls.push(crate::tempo_tx::TempoCall {
            to: tba,
            value_wei: 0,
            input: encode_tba_execute(target, 0, data),
        });
    }
    sponsored_batch(signer, calls, gas_limit).await
}

pub(crate) fn encode_release_name(token_id: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32);
    out.extend_from_slice(&selector("releaseName(uint256)"));
    out.extend_from_slice(&u256_be(token_id as u128));
    out
}

/// Public `releaseName(tokenId)` calldata — for the iframe-signed agent
/// path (the owner signs the sender hash via the apex signer).
pub fn release_name_calldata(token_id: u64) -> Vec<u8> {
    encode_release_name(token_id)
}

/// Public `register(string)` calldata as raw bytes — for the iframe-signed
/// agent batch path (`batch_create_subdomains`), where many register calls
/// are packed into ONE sponsored Tempo tx. Same ABI as the single claim.
/// NOTE: this is a bare `register` with no `approve` — by design, because the
/// CALLER prepends the allowance. ⛔ Do not "fix" that by adding an approve
/// here: `app::events::subdomains` reads `registration_cost()` and, when it is
/// non-zero, inserts ONE cumulative `approve(diamond, cost × n)` at index 0 of
/// the batch (each register's `transferFrom` decrements it). The old note
/// claimed registration was "FREE, current testnet config" and that the batch
/// "deliberately does not" approve — both stale: mainnet charges
/// `registrationCost()`, and the batch has approved since paid claims landed.
pub fn register_calldata(name: &str) -> Vec<u8> {
    // `encode_register` returns 0x-hex; strip it back to bytes. Infallible
    // for our own well-formed output, so a decode error degrades to empty
    // calldata (the tx reverts harmlessly rather than panicking in wasm).
    hex_to_bytes(&encode_register(name)).unwrap_or_default()
}

/// `$LH.approve(diamond, amount)` as a ready [`crate::tempo_tx::TempoCall`].
/// Prepend ONE of these to a batch of `register` calls when
/// `registrationCost()` is non-zero: the allowance is CUMULATIVE (each
/// register's `transferFrom` decrements it), so `cost × names` covers the
/// whole batch. Without it a paid batch register reverts on the pull.
pub fn approve_credits_call(amount_wei: u128) -> Result<crate::tempo_tx::TempoCall, String> {
    let diamond = parse_eth_address(REGISTRY_ADDRESS())?;
    let token = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS())?;
    Ok(crate::tempo_tx::TempoCall {
        to: token,
        value_wei: 0,
        input: encode_approve(&diamond, amount_wei),
    })
}

/// `fundGuild(guildId, amount)` to the diamond as a ready [`crate::tempo_tx::TempoCall`].
/// Pair with [`approve_credits_call`] in a TBA batch so an agent's token-bound
/// account can TITHE its own earnings up to a guild treasury (revenue→treasury).
pub fn fund_guild_call(guild_id: u64, amount_wei: u128) -> Result<crate::tempo_tx::TempoCall, String> {
    let diamond = parse_eth_address(REGISTRY_ADDRESS())?;
    Ok(crate::tempo_tx::TempoCall {
        to: diamond,
        value_wei: 0,
        input: super::encode_fund_guild(guild_id, amount_wei),
    })
}

/// Release (recycle) a subdomain — burn the NFT + free the name — via a
/// sponsored tx. `sender` must own the token. DESTRUCTIVE: the UI/tool
/// MUST require typed confirmation before calling this. Refuses the MAIN
/// on-chain.
pub async fn release_name_sponsored(
    sender: &SigningKey,
    token_id: u64,
) -> Result<String, String> {
    // 1M, not a flat 400k: a name burn runs ~375-425k all-in (cold-slot clears
    // + ~275k sponsorship), so 400k OOG-reverted while the UI reported success.
    // Over-budget is free — the sponsor pays gas USED, not the limit.
    sponsored_diamond_call(sender, encode_release_name(token_id), 1_000_000)
        .await
}

// --- Registration cost (LocalharnessRegistryFacet on the diamond) ---

/// `eth_call mainCost()` — the LH amount the diamond's `registerMain`
/// pulls from the caller via transferFrom on every MAIN change. Zero
/// means the gate is off.
pub async fn main_cost() -> Result<u128, String> {
    let result = read_view(selector("mainCost()"), &[]).await?;
    decode_u256_as_u128(&result)
}

/// `eth_call treasuryBalance()` — total LH the diamond holds. Reads
/// the credits token's `balanceOf(diamond)`. Useful for surfacing
/// "X LH collected from registrations" in admin UIs.
pub async fn treasury_balance() -> Result<u128, String> {
    let result = read_view(selector("treasuryBalance()"), &[]).await?;
    decode_u256_as_u128(&result)
}

/// `eth_call registrationCost()` — the LH amount (in token wei, 18
/// decimals) the diamond's `register(name)` will pull from the sender
/// via transferFrom. Zero means the cost gate is disabled.
pub async fn registration_cost() -> Result<u128, String> {
    let result = read_view(selector("registrationCost()"), &[]).await?;
    decode_u256_as_u128(&result)
}

/// Encode `approve(spender, amount)` calldata for an ERC-20 token.
pub(crate) fn encode_approve(spender: &[u8; 20], amount_wei: u128) -> Vec<u8> {
    encode_addr_amount("approve(address,uint256)", spender, amount_wei)
}

/// ERC-20 `transfer(to, amount)` calldata — same shape as `encode_approve`
/// with the `transfer` selector.
pub(crate) fn encode_transfer(to: &[u8; 20], amount_wei: u128) -> Vec<u8> {
    encode_addr_amount("transfer(address,uint256)", to, amount_wei)
}

/// `fn(address,uint256)` calldata — `selector | addr(32) | amount(32)`. THE
/// shared shape behind every approve/transfer-class encoder (`approve` /
/// `transfer`), keyed only on the selector.
pub(crate) fn encode_addr_amount(signature: &str, addr: &[u8; 20], amount_wei: u128) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 64);
    out.extend_from_slice(&selector(signature));
    out.extend_from_slice(&addr_word(addr));
    out.extend_from_slice(&u256_be(amount_wei));
    out
}


/// Make a token-bound account EXECUTE an arbitrary call — the headless /
/// agent equivalent of the browser act-panel's "send" button. Fires ONE
/// sponsored Tempo tx calling `tba.execute(to, value, data, 0)` on the
/// `MultiSignerAccount` at `tba_addr` (operation 0 = CALL; the contract
/// rejects any other). With empty `data` and `value_wei = 0` this is a no-op
/// call; pass an ABI-encoded inner calldata (e.g. an ERC-20 `transfer`, a
/// guild `castVote`) to drive a real action — the TBA becomes the `msg.sender`
/// of the inner call, so an agent's wallet (its TBA) can vote in a parent DAO,
/// pay, or call any contract under its OWN identity.
///
/// Authorization is enforced ON-CHAIN by `MultiSignerAccount.execute`, which
/// reverts unless `msg.sender` (here `owner_signer`) is the NFT holder of the
/// owning token or an enrolled additional signer. This helper just signs as
/// that owner; the contract is the gate. `fee_payer` (the bundle sponsor) pays
/// the AlphaUSD fee so the owner holds no gas token.
///
/// The TBA must already be deployed (a counterfactual address has no code, so
/// `execute` would revert). Callers deploy first via
/// [`create_token_bound_account_sponsored`] — the CLI does this when
/// [`is_contract_deployed`] is false. Flat (address-keyed, no token id) — the
/// low-level primitive the [`tba_send_lh_sponsored`] wrapper builds on.
// Discrete params are the wire fields (owner+sponsor signers, TBA, target,
// value, inner calldata, fee token); bundling into a struct just moves noise.
#[allow(clippy::too_many_arguments)]
pub async fn tba_execute_call_sponsored(
    owner_signer: &SigningKey,
    tba_addr: &str,
    to: &str,
    value_wei: u128,
    data: &[u8],
) -> Result<String, String> {
    let target = parse_eth_address(to)?;
    // execute (~30k) + the inner call + Tempo sponsorship (~275k). The inner
    // call varies WIDELY: an ERC-20 transfer ~52k, a vote ~80k, but a GUILD
    // JOIN (`acceptGuildInvite` — cold roster + `guildsOf` enumerable pushes +
    // role SSTORE) is ~1.3M (live: a 600k cap OOG'd it — the receipt said
    // reverted while `cast run` replay falsely showed success, the classic
    // replay-vs-real-exec gap). 2M comfortably covers a guild-join-class inner
    // call with headroom; the sponsor is billed on gas USED, not the limit, so
    // the headroom is free. The cold first-deploy cost lives in
    // create_token_bound_account_sponsored (a separate tx).
    sponsored_call_to(
        owner_signer,
        tba_addr,
        encode_tba_execute(&target, value_wei, data),
        2_000_000,
    )
    .await
}

/// Sponsored `createTokenBoundAccount(token_id)` — deploys the
/// `MultiSignerAccount` for `token_id`'s deterministic TBA address via the
/// TbaFacet. Idempotent (a no-op if already deployed) and permissionless to
/// CALL, but only useful for a token the caller controls. Needed before the
/// TBA can `execute` / `addSigner` (a counterfactual address has no code). The
/// cold deploy is gas-hungry — CREATE2 of the full account bytecode is
/// ~742k live-measured — so the limit covers that plus Tempo sponsorship.
pub async fn create_token_bound_account_sponsored(
    owner_signer: &SigningKey,
    token_id: u64,
) -> Result<String, String> {
    sponsored_diamond_call(
        owner_signer,
        encode_create_tba(token_id),
        1_200_000,
    )
    .await
}

/// Make a TBA send `$LH` — `execute($LH_token, 0, transfer(recipient, amount))`
/// via [`tba_execute_call_sponsored`]. Flat (address-keyed, deploy NOT
/// batched); the headless CLI calls
/// [`create_token_bound_account_sponsored`] first when the TBA isn't deployed
/// yet, so this assumes a live TBA. The TBA must hold at least `amount_wei`.
pub async fn tba_send_lh_sponsored(
    owner_signer: &SigningKey,
    tba_addr: &str,
    recipient_hex: &str,
    amount_wei: u128,
) -> Result<String, String> {
    let recipient = parse_eth_address(recipient_hex)?;
    let transfer_data = encode_erc20_transfer(&recipient, amount_wei);
    tba_execute_call_sponsored(
        owner_signer,
        tba_addr,
        LOCALHARNESS_TOKEN_ADDRESS(),
        0,
        &transfer_data,
    )
    .await
}

/// ABI-encode an ERC-20 `transfer(address,uint256)` calldata. The inner
/// payload for a `$LH`-transfer-via-TBA (`execute($LH, 0, transfer(to, amt))`).
pub(crate) fn encode_erc20_transfer(recipient: &[u8; 20], amount_wei: u128) -> Vec<u8> {
    encode_addr_amount("transfer(address,uint256)", recipient, amount_wei)
}

pub(crate) fn encode_tba_execute(target: &[u8; 20], value_wei: u128, data: &[u8]) -> Vec<u8> {
    // execute(address,uint256,bytes,uint8) — ABI:
    //   selector(4) | target(32) | value(32) | dataOffset(32, =0x80) |
    //   operation(32, =0) | dataLength(32) | dataPadded
    let sel = selector("execute(address,uint256,bytes,uint8)");
    let mut target_padded = [0u8; 32];
    target_padded[12..].copy_from_slice(target);
    let data_len = data.len();
    let padded_len = data_len.div_ceil(32) * 32;
    // Static head = target(32) + value(32) + offset(32) + operation(32) = 128
    let data_offset: u128 = 0x80;

    let mut out = Vec::with_capacity(4 + 128 + 32 + padded_len);
    out.extend_from_slice(&sel);
    out.extend_from_slice(&target_padded);
    out.extend_from_slice(&u256_be(value_wei));
    out.extend_from_slice(&u256_be(data_offset));
    out.extend_from_slice(&u256_be(0)); // operation = 0 (CALL)
    out.extend_from_slice(&u256_be(data_len as u128));
    out.extend_from_slice(data);
    out.resize(out.len() + (padded_len - data_len), 0);
    out
}

pub(crate) fn encode_create_tba(token_id: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector("createTokenBoundAccount(uint256)"));
    data.extend_from_slice(&u256_be(token_id as u128));
    data
}

// `claim_and_maybe_set_main` (the legacy SELF-PAID first-claim) was removed
// as dead code together with `claim_name` — the sponsored flow below is the
// only live first-claim path.

/// First-claim convenience over Tempo's sponsored-tx flow: register `name`
/// on-chain, then IF the caller has no MAIN registered yet, set the
/// newly-minted token as their MAIN in a second tx (errors on the MAIN leg
/// are logged and swallowed — the claim is what matters for correctness).
/// The `sender` signs the intent (and needs zero balance);
/// `fee_payer` signs to cover gas in `fee_token` (typically AlphaUSD).
/// This is what the bundle uses for first-claim onboarding — the user
/// who just visited the page can claim a subdomain without holding
/// any tokens.
///
/// If the diamond's `registrationCost()` is non-zero, this batches a
/// `LocalharnessCredits.approve(diamond, cost)` call BEFORE register
/// in the same Tempo tx — register then pulls the credits via
/// `transferFrom` inside its own body. User pays the cost in LH from
/// their balance; the credits accumulate at the diamond's address.
pub async fn claim_and_maybe_set_main_sponsored(
    sender: &SigningKey,
    name: &str,
) -> Result<String, String> {
    // Routable-label guard at the single first-claim chokepoint (juno-qa): the
    // registry will happily mint a >63-char / bad-char label, but the DNS
    // gateway then silently chokes on it — a zombie agent the owner already
    // paid (sponsored) gas to create. Reject BEFORE the tx. Every client mint
    // path (CLI create/publish, the apex form, and the signer-iframe used by
    // the browser chat tools + tenant claim) funnels through here, so this is
    // the belt-and-suspenders backstop for the per-call-site checks.
    if !crate::subdomain::is_valid_subdomain_label(name) {
        return Err(format!(
            "'{name}' is not a routable subdomain label (1-63 chars of a-z, 0-9, \
             hyphen; no leading/trailing hyphen)"
        ));
    }
    let cost = registration_cost().await.unwrap_or(0);
    let register_input = hex_to_bytes(&encode_register(name))?;

    // `eth_estimateGas` on `register(name)` against the live diamond
    // reports ~1.32M gas for the inner call (ERC-721 mint + storage
    // writes + counterfactual TBA address derivation). Sponsorship
    // (fee_payer recovery + AlphaUSD transfer) adds ~275k. The
    // approve+transferFrom pair adds ~80k. Budget 2.2M for
    // headroom; sponsor pays in AlphaUSD and only consumed gas is
    // debited, so over-budgeting is free.
    let tx_hash = if cost > 0 {
        // `register`'s `transferFrom` pulls the cost from the sender's WALLET
        // ($LH token balance). A fiat buyer's $LH sits in the METER (`creditOf`),
        // not the wallet, so compute the wallet shortfall and PREPEND a
        // `withdrawCredits(shortfall)` meter→wallet bridge into the SAME atomic
        // tx (the exact pattern every escrow path — bounty/guild/party — uses).
        // With `fiatLockSecs = 0` the meter credits are immediately withdrawable,
        // so the bridge can cover the cost; the whole batch reverts atomically if
        // neither pot can. bridge_wei = 0 (wallet already covers it) falls back to
        // the plain approve→register batch.
        let sender_hex = address_to_hex(&wallet::address(sender));
        let wallet_bal = token_balance_of(&sender_hex).await.unwrap_or(0);
        let bridge_wei = cost.saturating_sub(wallet_bal);
        sponsored_escrow_diamond_call_bridged(
            sender, cost, register_input, 2_200_000, bridge_wei,
        )
        .await?
    } else {
        sponsored_diamond_call(sender, register_input, 2_200_000).await?
    };

    // After register, fetch the new tokenId and set MAIN if none.
    let sender_addr = address_to_hex(&wallet::address(sender));
    if let Ok(0) = main_of(&sender_addr).await {
        if let Ok(Status::Taken { agent_id }) = check_name(name).await {
            if let Err(err) =
                register_main_sponsored(sender, agent_id).await
            {
                log_main_warning(&err);
            }
        }
    }
    Ok(tx_hash)
}

/// SELF-PAID twin of [`claim_and_maybe_set_main_sponsored`], MINUS the MAIN leg:
/// register `name` with the `sender` signing AND paying its own gas in `fee_token`
/// (a USD-currency TIP-20 it holds, e.g. USDC.e) — NO relay, NO separate fee_payer.
/// The founding path for a WALLET-FUNDED owner the mainnet keyless relay refuses to
/// sponsor (`LH_RELAY_FUNDED`). Batches the SAME `approve(diamond, cost)` +
/// `register(name)` the sponsored path does; the 1-`$LH` `registrationCost()` is
/// pulled from the sender's WALLET by `register`'s internal `transferFrom`.
///
/// Deliberately does NOT set the owner's MAIN — role subdomains registered while
/// founding a company are owned NAMES, not the owner's primary identity, so we
/// never auto-promote one of them to `mainOf(owner)`.
pub async fn claim_name_self_paid(
    sender: &SigningKey,
    name: &str,
) -> Result<String, String> {
    if !crate::subdomain::is_valid_subdomain_label(name) {
        return Err(format!(
            "'{name}' is not a routable subdomain label (1-63 chars of a-z, 0-9, \
             hyphen; no leading/trailing hyphen)"
        ));
    }
    let cost = registration_cost().await.unwrap_or(0);
    let register_input = hex_to_bytes(&encode_register(name))?;
    let calls = if cost > 0 {
        escrow_call_batch(cost, register_input, 0)?
    } else {
        vec![crate::tempo_tx::TempoCall {
            to: parse_eth_address(REGISTRY_ADDRESS())?,
            value_wei: 0,
            input: register_input,
        }]
    };
    // 4M covers the batched `approve` (~255k) + AA overhead + the `register` mint
    // (~1.3M inner on testnet, more on mainnet) with margin — the self-paid twin of
    // the sponsored 2.2M budget, bumped so the mint isn't left under-gassed on
    // mainnet after the approve/overhead (same class as the createGuild OOG).
    // Self-pay bills gas USED, so the headroom is free.
    submit_tempo_self_paid(sender, calls, Some(ALPHA_USD_ADDRESS()), 4_000_000).await
}


#[cfg(test)]
mod tests {
    use super::*;

    // --- Calldata-encoder layout guards (network-free). A wrong ABI offset
    // here would send $LH / NFTs to the wrong place, so pin the layout. ---

    #[test]
    fn release_name_calldata_layout() {
        let cd = encode_release_name(7);
        assert_eq!(&cd[0..4], &selector("releaseName(uint256)"));
        assert_eq!(cd.len(), 36);
        assert_eq!(u64::from_be_bytes(cd[28..36].try_into().unwrap()), 7);
    }

    /// ERC-20 `transfer(address,uint256)` — the `send_lh` payload. A wrong
    /// selector or mis-padded address word sends `$LH` to the wrong account.
    /// Tests an address with the HIGH bit of every byte set, so a left/right
    /// padding mistake (top 12 bytes vs low 20) would be caught.
    #[test]
    fn transfer_calldata_layout() {
        let to = [0xFFu8; 20];
        let amount = 1_500_000_000_000_000_000u128; // 1.5 $LH
        let cd = encode_transfer(&to, amount);
        // keccak256("transfer(address,uint256)")[0..4] = 0xa9059cbb.
        assert_eq!(&cd[0..4], &[0xa9, 0x05, 0x9c, 0xbb]);
        assert_eq!(cd.len(), 4 + 64);
        // Address right-aligned in word 0: top 12 bytes ZERO, low 20 = `to`.
        assert_eq!(&cd[4..4 + 12], &[0u8; 12]);
        assert_eq!(&cd[4 + 12..4 + 32], &to);
        // Amount as a full uint256 in word 1 (16 high bytes zero, low 16 = u128).
        assert_eq!(&cd[4 + 32..4 + 48], &[0u8; 16]);
        assert_eq!(
            u128::from_be_bytes(cd[4 + 48..4 + 64].try_into().unwrap()),
            amount
        );
    }

    /// ERC-20 `approve(address,uint256)` with `u128::MAX` (the one-time
    /// "approve forever" the mcp-call path uses). The amount must land as
    /// 2^128-1 in the LOW 16 bytes of word 1, NOT wrap or shift.
    #[test]
    fn approve_calldata_layout_max_amount() {
        let spender = [0xABu8; 20];
        let cd = encode_approve(&spender, u128::MAX);
        // keccak256("approve(address,uint256)")[0..4] = 0x095ea7b3.
        assert_eq!(&cd[0..4], &[0x09, 0x5e, 0xa7, 0xb3]);
        assert_eq!(cd.len(), 4 + 64);
        assert_eq!(&cd[4 + 12..4 + 32], &spender);
        // High 16 bytes of the amount word are zero; low 16 are all 0xFF.
        assert_eq!(&cd[4 + 32..4 + 48], &[0u8; 16]);
        assert_eq!(&cd[4 + 48..4 + 64], &[0xFFu8; 16]);
    }

    /// Pin the `MultiSignerAccount.execute(address,uint256,bytes,uint8)`
    /// calldata layout — selector + the static head (target, value, data
    /// offset, operation) + the dynamic `bytes data` (length word + the
    /// 32-byte-padded body). This is the wire shape the TBA EXECUTE primitive
    /// drives; if it drifts, every headless TBA action reverts.
    #[test]
    fn tba_execute_calldata_layout() {
        let target = [0xABu8; 20];
        // 5-byte inner payload so we exercise the 32-byte padding.
        let data = [0x01, 0x02, 0x03, 0x04, 0x05];
        let value: u128 = 0x1234;
        let cd = encode_tba_execute(&target, value, &data);

        // Selector for the full 4-arg signature (CALL-only MultiSignerAccount).
        assert_eq!(&cd[0..4], &selector("execute(address,uint256,bytes,uint8)"));
        // Static head: target right-aligned in word 0.
        assert!(cd[4..16].iter().all(|&b| b == 0)); // left-pad zeros
        assert_eq!(&cd[16..36], &target); // 20-byte address in the low bytes
        // value in word 1.
        assert_eq!(&cd[36..68], &u256_be(value));
        // data offset in word 2 = 0x80 (static head is 4 words = 128 bytes).
        assert_eq!(&cd[68..100], &u256_be(0x80));
        // operation in word 3 = 0 (CALL — the contract reverts on anything else).
        assert!(cd[100..132].iter().all(|&b| b == 0));
        // dynamic region at offset 4(selector)+0x80 = 132: length word then body.
        assert_eq!(&cd[132..164], &u256_be(data.len() as u128));
        assert_eq!(&cd[164..164 + data.len()], &data);
        // The body is padded to a 32-byte boundary with zeros.
        assert_eq!(cd.len(), 164 + 32); // 5 bytes → one padded word
        assert!(cd[164 + data.len()..].iter().all(|&b| b == 0));

        // Empty data degenerates cleanly: head only, length 0, no body.
        let empty = encode_tba_execute(&target, 0, &[]);
        assert_eq!(empty.len(), 4 + 128 + 32); // selector + head + zero-length word
        assert_eq!(&empty[132..164], &u256_be(0));
    }

    /// Pin the `$LH`-transfer-via-TBA encoding: the inner payload is an ERC-20
    /// `transfer(address,uint256)` and `encode_tba_execute` wraps it as
    /// `execute($LH, 0, transfer(to, amt), 0)`. Confirms the nested calldata is
    /// byte-exact (offsets shift since the inner data is now 68 bytes).
    #[test]
    fn tba_transfer_lh_calldata_layout() {
        let recipient = [0xCDu8; 20];
        let amount: u128 = 1_000_000_000_000_000_000; // 1 $LH

        // Inner ERC-20 transfer calldata.
        let inner = encode_erc20_transfer(&recipient, amount);
        assert_eq!(&inner[0..4], &selector("transfer(address,uint256)"));
        assert_eq!(&inner[16..36], &recipient); // recipient right-aligned
        assert_eq!(&inner[36..68], &u256_be(amount));
        assert_eq!(inner.len(), 4 + 32 + 32); // selector + 2 words

        // Wrapped as a TBA execute to the $LH token, value 0.
        let token = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS()).unwrap();
        let cd = encode_tba_execute(&token, 0, &inner);
        assert_eq!(&cd[0..4], &selector("execute(address,uint256,bytes,uint8)"));
        assert_eq!(&cd[16..36], &token); // execute target = $LH token
        assert_eq!(&cd[36..68], &u256_be(0)); // value = 0 (ERC-20 carries amount)
        assert_eq!(&cd[68..100], &u256_be(0x80)); // data offset
        assert!(cd[100..132].iter().all(|&b| b == 0)); // operation CALL
        // dynamic: length = 68 (the inner transfer calldata), then the body.
        assert_eq!(&cd[132..164], &u256_be(inner.len() as u128));
        assert_eq!(&cd[164..164 + inner.len()], inner.as_slice());
        // 68 bytes pads to 96 (3 words); total = selector + head(128) + len(32) + 96.
        assert_eq!(cd.len(), 4 + 128 + 32 + 96);
    }

}
