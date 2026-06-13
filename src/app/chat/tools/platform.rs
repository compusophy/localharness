// =============================================================================
// Platform-level closure tools (browser-specific; not in the SDK builtins).
// =============================================================================

use crate::app::chat::access::{
    build_actor_setup, transfer_selector, u256_be, withdraw_credits_selector,
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

/// ERC-20 `transfer(to, amount)` TempoCall against the $LH token.
fn lh_transfer_call(
    to_hex: &str,
    amount_wei: u128,
) -> Result<crate::tempo_tx::TempoCall, crate::error::Error> {
    let to_bytes = parse_address(to_hex).map_err(crate::error::Error::other)?;
    let mut to_padded = [0u8; 32];
    to_padded[12..].copy_from_slice(&to_bytes);
    let mut calldata = Vec::with_capacity(4 + 64);
    calldata.extend_from_slice(&transfer_selector());
    calldata.extend_from_slice(&to_padded);
    calldata.extend_from_slice(&u256_be(amount_wei));
    let token_addr = parse_address(crate::registry::LOCALHARNESS_TOKEN_ADDRESS)
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
    let diamond = parse_address(crate::registry::REGISTRY_ADDRESS)
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
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Subdomain to register, e.g. \"alice\" \
                    becomes alice.localharness.xyz. 3-32 chars; lowercase \
                    letters, digits, and hyphens only."
            },
            "persona": {
                "type": "string",
                "description": "OPTIONAL system instruction / persona for the new \
                    agent — published on-chain as its system prompt (the persona \
                    that headless `call`s and the public face read). Omit to leave \
                    the default."
            },
            "prefund_lh": {
                "type": "string",
                "description": "OPTIONAL amount of $LH to prefund the new agent with, \
                    as a decimal string (\"5\", \"1.5\"). Transferred from YOUR \
                    wallet to the new subdomain's token-bound account (its own \
                    spendable wallet — used to pay other agents via x402). Omit, or \
                    pass \"0\", to skip. Must not exceed your $LH balance."
            }
        },
        "required": ["name"]
    });
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
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            let persona = args.get("persona").and_then(|v| v.as_str());
            let prefund_lh = args.get("prefund_lh").and_then(|v| v.as_str());
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

/// `create_and_publish_app(name, source)` — one-shot: register
/// `<name>.localharness.xyz` AND publish a compiled rustlite cartridge as
/// its public face, so "make me a clock subdomain" works in a single tool
/// call. Compiles `source` first (so a bad cartridge fails before the
/// on-chain register), claims the name via the iframe (master wallet ends
/// up holding the new tokenId), resolves the tokenId, then publishes via a
/// SPONSORED setMetadata batch (app.wasm bytes + public_face="app") in ONE
/// Tempo tx — exactly like the admin publish-app flow. Returns
/// `{ name, url, tx_hash }`.
pub(crate) fn create_and_publish_app_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Subdomain to register, e.g. \"clock\" \
                    becomes clock.localharness.xyz. 3-32 chars; lowercase \
                    letters, digits, and hyphens only."
            },
            "source": {
                "type": "string",
                "description": "rustlite cartridge source — the SAME dialect as \
                    run_cartridge. Exports `fn frame(t: i32)` (animated) or \
                    `fn render()` and draws via `use host::display;`. This becomes \
                    the subdomain's fullscreen public face."
            },
            "persona": {
                "type": "string",
                "description": "OPTIONAL system instruction / persona for the new \
                    agent — published on-chain as its system prompt (read by \
                    headless `call`s). Omit to leave the default."
            },
            "prefund_lh": {
                "type": "string",
                "description": "OPTIONAL amount of $LH to prefund the new agent with, \
                    as a decimal string (\"5\", \"1.5\"). Transferred from YOUR \
                    wallet to the new subdomain's token-bound account (its own \
                    spendable wallet). Omit, or pass \"0\", to skip. Must not exceed \
                    your $LH balance."
            }
        },
        "required": ["name", "source"]
    });
    ClosureTool::new(
        "create_and_publish_app",
        "One-shot: register a new <name>.localharness.xyz AND publish a compiled \
         rustlite cartridge as its fullscreen public face, in a single call (compile \
         + on-chain register + sponsored setMetadata publish). Use this for \"make me \
         a clock/<app> subdomain\". The ACTOR MODEL: optionally also set the new \
         agent's `persona` (on-chain system instruction) and `prefund_lh` it with $LH \
         (into its token-bound account), all in the SAME sponsored tx. create_subdomain \
         remains for registering a name-only subdomain. Returns { name, url, tx_hash, \
         persona_set?, prefunded_lh?, tba? }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            let source = args.get("source").and_then(|v| v.as_str()).unwrap_or("");
            let persona = args.get("persona").and_then(|v| v.as_str());
            let prefund_lh = args.get("prefund_lh").and_then(|v| v.as_str());
            let cleaned = crate::subdomain::validate(name)
                .map_err(|why| crate::error::Error::other(format!("invalid subdomain name: {why}")))?;
            if source.trim().is_empty() {
                return Err(crate::error::Error::other("source cannot be empty"));
            }
            // Compile FIRST so a bad cartridge fails before we register the
            // name on-chain. Surface the FULL rendering (LH code + line/col
            // locator + caret snippet) so the agent fixes the exact spot.
            let wasm = crate::rustlite::compile(source).map_err(|e| {
                crate::error::Error::other(format!("compile failed: {}", e.render(source)))
            })?;
            if wasm.len() > 16_384 {
                return Err(crate::error::Error::other(format!(
                    "app wasm too large to publish: {} bytes (max 16384)",
                    wasm.len()
                )));
            }
            // Register the name. The owner's master wallet ends up holding
            // the new tokenId, so it's authorized to setMetadata below.
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
            // Publish: app wasm bytes + public_face="app" in ONE sponsored
            // Tempo tx (two setMetadata calls), exactly like the admin
            // publish-app flow. Owner signs the sender_hash via the apex
            // iframe; the sponsor pays gas.
            let registry_addr = parse_address(crate::app::registry::REGISTRY_ADDRESS)
                .map_err(crate::error::Error::other)?;
            let mk = |input: Vec<u8>| crate::tempo_tx::TempoCall {
                to: registry_addr,
                value_wei: 0,
                input,
            };
            let mut calls = vec![
                mk(crate::app::registry::encode_set_app_wasm(token_id, &wasm)),
                mk(crate::app::registry::encode_set_public_face(token_id, "app")),
            ];
            // Length-scaled: the old `1.3M + words*40k` here was ~6x below
            // the measured ~7.6k gas/BYTE and silently OOG-reverted any
            // non-trivial publish (see `gas::set_metadata_gas`).
            let mut gas = crate::app::gas::set_metadata_gas(wasm.len());
            // ACTOR MODEL: fold optional persona + prefund into the SAME tx.
            let setup =
                build_actor_setup(&owner, token_id, &cleaned, persona, prefund_lh).await?;
            calls.extend(setup.calls);
            gas += setup.extra_gas;
            let tx_hash = crate::app::events::run_sponsored_tempo_call(
                &owner,
                calls,
                gas,
                "create + publish app",
            )
            .await
            .map_err(|e| crate::error::Error::other(format!("publish failed: {e}")))?;
            let mut result = serde_json::json!({
                "name": cleaned,
                "url": format!("https://{cleaned}.localharness.xyz/"),
                "tx_hash": tx_hash,
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
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Subdomain whose published cartridge to embed, \
                    e.g. \"pong\" embeds pong.localharness.xyz's app inline."
            }
        },
        "required": ["name"]
    });
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
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
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
            // `#embed-canvas` card once the inline card has painted.
            crate::app::display::stash_pending_embed(wasm);
            Ok(serde_json::json!({
                "name": cleaned,
                "url": format!("https://{cleaned}.localharness.xyz/"),
                "embedded": true,
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
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Subdomain to release/recycle — burns the NFT, frees the name."
            },
            "confirmation": {
                "type": "string",
                "description": "Single-use confirmation code. OMIT (or pass \"\") on the \
                    first call — it returns a challenge code that is shown to the owner. \
                    Relay it, wait for the owner to TYPE that code in chat, then retry \
                    with the code here. Never invent it; only the platform issues it."
            }
        },
        "required": ["name"]
    });
    ClosureTool::new(
        "release_subdomain",
        "DESTRUCTIVE + IRREVERSIBLE: burn a subdomain NFT and free its name. The first \
         call does NOT execute: it returns a single-use confirmation code (also shown to \
         the owner in the UI). Ask the owner to TYPE that code in chat, then retry with \
         `confirmation` set to it — the call only executes after the owner's message \
         contains the code. Refuses your MAIN. Returns the tx hash.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            if name.is_empty() {
                return Err(crate::error::Error::other("name is required"));
            }
            // The typed-confirmation gate (confirm_guard) runs BEFORE this body
            // and denies any call without a user-typed challenge code. This
            // belt-and-suspenders check only guards a registration path that
            // forgot the hook.
            let confirmed = args
                .get("confirmation")
                .and_then(|v| v.as_str())
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
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "names": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Subdomain names to register in ONE tx, e.g. \
                    [\"alice\",\"bob\"] -> alice.localharness.xyz, \
                    bob.localharness.xyz. Each: 3-32 chars, lowercase letters, \
                    digits, hyphens. Already-taken or invalid names are skipped \
                    and reported back. Max 20 per call."
            }
        },
        "required": ["names"]
    });
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
            let requested: Vec<String> = args
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
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What to look for — capabilities, topics, or \
                        keywords matched (case-insensitively) against agent names \
                        and personas. Several keywords are ORed and ranked by \
                        overlap (e.g. \"solidity audit security\"). \
                        Empty returns recent agents."
                }
            },
            "required": ["query"]
        }),
        |args: serde_json::Value, _ctx| async move {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
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
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "recipient": {
                "type": "string",
                "description": "Who receives the $LH: either a raw 0x… 20-byte \
                    address, or a subdomain name like \"alice\" (the funds go to \
                    that subdomain's on-chain OWNER address)."
            },
            "amount": {
                "type": "string",
                "description": "Amount of $LH to send, as a decimal string \
                    (e.g. \"5\", \"1.5\", \"0.01\"). Must be greater than 0."
            },
            "confirmation": {
                "type": "string",
                "description": "Single-use confirmation code. OMIT (or pass \"\") on the \
                    first call — it returns a challenge code shown to the owner. Relay \
                    it, wait for the owner to TYPE the code in chat, then retry with it. \
                    Never invent it; only the platform issues it."
            }
        },
        "required": ["recipient", "amount"]
    });
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

            let recipient_arg = args
                .get("recipient")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let amount_arg = args
                .get("amount")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();

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
                total_wei = total_wei.saturating_add(amount_wei);
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
         account (TBA — where bounty rewards and x402 earnings land). \
         Read-only, costs nothing. Returns decimal $LH figures plus raw wei.",
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
                "tba_address": tba_hex,
                "tba_lh": crate::app::format_wei_as_test_eth(tba_balance),
                "tba_wei": tba_balance.to_string(),
                "spendable_total_lh": crate::app::format_wei_as_test_eth(
                    wallet.saturating_add(meter)
                ),
            }))
        },
    )
}
