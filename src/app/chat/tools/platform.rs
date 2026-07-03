// =============================================================================
// Platform-level closure tools (browser-specific; not in the SDK builtins).
// =============================================================================

use crate::app::chat::access::{
    build_actor_setup, lh_transfer_calldata, u256_be, withdraw_credits_selector,
};
use crate::encoding::parse_address;
use crate::tools::ClosureTool;

/// Resolve a `recipient` arg (raw 0x… address or subdomain name) to the
/// 0x… address that receives $LH (a name pays its on-chain OWNER).
async fn resolve_lh_recipient(recipient_arg: &str) -> Result<String, crate::error::Error> {
    use crate::encoding::Recipient;
    let kind = crate::encoding::classify_recipient(recipient_arg)
        .map_err(crate::error::Error::other)?;
    match kind {
        Recipient::Address(addr) => Ok(addr),
        Recipient::Name(name) => crate::app::registry::owner_of_name(&name)
            .await
            .map_err(crate::error::Error::other)?
            .ok_or_else(|| {
                crate::error::Error::other(format!(
                    "no on-chain owner for subdomain \"{name}\" — is it registered?"
                ))
            }),
    }
}

/// Resolve a $LH recipient to a NOTIFIABLE subdomain name (#50): the name
/// directly if `recipient_arg` was a name, else the owner address's MAIN name
/// (reverse `main_of` → `name_of_id`). `None` when the recipient has no
/// registered identity to notify (a bare address). The proxy `/api/notify`
/// only routes to a name, so this is how a raw-address transfer still pings.
async fn notifiable_recipient_name(recipient_arg: &str, to_hex: &str) -> Option<String> {
    use crate::encoding::Recipient;
    if let Ok(Recipient::Name(name)) = crate::encoding::classify_recipient(recipient_arg) {
        return Some(name);
    }
    // Raw address: reverse-resolve to its MAIN identity's name, if any.
    let main_id = crate::app::registry::main_of(to_hex).await.ok()?;
    if main_id == 0 {
        return None;
    }
    crate::app::registry::name_of_id(main_id).await.ok().filter(|n| !n.is_empty())
}

/// Fire-and-forget a cross-agent notification to the $LH recipient that funds
/// arrived (#50): piggybacks the existing cross-agent notify (`notify_cross_agent`
/// → proxy `/api/notify`, which lands in the recipient's bell + buzzes any
/// enrolled phone). Best-effort — it must NEVER fail or block the transfer that
/// already settled on-chain; an unregistered/un-enrolled recipient is silently
/// skipped. NOT a transfer-watch system — it just rides the send.
fn notify_recipient_of_incoming_lh(recipient_arg: String, to_hex: String, amount: String) {
    wasm_bindgen_futures::spawn_local(async move {
        let Some(name) = notifiable_recipient_name(&recipient_arg, &to_hex).await else {
            return;
        };
        let title = format!("+{amount} $LH received");
        let body = "incoming $LH transfer — check your wallet".to_string();
        // The proxy stamps the SENDER's chain-verified identity into the title,
        // so the recipient sees who paid them. Swallow any error (no identity /
        // not enrolled / metered-out): the money already moved.
        let _ = crate::app::chat::tools::misc::notify_cross_agent(&name, &title, &body).await;
    });
}

/// ERC-20 `transfer(to, amount)` TempoCall against the $LH token. Calldata comes
/// from the ONE shared builder (`access::lh_transfer_calldata`) so this can't
/// diverge from the visitor-pay / prefund encodings.
fn lh_transfer_call(
    to_hex: &str,
    amount_wei: u128,
) -> Result<crate::tempo_tx::TempoCall, crate::error::Error> {
    let to_bytes = parse_address(to_hex).map_err(crate::error::Error::other)?;
    let calldata = lh_transfer_calldata(&to_bytes, amount_wei);
    let token_addr = parse_address(crate::registry::LOCALHARNESS_TOKEN_ADDRESS())
        .map_err(crate::error::Error::other)?;
    Ok(crate::tempo_tx::TempoCall {
        to: token_addr,
        value_wei: 0,
        input: calldata,
    })
}

/// The meter auto-bridge for direct transfers (on-chain feedback #48): when
/// the sender's wallet can't cover `needed_wei` but their unspent chat-meter
/// credits can, return a `withdrawCredits(shortfall)` call to PREPEND to the
/// SAME Tempo tx — bridge + spend land atomically in one sponsored
/// submission (0x76 carries a calls array). Pot-aware error when both pots
/// together are short.
async fn meter_bridge_call(
    from_hex: &str,
    needed_wei: u128,
) -> Result<Option<crate::tempo_tx::TempoCall>, crate::error::Error> {
    // The pot math (0 = wallet covers / shortfall = meter covers / pot-aware
    // error) is the SAME pre-flight every escrow path runs — never re-fork it.
    let shortfall = crate::app::chat::access::escrow_bridge_wei(from_hex, needed_wei)
        .await
        .map_err(crate::error::Error::other)?;
    if shortfall == 0 {
        return Ok(None);
    }
    let mut calldata = Vec::with_capacity(4 + 32);
    calldata.extend_from_slice(&withdraw_credits_selector());
    calldata.extend_from_slice(&u256_be(shortfall));
    let diamond = parse_address(crate::registry::REGISTRY_ADDRESS())
        .map_err(crate::error::Error::other)?;
    Ok(Some(crate::tempo_tx::TempoCall {
        to: diamond,
        value_wei: 0,
        input: calldata,
    }))
}

/// `create_subdomain(name)` — register `<name>.localharness.xyz` on the
/// LocalharnessRegistry diamond, signed by the owner's apex wallet via
/// the iframe signer. Returns the tx hash. Sanitises the input the same
/// way `tenant::sanitize` does for the apex claim form.
pub(crate) fn create_subdomain_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Schema + lenient extraction from ONE hoisted table
    // (`crate::tool_params::CreateSubdomainParams`), byte-identity-tested natively.
    let schema = crate::tool_params::CreateSubdomainParams::schema();
    ClosureTool::new(
        "create_subdomain",
        "Register a new <name>.localharness.xyz subdomain on-chain (the ACTOR MODEL). \
         The owner's master wallet pays gas and ends up holding the resulting ERC-721 \
         NFT. OPTIONALLY spawn the actor WITH behavior + funds in one call: `persona` \
         publishes its on-chain system instruction; `prefund_lh` moves that much $LH \
         from your wallet into the new agent's token-bound account (its own wallet). \
         Returns { name, url, owner, tx_hash, persona_set?, prefunded_lh?, tba? }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let params = crate::tool_params::CreateSubdomainParams::lenient(&args);
            let name = params.name.trim();
            let persona = params.persona.as_deref();
            let prefund_lh = params.prefund_lh.as_deref();
            // Validate (don't silently mangle) — an invalid name returns a clear
            // reason to the agent instead of minting a DIFFERENT name (#66/#60).
            let cleaned = crate::subdomain::validate(name)
                .map_err(|why| crate::error::Error::other(format!("invalid subdomain name: {why}")))?;
            // Register the name first (master wallet ends up holding the new id).
            let (owner, claim_tx) = crate::app::verify::claim_name_via_iframe(&cleaned)
                .await
                .map_err(|e| crate::error::Error::other(format!("claim failed: {e}")))?;
            // Proactively push this device's Gemini key to the MAIN slot so the
            // new subdomain inherits it (no re-save).
            {
                let n = cleaned.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    crate::app::events::sync_local_key_to_main(&n).await;
                });
            }

            // Optional ACTOR-MODEL extras: persona + prefund. Only if asked.
            let want_persona = persona.map(|p| !p.trim().is_empty()).unwrap_or(false);
            let want_prefund = prefund_lh
                .map(|p| {
                    let t = p.trim();
                    !t.is_empty() && t != "0"
                })
                .unwrap_or(false);
            let mut result = serde_json::json!({
                "name": cleaned,
                "url": format!("https://{cleaned}.localharness.xyz/"),
                "owner": owner,
                "tx_hash": claim_tx,
            });
            if want_persona || want_prefund {
                // Resolve the freshly-minted tokenId for the metadata/TBA ops.
                let token_id = match crate::app::registry::id_of_name(&cleaned).await {
                    Ok(id) if id != 0 => id,
                    Ok(_) => {
                        return Err(crate::error::Error::other(
                            "registered but tokenId not yet visible on-chain — retry \
                             persona/prefund shortly",
                        ))
                    }
                    Err(e) => return Err(crate::error::Error::other(format!("id_of_name: {e}"))),
                };
                let setup = build_actor_setup(
                    &owner,
                    token_id,
                    &cleaned,
                    persona,
                    prefund_lh,
                )
                .await?;
                if !setup.calls.is_empty() {
                    let tx_hash = crate::app::events::run_sponsored_tempo_call(
                        &owner,
                        setup.calls,
                        setup.extra_gas,
                        "spawn actor (persona + prefund)",
                    )
                    .await
                    .map_err(|e| {
                        crate::error::Error::other(format!("actor setup failed: {e}"))
                    })?;
                    result["setup_tx_hash"] = serde_json::json!(tx_hash);
                    result["persona_set"] = serde_json::json!(setup.persona_set);
                    if let Some(amt) = setup.prefunded_lh {
                        result["prefunded_lh"] = serde_json::json!(amt);
                    }
                    if let Some(tba) = setup.tba {
                        result["tba"] = serde_json::json!(tba);
                    }
                }
            }
            Ok(result)
        },
    )
}

/// Publish a compiled rustlite cartridge as `name`'s app face — the ONE
/// publish-app shape shared by `create_and_publish_app` (fresh + update),
/// `publish_app_to` (cross-subdomain), and `publish_public_face` ("app").
///
/// OFF-CHAIN (free, no gas) when this device's EOA directly owns `name`: the
/// compiled wasm goes to the app store (`registry::publish_app_to_store`), which
/// the proxy authorizes via on-chain ownership (`ownerOf(name) == token signer`).
/// On-chain `setMetadata` publishing cost ~$0.32–$2.80/cart and drained the gas
/// sponsor; this kills that. A NON-EOA owner (TBA consolidation) or absent local
/// signer falls back to the legacy on-chain `setMetadata` batch (the agent tools
/// never had a TBA path, so this is unchanged for them). `Ok(Some(tx))` when it
/// went on-chain; `Ok(None)` when published off-chain.
async fn publish_app_face(
    name: &str,
    token_id: u64,
    source: &str,
    owner: &str,
) -> Result<Option<String>, crate::error::Error> {
    if source.trim().is_empty() {
        return Err(crate::error::Error::other("source cannot be empty"));
    }
    // Compile FIRST — a bad cartridge fails before any write. Surface the FULL
    // rendering (LH code + line/col + caret) so the agent can fix it.
    let wasm = crate::rustlite::compile(source).map_err(|e| {
        crate::error::Error::other(format!("compile failed: {}", e.render(source)))
    })?;
    if wasm.len() > crate::app::registry::APP_STORE_MAX_WASM_BYTES {
        return Err(crate::error::Error::other(format!(
            "app wasm too large to publish: {} bytes (max {})",
            wasm.len(),
            crate::app::registry::APP_STORE_MAX_WASM_BYTES
        )));
    }
    // OFF-CHAIN when the device's MASTER wallet owns the name. The off-chain
    // token MUST be signed by the OWNER — the proxy authorizes via
    // ownerOf(name) == token signer — so read `APP.wallet` (the master) DIRECTLY,
    // NOT credit_signer(): credit_signer can return, or even MINT, a per-origin
    // DEVICE key (a linked second device) that is NOT the owner, which would both
    // fail the proxy's ownerOf gate and silently route us to the costly on-chain
    // path. A pure read; no key generation. The off-chain POST is THE path here
    // (its error surfaces directly), not a try-then-fall-through.
    let master = crate::app::APP
        .with(|c| c.borrow().wallet.as_ref().map(|w| (w.signer.clone(), w.address)));
    if let Some((signer, addr)) = master {
        if owner.eq_ignore_ascii_case(&crate::encoding::bytes_to_hex_str(&addr)) {
            let now = (js_sys::Date::now() / 1000.0) as u64;
            let token = crate::registry::proxy_auth_token(&signer, now, "publish");
            crate::app::registry::publish_app_to_store(name, &token, &wasm, source)
                .await
                .map_err(|e| crate::error::Error::other(format!("publish failed: {e}")))?;
            return Ok(None);
        }
    }
    // ON-CHAIN fallback — reached only when the owner ISN'T this device's master
    // wallet: a TBA-owned name (consolidation), or a linked device without the
    // seed loaded (off-chain publish needs the owner to sign; full linked-device
    // off-chain support = a proxy authorized-signer follow-up). Logged so this
    // (sponsor-gas-priced) regression is observable, not silent. The legacy
    // sponsored setMetadata batch; length-scaled gas (~7.6k gas/BYTE).
    crate::app::debuglog::log(&format!(
        "publish_app_face: off-chain unavailable for {name} (owner {owner} is not the \
         local master wallet) — on-chain fallback"
    ));
    let registry_addr = parse_address(crate::app::registry::REGISTRY_ADDRESS())
        .map_err(crate::error::Error::other)?;
    let mk = |input: Vec<u8>| crate::tempo_tx::TempoCall {
        to: registry_addr,
        value_wei: 0,
        input,
    };
    let calls = vec![
        mk(crate::app::registry::encode_set_app_wasm(token_id, &wasm)),
        mk(crate::app::registry::encode_set_public_face(token_id, "app")),
    ];
    let gas = crate::app::gas::set_metadata_gas(wasm.len());
    let tx = crate::app::events::run_sponsored_tempo_call(owner, calls, gas, "publish app")
        .await
        .map_err(|e| crate::error::Error::other(format!("publish failed: {e}")))?;
    Ok(Some(tx))
}

/// Publish an HTML page as `name`'s public face — the HTML-face sibling of
/// [`publish_app_face`]. OFF-CHAIN (free) when the device MASTER wallet owns the
/// name (read `APP.wallet` directly — see publish_app_face for why not
/// credit_signer); on-chain `setMetadata` fallback for a TBA-owned name. `Ok(Some
/// (tx))` when on-chain, `Ok(None)` when off-chain.
async fn publish_html_face(
    name: &str,
    token_id: u64,
    html: &[u8],
    owner: &str,
) -> Result<Option<String>, crate::error::Error> {
    if html.is_empty() {
        return Err(crate::error::Error::other("index.html is empty"));
    }
    if html.len() > crate::app::registry::APP_STORE_MAX_WASM_BYTES {
        return Err(crate::error::Error::other(format!(
            "index.html too large to publish: {} bytes (max {})",
            html.len(),
            crate::app::registry::APP_STORE_MAX_WASM_BYTES
        )));
    }
    let master = crate::app::APP
        .with(|c| c.borrow().wallet.as_ref().map(|w| (w.signer.clone(), w.address)));
    if let Some((signer, addr)) = master {
        if owner.eq_ignore_ascii_case(&crate::encoding::bytes_to_hex_str(&addr)) {
            let now = (js_sys::Date::now() / 1000.0) as u64;
            let token = crate::registry::proxy_auth_token(&signer, now, "publish");
            let html_str = String::from_utf8_lossy(html).into_owned();
            crate::app::registry::publish_html_to_store(name, &token, &html_str)
                .await
                .map_err(|e| crate::error::Error::other(format!("publish failed: {e}")))?;
            return Ok(None);
        }
    }
    // ON-CHAIN fallback (TBA-owned name / no local master) — legacy sponsored
    // setMetadata batch.
    crate::app::debuglog::log(&format!(
        "publish_html_face: off-chain unavailable for {name} (owner {owner} is not the \
         local master wallet) — on-chain fallback"
    ));
    let registry_addr = parse_address(crate::app::registry::REGISTRY_ADDRESS())
        .map_err(crate::error::Error::other)?;
    let mk = |input: Vec<u8>| crate::tempo_tx::TempoCall {
        to: registry_addr,
        value_wei: 0,
        input,
    };
    let calls = vec![
        mk(crate::app::registry::encode_set_public_html(token_id, html)),
        mk(crate::app::registry::encode_set_public_face(token_id, "html")),
    ];
    let gas = crate::app::gas::set_metadata_gas(html.len());
    let tx = crate::app::events::run_sponsored_tempo_call(owner, calls, gas, "publish html")
        .await
        .map_err(|e| crate::error::Error::other(format!("publish failed: {e}")))?;
    Ok(Some(tx))
}

/// Resolve a registered name's `(token_id, owner)` for an OWNER-AUTHORIZED
/// write, asserting the master wallet that signs (`signer_owner`) holds it.
/// `None` = unregistered (caller decides whether to register). `Err` = the
/// name is owned by someone ELSE (refuse) or an RPC failure.
async fn owned_token_for_publish(
    name: &str,
    signer_owner: &str,
) -> Result<Option<(u64, String)>, crate::error::Error> {
    let owner = match crate::app::registry::owner_of_name(name).await {
        Ok(Some(o)) => o,
        Ok(None) => return Ok(None),
        Err(e) => return Err(crate::error::Error::other(format!("owner_of_name: {e}"))),
    };
    if !owner.eq_ignore_ascii_case(signer_owner) {
        return Err(crate::error::Error::other(format!(
            "\"{name}\" is owned by {owner}, not you ({signer_owner}) — you can only \
             publish to subdomains you own"
        )));
    }
    let token_id = match crate::app::registry::id_of_name(name).await {
        Ok(id) if id != 0 => id,
        Ok(_) => {
            return Err(crate::error::Error::other(format!(
                "\"{name}\" has an owner but no tokenId yet — retry shortly"
            )))
        }
        Err(e) => return Err(crate::error::Error::other(format!("id_of_name: {e}"))),
    };
    Ok(Some((token_id, owner)))
}

/// `create_and_publish_app(name, source)` — OWNERSHIP-AWARE one-shot publish:
/// - `name` UNREGISTERED → register `<name>.localharness.xyz` + publish the
///   compiled cartridge as its public face (a fresh subdomain for the app).
/// - `name` already owned by THE CALLER → UPDATE in place: re-publish the
///   cartridge OFF-CHAIN, NO re-register, no duplicate.
/// - `name` owned by SOMEONE ELSE → refuse with a clear error.
///
/// Compiles `source` first (a bad cartridge fails before any write), then
/// publishes the cartridge OFF-CHAIN to the app store (free, no gas — the chain
/// keeps only ownership). For a FRESH name the optional persona + prefund are set
/// on-chain separately (small, sponsored). A brand-new app never silently
/// overwrites the owner's MAIN. Returns `{ name, url, tx_hash, off_chain, updated }`.
pub(crate) fn create_and_publish_app_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Hoisted table: `crate::tool_params::CreateAndPublishAppParams`.
    let schema = crate::tool_params::CreateAndPublishAppParams::schema();
    ClosureTool::new(
        "create_and_publish_app",
        "Publish a compiled rustlite cartridge as <name>.localharness.xyz's fullscreen \
         public face (compile + OFF-CHAIN publish — free, no gas). OWNERSHIP-AWARE: if \
         `name` is UNREGISTERED it registers a NEW subdomain first; if YOU already own \
         `name` it UPDATES that app in place (no re-register, no duplicate); if someone \
         ELSE owns `name` it refuses. Use this for \"make me a clock subdomain\" AND \
         \"update my <name> app\". The ACTOR MODEL (fresh names only): optionally also \
         set the new agent's `persona` (on-chain system instruction) and `prefund_lh` it \
         with $LH (into its token-bound account), set on-chain after the app publishes. \
         create_subdomain remains for registering a name-only subdomain. Returns \
         { name, url, tx_hash, off_chain, updated, persona_set?, prefunded_lh?, tba? }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let params = crate::tool_params::CreateAndPublishAppParams::lenient(&args);
            let name = params.name.trim();
            let source = params.source.as_str();
            let persona = params.persona.as_deref();
            let prefund_lh = params.prefund_lh.as_deref();
            let cleaned = crate::subdomain::validate(name)
                .map_err(|why| crate::error::Error::other(format!("invalid subdomain name: {why}")))?;
            // Compile FIRST (also bounds-checks size) so a bad cartridge fails
            // before any register/setMetadata write. This is the SAME shape the
            // update path uses; resolve the tokenId after we know it compiles.
            // (token_id is patched in once known — encode below.)
            if source.trim().is_empty() {
                return Err(crate::error::Error::other("source cannot be empty"));
            }
            // Who would sign? The owner of the current host subdomain — the
            // master wallet that holds ALL this identity's names. Used to decide
            // OWN vs SOMEONE-ELSE for an already-registered target.
            let signer_owner = crate::app::tenant::current_tenant_owner()
                .await
                .map(|(_, o)| o)
                .ok();

            // Branch on the target's on-chain ownership.
            let existing = match &signer_owner {
                Some(o) => owned_token_for_publish(&cleaned, o).await?,
                // Off a tenant host (preview/localhost) we can't prove the
                // signer's identity; fall back to "register if free", and a
                // taken name will be refused by the claim path.
                None => match crate::app::registry::owner_of_name(&cleaned).await {
                    Ok(Some(_)) => {
                        return Err(crate::error::Error::other(format!(
                            "\"{cleaned}\" is already registered — run this on your own \
                             subdomain so ownership can be verified before updating it"
                        )))
                    }
                    Ok(None) => None,
                    Err(e) => return Err(crate::error::Error::other(format!("owner_of_name: {e}"))),
                },
            };

            // UPDATE path: the caller already owns `name` → re-publish in place,
            // NO re-register (which would fail), NO persona/prefund (those are
            // spawn-time actor setup). One sponsored setMetadata batch.
            if let Some((token_id, owner)) = existing {
                let tx = publish_app_face(&cleaned, token_id, source, &owner).await?;
                let off_chain = tx.is_none();
                return Ok(serde_json::json!({
                    "name": cleaned,
                    "url": format!("https://{cleaned}.localharness.xyz/"),
                    "tx_hash": tx.unwrap_or_else(|| "off-chain".to_string()),
                    "off_chain": off_chain,
                    "updated": true,
                }));
            }

            // FRESH path: register the name, then publish. The owner's master
            // wallet ends up holding the new tokenId, so it's authorized to
            // setMetadata below.
            let (owner, _claim_tx) = crate::app::verify::claim_name_via_iframe(&cleaned)
                .await
                .map_err(|e| crate::error::Error::other(format!("claim failed: {e}")))?;
            // Inherit this device's Gemini key onto the new subdomain.
            {
                let n = cleaned.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    crate::app::events::sync_local_key_to_main(&n).await;
                });
            }
            // Resolve the freshly-minted tokenId.
            let token_id = match crate::app::registry::id_of_name(&cleaned).await {
                Ok(id) if id != 0 => id,
                Ok(_) => {
                    return Err(crate::error::Error::other(
                        "registered but tokenId not yet visible on-chain — retry publish shortly",
                    ))
                }
                Err(e) => return Err(crate::error::Error::other(format!("id_of_name: {e}"))),
            };
            // Publish the app OFF-CHAIN (free) to the app store — the owner's
            // master wallet (just minted the name) signs the proxy auth token.
            let app_tx = publish_app_face(&cleaned, token_id, source, &owner).await?;
            // ACTOR MODEL: persona + prefund stay ON-CHAIN (they're identity /
            // economy primitives, and small/cheap unlike the app bytes). Submit
            // them as their own sponsored batch only if either was requested.
            let setup =
                build_actor_setup(&owner, token_id, &cleaned, persona, prefund_lh).await?;
            let setup_tx = if setup.calls.is_empty() {
                None
            } else {
                Some(
                    crate::app::events::run_sponsored_tempo_call(
                        &owner,
                        setup.calls,
                        setup.extra_gas,
                        "actor setup (persona/prefund)",
                    )
                    .await
                    .map_err(|e| crate::error::Error::other(format!("actor setup failed: {e}")))?,
                )
            };
            let off_chain = app_tx.is_none();
            // Report the most relevant on-chain tx (app fallback, else setup).
            let tx_hash = app_tx
                .or(setup_tx)
                .unwrap_or_else(|| "off-chain".to_string());
            let mut result = serde_json::json!({
                "name": cleaned,
                "url": format!("https://{cleaned}.localharness.xyz/"),
                "tx_hash": tx_hash,
                "off_chain": off_chain,
                "updated": false,
            });
            if setup.persona_set {
                result["persona_set"] = serde_json::json!(true);
            }
            if let Some(amt) = setup.prefunded_lh {
                result["prefunded_lh"] = serde_json::json!(amt);
            }
            if let Some(tba) = setup.tba {
                result["tba"] = serde_json::json!(tba);
            }
            Ok(result)
        },
    )
}

/// `publish_app_to(name, source, confirmation)` — UPDATE-FROM-MAIN: publish a
/// compiled cartridge to ANY subdomain the caller OWNS, even one DIFFERENT from
/// the current host. The owner's master wallet (the one that signs the current
/// host's sponsored writes) holds all their subdomain NFTs, so it can sign a
/// `setMetadata` for any owned tokenId — no new ownership/actor model needed,
/// just targeting a chosen owned name. From a MAIN session this updates any
/// alt's app. The target MUST already be registered AND owned by the caller
/// (refuses unregistered names — use `create_and_publish_app` to mint a fresh
/// one — and names owned by someone else). MOVES on-chain state, so it rides
/// the typed-confirmation gate (`chat::confirm_guard`). NOT granted to
/// subagents. Returns `{ name, url, tx_hash, updated: true }`.
pub(crate) fn publish_app_to_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Hoisted table: `crate::tool_params::PublishAppToParams`.
    let schema = crate::tool_params::PublishAppToParams::schema();
    ClosureTool::new(
        "publish_app_to",
        "Publish (UPDATE) a rustlite cartridge to ANOTHER subdomain you OWN — the \
         update-from-MAIN path. The owner's master wallet holds all their subdomain \
         NFTs, so from one session you can re-publish any of your alts' apps. The \
         target must ALREADY exist and be owned by you (to mint a NEW subdomain use \
         create_and_publish_app; that tool also updates the CURRENT name in place). \
         CHANGES on-chain state — the first call does NOT execute: it returns a \
         single-use confirmation code (also shown to the owner in the UI). Say which \
         subdomain you'll update, ask the owner to TYPE the code, then retry with \
         `confirmation` set to it. Returns { name, url, tx_hash, updated: true }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let params = crate::tool_params::PublishAppToParams::lenient(&args);
            let name = params.name.trim();
            let source = params.source.as_str();
            // Belt-and-suspenders: the confirm_guard hook denies any unconfirmed
            // call before this body runs; this guards a registration path that
            // forgot the hook (same posture as send_lh / release_subdomain).
            let confirmed = params
                .confirmation
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !confirmed {
                return Err(crate::error::Error::other(
                    "publish_app_to requires the platform-issued confirmation code",
                ));
            }
            let cleaned = crate::subdomain::validate(name)
                .map_err(|why| crate::error::Error::other(format!("invalid subdomain name: {why}")))?;
            if source.trim().is_empty() {
                return Err(crate::error::Error::other("source cannot be empty"));
            }
            // The signer = the current host's owner (the master wallet holding
            // ALL this identity's names). Required so we can prove ownership of a
            // DIFFERENT target name before writing to it.
            let (_, signer_owner) = crate::app::tenant::current_tenant_owner()
                .await
                .map_err(crate::error::Error::other)?;
            // Resolve + ownership-gate the target. None = unregistered (refuse —
            // this tool only UPDATES owned names); Err = owned-by-other / RPC.
            let (token_id, owner) = owned_token_for_publish(&cleaned, &signer_owner)
                .await?
                .ok_or_else(|| {
                    crate::error::Error::other(format!(
                        "\"{cleaned}\" is not registered — use create_and_publish_app to \
                         mint and publish a new subdomain"
                    ))
                })?;
            let tx = publish_app_face(&cleaned, token_id, source, &owner).await?;
            let off_chain = tx.is_none();
            Ok(serde_json::json!({
                "name": cleaned,
                "url": format!("https://{cleaned}.localharness.xyz/"),
                "tx_hash": tx.unwrap_or_else(|| "off-chain".to_string()),
                "off_chain": off_chain,
                "updated": true,
            }))
        },
    )
}

/// `embed_app(name)` — fetch ANOTHER subdomain's published cartridge and
/// render it INLINE in the chat transcript as a live, interactive card (NOT
/// an iframe — cartridges are framebuffer wasm; an iframe of a subdomain that
/// itself boots a cartridge hits recursion/partitioning limits). Resolves
/// `name` → tokenId → on-chain `app.wasm`; if the subdomain has a published
/// cartridge, stashes its bytes for the transcript's `#embed-canvas` card to
/// launch (via `display::run_in_canvas`) and returns `{ name, url,
/// embedded: true }`. A subdomain with no published app (directory/html face,
/// or never published) returns a clear error.
///
/// v1 limitations (documented for the agent): (1) SINGLE-WORKER — embedding
/// replaces any cartridge already running (a prior embed or the fullscreen
/// overlay); only one live embed at a time. (2) The embedded cartridge's
/// host_agent FEED context (subscribe/viewer_is_owner/…) resolves against the
/// HOST page's subdomain, not the embedded one — cross-subdomain feed identity
/// is a follow-up.
pub(crate) fn embed_app_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Hoisted table: `crate::tool_params::EmbedAppParams`.
    let schema = crate::tool_params::EmbedAppParams::schema();
    ClosureTool::new(
        "embed_app",
        "Embed another subdomain's published cartridge INLINE in this chat as a \
         live, interactive card (the cartridge runs in the framebuffer, like the \
         display — NOT an iframe). Use this to show/play <name>'s app right here \
         (\"embed pong\", \"show me <name>'s app\"). Single live embed at a time: \
         embedding replaces any cartridge already running. Only works when <name> \
         has PUBLISHED a cartridge (an app public face) — directory/html faces or \
         unpublished names return an error. Returns { name, url, embedded: true }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let params = crate::tool_params::EmbedAppParams::lenient(&args);
            let name = params.name.trim();
            let cleaned = crate::app::tenant::sanitize(name);
            if cleaned.is_empty() {
                return Err(crate::error::Error::other("name cannot be empty"));
            }
            let token_id = match crate::app::registry::id_of_name(&cleaned).await {
                Ok(id) if id != 0 => id,
                Ok(_) => {
                    return Err(crate::error::Error::other(format!(
                        "\"{cleaned}\" is not registered"
                    )))
                }
                Err(e) => return Err(crate::error::Error::other(format!("id_of_name: {e}"))),
            };
            let wasm = match crate::app::registry::app_wasm_of(token_id).await {
                Ok(Some(bytes)) if !bytes.is_empty() => bytes,
                Ok(_) => {
                    return Err(crate::error::Error::other(format!(
                        "{cleaned} has no published cartridge — only directory/html \
                         faces or unpublished"
                    )))
                }
                Err(e) => return Err(crate::error::Error::other(format!("app_wasm_of: {e}"))),
            };
            // Stash the bytes; `chat::stream_turn` launches them into the
            // `#embed-canvas` card once the inline card has painted. Remember
            // WHICH app, so a crash report names the embedded cartridge.
            crate::app::display::set_cartridge_ref(Some(format!("embedded app: {cleaned}")));
            crate::app::display::stash_pending_embed(wasm);
            Ok(serde_json::json!({
                "name": cleaned,
                "url": format!("https://{cleaned}.localharness.xyz/"),
                "embedded": true,
            }))
        },
    )
}

/// `publish_public_face(choice)` — publish THIS agent's OWN public face from
/// chat (the agent-tool mirror of admin → public face, feature request #27).
/// `choice` is "directory" | "app" | "html": "app" compiles + publishes this
/// device's local `app.rl` cartridge OFF-CHAIN to the app store (free, no gas —
/// no on-chain `public_face` write needed; the published cartridge IS the face);
/// "html"/"directory" set the on-chain face choice (+ html bytes) in a sponsored
/// Tempo tx. Owner-only, own subdomain only. Mirrors
/// `events::public_face::run_set_public_face` minus the DOM. Reversible.
pub(crate) fn publish_public_face_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Hoisted table: `crate::tool_params::PublishPublicFaceParams`.
    let schema = crate::tool_params::PublishPublicFaceParams::schema();
    ClosureTool::new(
        "publish_public_face",
        "Publish YOUR OWN public face — what a visitor to \
         https://<you>.localharness.xyz/ sees — the chat equivalent of admin → \
         public face. `choice`: \"app\" compiles + publishes this device's local \
         app.rl as a fullscreen cartridge OFF-CHAIN (free, no gas); \"html\" \
         publishes local index.html; \"directory\" sets a profile landing. \
         Zero-click. Works only on your own subdomain. After it succeeds, give the \
         user the returned `url`. Returns { choice, url, tx_hash, off_chain? }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let choice = crate::tool_params::PublishPublicFaceParams::lenient(&args)
                .choice
                .trim()
                .to_lowercase();
            if !matches!(choice.as_str(), "directory" | "app" | "html") {
                return Err(crate::error::Error::other(
                    "choice must be \"directory\", \"app\", or \"html\"",
                ));
            }
            let Some(name) = crate::app::tenant::current_name() else {
                return Err(crate::error::Error::other(
                    "publish_public_face only works on your own subdomain",
                ));
            };
            let token_id = match crate::app::registry::id_of_name(&name).await {
                Ok(id) if id != 0 => id,
                _ => return Err(crate::error::Error::other("name isn't registered on-chain")),
            };
            let owner = match crate::app::registry::owner_of_name(&name).await {
                Ok(Some(o)) => o,
                _ => return Err(crate::error::Error::other("name isn't registered on-chain")),
            };
            let registry_addr = parse_address(crate::app::registry::REGISTRY_ADDRESS())
                .map_err(crate::error::Error::other)?;
            let mk = |input: Vec<u8>| crate::tempo_tx::TempoCall {
                to: registry_addr,
                value_wei: 0,
                input,
            };
            // Build the call batch + gas for the chosen face — the same shapes
            // as the admin flow (storing bytes is ~7.6k gas/BYTE on top of the
            // ~275k Tempo sponsorship; `set_metadata_gas` length-scales it).
            let (calls, gas): (Vec<crate::tempo_tx::TempoCall>, u128) = match choice.as_str() {
                "directory" => (
                    vec![mk(crate::app::registry::encode_set_public_face(token_id, "directory"))],
                    500_000,
                ),
                "app" => {
                    // OFF-CHAIN: publish this device's local app.rl to the app
                    // store (free). publish_app_face compiles + size-caps + POSTs
                    // (EOA owner) or falls back on-chain (TBA). Early-return — the
                    // app face needs no on-chain `public_face` write now (the
                    // published cartridge IS the face).
                    let fs = crate::app::shared_opfs();
                    let src = match fs.read("app.rl").await {
                        Ok(b) if !b.is_empty() => String::from_utf8_lossy(&b).into_owned(),
                        _ => {
                            return Err(crate::error::Error::other(
                                "no app.rl on this device — build one first (run_cartridge), \
                                 then publish",
                            ))
                        }
                    };
                    let tx = publish_app_face(&name, token_id, &src, &owner).await?;
                    let off_chain = tx.is_none();
                    return Ok(serde_json::json!({
                        "choice": choice,
                        "url": format!("https://{name}.localharness.xyz/"),
                        "tx_hash": tx.unwrap_or_else(|| "off-chain".to_string()),
                        "off_chain": off_chain,
                    }));
                }
                "html" => {
                    // OFF-CHAIN: publish this device's local index.html to the app
                    // store (free; on-chain fallback for a TBA owner). Early-return.
                    let fs = crate::app::shared_opfs();
                    let html = match fs.read("index.html").await {
                        Ok(b) if !b.is_empty() => b,
                        _ => {
                            return Err(crate::error::Error::other(
                                "no index.html on this device — create one first, then publish",
                            ))
                        }
                    };
                    let tx = publish_html_face(&name, token_id, &html, &owner).await?;
                    let off_chain = tx.is_none();
                    return Ok(serde_json::json!({
                        "choice": choice,
                        "url": format!("https://{name}.localharness.xyz/"),
                        "tx_hash": tx.unwrap_or_else(|| "off-chain".to_string()),
                        "off_chain": off_chain,
                    }));
                }
                _ => unreachable!(),
            };
            let tx_hash =
                crate::app::events::run_sponsored_tempo_call(&owner, calls, gas, "publish public face")
                    .await
                    .map_err(|e| crate::error::Error::other(format!("publish failed: {e}")))?;
            Ok(serde_json::json!({
                "choice": choice,
                "url": format!("https://{name}.localharness.xyz/"),
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `release_subdomain(name, confirmation)` — DESTRUCTIVE: burn the NFT +
/// free the name. Gated by the dispatch-layer typed-confirmation challenge
/// (`chat::confirm_guard`): the first call is denied with a single-use code
/// the OWNER must type in chat; only the retry carrying that code executes.
/// The model cannot auto-fill it (the code is random and must appear in the
/// latest USER message).
pub(crate) fn release_subdomain_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Hoisted table: `crate::tool_params::ReleaseSubdomainParams`.
    let schema = crate::tool_params::ReleaseSubdomainParams::schema();
    ClosureTool::new(
        "release_subdomain",
        "DESTRUCTIVE + IRREVERSIBLE: burn a subdomain NFT and free its name. The first \
         call does NOT execute: it returns a single-use confirmation code (also shown to \
         the owner in the UI). Ask the owner to TYPE that code in chat, then retry with \
         `confirmation` set to it — the call only executes after the owner's message \
         contains the code. Refuses your MAIN. Returns the tx hash.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let params = crate::tool_params::ReleaseSubdomainParams::lenient(&args);
            let name = params.name.trim().to_string();
            if name.is_empty() {
                return Err(crate::error::Error::other("name is required"));
            }
            // The typed-confirmation gate (confirm_guard) runs BEFORE this body
            // and denies any call without a user-typed challenge code. This
            // belt-and-suspenders check only guards a registration path that
            // forgot the hook.
            let confirmed = params
                .confirmation
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !confirmed {
                return Err(crate::error::Error::other(
                    "release_subdomain requires the platform-issued confirmation code",
                ));
            }
            match crate::app::events::run_release_subdomain(&name).await {
                Ok(tx) => Ok(serde_json::json!({ "released": name, "tx_hash": tx })),
                Err(e) => Err(crate::error::Error::other(format!("release failed: {e}"))),
            }
        },
    )
}

/// `bulk_release_subdomains(confirmation, names?)` — DESTRUCTIVE batch burn.
/// With no `names`, targets EVERY non-MAIN subdomain the owner holds; with
/// `names`, only that subset. Gated by the dispatch-layer typed-confirmation
/// challenge (`chat::confirm_guard`) — ONE single-use code for the whole
/// batch, typed by the owner. Refuses the MAIN. Withheld from subagents
/// (only registered on the main agent).
pub(crate) fn bulk_release_subdomains_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "names": {
                "type": "array",
                "items": { "type": "string" },
                "description": "OPTIONAL subset of subdomain names to release in one \
                    batch. Omit to target EVERY non-MAIN subdomain the owner holds."
            },
            "confirmation": {
                "type": "string",
                "description": "Single-use confirmation code. OMIT (or pass \"\") on the \
                    first call — it returns a challenge code shown to the owner. Show the \
                    owner the exact list that will be burned (list_subdomains is the \
                    read-only source), ask them to TYPE the code, then retry with it. \
                    Never invent it; only the platform issues it."
            }
        },
        "required": []
    });
    ClosureTool::new(
        "bulk_release_subdomains",
        "DESTRUCTIVE + IRREVERSIBLE: burn MANY subdomain NFTs and free their names in \
         ONE batch. With no `names`, releases EVERY non-MAIN subdomain the owner holds; \
         with `names`, only that subset. The first call does NOT execute: it returns a \
         single-use confirmation code (also shown to the owner in the UI). Show the owner \
         the exact list that will be burned (use list_subdomains), ask them to TYPE the \
         code, then retry with `confirmation` set to it. ONE code for the whole batch. \
         Always refuses your MAIN. Returns the released names + tx hash.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            // The typed-confirmation gate (confirm_guard) runs BEFORE this
            // body; an unconfirmed call never reaches it. Belt-and-suspenders
            // for any registration path that forgot the hook.
            let confirmed = args
                .get("confirmation")
                .and_then(|v| v.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !confirmed {
                return Err(crate::error::Error::other(
                    "bulk_release_subdomains requires the platform-issued confirmation code",
                ));
            }

            // Resolve the kill-list: explicit subset, else all non-MAIN holdings.
            let (_, owner) = crate::app::tenant::current_tenant_owner()
                .await
                .map_err(crate::error::Error::other)?;
            let main_id = crate::app::registry::main_of(&owner)
                .await
                .map_err(crate::error::Error::other)?;

            let explicit: Vec<String> = args
                .get("names")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();

            let targets: Vec<String> = if explicit.is_empty() {
                let tokens = crate::app::registry::list_owned_tokens(&owner)
                    .await
                    .map_err(crate::error::Error::other)?;
                tokens
                    .into_iter()
                    .filter(|t| main_id == 0 || t.token_id != main_id)
                    .map(|t| t.name)
                    .collect()
            } else {
                explicit
            };

            if targets.is_empty() {
                return Ok(serde_json::json!({
                    "status": "nothing_to_release",
                    "note": "no non-MAIN subdomains to release"
                }));
            }

            match crate::app::events::run_bulk_release(&targets).await {
                Ok((released, tx)) => Ok(serde_json::json!({
                    "released": released,
                    "count": released.len(),
                    "tx_hash": tx,
                })),
                Err(e) => Err(crate::error::Error::other(format!("bulk release failed: {e}"))),
            }
        },
    )
}

/// `batch_create_subdomains(names)` — register MANY subdomains in ONE
/// sponsored multi-call tx (the mirror of `bulk_release_subdomains`, but
/// ADDITIVE: NO destructive confirmation). The sanctioned mass-registration
/// path — one tx instead of an N-deep `create_subdomain` loop. Names are
/// sanitised + availability-checked; taken/invalid names are skipped and
/// reported. Capped at MAX_BATCH_CREATE to bound a confused model. Not
/// granted to subagents (same restraint as bulk_release).
pub(crate) fn batch_create_subdomains_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Hoisted table: `crate::tool_params::BatchCreateSubdomainsParams`,
    // byte-identity-tested natively.
    let schema = crate::tool_params::BatchCreateSubdomainsParams::schema();
    ClosureTool::new(
        "batch_create_subdomains",
        "Register MANY <name>.localharness.xyz subdomains on-chain in a SINGLE \
         sponsored transaction. PREFER THIS over calling create_subdomain in a \
         loop when registering more than one name — it is one tx, not N. The \
         owner's master wallet ends up holding every resulting ERC-721 NFT. \
         Taken or invalid names are skipped (not an error) and listed in \
         `skipped`. Max 20 names per call. Returns { registered, skipped, \
         count, tx_hash, urls }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            const MAX_BATCH_CREATE: usize = 20;
            let requested: Vec<String> = crate::tool_params::BatchCreateSubdomainsParams::lenient(&args)
                .names
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if requested.is_empty() {
                return Err(crate::error::Error::other("names cannot be empty"));
            }
            if requested.len() > MAX_BATCH_CREATE {
                return Err(crate::error::Error::other(format!(
                    "too many names: {} (max {MAX_BATCH_CREATE} per batch) — \
                     split into multiple calls",
                    requested.len()
                )));
            }
            match crate::app::events::run_batch_create_subdomains(&requested).await {
                Ok((registered, tx)) => {
                    let skipped: Vec<&String> = requested
                        .iter()
                        .filter(|r| {
                            let c = crate::app::tenant::sanitize(r);
                            !registered.iter().any(|reg| reg == &c)
                        })
                        .collect();
                    Ok(serde_json::json!({
                        "registered": registered,
                        "skipped": skipped,
                        "count": registered.len(),
                        "tx_hash": tx,
                        "urls": registered.iter()
                            .map(|n| format!("https://{n}.localharness.xyz/"))
                            .collect::<Vec<_>>(),
                    }))
                }
                Err(e) => Err(crate::error::Error::other(format!(
                    "batch create failed: {e}"
                ))),
            }
        },
    )
}

/// `list_subdomains()` — enumerate every subdomain this agent's owner
/// holds (their identity's holdings). Read-only.
pub(crate) fn list_subdomains_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    ClosureTool::new(
        "list_subdomains",
        "List every subdomain owned by this agent's owner (their identity's holdings on \
         the registry). Read-only. Use when the user asks what subdomains/agents they have.",
        serde_json::json!({ "type": "object", "properties": {} }),
        |_args: serde_json::Value, _ctx| async move {
            let (_, owner) = crate::app::tenant::current_tenant_owner()
                .await
                .map_err(crate::error::Error::other)?;
            let tokens = crate::app::registry::list_owned_tokens(&owner)
                .await
                .map_err(crate::error::Error::other)?;
            let subdomains: Vec<_> = tokens
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "url": format!("https://{}.localharness.xyz/", t.name),
                        "token_id": t.token_id,
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "owner": owner,
                "count": subdomains.len(),
                "subdomains": subdomains,
            }))
        },
    )
}

/// `discover_agents(query)` — find peer agents by capability/persona. The
/// browser twin of the `localharness discover` CLI command: a read-only
/// registry scan (no `$LH`, no tx) that reuses [`registry::discover_agents`]
/// (which ranks `(name, persona)` matches — name hits above persona hits). The
/// agent uses it to LOCATE a peer to delegate to, then `call_agent`s it.
/// Returns `{ agents: [{ name, persona }], count }`; persona snippets are
/// truncated to a char-safe ~160-char preview. Safe to grant broadly.
pub(crate) fn discover_agents_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    /// Char-safe truncation of a persona to a short preview (never splits a
    /// UTF-8 codepoint; appends an ellipsis when clipped).
    fn snippet(persona: &str) -> String {
        const MAX: usize = 160;
        let trimmed = persona.trim();
        if trimmed.chars().count() <= MAX {
            return trimmed.to_string();
        }
        let mut s: String = trimmed.chars().take(MAX).collect();
        s.push('…');
        s
    }
    ClosureTool::new(
        "discover_agents",
        "Find peer agents by capability or persona. Read-only registry scan: \
         returns the agents whose subdomain NAME or on-chain persona matches \
         `query`. MULTI-KEYWORD: the query is split on whitespace and an agent \
         matches ANY keyword, ranked by how many it matches (name matches above \
         persona matches) — so ONE call with \"game tool puzzle\" replaces a \
         sequential call per keyword. Use this to LOCATE an agent to delegate \
         to, then call_agent it. Returns { agents: [ { name, persona } ], \
         count } (persona is a short preview).",
        // Hoisted table: `crate::tool_params::DiscoverAgentsParams`.
        crate::tool_params::DiscoverAgentsParams::schema(),
        |args: serde_json::Value, _ctx| async move {
            let query = crate::tool_params::DiscoverAgentsParams::lenient(&args).query;
            // Reuse the registry's ranked discovery (same core as the
            // `localharness discover` CLI). 100 = how many recent agents to scan.
            let matches = crate::app::registry::discover_agents(&query, 100)
                .await
                .map_err(crate::error::Error::other)?;
            let agents: Vec<_> = matches
                .iter()
                .map(|(name, persona)| {
                    serde_json::json!({
                        "name": name,
                        "persona": snippet(persona),
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "count": agents.len(),
                "agents": agents,
            }))
        },
    )
}

/// `send_lh(recipient, amount)` — transfer real `$LH` credits from the owner's
/// wallet. `recipient` is either a raw `0x…` address or a subdomain name (whose
/// on-chain OWNER address receives the funds). `amount` is a human-typed `$LH`
/// figure (18-decimal token; "5", "1.5", "0.000001"). Builds an ERC-20
/// `transfer(to, amount_wei)` against the `$LH` token and routes it through the
/// SAME sponsored Tempo path as the per-turn payment + the "act" panel
/// (`run_sponsored_tempo_call`): the owner's apex wallet signs the intent, the
/// bundle sponsor pays gas in AlphaUSD. NOT granted to subagents (it moves
/// value). Gated by the dispatch-layer typed-confirmation challenge
/// (`chat::confirm_guard`): the owner types a single-use code before any
/// transfer executes. Amount must parse to > 0.
pub(crate) fn send_lh_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Schema + typed extraction come from ONE hoisted table
    // (`crate::tool_params::SendLhParams`), byte-identity-tested natively —
    // this wasm-gated file is outside every default check.
    let schema = crate::tool_params::SendLhParams::schema();
    ClosureTool::new(
        "send_lh",
        "Transfer real $LH credits from the owner's wallet to a recipient. \
         `recipient` is a raw 0x… address OR a subdomain name (funds go to that \
         name's on-chain owner). `amount` is a decimal $LH figure (must be > 0). \
         MOVES VALUE — the first call does NOT execute: it returns a single-use \
         confirmation code (also shown to the owner in the UI). State the \
         recipient + amount, ask the owner to TYPE the code, then retry with \
         `confirmation` set to it. Returns { amount, recipient (input), \
         resolved_recipient, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            use crate::encoding::parse_token_amount;

            // Lenient extraction (missing/wrong-typed → defaults), semantics
            // identical to the old inline `.get().and_then().unwrap_or()` chains.
            let params = crate::tool_params::SendLhParams::lenient(&args);
            let recipient_arg = params.recipient.trim().to_string();
            let amount_arg = params.amount.trim().to_string();

            // Amount: parse to 18-decimal wei (same units as the act panel /
            // per-turn payment), reject zero / garbage.
            let amount_wei = parse_token_amount(&amount_arg).ok_or_else(|| {
                crate::error::Error::other(format!(
                    "could not parse amount \"{amount_arg}\" — pass a decimal $LH \
                     figure like \"5\" or \"1.5\""
                ))
            })?;
            if amount_wei == 0 {
                return Err(crate::error::Error::other(
                    "amount must be greater than 0",
                ));
            }
            // Belt-and-suspenders: confirm_guard denies any unconfirmed call before
            // this body runs; this guards a path that forgot the hook (send_lh moves
            // real $LH — same posture as release_subdomain).
            let confirmed = params
                .confirmation
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !confirmed {
                return Err(crate::error::Error::other(
                    "send_lh requires the platform-issued confirmation code",
                ));
            }

            // Recipient: address used directly; name → on-chain owner address.
            let to_hex = resolve_lh_recipient(&recipient_arg).await?;

            // Sender = this subdomain's on-chain owner (the apex wallet that
            // signs via the iframe), matching list_subdomains / bulk_release.
            let (_, from) = crate::app::tenant::current_tenant_owner()
                .await
                .map_err(crate::error::Error::other)?;

            // Meter auto-bridge (feedback #48): a wallet shortfall covered by
            // unspent chat credits rides as a withdrawCredits call in the SAME
            // tx, so the transfer lands atomically. Then the ERC-20
            // transfer(to, amount) — the same calldata shape the per-turn
            // payment + act panel build.
            let mut calls = Vec::with_capacity(2);
            let bridged = match meter_bridge_call(&from, amount_wei).await? {
                Some(bridge) => {
                    calls.push(bridge);
                    true
                }
                None => false,
            };
            calls.push(lh_transfer_call(&to_hex, amount_wei)?);

            let amount_display = amount_arg.clone();
            let purpose = format!("send {amount_display} $LH to {to_hex}");
            // 500k mirrors the per-turn payment's ERC-20 transfer budget (+150k
            // when the bridge call rides along); the sponsor is billed on gas
            // USED, not the limit.
            let gas = if bridged { 650_000 } else { 500_000 };
            let tx_hash =
                crate::app::events::run_sponsored_tempo_call(&from, calls, gas, &purpose)
                    .await
                    .map_err(|e| crate::error::Error::other(format!("send_lh failed: {e}")))?;

            // #50: ping the recipient that funds arrived (best-effort, rides the
            // send — never a transfer-watch system). Fire-and-forget so it can't
            // fail or delay the tool result for a settled transfer.
            notify_recipient_of_incoming_lh(
                recipient_arg.clone(),
                to_hex.clone(),
                amount_display.clone(),
            );

            Ok(serde_json::json!({
                "amount": amount_display,
                "recipient": recipient_arg,
                "resolved_recipient": to_hex,
                "bridged_from_meter": bridged,
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `batch_send_lh(transfers)` — N transfers in ONE sponsored Tempo tx
/// (feedback #49: tx type 0x76 natively carries a calls array, so batching
/// costs one submission instead of N). The meter auto-bridge covers the
/// TOTAL if the wallet is short. Gated by the dispatch-layer
/// typed-confirmation challenge (`chat::confirm_guard`), same as `send_lh`.
pub(crate) fn batch_send_lh_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "transfers": {
                "type": "array",
                "description": "Up to 20 transfers, executed atomically in one \
                    on-chain transaction.",
                "items": {
                    "type": "object",
                    "properties": {
                        "recipient": {
                            "type": "string",
                            "description": "0x… address or subdomain name (funds \
                                go to the name's on-chain owner)."
                        },
                        "amount": {
                            "type": "string",
                            "description": "Decimal $LH amount, e.g. \"1\" or \
                                \"0.5\". Must be greater than 0."
                        }
                    },
                    "required": ["recipient", "amount"]
                }
            },
            "confirmation": {
                "type": "string",
                "description": "Single-use confirmation code. OMIT (or pass \"\") on the \
                    first call — it returns a challenge code shown to the owner. Show the \
                    full transfer list, ask the owner to TYPE the code in chat, then \
                    retry with it. Never invent it; only the platform issues it."
            }
        },
        "required": ["transfers"]
    });
    ClosureTool::new(
        "batch_send_lh",
        "Transfer $LH to MULTIPLE recipients in ONE on-chain transaction (up \
         to 20). Each transfer names a 0x… address or a subdomain (paid to its \
         on-chain owner). Far cheaper than repeated send_lh calls. MOVES VALUE \
         — the first call does NOT execute: it returns a single-use confirmation \
         code (also shown to the owner in the UI). Show the full list, ask the \
         owner to TYPE the code, then retry with `confirmation` set to it. ONE \
         code for the whole batch. Returns { count, total, transfers: \
         [{recipient, resolved, amount}], tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            use crate::encoding::parse_token_amount;

            let items = args
                .get("transfers")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if items.is_empty() {
                return Err(crate::error::Error::other(
                    "batch_send_lh: transfers must be a non-empty array",
                ));
            }
            if items.len() > 20 {
                return Err(crate::error::Error::other(
                    "batch_send_lh: at most 20 transfers per batch",
                ));
            }
            // Belt-and-suspenders: confirm_guard denies any unconfirmed call before
            // this body runs; this guards a path that forgot the hook (batch_send_lh
            // moves real $LH to many recipients — same posture as send_lh).
            let confirmed = args
                .get("confirmation")
                .and_then(|v| v.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !confirmed {
                return Err(crate::error::Error::other(
                    "batch_send_lh requires the platform-issued confirmation code",
                ));
            }

            let mut resolved: Vec<(String, String, u128, String)> =
                Vec::with_capacity(items.len());
            let mut total_wei: u128 = 0;
            for item in &items {
                let recipient = item
                    .get("recipient")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let amount_str = item
                    .get("amount")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let amount_wei = parse_token_amount(&amount_str).ok_or_else(|| {
                    crate::error::Error::other(format!(
                        "could not parse amount \"{amount_str}\" for \"{recipient}\""
                    ))
                })?;
                if amount_wei == 0 {
                    return Err(crate::error::Error::other(format!(
                        "amount for \"{recipient}\" must be greater than 0"
                    )));
                }
                let to_hex = resolve_lh_recipient(&recipient).await?;
                // checked, not saturating: a hostile/overflowing total must be a
                // clear error (matching parse_token_amount's reject-don't-wrap
                // contract), not a silently-clamped wrong bridge/display amount.
                total_wei = total_wei.checked_add(amount_wei).ok_or_else(|| {
                    crate::error::Error::other(
                        "batch total exceeds the maximum representable amount — split the batch",
                    )
                })?;
                resolved.push((recipient, to_hex, amount_wei, amount_str));
            }

            let (_, from) = crate::app::tenant::current_tenant_owner()
                .await
                .map_err(crate::error::Error::other)?;

            let mut calls = Vec::with_capacity(resolved.len() + 1);
            let bridged = match meter_bridge_call(&from, total_wei).await? {
                Some(bridge) => {
                    calls.push(bridge);
                    true
                }
                None => false,
            };
            for (_, to_hex, amount_wei, _) in &resolved {
                calls.push(lh_transfer_call(to_hex, *amount_wei)?);
            }

            let purpose = format!(
                "batch-send {} $LH to {} recipients",
                crate::app::format_wei_as_test_eth(total_wei),
                resolved.len()
            );
            // 500k base (first transfer + sponsorship overhead) + ~80k per
            // additional warm transfer + 150k when the bridge rides along.
            let gas = 500_000
                + 80_000 * (resolved.len() as u128 - 1)
                + if bridged { 150_000 } else { 0 };
            let tx_hash =
                crate::app::events::run_sponsored_tempo_call(&from, calls, gas, &purpose)
                    .await
                    .map_err(|e| {
                        crate::error::Error::other(format!("batch_send_lh failed: {e}"))
                    })?;

            // #50: ping each recipient that funds arrived (best-effort, rides
            // the batch). One fire-and-forget notify per transfer.
            for (recipient, to_hex, _, amount_str) in &resolved {
                notify_recipient_of_incoming_lh(
                    recipient.clone(),
                    to_hex.clone(),
                    amount_str.clone(),
                );
            }

            let transfers: Vec<serde_json::Value> = resolved
                .iter()
                .map(|(recipient, to_hex, _, amount_str)| {
                    serde_json::json!({
                        "recipient": recipient,
                        "resolved": to_hex,
                        "amount": amount_str,
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "count": transfers.len(),
                "total": crate::app::format_wei_as_test_eth(total_wei),
                "bridged_from_meter": bridged,
                "transfers": transfers,
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `check_balances()` — read-only snapshot of every $LH pot the agent can
/// spend from (feedback #47: agents could not inspect their own balances,
/// making insufficient-funds reverts undiagnosable). No arguments.
pub(crate) fn check_balances_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {}
    });
    ClosureTool::new(
        "check_balances",
        "Read this agent's $LH balances: the owner WALLET (pays send_lh and \
         x402 agent calls), the chat METER (pays model usage; auto-bridges \
         into the wallet when it is short), and this subdomain's token-bound \
         account (TBA — where bounty rewards and x402 earnings land). The meter \
         splits into a WITHDRAWABLE portion (sendable / bridgeable to the wallet) \
         and a LOCKED portion (fiat-minted $LH, spend-only on inference until its \
         unlock time) — so a send_lh/bridge that would revert InsufficientCredits \
         (LH2024) is visible BEFORE attempting it. Read-only, costs nothing. \
         Returns decimal $LH figures plus raw wei.",
        schema,
        |_args: serde_json::Value, _ctx| async move {
            let (name, owner) = crate::app::tenant::current_tenant_owner()
                .await
                .map_err(crate::error::Error::other)?;
            let wallet = crate::app::registry::token_balance_of(&owner)
                .await
                .unwrap_or(0);
            let meter = crate::app::registry::credit_balance_of(&owner)
                .await
                .unwrap_or(0);
            // Lock split: `withdrawableOf` is the unlocked part the meter→wallet
            // bridge can pull; the rest is locked fiat-origin $LH (spend-only).
            let withdrawable = crate::app::registry::withdrawable_credit_of(&owner)
                .await
                .unwrap_or(meter);
            let meter_locked = meter.saturating_sub(withdrawable);
            // Raw recorded lock (amount, unlockAt) so the agent can say WHEN it frees.
            let (_lock_amt, unlock_at) = crate::app::registry::fiat_locked_of(&owner)
                .await
                .unwrap_or((0, 0));
            let tba_hex = crate::app::registry::tba_of_name(&name)
                .await
                .ok()
                .flatten();
            let tba_balance = match &tba_hex {
                Some(addr) => crate::app::registry::token_balance_of(addr)
                    .await
                    .unwrap_or(0),
                None => 0,
            };
            Ok(serde_json::json!({
                "owner_address": owner,
                "wallet_lh": crate::app::format_wei_as_test_eth(wallet),
                "wallet_wei": wallet.to_string(),
                "meter_lh": crate::app::format_wei_as_test_eth(meter),
                "meter_wei": meter.to_string(),
                "meter_withdrawable_lh": crate::app::format_wei_as_test_eth(withdrawable),
                "meter_withdrawable_wei": withdrawable.to_string(),
                "meter_locked_lh": crate::app::format_wei_as_test_eth(meter_locked),
                "meter_locked_wei": meter_locked.to_string(),
                "meter_lock_unlock_at": unlock_at,
                "tba_address": tba_hex,
                "tba_lh": crate::app::format_wei_as_test_eth(tba_balance),
                "tba_wei": tba_balance.to_string(),
                // Spendable on the WALLET path (send_lh / x402): wallet + the
                // UNLOCKED meter only — locked fiat-$LH can't be bridged out.
                "spendable_total_lh": crate::app::format_wei_as_test_eth(
                    wallet.saturating_add(withdrawable)
                ),
            }))
        },
    )
}

/// `query_balance(target)` — read the LIVE on-chain $LH balance of ANY agent
/// (by name) or 0x address. Agents were guessing peers' balances instead of
/// reading them (krafto on-chain #263); this is the read tool so they stop.
/// Read-only, costs nothing.
pub(crate) fn query_balance_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Hoisted table: `crate::tool_params::QueryBalanceParams`.
    let schema = crate::tool_params::QueryBalanceParams::schema();
    ClosureTool::new(
        "query_balance",
        "Read the LIVE on-chain $LH balance of ANY agent (by name) or 0x address — \
         use this instead of GUESSING a peer's balance. For a name it returns both \
         the owner WALLET and the agent's token-bound account (TBA, where earnings \
         land); for a raw address, that address's balance. Read-only, costs nothing. \
         Decimal $LH plus raw wei.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let target = crate::tool_params::QueryBalanceParams::lenient(&args)
                .target
                .trim()
                .to_string();
            if target.is_empty() {
                return Err(crate::error::Error::other(
                    "query_balance: target (an agent name or 0x address) is required",
                ));
            }
            // A raw 0x address is queried directly; anything else is a name.
            if target.starts_with("0x") && target.len() == 42 {
                let bal = crate::app::registry::token_balance_of(&target)
                    .await
                    .unwrap_or(0);
                return Ok(serde_json::json!({
                    "target": target,
                    "resolved_as": "address",
                    "lh": crate::app::format_wei_as_test_eth(bal),
                    "wei": bal.to_string(),
                }));
            }
            let name = target
                .trim_end_matches(".localharness.xyz")
                .to_lowercase();
            let owner = crate::app::registry::owner_of_name(&name)
                .await
                .ok()
                .flatten();
            let Some(owner) = owner else {
                return Err(crate::error::Error::other(format!(
                    "query_balance: no agent named '{name}' is registered on-chain"
                )));
            };
            let tba = crate::app::registry::tba_of_name(&name).await.ok().flatten();
            let wallet = crate::app::registry::token_balance_of(&owner)
                .await
                .unwrap_or(0);
            let tba_balance = match &tba {
                Some(addr) => crate::app::registry::token_balance_of(addr)
                    .await
                    .unwrap_or(0),
                None => 0,
            };
            Ok(serde_json::json!({
                "target": name,
                "resolved_as": "name",
                "owner_address": owner,
                "wallet_lh": crate::app::format_wei_as_test_eth(wallet),
                "wallet_wei": wallet.to_string(),
                "tba_address": tba,
                "tba_lh": crate::app::format_wei_as_test_eth(tba_balance),
                "tba_wei": tba_balance.to_string(),
            }))
        },
    )
}
