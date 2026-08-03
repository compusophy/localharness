// =============================================================================
// Platform-level closure tools (browser-specific; not in the SDK builtins).
// =============================================================================

use crate::app::chat::access::{
    build_actor_setup, lh_transfer_calldata, u256_be, withdraw_credits_selector,
};
use crate::encoding::parse_address;
use crate::tools::ClosureTool;

/// Resolve a `recipient` arg (raw 0x… address or subdomain name) to the
/// 0x… address that receives $LH (a name pays its on-chain OWNER). `tool`
/// names the caller for the arg-rejection (`Error::bad_args`) arm.
async fn resolve_lh_recipient(tool: &str, recipient_arg: &str) -> Result<String, crate::error::Error> {
    use crate::encoding::Recipient;
    let kind = crate::encoding::classify_recipient(recipient_arg)
        .map_err(|e| crate::error::Error::bad_args(tool, e))?;
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

/// `create_subdomain(name, source?, persona?, prefund_lh?)` — register
/// `<name>.localharness.xyz` on the LocalharnessRegistry diamond (the ACTOR
/// MODEL), signed by the owner's apex wallet via the iframe signer, and
/// OPTIONALLY publish an app onto it in the SAME call (telemetry #86 merged the
/// old `create_and_publish_app` in here — one tool, `source` is what makes it
/// an app):
/// - `name` only → a bare name-only sponsored mint.
/// - `name` + `source` → compile the rustlite `source` FIRST (a bad cartridge
///   fails before any on-chain write), then publish it as the subdomain's
///   fullscreen public face (OFF-CHAIN, free). OWNERSHIP-AWARE when a source is
///   given: an UNREGISTERED name is registered first; a name YOU already own is
///   UPDATED in place (no re-register, no duplicate); a name owned by SOMEONE
///   ELSE is refused. Auto-embeds the just-published cartridge inline.
/// - OPTIONAL `persona` (on-chain system instruction) + `prefund_lh` (move $LH
///   into the new agent's token-bound account) configure the spawned actor on
///   the FRESH-mint paths.
///
/// Returns a superset reporting what happened: name-only →
/// `{ name, url, owner, tx_hash, persona_set?, prefunded_lh?, tba? }`; with a
/// published app → `{ name, url, published: true, off_chain: true, updated,
/// tx_hash?, persona_set?, prefunded_lh?, tba? }`.
pub(crate) fn create_subdomain_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Schema + lenient extraction from ONE hoisted table
    // (`crate::tool_params::CreateSubdomainParams`), byte-identity-tested natively.
    let schema = crate::tool_params::CreateSubdomainParams::schema();
    ClosureTool::new(
        "create_subdomain",
        "Register a new <name>.localharness.xyz subdomain on-chain (the ACTOR MODEL) — \
         the owner's master wallet pays gas and ends up holding the resulting ERC-721 \
         NFT. Give ONLY `name` for a bare name-only subdomain (\"create/make/spin up a \
         subdomain\"); NEVER run_cartridge, which does not create a subdomain. Give \
         `source` too (a rustlite cartridge, the SAME dialect as run_cartridge) to ALSO \
         publish it as the subdomain's fullscreen public face in one call — the way to \
         make a subdomain that IS an app (\"make me a clock/<app> subdomain\"). Compiles \
         FIRST, so a bad cartridge fails before any write; publishes OFF-CHAIN (free, no \
         gas). OWNERSHIP-AWARE with a source: an UNREGISTERED name is registered first, a \
         name YOU already own is UPDATED in place (no re-register, no duplicate), a name \
         owned by someone ELSE is refused. OPTIONAL actor extras: `persona` publishes the \
         new agent's on-chain system instruction; `prefund_lh` moves that much $LH from \
         your wallet into its token-bound account (its own spendable wallet). Returns \
         { name, url, ... }: name-only adds { owner, tx_hash }; a published app adds \
         { published: true, off_chain: true, updated }. Give the user the returned url as \
         a clickable link.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let params = crate::tool_params::CreateSubdomainParams::lenient(&args);
            let name = params.name.trim();
            // A blank/whitespace `source` means "no app" — a name-only mint.
            let source = params.source.as_deref().map(str::trim).filter(|s| !s.is_empty());
            let persona = params.persona.as_deref();
            let prefund_lh = params.prefund_lh.as_deref();
            // Validate (don't silently mangle) — an invalid name returns a clear
            // reason to the agent instead of minting a DIFFERENT name (#66/#60).
            let cleaned = crate::subdomain::validate(name).map_err(|why| {
                crate::error::Error::bad_args("create_subdomain", format!("invalid subdomain name: {why}"))
            })?;
            match source {
                // name + source → compile + register + publish (ownership-aware).
                Some(src) => create_subdomain_with_app(&cleaned, src, persona, prefund_lh).await,
                // name only → the sponsored mint (+ optional actor setup).
                None => create_subdomain_name_only(&cleaned, persona, prefund_lh).await,
            }
        },
    )
}

/// The name-only `create_subdomain` path (no `source`): a sponsored mint plus
/// optional actor-model persona/prefund. The master wallet ends up holding the
/// new id.
async fn create_subdomain_name_only(
    cleaned: &str,
    persona: Option<&str>,
    prefund_lh: Option<&str>,
) -> Result<serde_json::Value, crate::error::Error> {
    // Register the name first (master wallet ends up holding the new id).
    let (owner, claim_tx) = crate::app::verify::claim_name_via_iframe(cleaned)
        .await
        .map_err(|e| crate::error::Error::other(format!("claim failed: {e}")))?;
    // Proactively push this device's Gemini key to the MAIN slot so the new
    // subdomain inherits it (no re-save).
    {
        let n = cleaned.to_string();
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
        let token_id = match crate::app::registry::id_of_name(cleaned).await {
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
            "create_subdomain",
            &owner,
            token_id,
            cleaned,
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
            .map_err(|e| crate::error::Error::other(format!("actor setup failed: {e}")))?;
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
}

/// The `create_subdomain` WITH-app path (a non-empty `source`) — OWNERSHIP-AWARE
/// one-shot publish (was `create_and_publish_app`):
/// - `cleaned` UNREGISTERED → register `<name>.localharness.xyz` + publish the
///   compiled cartridge as its public face (a fresh subdomain for the app).
/// - `cleaned` already owned by THE CALLER → UPDATE in place: re-publish the
///   cartridge OFF-CHAIN, NO re-register, no duplicate.
/// - `cleaned` owned by SOMEONE ELSE → refuse with a clear error.
///
/// Compiles inside `publish_app_face` FIRST (a bad cartridge fails before any
/// write), then publishes OFF-CHAIN to the app store (free, no gas — the chain
/// keeps only ownership). For a FRESH name the optional persona + prefund are set
/// on-chain separately (small, sponsored). A brand-new app never silently
/// overwrites the owner's MAIN. Stashes the compiled wasm so the cartridge loop
/// auto-embeds it inline.
async fn create_subdomain_with_app(
    cleaned: &str,
    source: &str,
    persona: Option<&str>,
    prefund_lh: Option<&str>,
) -> Result<serde_json::Value, crate::error::Error> {
    // Who would sign? The owner of the current host subdomain — the master
    // wallet that holds ALL this identity's names. Used to decide OWN vs
    // SOMEONE-ELSE for an already-registered target.
    let signer_owner = crate::app::tenant::current_tenant_owner()
        .await
        .map(|(_, o)| o)
        .ok();

    // Branch on the target's on-chain ownership.
    let existing = match &signer_owner {
        Some(o) => owned_token_for_publish(cleaned, o).await?,
        // Off a tenant host (preview/localhost) we can't prove the signer's
        // identity; fall back to "register if free", and a taken name will be
        // refused by the claim path.
        None => match crate::app::registry::owner_of_name(cleaned).await {
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

    // UPDATE path: the caller already owns `name` → re-publish in place, NO
    // re-register (which would fail), NO persona/prefund (those are spawn-time
    // actor setup). Store-only publish, no tx.
    if let Some((_token_id, owner)) = existing {
        let wasm = publish_app_face(cleaned, source, &owner).await?;
        stash_published_app_embed(cleaned, wasm);
        return Ok(serde_json::json!({
            "name": cleaned,
            "url": format!("https://{cleaned}.localharness.xyz/"),
            "published": true,
            "off_chain": true,
            "updated": true,
        }));
    }

    // FRESH path: register the name, then publish. The owner's master wallet
    // ends up holding the new tokenId, so it's authorized to setMetadata below.
    let (owner, _claim_tx) = crate::app::verify::claim_name_via_iframe(cleaned)
        .await
        .map_err(|e| crate::error::Error::other(format!("claim failed: {e}")))?;
    // Inherit this device's Gemini key onto the new subdomain.
    {
        let n = cleaned.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            crate::app::events::sync_local_key_to_main(&n).await;
        });
    }
    // Resolve the freshly-minted tokenId.
    let token_id = match crate::app::registry::id_of_name(cleaned).await {
        Ok(id) if id != 0 => id,
        Ok(_) => {
            return Err(crate::error::Error::other(
                "registered but tokenId not yet visible on-chain — retry publish shortly",
            ))
        }
        Err(e) => return Err(crate::error::Error::other(format!("id_of_name: {e}"))),
    };
    // Publish the app OFF-CHAIN (free) to the app store — the owner's master
    // wallet (just minted the name) signs the proxy auth token.
    let wasm = publish_app_face(cleaned, source, &owner).await?;
    stash_published_app_embed(cleaned, wasm);
    // ACTOR MODEL: persona + prefund stay ON-CHAIN (identity / economy
    // primitives, small/cheap unlike the app bytes). Submit them as their own
    // sponsored batch only if either was requested.
    let setup =
        build_actor_setup("create_subdomain", &owner, token_id, cleaned, persona, prefund_lh)
            .await?;
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
    let mut result = serde_json::json!({
        "name": cleaned,
        "url": format!("https://{cleaned}.localharness.xyz/"),
        "published": true,
        "off_chain": true,
        "updated": false,
    });
    // The publish itself is store-only; the only tx that can exist is the actor
    // setup (persona/prefund). Omitted otherwise so no model ever relays a fake
    // "hash".
    if let Some(tx) = setup_tx {
        result["tx_hash"] = serde_json::json!(tx);
    }
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
}

/// The device's MASTER-wallet signer, asserted to be `owner`. Publishing is
/// STORE-ONLY: the proxy authorizes via `ownerOf(name) == token signer`, so the
/// token MUST be signed by the OWNER — read `APP.wallet` (the master) DIRECTLY,
/// NOT credit_signer(), which can return (or even MINT) a per-origin DEVICE key
/// that is not the owner. No on-chain fallback exists anymore (purged with the
/// pre-1.0.0 reset): a TBA-owned name or a linked device without the seed gets
/// an honest error until the store gains TBA/authorized-signer auth.
fn owner_master_signer(
    tool: &str,
    owner: &str,
) -> Result<k256::ecdsa::SigningKey, crate::error::Error> {
    let master = crate::app::APP
        .with(|c| c.borrow().wallet.as_ref().map(|w| (w.signer.clone(), w.address)));
    match master {
        Some((signer, addr))
            if owner.eq_ignore_ascii_case(&crate::encoding::bytes_to_hex_str(&addr)) =>
        {
            Ok(signer)
        }
        _ => Err(crate::error::Error::other(format!(
            "{tool}: publishing needs this device to hold the owner wallet of the name \
             (owner {owner}); TBA-owned names / linked devices without the seed can't \
             publish yet"
        ))),
    }
}

/// Publish a compiled rustlite cartridge as `name`'s app face — the ONE
/// publish-app shape shared by `create_subdomain` (fresh mint + in-place
/// update), `publish_app_to` (cross-subdomain), and `publish_public_face` ("app").
///
/// STORE-ONLY (free, no gas, no tx): the compiled wasm goes to the app store
/// (`registry::publish_app_to_store`), which the proxy authorizes via on-chain
/// ownership and which stamps the `<name>/face` choice record in the SAME
/// publish — bytes + routing in one authed POST, so the "stored but never
/// routed" bug (krafto) is structurally impossible. Returns the compiled wasm
/// so callers can auto-embed the just-published app inline (close the
/// cartridge loop) without recompiling.
async fn publish_app_face(
    name: &str,
    source: &str,
    owner: &str,
) -> Result<Vec<u8>, crate::error::Error> {
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
    let signer = owner_master_signer("publish", owner)?;
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let token = crate::registry::proxy_auth_token(&signer, now, "publish");
    crate::app::registry::publish_app_to_store(name, &token, &wasm, source)
        .await
        .map_err(|e| crate::error::Error::other(format!("publish failed: {e}")))?;
    Ok(wasm)
}

/// Publish an HTML page as `name`'s public face — the HTML-face sibling of
/// [`publish_app_face`]. STORE-ONLY (free); same owner-master-signer rule.
async fn publish_html_face(
    name: &str,
    html: &[u8],
    owner: &str,
) -> Result<(), crate::error::Error> {
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
    let signer = owner_master_signer("publish", owner)?;
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let token = crate::registry::proxy_auth_token(&signer, now, "publish");
    let html_str = String::from_utf8_lossy(html).into_owned();
    crate::app::registry::publish_html_to_store(name, &token, &html_str)
        .await
        .map_err(|e| crate::error::Error::other(format!("publish failed: {e}")))
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

/// AUTO-EMBED on a successful app publish via `create_subdomain` (close the
/// cartridge loop): stash the just-published cartridge for the transcript card
/// `chat::stream_turn` paints for THIS tool result — the SAME
/// stash-then-`launch_pending_embed` path embed_app/run_cartridge use, so the
/// build ends with the cartridge PLAYING inline, deterministically (never
/// reliant on the model calling embed_app). `run_wasm_inline` also remembers
/// the bytes for the card's [fullscreen] relaunch. One stash per build; a
/// re-publish paints a fresh card and overwrites the stash (debounced by the
/// launch path's take()).
fn stash_published_app_embed(name: &str, wasm: Vec<u8>) {
    crate::app::display::set_cartridge_ref(Some(format!("published app: {name}")));
    crate::app::display::run_wasm_inline(&wasm);
}

/// `publish_app_to(name, source, confirmation)` — UPDATE-FROM-MAIN: publish a
/// compiled cartridge to ANY subdomain the caller OWNS, even one DIFFERENT from
/// the current host. The owner's master wallet (the one that signs the current
/// host's sponsored writes) holds all their subdomain NFTs, so it can sign a
/// `setMetadata` for any owned tokenId — no new ownership/actor model needed,
/// just targeting a chosen owned name. From a MAIN session this updates any
/// alt's app. The target MUST already be registered AND owned by the caller
/// (refuses unregistered names — use `create_subdomain` with a `source` to mint
/// a fresh one — and names owned by someone else). OVERWRITES what that name serves to
/// every visitor, so it rides the typed-confirmation gate
/// (`chat::confirm_guard`). NOT granted to subagents. Publishes off-chain (the
/// store; tx only on the TBA fallback). Returns `{ name, url, off_chain,
/// updated: true }`.
pub(crate) fn publish_app_to_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Hoisted table: `crate::tool_params::PublishAppToParams`.
    let schema = crate::tool_params::PublishAppToParams::schema();
    ClosureTool::new(
        "publish_app_to",
        "Publish (UPDATE) a rustlite cartridge to ANOTHER subdomain you OWN — the \
         update-from-MAIN path. The owner's master wallet holds all their subdomain \
         NFTs, so from one session you can re-publish any of your alts' apps. The \
         target must ALREADY exist and be owned by you (to mint a NEW subdomain use \
         create_subdomain with a `source`; that also updates the CURRENT name in place). \
         OVERWRITES that subdomain's published app (off-chain, free) — the first \
         call does NOT execute: it returns a single-use confirmation code (also \
         shown to the owner in the UI). Say which subdomain you'll update, ask the \
         owner to TYPE the code, then retry with `confirmation` set to it. \
         Returns { name, url, off_chain, updated: true }.",
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
                return Err(crate::error::Error::bad_args(
                    "publish_app_to",
                    "publish_app_to requires the platform-issued confirmation code",
                ));
            }
            let cleaned = crate::subdomain::validate(name).map_err(|why| {
                crate::error::Error::bad_args("publish_app_to", format!("invalid subdomain name: {why}"))
            })?;
            if source.trim().is_empty() {
                return Err(crate::error::Error::bad_args("publish_app_to", "source cannot be empty"));
            }
            // The signer = the current host's owner (the master wallet holding
            // ALL this identity's names). Required so we can prove ownership of a
            // DIFFERENT target name before writing to it.
            let (_, signer_owner) = crate::app::tenant::current_tenant_owner()
                .await
                .map_err(crate::error::Error::other)?;
            // Resolve + ownership-gate the target. None = unregistered (refuse —
            // this tool only UPDATES owned names); Err = owned-by-other / RPC.
            let (_token_id, owner) = owned_token_for_publish(&cleaned, &signer_owner)
                .await?
                .ok_or_else(|| {
                    crate::error::Error::other(format!(
                        "\"{cleaned}\" is not registered — use create_subdomain with a \
                         `source` to mint and publish a new subdomain"
                    ))
                })?;
            // (No auto-embed here: the target is a DIFFERENT subdomain being
            // updated from MAIN; the wasm is dropped.)
            let _wasm = publish_app_face(&cleaned, source, &owner).await?;
            Ok(serde_json::json!({
                "name": cleaned,
                "url": format!("https://{cleaned}.localharness.xyz/"),
                "off_chain": true,
                "updated": true,
            }))
        },
    )
}

/// `embed_app(name)` — fetch ANOTHER subdomain's published cartridge and
/// render it INLINE in the chat transcript as a live, interactive card (NOT
/// an iframe — cartridges are framebuffer wasm; an iframe of a subdomain that
/// itself boots a cartridge hits recursion/partitioning limits). Resolves
/// `name` → published `app.wasm` (the off-chain store); if the subdomain has a published
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
                return Err(crate::error::Error::bad_args("embed_app", "name cannot be empty"));
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
/// `choice` is "directory" | "app" | "html". EVERY choice publishes to the app
/// store (free, no gas, no tx — the store stamps the `<name>/face` choice
/// record; content publishes carry their bytes in the same POST). Nothing
/// touches the chain. Owner-only, own subdomain only. Mirrors
/// `events::public_face::run_set_public_face` minus the DOM. Reversible.
pub(crate) fn publish_public_face_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Hoisted table: `crate::tool_params::PublishPublicFaceParams`.
    let schema = crate::tool_params::PublishPublicFaceParams::schema();
    ClosureTool::new(
        "publish_public_face",
        "Publish YOUR OWN public face — what a visitor to \
         https://<you>.localharness.xyz/ sees — the chat equivalent of admin → \
         public face. `choice`: \"app\" compiles + publishes this device's local \
         app.rl as a fullscreen cartridge; \"html\" publishes local index.html; \
         \"directory\" sets a profile landing. All OFF-CHAIN to the app store \
         (free, no gas, no transaction). Zero-click. Works only on your own \
         subdomain. After it succeeds, give the user the returned `url`. \
         Returns { choice, url, off_chain }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let choice = crate::tool_params::PublishPublicFaceParams::lenient(&args)
                .choice
                .trim()
                .to_lowercase();
            if !matches!(choice.as_str(), "directory" | "app" | "html") {
                return Err(crate::error::Error::bad_args(
                    "publish_public_face",
                    "choice must be \"directory\", \"app\", or \"html\"",
                ));
            }
            let Some(name) = crate::app::tenant::current_name() else {
                return Err(crate::error::Error::other(
                    "publish_public_face only works on your own subdomain",
                ));
            };
            let owner = match crate::app::registry::owner_of_name(&name).await {
                Ok(Some(o)) => o,
                _ => return Err(crate::error::Error::other("name isn't registered on-chain")),
            };
            // Every face publishes to the STORE (which stamps the choice);
            // nothing here touches the chain.
            match choice.as_str() {
                "directory" => {
                    // FACE-ONLY store write (free).
                    let signer = owner_master_signer("publish_public_face", &owner)?;
                    let now = (js_sys::Date::now() / 1000.0) as u64;
                    let token = crate::registry::proxy_auth_token(&signer, now, "publish");
                    crate::app::registry::publish_face_to_store(&name, &token, "directory")
                        .await
                        .map_err(|e| {
                            crate::error::Error::other(format!("publish failed: {e}"))
                        })?;
                }
                "app" => {
                    // Publish this device's local app.rl — bytes + face
                    // record in one POST.
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
                    let _wasm = publish_app_face(&name, &src, &owner).await?;
                }
                "html" => {
                    // Publish this device's local index.html — same shape.
                    let fs = crate::app::shared_opfs();
                    let html = match fs.read("index.html").await {
                        Ok(b) if !b.is_empty() => b,
                        _ => {
                            return Err(crate::error::Error::other(
                                "no index.html on this device — create one first, then publish",
                            ))
                        }
                    };
                    publish_html_face(&name, &html, &owner).await?;
                }
                _ => unreachable!(),
            }
            Ok(serde_json::json!({
                "choice": choice,
                "url": format!("https://{name}.localharness.xyz/"),
                "off_chain": true,
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
                return Err(crate::error::Error::bad_args("release_subdomain", "name is required"));
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
                return Err(crate::error::Error::bad_args(
                    "release_subdomain",
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
/// (only registered on the main agent). More than 8 targets are auto-chunked
/// into sequential sponsored txs (`crate::relay_chunk` owns the relay's
/// per-tx call cap + the reserved-slot rules; telemetry #85/#88).
pub(crate) fn bulk_release_subdomains_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "names": {
                "type": "array",
                "items": { "type": "string" },
                "description": "OPTIONAL subset of subdomain names to release in one \
                    batch. Omit to target EVERY non-MAIN subdomain the owner holds. \
                    At most 28 names per call — for more, pass explicit subsets in \
                    separate calls."
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
         with `names`, only that subset. At most 28 names per call — for more, pass \
         explicit `names` subsets in separate calls. The first call does NOT execute: it \
         returns a single-use confirmation code (also shown to the owner in the UI). Show \
         the owner the exact list that will be burned (use list_subdomains), ask them to \
         TYPE the code, then retry with `confirmation` set to it. ONE code for the whole \
         batch; more than 8 names are split across multiple sponsored txs automatically \
         (each tx burns at most 8). Returns { released, count, tx_hashes, failed, \
         unconfirmed, unattempted } — `failed` lists chunks whose tx FAILED (those names \
         were NOT burned); `unconfirmed` lists chunks whose receipt TIMED OUT (the tx MAY \
         still land — check its tx_hash before retrying; the batch stops there); \
         `unattempted` lists names never tried (the batch stops early after 2 \
         consecutive failed chunks, an unconfirmed chunk, or a user Stop).",
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
                return Err(crate::error::Error::bad_args(
                    "bulk_release_subdomains",
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
            // Hard total bound (relay_chunk::MAX_BATCH_ITEMS): the spend/burn
            // ceiling one confirmed call may carry.
            if let Some(msg) =
                crate::relay_chunk::over_batch_limit("bulk_release_subdomains", targets.len())
            {
                return Err(crate::error::Error::bad_args(
                    "bulk_release_subdomains",
                    format!("{msg} Pass explicit `names` subsets."),
                ));
            }
            // Each release is one call; the relay caps a sponsored tx at 8
            // calls, so >8 targets auto-chunk into sequential sponsored txs
            // (crate::relay_chunk; telemetry #85/#88 — the old client-side
            // reject). A failed chunk is reported and the rest continue —
            // UNLESS the breaker trips (2 consecutive failures), a chunk's
            // receipt times out (chain state unknown — stop immediately), or
            // the user pressed Stop (the dwell idiom): then the remaining
            // chunks report `unattempted`.
            let ranges = crate::relay_chunk::chunk_ranges(targets.len(), false);
            let mut outcomes: Vec<crate::relay_chunk::ChunkOutcome> =
                Vec::with_capacity(ranges.len());
            let mut released_all: Vec<String> = Vec::new();
            for r in &ranges {
                if crate::app::chat::turn_cancelled()
                    || crate::relay_chunk::should_stop(&outcomes)
                {
                    break;
                }
                match crate::app::events::run_bulk_release(&targets[r.clone()]).await {
                    Ok((released, tx)) => {
                        released_all.extend(released);
                        outcomes.push(crate::relay_chunk::ChunkOutcome::Landed(tx));
                    }
                    Err(e) => outcomes.push(crate::relay_chunk::classify_failure(e)),
                }
            }
            let fold = crate::relay_chunk::fold_outcomes(&ranges, &outcomes);
            // Total loss (nothing burned, nothing pending) keeps the plain
            // error contract; any landed OR unconfirmed outcome must return
            // the structured breakdown instead (an Err would falsely claim
            // nothing happened).
            if released_all.is_empty() && fold.unconfirmed.is_empty() {
                if let Some((_, e)) = fold.chunk_errors.first() {
                    return Err(crate::error::Error::other(format!("bulk release failed: {e}")));
                }
            }
            let failed: Vec<serde_json::Value> = fold
                .chunk_errors
                .iter()
                .map(|(ci, err)| serde_json::json!({
                    "names": targets[ranges[*ci].clone()],
                    "error": err,
                }))
                .collect();
            // Receipt TIMEOUT ≠ revert: these txs MAY still land. Surface the
            // hash and never claim the names were (or were not) burned.
            let unconfirmed: Vec<serde_json::Value> = fold
                .unconfirmed_txs
                .iter()
                .map(|(ci, tx)| serde_json::json!({
                    "names": targets[ranges[*ci].clone()],
                    "tx_hash": tx,
                    "note": "receipt timed out — the tx may still land; check the \
                             tx hash before retrying these names",
                }))
                .collect();
            let unattempted: Vec<&String> =
                fold.unattempted.iter().map(|&i| &targets[i]).collect();
            Ok(serde_json::json!({
                "released": released_all,
                "count": released_all.len(),
                "tx_hashes": fold.tx_hashes,
                "failed": failed,
                "unconfirmed": unconfirmed,
                "unattempted": unattempted,
            }))
        },
    )
}

/// `batch_create_subdomains(names, confirmation)` — register MANY subdomains
/// in batched sponsored multi-call txs (the mirror of `bulk_release_subdomains`).
/// VALUE-MOVING on mainnet (each registration pulls `registrationCost()` — live:
/// 1 $LH — from the owner's wallet), so it rides the typed-confirmation gate
/// (`chat::confirm_guard`); the old "additive, no confirmation" rationale died
/// with the hard cap. The sanctioned mass-registration path — a few txs instead
/// of an N-deep `create_subdomain` loop. Names are sanitised +
/// availability-checked; taken/invalid names are skipped and reported. >7 names
/// auto-chunk (`crate::relay_chunk`; the paid-claim approve reserves one slot
/// per chunk — telemetry #85/#88); at most `MAX_BATCH_ITEMS` per call. Not
/// granted to subagents (same restraint as bulk_release).
pub(crate) fn batch_create_subdomains_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Hoisted table: `crate::tool_params::BatchCreateSubdomainsParams`,
    // byte-identity-tested natively.
    let schema = crate::tool_params::BatchCreateSubdomainsParams::schema();
    ClosureTool::new(
        "batch_create_subdomains",
        "Register MANY <name>.localharness.xyz subdomains on-chain in batched \
         sponsored transactions. PREFER THIS over calling create_subdomain in \
         a loop when registering more than one name. The owner's master wallet \
         ends up holding every resulting ERC-721 NFT. SPENDS $LH (each \
         registration costs the on-chain registration fee — 1 $LH on mainnet), \
         so the first call does NOT execute: it returns a single-use \
         confirmation code (also shown to the owner in the UI). List the names, \
         ask the owner to TYPE the code, then retry with `confirmation` set to \
         it. At most 28 names per call — split a bigger request into separate \
         calls. Taken or invalid names are skipped (not an error) and listed in \
         `skipped`. More than 7 names are split across multiple sponsored txs \
         automatically (each tx carries at most 8 calls). `failed` lists chunks \
         whose tx FAILED (those names were NOT registered); `unconfirmed` lists \
         chunks whose receipt TIMED OUT (the tx MAY still land — check its \
         tx_hash before retrying; the batch stops there); `unattempted` lists \
         names never tried (the batch stops early after 2 consecutive failed \
         chunks, an unconfirmed chunk, or a user Stop). Returns { registered, \
         skipped, count, tx_hashes, failed, unconfirmed, unattempted, urls }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let params = crate::tool_params::BatchCreateSubdomainsParams::lenient(&args);
            let requested: Vec<String> = params
                .names
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if requested.is_empty() {
                return Err(crate::error::Error::bad_args("batch_create_subdomains", "names cannot be empty"));
            }
            // Hard total bound (relay_chunk::MAX_BATCH_ITEMS): the mint/spend
            // ceiling one confirmed call may carry.
            if let Some(msg) =
                crate::relay_chunk::over_batch_limit("batch_create_subdomains", requested.len())
            {
                return Err(crate::error::Error::bad_args("batch_create_subdomains", msg));
            }
            // Belt-and-suspenders: confirm_guard denies any unconfirmed call
            // before this body runs; this guards a registration path that
            // forgot the hook (batch_create mints names for real $LH — same
            // posture as bulk_release / batch_send_lh).
            let confirmed = params
                .confirmation
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !confirmed {
                return Err(crate::error::Error::bad_args(
                    "batch_create_subdomains",
                    "batch_create_subdomains requires the platform-issued confirmation code",
                ));
            }
            // A PAID claim (mainnet registrationCost > 0) inserts ONE cumulative
            // `approve` at the head of EVERY chunk's tx (events/subdomains.rs),
            // so each chunk carries at most 7 names — the reserved-slot rule the
            // old hard cap encoded (telemetry #88). The slot is reserved
            // UNCONDITIONALLY: on a registrationCost()==0 chain no approve rides
            // and the slot is wasted — accepted (one spare call per chunk beats
            // an async cost read inside the pure partitioner). >7 names
            // auto-chunk into sequential sponsored txs (telemetry #85); a failed
            // chunk is reported and the rest run UNLESS the breaker trips (2
            // consecutive failures), a receipt times out (stop immediately —
            // chain state unknown), or the user pressed Stop (the dwell idiom).
            let ranges = crate::relay_chunk::chunk_ranges(requested.len(), true);
            let mut outcomes: Vec<crate::relay_chunk::ChunkOutcome> =
                Vec::with_capacity(ranges.len());
            let mut registered_all: Vec<String> = Vec::new();
            for r in &ranges {
                if crate::app::chat::turn_cancelled()
                    || crate::relay_chunk::should_stop(&outcomes)
                {
                    break;
                }
                let chunk = requested[r.clone()].to_vec();
                match crate::app::events::run_batch_create_subdomains(&chunk).await {
                    Ok((registered, tx)) => {
                        registered_all.extend(registered);
                        outcomes.push(crate::relay_chunk::ChunkOutcome::Landed(tx));
                    }
                    // Every name in this chunk was taken/invalid — nothing was
                    // submitted. Handled (all end up in `skipped`), not a
                    // failure. EXACT-STRING sentinel: matched by equality
                    // against the shared const, so the producer
                    // (events/subdomains.rs) must never wrap or reword it.
                    Err(e) if e == crate::app::events::NO_VALID_NAMES => {
                        outcomes.push(crate::relay_chunk::ChunkOutcome::Landed(String::new()));
                    }
                    Err(e) => outcomes.push(crate::relay_chunk::classify_failure(e)),
                }
            }
            let fold = crate::relay_chunk::fold_outcomes(&ranges, &outcomes);
            if registered_all.is_empty() && fold.unconfirmed.is_empty() {
                // Total loss (and nothing pending): preserve the old single-tx
                // error contract (first chunk error, or the all-skipped
                // sentinel). With an unconfirmed chunk the structured result
                // below reports it instead — an Err would falsely claim no
                // registration can have happened.
                let msg = fold
                    .chunk_errors
                    .first()
                    .map(|(_, e)| e.clone())
                    .unwrap_or_else(|| crate::app::events::NO_VALID_NAMES.to_string());
                return Err(crate::error::Error::other(format!("batch create failed: {msg}")));
            }
            // Skipped = names whose chunk was handled but which did not register
            // (taken/invalid). Failed-chunk names are NOT "skipped" — they were
            // attempted and their tx failed; they ride `failed` instead.
            let registered_set: std::collections::HashSet<&str> =
                registered_all.iter().map(|s| s.as_str()).collect();
            let skipped: Vec<&String> = fold
                .landed
                .iter()
                .map(|&i| &requested[i])
                .filter(|r| {
                    let c = crate::app::tenant::sanitize(r);
                    !registered_set.contains(c.as_str())
                })
                .collect();
            let failed: Vec<serde_json::Value> = fold
                .chunk_errors
                .iter()
                .map(|(ci, err)| serde_json::json!({
                    "names": requested[ranges[*ci].clone()],
                    "error": err,
                }))
                .collect();
            // Receipt TIMEOUT ≠ revert: these txs MAY still land — surface the
            // hash, never claim the names did or did not register.
            let unconfirmed: Vec<serde_json::Value> = fold
                .unconfirmed_txs
                .iter()
                .map(|(ci, tx)| serde_json::json!({
                    "names": requested[ranges[*ci].clone()],
                    "tx_hash": tx,
                    "note": "receipt timed out — the tx may still land; check the \
                             tx hash before retrying these names",
                }))
                .collect();
            let unattempted: Vec<&String> =
                fold.unattempted.iter().map(|&i| &requested[i]).collect();
            Ok(serde_json::json!({
                "registered": registered_all,
                "skipped": skipped,
                "count": registered_all.len(),
                "tx_hashes": fold.tx_hashes,
                "failed": failed,
                "unconfirmed": unconfirmed,
                "unattempted": unattempted,
                "urls": registered_all.iter()
                    .map(|n| format!("https://{n}.localharness.xyz/"))
                    .collect::<Vec<_>>(),
            }))
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
                crate::error::Error::bad_args("send_lh", format!(
                    "could not parse amount \"{amount_arg}\" — pass a decimal $LH \
                     figure like \"5\" or \"1.5\""
                ))
            })?;
            if amount_wei == 0 {
                return Err(crate::error::Error::bad_args(
                    "send_lh",
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
                return Err(crate::error::Error::bad_args(
                    "send_lh",
                    "send_lh requires the platform-issued confirmation code",
                ));
            }

            // Recipient: address used directly; name → on-chain owner address.
            let to_hex = resolve_lh_recipient("send_lh", &recipient_arg).await?;

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

/// `batch_send_lh(transfers)` — N transfers in batched sponsored Tempo txs
/// (feedback #49: tx type 0x76 natively carries a calls array, so batching
/// costs a few submissions instead of N). >7 transfers auto-chunk — each
/// chunk's tx reserves ONE slot for the meter bridge, which re-checks the
/// LIVE wallet balance per chunk (`crate::relay_chunk`; telemetry #85).
/// Gated by the dispatch-layer typed-confirmation challenge
/// (`chat::confirm_guard`), same as `send_lh`.
pub(crate) fn batch_send_lh_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "transfers": {
                "type": "array",
                "description": "The transfers to execute. More than 7 are split \
                    across multiple sponsored transactions automatically (at \
                    most 7 ride each tx); at most 28 transfers per call — split \
                    a bigger batch into separate calls.",
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
        "Transfer $LH to MULTIPLE recipients in batched on-chain transactions \
         — more than 7 transfers are split across multiple sponsored txs \
         automatically; at most 28 transfers per call (split a bigger payroll \
         into separate calls). Each transfer names a 0x… address or a subdomain \
         (paid to its on-chain owner). Far cheaper than repeated send_lh calls. \
         MOVES VALUE — the first call does NOT execute: it returns a single-use \
         confirmation code (also shown to the owner in the UI). Show the full \
         list, ask the owner to TYPE the code, then retry with `confirmation` \
         set to it. ONE code for the whole batch. Per-transfer `status` is \
         \"landed\", \"failed\" (that chunk's tx FAILED — those transfers did \
         NOT move), \"unconfirmed\" (the chunk's receipt TIMED OUT — the tx MAY \
         still land; check its tx_hash before re-sending, or you risk paying \
         twice; the batch stops there), or \"unattempted\" (never tried — the \
         batch stops early after 2 consecutive failed chunks, an unconfirmed \
         chunk, or a user Stop). Returns { count, total, transfers: \
         [{recipient, resolved, amount, status}], tx_hashes, failed, \
         unconfirmed, unattempted }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            use crate::encoding::parse_token_amount;

            let items = args
                .get("transfers")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if items.is_empty() {
                return Err(crate::error::Error::bad_args(
                    "batch_send_lh",
                    "batch_send_lh: transfers must be a non-empty array",
                ));
            }
            // Hard total bound (relay_chunk::MAX_BATCH_ITEMS): the value-move
            // ceiling one confirmed call may carry.
            if let Some(msg) = crate::relay_chunk::over_batch_limit("batch_send_lh", items.len())
            {
                return Err(crate::error::Error::bad_args("batch_send_lh", msg));
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
                return Err(crate::error::Error::bad_args(
                    "batch_send_lh",
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
                    crate::error::Error::bad_args("batch_send_lh", format!(
                        "could not parse amount \"{amount_str}\" for \"{recipient}\""
                    ))
                })?;
                if amount_wei == 0 {
                    return Err(crate::error::Error::bad_args("batch_send_lh", format!(
                        "amount for \"{recipient}\" must be greater than 0"
                    )));
                }
                let to_hex = resolve_lh_recipient("batch_send_lh", &recipient).await?;
                // checked, not saturating: a hostile/overflowing total must be a
                // clear error (matching parse_token_amount's reject-don't-wrap
                // contract), not a silently-clamped wrong bridge/display amount.
                total_wei = total_wei.checked_add(amount_wei).ok_or_else(|| {
                    crate::error::Error::bad_args(
                        "batch_send_lh",
                        "batch total exceeds the maximum representable amount — lower the amounts",
                    )
                })?;
                resolved.push((recipient, to_hex, amount_wei, amount_str));
            }

            let (_, from) = crate::app::tenant::current_tenant_owner()
                .await
                .map_err(crate::error::Error::other)?;

            // >7 transfers auto-chunk into sequential sponsored txs — the meter
            // bridge reserves ONE slot in every chunk's tx, the reserved-slot
            // rule the old hard cap encoded (crate::relay_chunk; telemetry #85).
            // The bridge decision re-runs per chunk against the LIVE balance
            // (earlier chunks may have drained the wallet). A failed chunk is
            // reported and the rest run UNLESS the breaker trips (2 consecutive
            // failures), a receipt times out — then the loop stops IMMEDIATELY:
            // that tx may still mine, so a further chunk's live-balance bridge
            // read could double-bridge/double-pay — or the user pressed Stop
            // (the dwell idiom).
            let ranges = crate::relay_chunk::chunk_ranges(resolved.len(), true);
            let mut outcomes: Vec<crate::relay_chunk::ChunkOutcome> =
                Vec::with_capacity(ranges.len());
            let mut bridged_any = false;
            for r in &ranges {
                if crate::app::chat::turn_cancelled()
                    || crate::relay_chunk::should_stop(&outcomes)
                {
                    break;
                }
                let chunk = &resolved[r.clone()];
                let chunk_total: u128 = chunk.iter().map(|(_, _, w, _)| *w).sum();
                let attempt: Result<(String, bool), String> = async {
                    let mut calls = Vec::with_capacity(chunk.len() + 1);
                    let bridged = match meter_bridge_call(&from, chunk_total)
                        .await
                        .map_err(|e| e.to_string())?
                    {
                        Some(bridge) => {
                            calls.push(bridge);
                            true
                        }
                        None => false,
                    };
                    for (_, to_hex, amount_wei, _) in chunk {
                        calls.push(
                            lh_transfer_call(to_hex, *amount_wei).map_err(|e| e.to_string())?,
                        );
                    }
                    let purpose = format!(
                        "batch-send {} $LH to {} recipients",
                        crate::app::format_wei_as_test_eth(chunk_total),
                        chunk.len()
                    );
                    // 500k base (first transfer + sponsorship overhead) + ~80k per
                    // additional warm transfer + 150k when the bridge rides along.
                    let gas = 500_000
                        + 80_000 * (chunk.len() as u128 - 1)
                        + if bridged { 150_000 } else { 0 };
                    let tx =
                        crate::app::events::run_sponsored_tempo_call(&from, calls, gas, &purpose)
                            .await?;
                    Ok((tx, bridged))
                }
                .await;
                match attempt {
                    Ok((tx, bridged)) => {
                        bridged_any |= bridged;
                        outcomes.push(crate::relay_chunk::ChunkOutcome::Landed(tx));
                    }
                    Err(e) => outcomes.push(crate::relay_chunk::classify_failure(e)),
                }
            }
            let fold = crate::relay_chunk::fold_outcomes(&ranges, &outcomes);
            if fold.landed.is_empty() && fold.unconfirmed.is_empty() {
                if let Some((_, e)) = fold.chunk_errors.first() {
                    return Err(crate::error::Error::other(format!("batch_send_lh failed: {e}")));
                }
            }

            // #50: ping each recipient whose transfer LANDED (best-effort,
            // rides the batch). One fire-and-forget notify per landed transfer.
            for &i in &fold.landed {
                let (recipient, to_hex, _, amount_str) = &resolved[i];
                notify_recipient_of_incoming_lh(
                    recipient.clone(),
                    to_hex.clone(),
                    amount_str.clone(),
                );
            }

            let landed_total: u128 = fold.landed.iter().map(|&i| resolved[i].2).sum();
            // O(1) membership for the per-transfer status labels (the naive
            // per-item `Vec::contains` scan was O(n²) across the batch).
            use std::collections::HashSet;
            let landed_set: HashSet<usize> = fold.landed.iter().copied().collect();
            let failed_set: HashSet<usize> = fold.failed.iter().copied().collect();
            let unconfirmed_set: HashSet<usize> = fold.unconfirmed.iter().copied().collect();
            let transfers: Vec<serde_json::Value> = resolved
                .iter()
                .enumerate()
                .map(|(i, (recipient, to_hex, _, amount_str))| {
                    // Honest four-way label: an UNATTEMPTED transfer was never
                    // tried (not "failed"), and an UNCONFIRMED one may still
                    // land (its money claim is UNKNOWN, never "did not move").
                    let status = if landed_set.contains(&i) {
                        "landed"
                    } else if failed_set.contains(&i) {
                        "failed"
                    } else if unconfirmed_set.contains(&i) {
                        "unconfirmed"
                    } else {
                        "unattempted"
                    };
                    serde_json::json!({
                        "recipient": recipient,
                        "resolved": to_hex,
                        "amount": amount_str,
                        "status": status,
                    })
                })
                .collect();
            let failed: Vec<serde_json::Value> = fold
                .chunk_errors
                .iter()
                .map(|(ci, err)| serde_json::json!({
                    "recipients": ranges[*ci].clone()
                        .map(|i| resolved[i].0.clone())
                        .collect::<Vec<_>>(),
                    "error": err,
                }))
                .collect();
            // Receipt TIMEOUT ≠ revert: the tx MAY still land. Report the hash
            // and never a "did NOT move" claim — re-sending unchecked risks
            // paying these recipients twice.
            let unconfirmed: Vec<serde_json::Value> = fold
                .unconfirmed_txs
                .iter()
                .map(|(ci, tx)| serde_json::json!({
                    "recipients": ranges[*ci].clone()
                        .map(|i| resolved[i].0.clone())
                        .collect::<Vec<_>>(),
                    "tx_hash": tx,
                    "note": "receipt timed out — the transfer may still land; check \
                             the tx hash before re-sending or you risk paying twice",
                }))
                .collect();
            let unattempted: Vec<String> =
                fold.unattempted.iter().map(|&i| resolved[i].0.clone()).collect();
            // `count`/`total` name what actually MOVED — never the request size.
            Ok(serde_json::json!({
                "count": fold.landed.len(),
                "total": crate::app::format_wei_as_test_eth(landed_total),
                "bridged_from_meter": bridged_any,
                "transfers": transfers,
                "tx_hashes": fold.tx_hashes,
                "failed": failed,
                "unconfirmed": unconfirmed,
                "unattempted": unattempted,
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
                return Err(crate::error::Error::bad_args(
                    "query_balance",
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
