//! Gemini key sync — seal / restore the key on-chain under the owner MAIN slot.

use crate::encoding::parse_address;

use crate::app::dom;

fn decode_hex_local(s: &str) -> Option<Vec<u8>> {
    let t = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    if t.len() % 2 != 0 {
        return None;
    }
    (0..t.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(t.get(i..i + 2)?, 16).ok())
        .collect()
}

/// #18: seal the current Gemini key with the seed-derived key (via the
/// apex iframe) and store the ciphertext on-chain under the gemini-key
/// metadata key (owner-signed, sponsored). A device that imports the seed
/// can then `restore` it without re-pasting.
pub(super) async fn run_sync_key() {
    let msg = "key-sync-msg";
    let set_err = |m: &str| dom::swap_inner(msg, &dom::msg_span(dom::Msg::Error, &format!("{m}")));

    let name = match crate::app::tenant::current() {
        crate::app::tenant::Host::Tenant(n) => n,
        _ => {
            set_err("only on a subdomain");
            return;
        }
    };
    let owner_hex = crate::app::APP.with(|cell| {
        use crate::app::VerifyState;
        match &cell.borrow().verify_state {
            VerifyState::Verified { address } => Some(address.clone()),
            _ => None,
        }
    });
    let Some(owner_hex) = owner_hex else {
        set_err("verify as owner first");
        return;
    };
    let key = dom::input_by_id("key").map(|i| i.value()).unwrap_or_default();
    if key.trim().is_empty() {
        set_err("enter your key first");
        return;
    }

    dom::swap_inner(msg, "<span style=\"color:var(--muted)\">sealing…</span>");
    let ct_hex = match crate::app::verify::seal_key_via_iframe(&key).await {
        Ok(h) => h,
        Err(e) => {
            set_err(&format!("seal: {e}"));
            return;
        }
    };
    let Some(ct) = decode_hex_local(&ct_hex) else {
        set_err("bad ciphertext from signer");
        return;
    };
    let id = match gemini_key_slot_id(&name).await {
        Ok(id) => id,
        Err(e) => {
            set_err(&e);
            return;
        }
    };
    let registry_addr = match parse_address(crate::app::registry::REGISTRY_ADDRESS) {
        Ok(a) => a,
        Err(e) => {
            set_err(&e);
            return;
        }
    };
    let call = crate::tempo_tx::TempoCall {
        to: registry_addr,
        value_wei: 0,
        input: crate::app::registry::encode_set_gemini_key(id, &ct),
    };
    let words = (ct.len() / 32 + 1) as u128;
    let gas = 1_200_000 + words * 40_000;
    dom::swap_inner(msg, "<span style=\"color:var(--muted)\">syncing on-chain…</span>");
    match super::run_sponsored_tempo_call(&owner_hex, vec![call], gas, "sync key").await {
        Ok(_) => dom::swap_inner(
            msg,
            &dom::msg_span(dom::Msg::Accent, "synced ✓ — import your seed on another device to restore"),
        ),
        Err(e) => set_err(&format!("sync failed: {e}")),
    }
}

/// #18: fetch this subdomain's on-chain key ciphertext, decrypt it with
/// the seed-derived key (via the apex iframe — requires the seed to be
/// present on this device), and set it as the active Gemini key.
pub(super) async fn run_restore_key() {
    let msg = "key-sync-msg";
    let set_err = |m: &str| dom::swap_inner(msg, &dom::msg_span(dom::Msg::Error, &format!("{m}")));

    let name = match crate::app::tenant::current() {
        crate::app::tenant::Host::Tenant(n) => n,
        _ => {
            set_err("only on a subdomain");
            return;
        }
    };
    let id = match gemini_key_slot_id(&name).await {
        Ok(id) => id,
        Err(e) => {
            set_err(&e);
            return;
        }
    };
    dom::swap_inner(msg, "<span style=\"color:var(--muted)\">fetching…</span>");
    let ct = match crate::app::registry::gemini_key_of(id).await {
        Ok(Some(b)) => b,
        Ok(None) => {
            set_err("no synced key on-chain yet");
            return;
        }
        Err(e) => {
            set_err(&format!("read: {e}"));
            return;
        }
    };
    let ct_hex = format!(
        "0x{}",
        ct.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let plaintext = match crate::app::verify::open_key_via_iframe(&ct_hex).await {
        Ok(p) => p,
        Err(e) => {
            set_err(&format!("open: {e} — import your seed on this device first"));
            return;
        }
    };
    if let Some(input) = dom::input_by_id("key") {
        input.set_value(&plaintext);
    }
    crate::app::key_store::save(&plaintext).await;
    super::refresh_keymeta();
    dom::swap_inner(
        msg,
        &dom::msg_span(dom::Msg::Accent, "restored ✓ — applies on next session"),
    );
}

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
    let Some(ct) = decode_hex_local(&ct_hex) else { return };
    let Ok(registry_addr) = parse_address(crate::app::registry::REGISTRY_ADDRESS) else { return };
    let call = crate::tempo_tx::TempoCall {
        to: registry_addr,
        value_wei: 0,
        input: crate::app::registry::encode_set_gemini_key(slot_id, &ct),
    };
    let words = (ct.len() / 32 + 1) as u128;
    let gas = 1_200_000 + words * 40_000;
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
    let ct_hex = format!(
        "0x{}",
        ct.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
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
