// =============================================================================
// Platform-level closure tools (browser-specific; not in the SDK builtins).
// =============================================================================

use crate::app::chat::access::{build_actor_setup, transfer_selector, u256_be};
use crate::encoding::parse_address;
use crate::tools::ClosureTool;

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
            let cleaned = crate::app::tenant::sanitize(name);
            if cleaned.len() < 3 || cleaned.len() > 32 {
                return Err(crate::error::Error::other("invalid name"));
            }
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
            let cleaned = crate::app::tenant::sanitize(name);
            if cleaned.len() < 3 || cleaned.len() > 32 {
                return Err(crate::error::Error::other("invalid name"));
            }
            if source.trim().is_empty() {
                return Err(crate::error::Error::other("source cannot be empty"));
            }
            // Compile FIRST so a bad cartridge fails before we register the
            // name on-chain. Surface a clear error so the agent reports it.
            let wasm = crate::rustlite::compile(source)
                .map_err(|e| crate::error::Error::other(format!("compile failed: {e}")))?;
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

/// `release_subdomain(name, confirmation)` — DESTRUCTIVE: burn the NFT +
/// free the name. Gated: `confirmation` must EXACTLY equal `name`, which
/// forces a typed confirmation in chat (the owner types the name). The
/// system prompt also forbids auto-filling it.
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
                "description": "Must EXACTLY equal `name`. Pass ONLY after the owner has \
                    TYPED the exact name in this chat. Never auto-fill or invent it."
            }
        },
        "required": ["name", "confirmation"]
    });
    ClosureTool::new(
        "release_subdomain",
        "DESTRUCTIVE + IRREVERSIBLE: burn a subdomain NFT and free its name. Requires \
         `confirmation` to exactly equal `name` (the owner must type the name). Refuses \
         your MAIN. Returns the tx hash.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let confirmation = args
                .get("confirmation")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if name.is_empty() {
                return Err(crate::error::Error::other("name is required"));
            }
            if confirmation != name {
                return Err(crate::error::Error::other(format!(
                    "release_subdomain NOT executed — confirmation must exactly equal \"{name}\". \
                     Ask the owner to TYPE \"{name}\" to confirm, then retry. Do not auto-fill it."
                )));
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
/// `names`, only that subset. Single master confirmation (NOT per-name): the
/// owner must type the literal phrase `release all non-main`. An empty/absent
/// confirmation returns the list it WOULD release (so the agent can show the
/// user first) and performs NO write. Refuses the MAIN. Withheld from
/// subagents (only registered on the main agent).
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
                "description": "Must EXACTLY equal `release all non-main`. Pass ONLY after \
                    the owner has TYPED that exact phrase in this chat. First call this \
                    tool with confirmation empty to GET the list of names that will be \
                    released, show the user, and ask them to type the phrase. Never \
                    auto-fill or invent it."
            }
        },
        "required": []
    });
    ClosureTool::new(
        "bulk_release_subdomains",
        "DESTRUCTIVE + IRREVERSIBLE: burn MANY subdomain NFTs and free their names in \
         ONE batch. With no `names`, releases EVERY non-MAIN subdomain the owner holds; \
         with `names`, only that subset. Requires a SINGLE master `confirmation` equal to \
         \"release all non-main\" (the owner types it once — NOT one confirmation per \
         name). ALWAYS call first with confirmation empty to receive the list of names it \
         will release, show the user, then ask them to type the phrase and retry. Always \
         refuses your MAIN. Returns the released names + tx hash.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            const CONFIRM_PHRASE: &str = "release all non-main";
            let confirmation = args
                .get("confirmation")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_lowercase();

            // Resolve the kill-list: explicit subset, else all non-MAIN holdings.
            let tenant = match crate::app::tenant::current() {
                crate::app::tenant::Host::Tenant(n) => n,
                _ => return Err(crate::error::Error::other("not running on a subdomain")),
            };
            let owner = crate::app::registry::owner_of_name(&tenant)
                .await
                .map_err(crate::error::Error::other)?
                .ok_or_else(|| crate::error::Error::other("no on-chain owner"))?;
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

            // REPORT-BEFORE-CONFIRM: no valid confirmation -> list + STOP.
            if confirmation != CONFIRM_PHRASE {
                return Ok(serde_json::json!({
                    "status": "confirmation_required",
                    "count": targets.len(),
                    "will_release": targets,
                    "instruction": format!(
                        "These {} subdomain(s) will be PERMANENTLY released (burned). \
                         Show this list to the owner. To proceed, the owner must TYPE the \
                         exact phrase \"{}\" — then call bulk_release_subdomains again with \
                         that confirmation. Do NOT auto-fill it.",
                        targets.len(), CONFIRM_PHRASE
                    )
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
            let name = match crate::app::tenant::current() {
                crate::app::tenant::Host::Tenant(n) => n,
                _ => return Err(crate::error::Error::other("not running on a subdomain")),
            };
            let owner = crate::app::registry::owner_of_name(&name)
                .await
                .map_err(crate::error::Error::other)?
                .ok_or_else(|| crate::error::Error::other("no on-chain owner"))?;
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
         `query` (ranked — name matches first, then persona matches). Use this \
         to LOCATE an agent to delegate to, then call_agent it. Returns \
         { agents: [ { name, persona } ], count } (persona is a short preview).",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What to look for — a capability, topic, or \
                        keyword matched (case-insensitively) against agent names \
                        and personas (e.g. \"solidity\", \"image\", \"research\"). \
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
/// value). No typed-confirmation gate — a transfer is an intended action, unlike
/// the destructive `release_subdomain` burn — but the amount must parse to > 0.
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
            }
        },
        "required": ["recipient", "amount"]
    });
    ClosureTool::new(
        "send_lh",
        "Transfer real $LH credits from the owner's wallet to a recipient. \
         `recipient` is a raw 0x… address OR a subdomain name (funds go to that \
         name's on-chain owner). `amount` is a decimal $LH figure (must be > 0). \
         Moves value: confirm the recipient + amount with the owner before \
         calling. Returns { amount, recipient (input), resolved_recipient, \
         tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            use crate::encoding::{parse_token_amount, Recipient};

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
            let kind = crate::encoding::classify_recipient(&recipient_arg)
                .map_err(crate::error::Error::other)?;
            let to_hex = match kind {
                Recipient::Address(addr) => addr,
                Recipient::Name(name) => crate::app::registry::owner_of_name(&name)
                    .await
                    .map_err(crate::error::Error::other)?
                    .ok_or_else(|| {
                        crate::error::Error::other(format!(
                            "no on-chain owner for subdomain \"{name}\" — is it registered?"
                        ))
                    })?,
            };

            // Sender = this subdomain's on-chain owner (the apex wallet that
            // signs via the iframe), matching list_subdomains / bulk_release.
            let tenant = match crate::app::tenant::current() {
                crate::app::tenant::Host::Tenant(n) => n,
                _ => {
                    return Err(crate::error::Error::other(
                        "not running on a subdomain — no owner wallet to send from",
                    ))
                }
            };
            let from = crate::app::registry::owner_of_name(&tenant)
                .await
                .map_err(crate::error::Error::other)?
                .ok_or_else(|| crate::error::Error::other("no on-chain owner"))?;

            // ERC-20 transfer(to, amount_wei) against the $LH token — the exact
            // calldata shape the per-turn payment + act panel build.
            let to_bytes = crate::encoding::parse_address(&to_hex)
                .map_err(crate::error::Error::other)?;
            let mut to_padded = [0u8; 32];
            to_padded[12..].copy_from_slice(&to_bytes);
            let mut calldata = Vec::with_capacity(4 + 32 + 32);
            calldata.extend_from_slice(&transfer_selector());
            calldata.extend_from_slice(&to_padded);
            calldata.extend_from_slice(&u256_be(amount_wei));

            let token_addr =
                crate::encoding::parse_address(crate::registry::LOCALHARNESS_TOKEN_ADDRESS)
                    .map_err(crate::error::Error::other)?;
            let call = crate::tempo_tx::TempoCall {
                to: token_addr,
                value_wei: 0,
                input: calldata,
            };

            let amount_display = amount_arg.clone();
            let purpose = format!("send {amount_display} $LH to {to_hex}");
            // 500k mirrors the per-turn payment's ERC-20 transfer budget; the
            // sponsor is billed on gas USED, not the limit.
            let tx_hash = crate::app::events::run_sponsored_tempo_call(
                &from,
                vec![call],
                500_000,
                &purpose,
            )
            .await
            .map_err(|e| crate::error::Error::other(format!("send_lh failed: {e}")))?;

            Ok(serde_json::json!({
                "amount": amount_display,
                "recipient": recipient_arg,
                "resolved_recipient": to_hex,
                "tx_hash": tx_hash,
            }))
        },
    )
}
