//! Subdomain ops — release / bulk-release / batch-create (headless helpers
//! shared with the chat tools).

use crate::encoding::parse_address;

/// Release (recycle) a subdomain on-chain via the iframe-signed sponsored
/// path (the owner signs the sender hash through the apex signer). The
/// CALLER (tool / UI) MUST do the typed-confirmation gate BEFORE calling
/// this — this only performs the on-chain release.
pub(crate) async fn run_release_subdomain(name: &str) -> Result<String, String> {
    let token_id = match crate::app::registry::check_name(name).await? {
        crate::app::registry::Status::Taken { agent_id } => agent_id,
        _ => return Err(format!("'{name}' is not registered")),
    };
    let owner = crate::app::registry::owner_of_name(name)
        .await
        .map_err(|e| format!("owner: {e}"))?
        .ok_or_else(|| "no on-chain owner".to_string())?;
    let diamond = parse_address(crate::app::registry::REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond,
        value_wei: 0,
        input: crate::app::registry::release_name_calldata(token_id),
    };
    // A burn (clears the name's cold slots) runs ~100-150k inner + ~275k
    // sponsorship ≈ ~375-425k — a flat 400k had near-zero margin and silently
    // OOG-reverted (chain reverts, name stays `isTaken`, UI reports success:
    // the feedback/redeem OOG bug class). Over-budget is free (the sponsor is
    // billed on gas USED, not the limit), so headroom is the right call.
    super::run_sponsored_tempo_call(&owner, vec![call], 1_000_000, "release subdomain").await
}

/// Bulk-release (burn) several subdomains in ONE sponsored, iframe-signed
/// tx. `names` are resolved to token ids; the owner's MAIN is refused
/// up-front (and again on-chain). The CALLER (tool) MUST gate on a single
/// typed master confirmation BEFORE calling — this only performs the write.
/// Returns (released_names, tx_hash).
pub(crate) async fn run_bulk_release(
    names: &[String],
) -> Result<(Vec<String>, String), String> {
    if names.is_empty() {
        return Err("no subdomains to release".into());
    }
    // Resolve owner + MAIN from the current tenant, same preamble as
    // consolidate (owner_main_tba) but we only need the owner hex + MAIN id.
    let tenant = match crate::app::tenant::current() {
        crate::app::tenant::Host::Tenant(n) => n,
        _ => return Err("not running on a subdomain".into()),
    };
    let owner = crate::app::registry::owner_of_name(&tenant)
        .await
        .map_err(|e| format!("owner: {e}"))?
        .ok_or_else(|| "no on-chain owner".to_string())?;
    let main_id = crate::app::registry::main_of(&owner)
        .await
        .map_err(|e| format!("mainOf: {e}"))?;

    let diamond = parse_address(crate::app::registry::REGISTRY_ADDRESS)?;
    let mut released: Vec<String> = Vec::with_capacity(names.len());
    let mut calls: Vec<crate::tempo_tx::TempoCall> = Vec::with_capacity(names.len());
    for raw in names {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        let token_id = match crate::app::registry::check_name(name).await? {
            crate::app::registry::Status::Taken { agent_id } => agent_id,
            _ => return Err(format!("'{name}' is not registered")),
        };
        if main_id != 0 && token_id == main_id {
            return Err(format!(
                "'{name}' is your MAIN identity and cannot be released"
            ));
        }
        // Defensive: only burn names this owner actually holds — a stray
        // name would revert the WHOLE batch on-chain (and waste sponsor gas).
        let holder = crate::app::registry::owner_of_name(name)
            .await
            .map_err(|e| format!("owner of {name}: {e}"))?
            .ok_or_else(|| format!("no on-chain owner for '{name}'"))?;
        if holder.to_lowercase() != owner.to_lowercase() {
            return Err(format!("'{name}' is not owned by this identity"));
        }
        calls.push(crate::tempo_tx::TempoCall {
            to: diamond,
            value_wei: 0,
            input: crate::app::registry::release_name_calldata(token_id),
        });
        released.push(name.to_string());
    }
    if calls.is_empty() {
        return Err("no subdomains to release after filtering".into());
    }
    // 1M base headroom + ~250k per extra burn (see release_names_sponsored).
    let gas = 1_000_000 + (calls.len() as u128).saturating_sub(1) * 250_000;
    let tx = super::run_sponsored_tempo_call(&owner, calls, gas, "bulk release subdomains").await?;
    Ok((released, tx))
}

/// Batch-register N subdomains in ONE sponsored, iframe-signed tx — the
/// sanctioned mass-registration path (vs. a sequential `create_subdomain`
/// loop, which spends N sponsored txs + N auto-continue iterations). Names
/// are sanitised, deduped, and availability-checked up front; an already-
/// taken or invalid name is SKIPPED (a single bad `register` would revert
/// the whole multicall on-chain and waste sponsor gas — same defensive
/// lesson as `run_bulk_release`'s holder check). Returns (registered_names,
/// tx_hash). The owner context is resolved from the current tenant.
pub(crate) async fn run_batch_create_subdomains(
    names: &[String],
) -> Result<(Vec<String>, String), String> {
    if names.is_empty() {
        return Err("no names to register".into());
    }
    // Resolve the owner EOA from the current tenant (same preamble as
    // run_bulk_release) — run_sponsored_tempo_call recovers + verifies the
    // sender address against this, so it must be the master wallet's address.
    let tenant = match crate::app::tenant::current() {
        crate::app::tenant::Host::Tenant(n) => n,
        _ => return Err("not running on a subdomain".into()),
    };
    let owner = crate::app::registry::owner_of_name(&tenant)
        .await
        .map_err(|e| format!("owner: {e}"))?
        .ok_or_else(|| "no on-chain owner".to_string())?;

    let diamond = parse_address(crate::app::registry::REGISTRY_ADDRESS)?;
    let mut registered: Vec<String> = Vec::with_capacity(names.len());
    let mut calls: Vec<crate::tempo_tx::TempoCall> = Vec::with_capacity(names.len());
    for raw in names {
        let cleaned = crate::app::tenant::sanitize(raw);
        // Reject silently-mangled or out-of-range names rather than minting a
        // different name than asked. No explanatory text — just skip + report.
        if cleaned.len() < 3
            || cleaned.len() > 32
            || cleaned != raw.trim().to_ascii_lowercase()
        {
            continue;
        }
        if registered.iter().any(|n| n == &cleaned) {
            continue; // dedupe — a repeat register would revert the batch
        }
        // Availability pre-check: a register on a TAKEN name reverts the whole
        // multicall. Skip taken names (the tool reports which were skipped).
        match crate::app::registry::check_name(&cleaned).await? {
            crate::app::registry::Status::Available => {}
            _ => continue,
        }
        calls.push(crate::tempo_tx::TempoCall {
            to: diamond,
            value_wei: 0,
            input: crate::app::registry::register_calldata(&cleaned),
        });
        registered.push(cleaned);
    }
    if calls.is_empty() {
        return Err("no valid, available names to register".into());
    }
    // Each register is a full cold ERC-721 mint (~1.32M inner each, per the
    // eth_estimateGas note in registry.rs) + ONE ~275k sponsorship overhead
    // for the tx. 1.5M/name covers the mint + cold-SSTORE variance + margin;
    // +400k one-time. Over-budget is FREE — the sponsor is billed on gas USED,
    // not the limit (same lesson as the redeem/feedback OOG bug class), so
    // headroom is correct.
    let gas = 400_000 + (calls.len() as u128) * 1_500_000;
    let tx = super::run_sponsored_tempo_call(&owner, calls, gas, "batch create subdomains").await?;
    // Inherit this device's Gemini key onto each new subdomain (best-effort,
    // detached — same as the single create_subdomain flow).
    for name in &registered {
        let n = name.clone();
        wasm_bindgen_futures::spawn_local(async move {
            super::key_sync::sync_local_key_to_main(&n).await;
        });
    }
    Ok((registered, tx))
}
