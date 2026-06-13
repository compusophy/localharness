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
    fee_payer: &SigningKey,
    token_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    let cost = main_cost().await.unwrap_or(0);
    let input = encode_register_main(token_id);
    // registerMain inner: storage write + event (~50k). +approve
    // (~50k) + transferFrom (~30k) when cost > 0. + ~275k Tempo
    // sponsorship. 700k gives headroom either way.
    if cost > 0 {
        sponsored_escrow_diamond_call(sender, fee_payer, cost, input, fee_token, 700_000).await
    } else {
        sponsored_diamond_call(sender, fee_payer, input, fee_token, 700_000).await
    }
}

pub(crate) fn encode_register_main(token_id: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector("registerMain(uint256)"));
    data.extend_from_slice(&u256_be(token_id as u128));
    data
}

// --- MultiSignerAccount (TBA add/remove device signer) ---------------

/// `eth_call isAuthorizedSigner(signer)` on a TBA. Returns true if
/// `signer` is recognized by the TBA's MultiSignerAccount impl —
/// either as the NFT holder (implicit) or as a previously-added device.
pub async fn is_authorized_signer(tba_address: &str, signer_hex: &str) -> Result<bool, String> {
    let signer_bytes = hex_to_bytes(signer_hex)?;
    if signer_bytes.len() != 20 {
        return Err(format!("signer must be 20 bytes, got {}", signer_bytes.len()));
    }
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(&signer_bytes);
    let calldata = encode_call_hex(selector("isAuthorizedSigner(address)"), &[padded]);
    let result_hex = eth_call(tba_address, &calldata).await?;
    let trimmed = result_hex.trim().trim_start_matches("0x");
    Ok(trimmed.chars().last().map(|c| c == '1').unwrap_or(false))
}

/// Read `token()` on an ERC-6551 account → its owning tokenId (the 3rd
/// returned word: chainId, tokenContract, tokenId). Lets us route owner
/// actions through a TBA when we only know the TBA address.
pub async fn tba_token_id_of(tba_hex: &str) -> Result<u64, String> {
    let calldata = encode_call_hex(selector("token()"), &[]);
    let result = eth_call(tba_hex, &calldata).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 96 {
        return Err("token(): short response".into());
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[88..96]); // low 8 bytes of the tokenId word
    Ok(u64::from_be_bytes(buf))
}

/// Execute a batch of calls AS the TBA (the asset owner), signed by a
/// local key authorized on that TBA — the consolidation owner-action
/// path. Batches `createTokenBoundAccount(token_id)` (idempotent) + one
/// `TBA.execute(target, 0, data)` per entry. Sponsored.
pub async fn tba_execute_batch_sponsored(
    signer: &SigningKey,
    fee_payer: &SigningKey,
    token_id: u64,
    tba_hex: &str,
    targets: &[([u8; 20], Vec<u8>)],
    fee_token: &str,
    gas_limit: u128,
) -> Result<String, String> {
    let diamond = parse_eth_address(REGISTRY_ADDRESS)?;
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
    submit_tempo_sponsored(signer, fee_payer, calls, fee_token, gas_limit).await
}

/// Read `devicesOf(mainId)` — the identity's linked devices, from the
/// on-chain enumerable index in ONE call (no log scraping). Returns
/// lowercase `0x…` addresses.
pub async fn devices_of(main_id: u64) -> Result<Vec<String>, String> {
    let result = read_view(selector("devicesOf(uint256)"), &[u256_be(main_id as u128)]).await?;
    let bytes = hex_to_bytes(&result)?;
    // ABI dynamic address[]: [offset(32)][len(32)][addr0(32)]... — shared decode.
    Ok(decode_address_array(&bytes))
}

pub(crate) fn encode_unlink_device(main_id: u64, device: &[u8; 20]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 64);
    out.extend_from_slice(&selector("unlinkDevice(uint256,address)"));
    out.extend_from_slice(&u256_be(main_id as u128));
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(device);
    out.extend_from_slice(&padded);
    out
}

pub(crate) fn encode_erc721_transfer_from(from: &[u8; 20], to: &[u8; 20], token_id: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 96);
    out.extend_from_slice(&selector("transferFrom(address,address,uint256)"));
    out.extend_from_slice(&addr_word(from));
    out.extend_from_slice(&addr_word(to));
    out.extend_from_slice(&u256_be(token_id as u128));
    out
}

/// CONSOLIDATION: transfer every `token_id` (subdomains owned by `owner`)
/// into the MAIN's TBA, so one account owns them all and every linked
/// device controls them. `owner` signs (it currently holds the NFTs);
/// sponsored. One-way by design — move back later via TBA.execute.
pub async fn consolidate_into_main_sponsored(
    owner: &SigningKey,
    fee_payer: &SigningKey,
    main_tba_hex: &str,
    token_ids: &[u64],
    fee_token: &str,
) -> Result<String, String> {
    if token_ids.is_empty() {
        return Err("no subdomains to consolidate".into());
    }
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let to = parse_eth_address(main_tba_hex)?;
    let from = wallet::address(owner);
    let calls: Vec<_> = token_ids
        .iter()
        .map(|&tid| crate::tempo_tx::TempoCall {
            to: diamond_addr,
            value_wei: 0,
            input: encode_erc721_transfer_from(&from, &to, tid),
        })
        .collect();
    // ~60k per ERC-721 transfer + ~275k sponsorship.
    let gas = 300_000 + token_ids.len() as u128 * 90_000;
    submit_tempo_sponsored(owner, fee_payer, calls, fee_token, gas).await
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
/// NOTE: this is a bare `register` with no `approve` — correct only while
/// `registrationCost()` is 0 (FREE, current testnet config). A non-zero
/// cost would require an approve/transferFrom pair per name (handled by the
/// single-create path), which the batch deliberately does not do.
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
    let diamond = parse_eth_address(REGISTRY_ADDRESS)?;
    let token = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS)?;
    Ok(crate::tempo_tx::TempoCall {
        to: token,
        value_wei: 0,
        input: encode_approve(&diamond, amount_wei),
    })
}

/// Release (recycle) a subdomain — burn the NFT + free the name — via a
/// sponsored tx. `sender` must own the token. DESTRUCTIVE: the UI/tool
/// MUST require typed confirmation before calling this. Refuses the MAIN
/// on-chain.
pub async fn release_name_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    token_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    // 1M, not a flat 400k: a name burn runs ~375-425k all-in (cold-slot clears
    // + ~275k sponsorship), so 400k OOG-reverted while the UI reported success.
    // Over-budget is free — the sponsor pays gas USED, not the limit.
    sponsored_diamond_call(sender, fee_payer, encode_release_name(token_id), fee_token, 1_000_000)
        .await
}

/// Batch-release (burn) several names in ONE sponsored tx. `sender` must
/// own every `token_id`; the on-chain ReleaseFacet refuses a caller's MAIN
/// per-id (so a MAIN slipped into the list reverts the WHOLE batch — filter
/// it out before calling). DESTRUCTIVE: the UI/tool MUST require a single
/// typed master confirmation before calling this. Mirrors
/// `consolidate_into_main_sponsored`'s multi-call construction, but burns
/// instead of transfers. (Browser callers use the iframe-signed path in
/// `app::events::run_bulk_release`; this is the off-bundle/native twin of
/// `release_name_sponsored`.)
pub async fn release_names_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    token_ids: &[u64],
    fee_token: &str,
) -> Result<String, String> {
    if token_ids.is_empty() {
        return Err("no subdomains to release".into());
    }
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let calls: Vec<_> = token_ids
        .iter()
        .map(|&tid| crate::tempo_tx::TempoCall {
            to: diamond_addr,
            value_wei: 0,
            input: encode_release_name(tid),
        })
        .collect();
    // Each burn ~100-150k inner; +275k sponsorship once for the whole batch.
    // 1M base mirrors the single-release headroom (release_name_sponsored),
    // then ~250k/extra burn. Over-budget is free (sponsor billed on gas USED).
    let gas = 1_000_000 + (token_ids.len() as u128).saturating_sub(1) * 250_000;
    submit_tempo_sponsored(sender, fee_payer, calls, fee_token, gas).await
}

/// Sponsored TBA remove-signer + index unlink (the unlink half of the
/// device lifecycle). `sender` must be an authorized signer of the MAIN.
pub async fn remove_signer_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    token_id: u64,
    tba_address: &str,
    signer_hex: &str,
    fee_token: &str,
) -> Result<String, String> {
    let signer_addr = parse_eth_address(signer_hex)?;
    let tba_addr = parse_eth_address(tba_address)?;
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let remove_call = crate::tempo_tx::TempoCall {
        to: tba_addr,
        value_wei: 0,
        input: encode_remove_signer(&signer_addr),
    };
    // Also drop it from the on-chain index so the UI stops showing it.
    let unlink_call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_unlink_device(token_id, &signer_addr),
    };
    submit_tempo_sponsored(sender, fee_payer, vec![remove_call, unlink_call], fee_token, 600_000)
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
    let sel = selector("approve(address,uint256)");
    let mut spender_padded = [0u8; 32];
    spender_padded[12..].copy_from_slice(spender);
    let amount_padded = u256_be(amount_wei);
    let mut out = Vec::with_capacity(4 + 32 + 32);
    out.extend_from_slice(&sel);
    out.extend_from_slice(&spender_padded);
    out.extend_from_slice(&amount_padded);
    out
}

/// ERC-20 `transfer(to, amount)` calldata — same shape as `encode_approve`
/// with the `transfer` selector.
pub(crate) fn encode_transfer(to: &[u8; 20], amount_wei: u128) -> Vec<u8> {
    let sel = selector("transfer(address,uint256)");
    let mut to_padded = [0u8; 32];
    to_padded[12..].copy_from_slice(to);
    let amount_padded = u256_be(amount_wei);
    let mut out = Vec::with_capacity(4 + 32 + 32);
    out.extend_from_slice(&sel);
    out.extend_from_slice(&to_padded);
    out.extend_from_slice(&amount_padded);
    out
}


/// Sponsored Tempo tx that calls `tba.execute(target, value, data, 0)`
/// on a `MultiSignerAccount` TBA. The TBA must be deployed; we batch
/// `createTokenBoundAccount(token_id)` first so the call is safe on
/// counterfactual TBAs too (createTokenBoundAccount is idempotent).
///
/// `sender` must be one of the TBA's authorized signers: the NFT
/// holder of the owning token, or an EOA previously added via
/// `addSigner`. The TBA's `execute` revert "not authorised" otherwise.
// Discrete params are the TBA-execute tx fields (signers, token, target,
// value, calldata, fee token, gas); bundling them into a struct would
// just move the noise. Kept flat as a low-level wire helper.
#[allow(clippy::too_many_arguments)]
pub async fn tba_execute_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    token_id: u64,
    tba_address: &str,
    target_hex: &str,
    value_wei: u128,
    inner_data: Vec<u8>,
    fee_token: &str,
    gas_limit: u128,
) -> Result<String, String> {
    let tba_addr = parse_eth_address(tba_address)?;
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let target = parse_eth_address(target_hex)?;

    let create_call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_create_tba(token_id),
    };
    let execute_call = crate::tempo_tx::TempoCall {
        to: tba_addr,
        value_wei: 0,
        input: encode_tba_execute(&target, value_wei, &inner_data),
    };
    submit_tempo_sponsored(
        sender,
        fee_payer,
        vec![create_call, execute_call],
        fee_token,
        gas_limit,
    )
    .await
}

/// Build the call batch that makes `token_id`'s TBA send `$LH`:
/// `[diamond.createTokenBoundAccount(token_id) (idempotent),
///   tba.execute($LH_token, 0, transfer(recipient, amount), 0)]`.
///
/// Pure — no chain I/O, no signing — so the browser act panel
/// (`app::events::tba`, which routes it through the iframe-signed
/// `run_sponsored_tempo_call`) and the native sponsored wrapper
/// ([`tba_transfer_lh_sponsored`]) share ONE calldata home, and the layout
/// is pinned by native unit tests. Rejects a zero amount up front (a
/// zero-value `transfer` is never an intended act-panel send).
pub fn tba_send_lh_calls(
    token_id: u64,
    tba_hex: &str,
    recipient_hex: &str,
    amount_wei: u128,
) -> Result<Vec<crate::tempo_tx::TempoCall>, String> {
    if amount_wei == 0 {
        return Err("amount must be greater than 0".into());
    }
    let diamond = parse_eth_address(REGISTRY_ADDRESS)?;
    let tba = parse_eth_address(tba_hex)?;
    let recipient = parse_eth_address(recipient_hex)?;
    let token = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS)?;
    let transfer_data = encode_erc20_transfer(&recipient, amount_wei);
    Ok(vec![
        crate::tempo_tx::TempoCall {
            to: diamond,
            value_wei: 0,
            input: encode_create_tba(token_id),
        },
        crate::tempo_tx::TempoCall {
            to: tba,
            value_wei: 0,
            input: encode_tba_execute(&token, 0, &transfer_data),
        },
    ])
}

/// Gas budget for a `$LH`-send-from-TBA batch ([`tba_send_lh_calls`]):
/// create TBA — ~742k live-measured on a COLD first deploy (CREATE2 of the
/// full MultiSignerAccount), near-zero idempotent thereafter — + execute
/// (~30k) + inner ERC-20 transfer (~52k) + Tempo sponsorship (~275k). A
/// first transfer from an undeployed TBA needs ~1.1M, so 800k would revert
/// out-of-gas; 2M covers the cold path and is free on the warm one (the
/// sponsor is billed on gas USED, not the limit).
pub const TBA_SEND_LH_GAS: u128 = 2_000_000;

/// Convenience: send LH from `token_id`'s TBA to a recipient. Submits the
/// [`tba_send_lh_calls`] batch as ONE sponsored Tempo tx. The TBA must hold
/// enough LH to cover `amount_wei`; `sender` must be authorized on the TBA
/// (the NFT holder or an enrolled device signer).
pub async fn tba_transfer_lh_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    token_id: u64,
    tba_address: &str,
    recipient_hex: &str,
    amount_wei: u128,
    fee_token: &str,
) -> Result<String, String> {
    let calls = tba_send_lh_calls(token_id, tba_address, recipient_hex, amount_wei)?;
    submit_tempo_sponsored(sender, fee_payer, calls, fee_token, TBA_SEND_LH_GAS).await
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
/// low-level primitive the [`tba_execute_sponsored`] (token-id keyed, batches
/// the deploy) and [`tba_transfer_lh_sponsored`] wrappers build on.
// Discrete params are the wire fields (owner+sponsor signers, TBA, target,
// value, inner calldata, fee token); bundling into a struct just moves noise.
#[allow(clippy::too_many_arguments)]
pub async fn tba_execute_call_sponsored(
    owner_signer: &SigningKey,
    fee_payer: &SigningKey,
    tba_addr: &str,
    to: &str,
    value_wei: u128,
    data: &[u8],
    fee_token: &str,
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
        fee_payer,
        tba_addr,
        encode_tba_execute(&target, value_wei, data),
        fee_token,
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
    fee_payer: &SigningKey,
    token_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    sponsored_diamond_call(
        owner_signer,
        fee_payer,
        encode_create_tba(token_id),
        fee_token,
        1_200_000,
    )
    .await
}

/// Make a TBA send `$LH` — `execute($LH_token, 0, transfer(recipient, amount))`
/// via [`tba_execute_call_sponsored`]. The flat (address-keyed, deploy NOT
/// batched) sibling of [`tba_transfer_lh_sponsored`]; the headless CLI calls
/// [`create_token_bound_account_sponsored`] first when the TBA isn't deployed
/// yet, so this assumes a live TBA. The TBA must hold at least `amount_wei`.
pub async fn tba_send_lh_sponsored(
    owner_signer: &SigningKey,
    fee_payer: &SigningKey,
    tba_addr: &str,
    recipient_hex: &str,
    amount_wei: u128,
    fee_token: &str,
) -> Result<String, String> {
    let recipient = parse_eth_address(recipient_hex)?;
    let transfer_data = encode_erc20_transfer(&recipient, amount_wei);
    tba_execute_call_sponsored(
        owner_signer,
        fee_payer,
        tba_addr,
        LOCALHARNESS_TOKEN_ADDRESS,
        0,
        &transfer_data,
        fee_token,
    )
    .await
}

/// ABI-encode an ERC-20 `transfer(address,uint256)` calldata. The inner
/// payload for a `$LH`-transfer-via-TBA (`execute($LH, 0, transfer(to, amt))`).
pub(crate) fn encode_erc20_transfer(recipient: &[u8; 20], amount_wei: u128) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32 + 32);
    out.extend_from_slice(&selector("transfer(address,uint256)"));
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(recipient);
    out.extend_from_slice(&padded);
    out.extend_from_slice(&u256_be(amount_wei));
    out
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

pub(crate) fn encode_remove_signer(addr: &[u8; 20]) -> Vec<u8> {
    let sel = selector("removeSigner(address)");
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(addr);
    let mut out = Vec::with_capacity(4 + 32);
    out.extend_from_slice(&sel);
    out.extend_from_slice(&padded);
    out
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
    fee_payer: &SigningKey,
    name: &str,
    fee_token: &str,
) -> Result<String, String> {
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
        sponsored_escrow_diamond_call(sender, fee_payer, cost, register_input, fee_token, 2_200_000)
            .await?
    } else {
        sponsored_diamond_call(sender, fee_payer, register_input, fee_token, 2_200_000).await?
    };

    // After register, fetch the new tokenId and set MAIN if none.
    let sender_addr = address_to_hex(&wallet::address(sender));
    if let Ok(0) = main_of(&sender_addr).await {
        if let Ok(Status::Taken { agent_id }) = check_name(name).await {
            if let Err(err) =
                register_main_sponsored(sender, fee_payer, agent_id, fee_token).await
            {
                log_main_warning(&err);
            }
        }
    }
    Ok(tx_hash)
}


// `tba_signers` (deprecated SignerAdded/SignerRemoved log-scraping — Tempo
// caps eth_getLogs at 100k blocks) was removed as dead code; `devices_of`
// reads the DeviceRegistryFacet's enumerable index in ONE call instead.

#[cfg(test)]
mod tests {
    use super::*;

    // --- Calldata-encoder layout guards (network-free). A wrong ABI offset
    // here would send $LH / NFTs to the wrong place, so pin the layout. ---

    #[test]
    fn erc721_transfer_from_calldata_layout() {
        let from = [0xAAu8; 20];
        let to = [0xBBu8; 20];
        let cd = encode_erc721_transfer_from(&from, &to, 0x1234);
        // Canonical ERC-721/20 transferFrom(address,address,uint256) selector.
        assert_eq!(&cd[0..4], &[0x23, 0xb8, 0x72, 0xdd]);
        assert_eq!(cd.len(), 4 + 96);
        assert_eq!(&cd[4 + 12..4 + 32], &from); // from in word 0
        assert_eq!(&cd[4 + 44..4 + 64], &to); // to in word 1
        assert_eq!(u64::from_be_bytes(cd[4 + 88..4 + 96].try_into().unwrap()), 0x1234);
    }

    #[test]
    fn release_name_calldata_layout() {
        let cd = encode_release_name(7);
        assert_eq!(&cd[0..4], &selector("releaseName(uint256)"));
        assert_eq!(cd.len(), 36);
        assert_eq!(u64::from_be_bytes(cd[28..36].try_into().unwrap()), 7);
    }

    #[test]
    fn unlink_device_calldata_layout() {
        let dev = [0xCDu8; 20];
        let unlink = encode_unlink_device(3, &dev);
        assert_eq!(&unlink[0..4], &selector("unlinkDevice(uint256,address)"));
        assert_eq!(unlink.len(), 68);
        assert_eq!(u64::from_be_bytes(unlink[28..36].try_into().unwrap()), 3); // mainId
        assert_eq!(&unlink[36 + 12..36 + 32], &dev); // device in word 2
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
        let token = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS).unwrap();
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

    /// Pin the act-panel batch builder ([`tba_send_lh_calls`]): exactly TWO
    /// calls — the idempotent `createTokenBoundAccount(token_id)` against the
    /// DIAMOND, then `execute($LH, 0, transfer(recipient, amount), 0)` against
    /// the TBA — both zero-native-value. A wrong `to` here either deploys
    /// nothing (execute reverts: no code) or drives the wrong account, so the
    /// routing is as load-bearing as the calldata bytes.
    #[test]
    fn tba_send_lh_calls_batch_layout() {
        let tba_hex = format!("0x{}", "aa".repeat(20));
        let recipient_hex = format!("0x{}", "cd".repeat(20));
        let amount: u128 = 250_000_000_000_000_000; // 0.25 $LH
        let calls = tba_send_lh_calls(42, &tba_hex, &recipient_hex, amount).unwrap();
        assert_eq!(calls.len(), 2);

        // Call 0: diamond.createTokenBoundAccount(42), value 0.
        let diamond = parse_eth_address(REGISTRY_ADDRESS).unwrap();
        assert_eq!(calls[0].to, diamond);
        assert_eq!(calls[0].value_wei, 0);
        assert_eq!(calls[0].input, encode_create_tba(42));
        assert_eq!(
            u64::from_be_bytes(calls[0].input[28..36].try_into().unwrap()),
            42
        );

        // Call 1: tba.execute($LH, 0, transfer(recipient, amount), 0), value 0.
        assert_eq!(calls[1].to, [0xAAu8; 20]);
        assert_eq!(calls[1].value_wei, 0);
        let token = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS).unwrap();
        let inner = encode_erc20_transfer(&[0xCDu8; 20], amount);
        assert_eq!(calls[1].input, encode_tba_execute(&token, 0, &inner));
        // The execute target is the $LH token (word 0 of the head)…
        assert_eq!(&calls[1].input[16..36], &token);
        // …and the nested transfer rides at the dynamic-data offset.
        assert_eq!(&calls[1].input[164..164 + inner.len()], inner.as_slice());
    }

    /// The builder fails CLOSED on bad inputs: zero amount (never a real
    /// send), malformed TBA / recipient hex. No call batch may exist that a
    /// later layer would have to remember to reject.
    #[test]
    fn tba_send_lh_calls_rejects_bad_inputs() {
        let tba = format!("0x{}", "aa".repeat(20));
        let to = format!("0x{}", "cd".repeat(20));
        assert!(tba_send_lh_calls(1, &tba, &to, 0).is_err()); // zero amount
        assert!(tba_send_lh_calls(1, "0x1234", &to, 1).is_err()); // short TBA
        assert!(tba_send_lh_calls(1, &tba, "not-an-address", 1).is_err());
        assert!(tba_send_lh_calls(1, &tba, "", 1).is_err());
    }
}
