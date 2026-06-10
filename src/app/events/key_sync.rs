//! Gemini key sync — seal / restore the key on-chain under the owner MAIN slot.

use crate::encoding::{bytes_to_hex_str, hex_to_bytes, parse_address};

use crate::app::dom;

/// The on-chain tokenId the Gemini-key blob lives under: the owner's
/// MAIN, so every subdomain the owner holds shares ONE key (per the
/// "the subdomain IS the primary owner" model — a new subdomain should
/// reuse the MAIN's key, not prompt for a fresh one). Falls back to the
/// current subdomain's own id if the owner has no MAIN set.
pub(super) async fn gemini_key_slot_id(name: &str) -> Result<u64, String> {
    let owner = crate::app::registry::owner_of_name(name)
        .await
        .map_err(|e| format!("owner: {e}"))?
        .ok_or_else(|| "name not registered on-chain".to_string())?;
    let main_id = crate::app::registry::main_of(&owner).await.unwrap_or(0);
    if main_id != 0 {
        return Ok(main_id);
    }
    match crate::app::registry::id_of_name(name).await {
        Ok(id) if id != 0 => Ok(id),
        _ => Err("no token id for name".into()),
    }
}

/// Best-effort: seal the Gemini key with the seed-derived key (via the
/// apex iframe) and store it on-chain under the owner's MAIN slot, so any
/// other subdomain / seed-bearing device auto-restores it. No-op on any
/// failure — most importantly when the seed isn't on this device (the
/// iframe seal fails), which is fine: nothing to sync from here.
pub(super) async fn auto_sync_gemini_key(name: String, key: String) {
    let owner = match crate::app::registry::owner_of_name(&name).await {
        Ok(Some(o)) => o,
        _ => return,
    };
    let slot_id = match gemini_key_slot_id(&name).await {
        Ok(id) => id,
        Err(_) => return,
    };
    // Seal with the seed-derived key via the apex iframe. Fails (and we
    // bail) on a device that doesn't hold the seed.
    let ct_hex = match crate::app::verify::seal_key_via_iframe(&key).await {
        Ok(h) => h,
        Err(_) => return,
    };
    let Ok(ct) = hex_to_bytes(&ct_hex) else { return };
    let Ok(registry_addr) = parse_address(crate::app::registry::REGISTRY_ADDRESS) else { return };
    let call = crate::tempo_tx::TempoCall {
        to: registry_addr,
        value_wei: 0,
        input: crate::app::registry::encode_set_gemini_key(slot_id, &ct),
    };
    let gas = crate::app::gas::set_metadata_gas(ct.len());
    let _ = super::run_sponsored_tempo_call(&owner, vec![call], gas, "auto-sync key").await;
}

/// Proactively push THIS device's Gemini key to the owner's MAIN slot
/// on-chain so a just-created subdomain (and every other) inherits it
/// with no manual re-save. Best-effort + no-op without a local key or the
/// seed (the seal happens via the apex iframe). Called after a claim.
pub(crate) async fn sync_local_key_to_main(name: &str) {
    if let Some(key) = crate::app::key_store::load().await {
        auto_sync_gemini_key(name.to_string(), key).await;
    }
}

/// Try to pull the owner's MAIN Gemini key from chain and decrypt it
/// with this device's seed (via the apex iframe). On success the key is
/// saved to this origin's OPFS + sessionStorage and `true` is returned,
/// so the caller can skip the api-key modal. Returns `false` (silently)
/// when there's no synced key OR this device lacks the seed (e.g. a phone
/// linked by device key only — that path will use the wrapped-key blob).
pub(crate) async fn try_auto_restore_gemini_key(name: &str) -> bool {
    if crate::app::key_store::load().await.is_some() {
        return true;
    }
    let slot_id = match gemini_key_slot_id(name).await {
        Ok(id) => id,
        Err(_) => return false,
    };
    let ct = match crate::app::registry::gemini_key_of(slot_id).await {
        Ok(Some(b)) => b,
        _ => return false,
    };
    let ct_hex = bytes_to_hex_str(&ct);
    let plaintext = match crate::app::verify::open_key_via_iframe(&ct_hex).await {
        Ok(p) => p,
        Err(_) => return false,
    };
    crate::app::key_store::save(&plaintext).await;
    if let Ok(Some(storage)) = dom::session_storage() {
        let _ = storage.set_item("gemini_api_key", &plaintext);
    }
    true
}
