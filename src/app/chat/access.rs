//! Credit access + per-turn payment + ABI helpers for the chat path: how a
//! turn reaches the model (platform `$LH` credits via the proxy vs BYOK), the
//! visitor payment gate, and the calldata builders the platform tools share.
//! Hex/address codecs come from `crate::encoding`.

use crate::app::{dom, APP};
use crate::encoding::{bytes_to_hex_str, parse_address};

/// Returns `Ok(Some(tx_hash))` if a payment was collected, `Ok(None)`
/// if no payment was required (free agent, owner sending, unverified
/// origin), or `Err(_)` if the visitor refused or the on-chain leg
/// failed. Caller short-circuits the send on `Err`.
pub(crate) async fn collect_payment_if_required() -> Result<Option<String>, String> {
    use crate::app::VerifyState;

    let (pricing_wei, verify_state, tba) = APP.with(|cell| {
        let app = cell.borrow();
        (
            app.pricing_wei,
            app.verify_state.clone(),
            app.tba_address.clone(),
        )
    });
    // `None` = pricing not checked yet (verification still running); `Some(0)` = free.
    // Do NOT collapse None to 0 (`unwrap_or(0)`) — a fast visitor could send before the
    // price loads and bypass a PRICED agent's gate for free. Fail closed until known
    // (same posture as the TBA/verify checks below); the window is a few seconds.
    let Some(price_wei) = pricing_wei else {
        return Err("agent pricing is still loading (verification running) — retry in a moment".into());
    };
    if price_wei == 0 {
        return Ok(None);
    }
    let Some(tba) = tba else {
        // Priced but no TBA known — can't route the funds. Fail closed
        // rather than silently letting the visitor through for free.
        return Err("agent is priced but its TBA isn't known yet (verification still running?)".into());
    };
    let visitor_address = match verify_state {
        VerifyState::Verified { .. } => return Ok(None), // owner sends free
        VerifyState::Visitor { visitor_address, .. } => visitor_address,
        VerifyState::Pending | VerifyState::Unregistered | VerifyState::Failed { .. } => {
            // Without a recovered visitor address we can't build a tx
            // from-them. Fail closed.
            return Err(
                "agent is priced but owner verification didn't complete — refresh and retry"
                    .into(),
            );
        }
    };

    // A real collection is happening — surface it in the pending turn's
    // stage line (GitHub #19; free / owner turns never show "paying").
    super::stage::enter(crate::turn_stage::Stage::Paying);

    let purpose = format!(
        "pay {} LH per turn to this agent",
        crate::app::format_wei_as_test_eth(price_wei),
    );

    // Build ERC-20 transfer(tba, price_wei) calldata against the
    // credits token. Sponsored Tempo tx: visitor's wallet (at apex)
    // signs the sender_hash, the bundle sponsor pays gas in AlphaUSD.
    // Visitor holds zero of anything except the LH they're spending.
    let tba_bytes = parse_address(&tba)?;
    let calldata = lh_transfer_calldata(&tba_bytes, price_wei);

    let token_addr = parse_address(crate::registry::LOCALHARNESS_TOKEN_ADDRESS())?;
    let call = crate::tempo_tx::TempoCall {
        to: token_addr,
        value_wei: 0,
        input: calldata,
    };

    dom::set_status("payment: signing via apex…", false);
    let tx_hash = crate::app::events::run_sponsored_tempo_call(
        &visitor_address,
        vec![call],
        500_000,
        &purpose,
    )
    .await
    .map_err(|e| format!("payment: {e}"))?;

    Ok(Some(tx_hash))
}

/// The localharness credit proxy origin — a drop-in Gemini base URL
/// (its `vercel.json` rewrites `/v1beta/*` onto the edge fn). Single source
/// of truth lives in `registry` so the native CLI's headless `call` and the
/// browser share one origin.
const CREDIT_PROXY_URL: &str = crate::registry::CREDIT_PROXY_URL;

/// True when the user is on platform `$LH` credits (via the proxy).
/// Persisted in localStorage; **defaults to credits** — a new account
/// uses platform credits with no setup, and BYOK is opt-in via admin →
/// account. Only an explicit "byok" choice flips it off.
pub(crate) fn model_access_is_credits() -> bool {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("lh_model_access").ok().flatten())
        .map(|v| v != "byok")
        .unwrap_or(true)
}

/// Resolved model access for a chat session.
pub(crate) struct ModelAccess {
    /// Goes in the GeminiClient api-key slot: a Gemini key (BYOK) or the
    /// credit-proxy auth token (credits).
    pub(crate) cfg_auth: String,
    /// Proxy base URL in credits mode; `None` for BYOK (direct to Google).
    pub(crate) base_url: Option<url::Url>,
    /// STABLE restart-detection identity — never the rotating credits
    /// token (which changes every resolve).
    pub(crate) identity: String,
}

/// The LOCAL signing key for the credit path — master wallet on the
/// apex / seed-bearing origin, else a local per-origin key (loaded or
/// generated + persisted on first use). NEVER the cross-origin iframe
/// signer: the whole credit path is iframe-free.
pub(crate) async fn credit_signer() -> Option<(k256::ecdsa::SigningKey, [u8; 20])> {
    if let Some(pair) =
        APP.with(|c| c.borrow().wallet.as_ref().map(|w| (w.signer.clone(), w.address)))
    {
        return Some(pair);
    }
    // Breadcrumbed: this identity-creation path froze on iOS with no symptom —
    // the crumbs put the dying stage on the panic banner / ?debug=1 overlay.
    crate::app::debuglog::log("credit_signer: loading device key (opfs read)");
    if let Some(sk) = crate::app::wallet_store::load_device_key().await {
        crate::app::debuglog::log("credit_signer: device key loaded");
        let addr = crate::wallet::address(&sk);
        return Some((sk, addr));
    }
    crate::app::debuglog::log("credit_signer: no key — generating");
    let w = crate::wallet::generate();
    crate::app::debuglog::log("credit_signer: persisting device key (opfs write)");
    crate::app::wallet_store::persist_device_key(&w.private_key_hex)
        .await
        .ok()?;
    crate::app::debuglog::log("credit_signer: device key persisted");
    // `w` is Drop (zeroizes its hex) — clone the signer, copy the address.
    Some((w.signer.clone(), w.address))
}

/// The credit identity's 0x address if one already exists locally —
/// does NOT generate (so status refreshes don't mint a key). master
/// wallet, else a persisted device key, else None.
pub(crate) async fn credit_address_existing() -> Option<String> {
    if let Some(a) = APP.with(|c| c.borrow().wallet.as_ref().map(|w| w.address_hex())) {
        return Some(a);
    }
    let sk = crate::app::wallet_store::load_device_key().await?;
    Some(bytes_to_hex_str(&crate::wallet::address(&sk)))
}

/// Resolve how this turn reaches the model. Credits mode mints a fresh
/// proxy auth token `address:timestamp:signature` (personal-signed by
/// the local key); BYOK falls back to the stored Gemini key. `None`
/// only when BYOK has no key (caller then shows the key modal).
pub(crate) async fn resolve_credit_access() -> Option<ModelAccess> {
    if model_access_is_credits() {
        let (signer, addr) = credit_signer().await?;
        let addr_hex = bytes_to_hex_str(&addr); // lowercase 0x — matches the proxy
        let ts = (js_sys::Date::now() / 1000.0) as u64;
        let msg = format!("localharness-proxy:{addr_hex}:{ts}:gemini");
        let sig = crate::wallet::personal_sign(&signer, msg.as_bytes());
        return Some(ModelAccess {
            cfg_auth: format!("{addr_hex}:{ts}:{}", bytes_to_hex_str(&sig)),
            base_url: url::Url::parse(CREDIT_PROXY_URL).ok(),
            identity: format!("credits:{addr_hex}"),
        });
    }
    let key = read_api_key().await?;
    Some(ModelAccess {
        cfg_auth: key.clone(),
        base_url: None,
        identity: key,
    })
}

/// Credits mode: fund the PER-REQUEST METER so the proxy debits real `$LH` per
/// call (per-call billing — NOT a free session). Moves any `$LH` sitting in the
/// wallet into the `CreditMeterFacet` (approve + deposit, one sponsored tx); the
/// proxy then debits `creditOf` per request and the balance actually decrements.
/// The `wallet == 0` check makes this idempotent — once moved, there's nothing
/// to re-deposit. Best-effort + silent: a failure just falls through to the
/// proxy's gating (a still-active free session keeps the agent usable).
///
/// NOTE: deposited `$LH` lives in the meter and has no withdraw path — that's
/// fine, it's there to be spent on calls. (Old free sessions still bypass
/// metering until they expire ≤1h; the proxy now PREFERS the funded meter, so
/// once funded, billing is immediate regardless of a lingering session.)
pub(crate) async fn ensure_credit_meter() {
    let Some((signer, addr)) = credit_signer().await else {
        return;
    };
    let addr_hex = bytes_to_hex_str(&addr);
    let wallet = crate::app::registry::token_balance_of(&addr_hex)
        .await
        .unwrap_or(0);
    if wallet == 0 {
        return; // nothing to fund the meter with (already moved, or empty)
    }
    let Ok(fee_payer) = crate::app::sponsor::signer() else {
        return;
    };
    let _ = crate::app::registry::deposit_credits_sponsored(
        &signer,
        &fee_payer,
        wallet,
        crate::app::registry::ALPHA_USD_ADDRESS(),
    )
    .await;
}

/// Read the api key with graceful fallback. Tries the live `#key`
/// input first (if admin is open), then sessionStorage, then OPFS.
/// Returns `None` only if every layer is empty.
async fn read_api_key() -> Option<String> {
    if let Some(input) = dom::input_by_id("key") {
        let v = input.value().trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    if let Ok(Some(storage)) = dom::session_storage() {
        if let Ok(Some(cached)) = storage.get_item("gemini_api_key") {
            let trimmed = cached.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    if let Some(persisted) = crate::app::key_store::load().await {
        let trimmed = persisted.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    None
}

/// Pre-flight for EVERY browser escrow path (scheduleJob / createInvite /
/// postBounty / fundGuild — on-chain feedback #63): how much of `needed_wei`
/// must be auto-bridged out of the chat METER (`withdrawCredits`, prepended in
/// the same atomic tx) because the WALLET pot is short. Returns 0 when the
/// wallet covers it, the shortfall when the meter's WITHDRAWABLE (unlocked)
/// balance covers the gap, and a pot-aware error (mirrors
/// `remote_call::ask_via_proxy`'s wording) when both pots together can't cover
/// the escrow. The meter figure is the withdrawable slice, NOT the full balance:
/// locked fiat-$LH can't be pulled by `withdrawCredits`.
pub(crate) async fn escrow_bridge_wei(from_hex: &str, needed_wei: u128) -> Result<u128, String> {
    let wallet = crate::app::registry::token_balance_of(from_hex)
        .await
        .unwrap_or(0);
    if wallet >= needed_wei {
        return Ok(0);
    }
    let shortfall = needed_wei - wallet;
    // Only the UNLOCKED (`withdrawableOf`) slice of the meter can be bridged out:
    // fiat-minted $LH is LOCKED from withdrawal, so `withdrawCredits` reverts
    // (LH2024) if we count it. Match that on-chain limit here so a card-funded
    // user with a short wallet gets the clear "fund up" error up-front instead of
    // the whole atomic escrow tx reverting cryptically.
    let withdrawable = crate::app::registry::withdrawable_credit_of(from_hex)
        .await
        .unwrap_or(0);
    if withdrawable < shortfall {
        return Err(format!(
            "needs {} $LH but the wallet holds {} and the withdrawable chat meter {} — \
             fund up with a redeem code, an invite, or a $LH transfer first",
            crate::app::format_wei_as_test_eth(needed_wei),
            crate::app::format_wei_as_test_eth(wallet),
            crate::app::format_wei_as_test_eth(withdrawable),
        ));
    }
    Ok(shortfall)
}

pub(crate) fn u256_be(value: u128) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&value.to_be_bytes());
    out
}

pub(crate) fn transfer_selector() -> [u8; 4] {
    selector4(b"transfer(address,uint256)")
}

pub(crate) fn withdraw_credits_selector() -> [u8; 4] {
    selector4(b"withdrawCredits(uint256)")
}

/// First 4 bytes of keccak256 of an ABI function signature.
fn selector4(sig: &[u8]) -> [u8; 4] {
    use sha3::{Digest, Keccak256};
    let mut hasher = Keccak256::new();
    hasher.update(sig);
    let mut out = [0u8; 4];
    out.copy_from_slice(&hasher.finalize()[..4]);
    out
}

/// ERC-20 `transfer(to, amount_wei)` calldata against the `$LH` token — the ONE
/// transfer-calldata builder (the visitor pay path, `send_lh`'s `lh_transfer_call`,
/// and the actor prefund all route through this, so the encoding can't diverge).
/// `to` is a 20-byte address; `amount_wei` is 18-decimal token wei.
pub(crate) fn lh_transfer_calldata(to: &[u8; 20], amount_wei: u128) -> Vec<u8> {
    let mut to_padded = [0u8; 32];
    to_padded[12..].copy_from_slice(to);
    let mut calldata = Vec::with_capacity(4 + 32 + 32);
    calldata.extend_from_slice(&transfer_selector());
    calldata.extend_from_slice(&to_padded);
    calldata.extend_from_slice(&u256_be(amount_wei));
    calldata
}

/// `createTokenBoundAccount(tokenId)` calldata against the registry diamond.
/// Idempotent: deploys the ERC-6551 account so a counterfactual TBA can hold
/// funds (registry's own helper is private, so we mirror it here — chat.rs
/// already hand-builds calldata for `send_lh` the same way).
fn create_tba_calldata(token_id: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector4(b"createTokenBoundAccount(uint256)"));
    data.extend_from_slice(&u256_be(token_id as u128));
    data
}

/// Result of preparing the optional actor-model extras (persona + prefund) for
/// a freshly-registered subdomain. `calls` are appended to the same sponsored
/// Tempo tx that publishes / sets up the new token; `extra_gas` is added to the
/// base gas budget.
pub(crate) struct ActorSetup {
    pub(crate) calls: Vec<crate::tempo_tx::TempoCall>,
    pub(crate) extra_gas: u128,
    pub(crate) prefunded_lh: Option<String>,
    pub(crate) tba: Option<String>,
    pub(crate) persona_set: bool,
}

/// Build the optional persona + prefund calls for `create_subdomain` /
/// `create_and_publish_app` (the ACTOR MODEL).
///
/// **Billing-semantics finding → prefund recipient = the new subdomain's TBA.**
/// The credit proxy keys `$LH` usage by the *signing EOA address*
/// (`sessionExpiryOf(address)` / `creditOf(address)` in `proxy/api/gemini.ts`),
/// and the creator already OWNS the new name, so funds sent to the creator's
/// own wallet would be a no-op for "the new actor". The meaningful, spendable
/// wallet an actor controls is its **token-bound account (TBA)** — that's also
/// the x402 payee when one agent pays another (`proxy/api/mcp.ts` resolves
/// `tokenBoundAccountByName` → "payee (the agent's TBA)"). So prefund flows
/// CREATOR-wallet → new-name's TBA, giving the spawned actor operating funds it
/// controls. We batch `createTokenBoundAccount(tokenId)` first (idempotent) so
/// the counterfactual TBA exists to receive the transfer.
///
/// `creator` is the owner address paying / signing; `token_id` is the new
/// name's freshly-minted id; `name` is the (sanitised) subdomain.
pub(crate) async fn build_actor_setup(
    creator: &str,
    token_id: u64,
    name: &str,
    persona: Option<&str>,
    prefund_lh: Option<&str>,
) -> Result<ActorSetup, crate::error::Error> {
    let registry_addr =
        parse_address(crate::app::registry::REGISTRY_ADDRESS()).map_err(crate::error::Error::other)?;
    let mut calls: Vec<crate::tempo_tx::TempoCall> = Vec::new();
    let mut extra_gas: u128 = 0;
    let mut persona_set = false;
    let mut prefunded_lh = None;
    let mut tba_out = None;

    // PERSONA — publish the new subdomain's on-chain system prompt under the
    // persona metadata key (keccak256("localharness.persona")), the same slot
    // the CLI `persona` cmd + headless `call` read. setMetadata is gas-hungry,
    // so the budget scales with length (see `gas::set_metadata_gas`).
    if let Some(p) = persona {
        let p = p.trim();
        if !p.is_empty() {
            calls.push(crate::tempo_tx::TempoCall {
                to: registry_addr,
                value_wei: 0,
                input: crate::app::registry::encode_set_persona(token_id, p),
            });
            extra_gas += crate::app::gas::set_metadata_gas(p.len());
            persona_set = true;
        }
    }

    // PREFUND — move `$LH` from the CREATOR to the new name's TBA. Validate the
    // creator actually holds the amount first (clear error, before any write).
    if let Some(amt_str) = prefund_lh {
        let amt_str = amt_str.trim();
        if !amt_str.is_empty() {
            let amount_wei = crate::encoding::parse_token_amount(amt_str).ok_or_else(|| {
                crate::error::Error::other(format!(
                    "could not parse prefund_lh \"{amt_str}\" — pass a decimal $LH figure \
                     like \"5\" or \"1.5\""
                ))
            })?;
            if amount_wei > 0 {
                // Balance gate: refuse if the creator can't cover it.
                let bal = crate::app::registry::token_balance_of(creator)
                    .await
                    .map_err(crate::error::Error::other)?;
                if bal < amount_wei {
                    return Err(crate::error::Error::other(format!(
                        "insufficient $LH to prefund: need {amt_str}, creator holds \
                         {} wei — redeem a code or lower prefund_lh",
                        bal
                    )));
                }
                // Resolve the new name's TBA (counterfactual address). We batch
                // createTokenBoundAccount(tokenId) FIRST so it's deployed to
                // receive funds (idempotent if already deployed).
                let tba = crate::app::registry::tba_of_name(name)
                    .await
                    .map_err(crate::error::Error::other)?
                    .ok_or_else(|| {
                        crate::error::Error::other(
                            "could not resolve the new subdomain's token-bound account \
                             (TBA) to prefund — retry shortly",
                        )
                    })?;
                let tba_bytes = parse_address(&tba).map_err(crate::error::Error::other)?;
                let token_addr =
                    parse_address(crate::registry::LOCALHARNESS_TOKEN_ADDRESS())
                        .map_err(crate::error::Error::other)?;
                // 1) deploy the TBA (on the registry diamond)
                calls.push(crate::tempo_tx::TempoCall {
                    to: registry_addr,
                    value_wei: 0,
                    input: create_tba_calldata(token_id),
                });
                // 2) ERC-20 transfer creator → TBA (on the $LH token)
                calls.push(crate::tempo_tx::TempoCall {
                    to: token_addr,
                    value_wei: 0,
                    input: lh_transfer_calldata(&tba_bytes, amount_wei),
                });
                // TBA deploy (~mint-class cold SSTOREs) + ERC-20 transfer.
                extra_gas += 1_500_000 + 500_000;
                prefunded_lh = Some(amt_str.to_string());
                tba_out = Some(tba);
            }
        }
    }

    let _ = creator; // (used above only when prefunding)
    Ok(ActorSetup {
        calls,
        extra_gas,
        prefunded_lh,
        tba: tba_out,
        persona_set,
    })
}

pub(crate) fn short_hash(hash: &str) -> String {
    let stripped = hash.trim_start_matches("0x");
    if stripped.len() < 12 {
        return hash.to_string();
    }
    format!("0x{}…{}", &stripped[..6], &stripped[stripped.len() - 4..])
}
