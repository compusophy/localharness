//! Event delegation.
//!
//! HTMX-style — one click listener and one keydown listener at the
//! document level. UI elements declare intent through `data-action`
//! attributes; dispatch looks up the closest ancestor with one and
//! routes into a Rust handler. **No per-element closures.**
//!
//! Adding a new interaction is a 3-step process:
//! 1. Add `data-action="..."` to the relevant element in
//!    [`super::templates`].
//! 2. Add a variant to [`Action`] and parse it in [`Action::parse`].
//! 3. Handle the new variant in [`dispatch`].

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Document, Element, KeyboardEvent, MouseEvent};

use crate::filesystem::Filesystem;

use super::dom;
use super::templates;

/// Every user interaction maps to one of these. The closed enum makes
/// it obvious from one file what the app actually does. Variants with
/// payloads pull the value from the element's `data-arg` attribute.
#[derive(Debug, Clone)]
enum Action {
    Send,
    Reset,
    ClearKey,
    OpfsRefresh,
    OpfsWipe,
    OpfsWipeConfirm,
    OpfsWipeCancel,
    OpfsDelete(String),
    OpfsCloseViewer,
    OpfsNav(String),
    OpfsOpen(String),
    OpfsEdit(String),
    OpfsSave(String),
    ApexClaim,
    ClaimHere,
    ClaimOnChain,
    ImportOwner,
    RevealSeed,
    HideSeed,
    ImportSeed,
    CreateIdentity,
    ShowImport,
    CancelImport,
    HeaderAdminToggle,
    HeaderAdminClose,
    ResetArm,
    ResetConfirm,
    ResetCancel,
    PricingSave,
    ToggleFiles,
    ToggleFinancial,
    ToggleTerminal,
    ToggleView,
    ShowTab(String),
    FeedbackOpen,
    FeedbackClose,
    FeedbackSubmit,
    LhTransfer,
}

impl Action {
    fn parse(name: &str, arg: Option<String>) -> Option<Action> {
        Some(match name {
            "send" => Action::Send,
            "reset" => Action::Reset,
            "clear-key" => Action::ClearKey,
            "opfs-refresh" => Action::OpfsRefresh,
            "opfs-wipe" => Action::OpfsWipe,
            "opfs-wipe-confirm" => Action::OpfsWipeConfirm,
            "opfs-wipe-cancel" => Action::OpfsWipeCancel,
            "opfs-delete" => Action::OpfsDelete(arg.unwrap_or_default()),
            "opfs-close-viewer" => Action::OpfsCloseViewer,
            "opfs-nav" => Action::OpfsNav(arg.unwrap_or_default()),
            "opfs-open" => Action::OpfsOpen(arg.unwrap_or_default()),
            "opfs-edit" => Action::OpfsEdit(arg.unwrap_or_default()),
            "opfs-save" => Action::OpfsSave(arg.unwrap_or_default()),
            "apex-claim" => Action::ApexClaim,
            "claim-here" => Action::ClaimHere,
            "claim-on-chain" => Action::ClaimOnChain,
            "import-owner" => Action::ImportOwner,
            "reveal-seed" => Action::RevealSeed,
            "hide-seed" => Action::HideSeed,
            "import-seed" => Action::ImportSeed,
            "create-identity" => Action::CreateIdentity,
            "show-import" => Action::ShowImport,
            "cancel-import" => Action::CancelImport,
            "header-admin-toggle" => Action::HeaderAdminToggle,
            "header-admin-close" => Action::HeaderAdminClose,
            "reset-arm" => Action::ResetArm,
            "reset-confirm" => Action::ResetConfirm,
            "reset-cancel" => Action::ResetCancel,
            "pricing-save" => Action::PricingSave,
            "toggle-files" => Action::ToggleFiles,
            "toggle-financial" => Action::ToggleFinancial,
            "toggle-terminal" => Action::ToggleTerminal,
            "toggle-view" => Action::ToggleView,
            "show-tab" => Action::ShowTab(arg.unwrap_or_default()),
            "feedback-open" => Action::FeedbackOpen,
            "feedback-close" => Action::FeedbackClose,
            "feedback-submit" => Action::FeedbackSubmit,
            "lh-transfer" => Action::LhTransfer,
            _ => return None,
        })
    }
}

pub(crate) fn install_delegated_listeners(doc: &Document) -> Result<(), JsValue> {
    let click = Closure::<dyn FnMut(_)>::new(move |event: MouseEvent| {
        let Some(target) = event.target() else { return };
        let Ok(mut node) = target.dyn_into::<Element>() else { return };

        // Walk up from the event target looking for [data-action].
        // Take any [data-arg] from the SAME element so the two travel
        // as a single intent.
        let action = loop {
            if let Some(name) = node.get_attribute("data-action") {
                let arg = node.get_attribute("data-arg");
                break Action::parse(&name, arg);
            }
            match node.parent_element() {
                Some(parent) => node = parent,
                None => break None,
            }
        };

        if let Some(action) = action {
            event.prevent_default();
            dispatch(action);
        }
    });
    doc.add_event_listener_with_callback("click", click.as_ref().unchecked_ref())?;
    click.forget(); // listener lives for the lifetime of the document

    // Delegated input handler — routes per-element. The matrix is
    // small enough to dispatch by id; if it grows further, switch to
    // a `data-input` attribute pattern matching the click handler.
    let input_handler = Closure::<dyn FnMut(_)>::new(move |event: web_sys::Event| {
        let Some(target) = event.target() else { return };
        let Ok(el) = target.dyn_into::<Element>() else { return };
        match el.id().as_str() {
            "key" => on_key_input(),
            "apex-input" => on_apex_input(),
            _ => {}
        }
    });
    doc.add_event_listener_with_callback("input", input_handler.as_ref().unchecked_ref())?;
    input_handler.forget();

    // Delegated submit handler — apex / claim forms route through
    // this. preventDefault before dispatch so the browser doesn't try
    // to GET the page with form fields in the query string.
    let submit_handler = Closure::<dyn FnMut(_)>::new(move |event: web_sys::Event| {
        let Some(target) = event.target() else { return };
        let Ok(form) = target.dyn_into::<Element>() else { return };
        if let Some(name) = form.get_attribute("data-action") {
            if let Some(action) = Action::parse(&name, form.get_attribute("data-arg")) {
                event.prevent_default();
                dispatch(action);
            }
        }
    });
    doc.add_event_listener_with_callback("submit", submit_handler.as_ref().unchecked_ref())?;
    submit_handler.forget();

    // Enter inside the prompt textarea sends; Shift+Enter inserts a
    // newline (default browser behavior — we only intercept the bare
    // Enter case). Cmd/Ctrl+Enter still sends as a convention some
    // users have muscle-memory for.
    let keydown = Closure::<dyn FnMut(_)>::new(move |event: KeyboardEvent| {
        if event.key() != "Enter" {
            return;
        }
        let Some(target) = event.target() else { return };
        let Ok(el) = target.dyn_into::<Element>() else { return };
        if el.id() != "prompt" {
            return;
        }
        let mod_held = event.meta_key() || event.ctrl_key();
        let allow_newline = event.shift_key();
        if mod_held || !allow_newline {
            event.prevent_default();
            dispatch(Action::Send);
        }
    });
    doc.add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref())?;
    keydown.forget();

    Ok(())
}

/// Persist the Gemini key from the input field to sessionStorage +
/// OPFS, and refresh the "(N chars)" hint.
fn on_key_input() {
    if let Some(input) = dom::input_by_id("key") {
        let value = input.value();
        if let Ok(Some(storage)) = dom::session_storage() {
            let _ = storage.set_item("gemini_api_key", &value);
        }
        refresh_keymeta();
        wasm_bindgen_futures::spawn_local(async move {
            super::key_store::save(&value).await;
        });
    }
}

/// Live registry check as the user types a subdomain name. Sanitises
/// to the same charset the contract enforces, short-circuits on
/// too-short input, and queries `LocalharnessRegistry::idOfName` for
/// anything 3 chars or longer.
fn on_apex_input() {
    let Some(input) = dom::input_by_id("apex-input") else { return };
    let raw = input.value();
    let cleaned = super::tenant::sanitize(&raw);
    if cleaned != raw {
        // Reflect the canonical form so the user sees the live filter.
        input.set_value(&cleaned);
    }

    if cleaned.is_empty() {
        dom::swap_inner("apex-msg", "");
        return;
    }
    if cleaned.len() < 3 {
        dom::swap_inner(
            "apex-msg",
            "<span style=\"color:var(--muted)\">need at least 3 chars</span>",
        );
        return;
    }
    if cleaned.len() > 32 {
        dom::swap_inner(
            "apex-msg",
            "<span style=\"color:var(--error)\">max 32 chars</span>",
        );
        return;
    }

    // Stash this query string and compare again after the RPC returns —
    // if the user typed more characters meanwhile, drop the stale result.
    dom::swap_inner(
        "apex-msg",
        "<span style=\"color:var(--muted)\">checking registry…</span>",
    );
    let pending = cleaned.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let result = super::registry::check_name(&pending).await;
        // Only render if the field still matches what we checked.
        let still_pending = dom::input_by_id("apex-input")
            .map(|i| super::tenant::sanitize(&i.value()) == pending)
            .unwrap_or(false);
        if !still_pending {
            return;
        }
        let html = match result {
            Ok(super::registry::Status::Unknown) => {
                "<span style=\"color:var(--muted)\">registry pending deploy</span>".to_string()
            }
            Ok(super::registry::Status::Available) => format!(
                "<span style=\"color:var(--accent)\">✓ {pending} is available</span>"
            ),
            Ok(super::registry::Status::Taken { agent_id }) => format!(
                "<span style=\"color:var(--error)\">✗ {pending} is already registered (agentId {agent_id})</span>"
            ),
            Err(err) => format!(
                "<span style=\"color:var(--muted)\">registry error: {err}</span>"
            ),
        };
        dom::swap_inner("apex-msg", &html);
    });
}

/// Full apex claim flow: faucet → registration tx → confirm → redirect.
/// Splits out of `dispatch` so the spawn_local future stays readable.
async fn run_apex_claim(name: String) {
    let msg_id = "apex-msg";

    // 1. Confirm the name is still available right before sending.
    //    The live-check on input runs against `latest`, but a slow
    //    user might have been overtaken; cheap to re-query.
    dom::swap_inner(
        msg_id,
        "<span style=\"color:var(--muted)\">checking availability…</span>",
    );
    match super::registry::check_name(&name).await {
        Ok(super::registry::Status::Taken { agent_id }) => {
            dom::swap_inner(
                msg_id,
                &format!(
                    "<span style=\"color:var(--error)\">✗ {name} was just registered (agentId {agent_id})</span>"
                ),
            );
            return;
        }
        Ok(super::registry::Status::Unknown) => {
            dom::swap_inner(
                msg_id,
                "<span style=\"color:var(--error)\">registry not deployed — claim impossible</span>",
            );
            return;
        }
        Err(err) => {
            dom::swap_inner(
                msg_id,
                &format!("<span style=\"color:var(--error)\">availability check failed: {err}</span>"),
            );
            return;
        }
        Ok(super::registry::Status::Available) => {}
    }

    // 2. Pull the wallet out of App state. paint_apex loaded it at mount.
    let wallet_address = super::APP
        .with(|cell| cell.borrow().wallet.as_ref().map(|w| (w.signer.clone(), wallet_address_hex(&w.address))));
    let (signer, addr_hex) = match wallet_address {
        Some(pair) => pair,
        None => {
            dom::swap_inner(
                msg_id,
                "<span style=\"color:var(--error)\">wallet not loaded — refresh and try again</span>",
            );
            return;
        }
    };

    // 3. Faucet first. Idempotent enough for testnet; if the wallet
    //    is already funded, the call still succeeds (or rate-limits,
    //    which we treat as warning-not-fatal).
    dom::swap_inner(
        msg_id,
        "<span style=\"color:var(--muted)\">funding wallet from faucet…</span>",
    );
    if let Err(err) = super::registry::request_faucet_funds(&addr_hex).await {
        // Don't bail — the wallet might already have funds. Show but proceed.
        web_sys::console::warn_1(&JsValue::from_str(&format!("faucet: {err}")));
    }

    // 4. Build, sign, send, wait.
    dom::swap_inner(
        msg_id,
        "<span style=\"color:var(--muted)\">submitting registration on-chain…</span>",
    );
    match super::registry::claim_name(&signer, &name).await {
        Ok(tx_hash) => {
            dom::swap_inner(
                msg_id,
                &format!(
                    "<span style=\"color:var(--accent)\">✓ claimed (tx {})</span>",
                    short_hash(&tx_hash)
                ),
            );
            // 5. Hand off intent to the subdomain so it claims locally too.
            let target = format!("https://{name}.localharness.xyz/?claim=1");
            if let Ok(window) = dom::window() {
                let _ = window.location().assign(&target);
            }
        }
        Err(err) => {
            dom::swap_inner(
                msg_id,
                &format!(
                    "<span style=\"color:var(--error)\">claim failed: {err}</span>"
                ),
            );
        }
    }
}

fn wallet_address_hex(addr: &[u8; 20]) -> String {
    let mut s = String::with_capacity(42);
    s.push_str("0x");
    for b in addr {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn short_hash(tx_hash: &str) -> String {
    let stripped = tx_hash.trim_start_matches("0x");
    if stripped.len() < 12 {
        return tx_hash.to_string();
    }
    format!("{}…{}", &stripped[..6], &stripped[stripped.len() - 4..])
}

/// Recompute the "(N chars)" hint next to the key input. Called from
/// both the input listener and the mount restore path, so it lives
/// here.
pub(crate) fn refresh_keymeta() {
    if let Some(input) = dom::input_by_id("key") {
        let html = templates::keymeta(&input.value()).into_string();
        dom::swap_inner("keymeta", &html);
    }
}

fn dispatch(action: Action) {
    match action {
        Action::Send => {
            // Chat is async; defer to a spawn_local future so the
            // click handler returns immediately.
            wasm_bindgen_futures::spawn_local(async move {
                super::chat::run_send().await;
            });
        }
        Action::Reset => reset_pressed(),
        Action::ClearKey => clear_key_pressed(),
        Action::OpfsRefresh => {
            wasm_bindgen_futures::spawn_local(async move {
                super::opfs::refresh().await;
            });
        }
        Action::OpfsCloseViewer => super::opfs::close_viewer(),
        Action::OpfsNav(target) => {
            wasm_bindgen_futures::spawn_local(async move {
                super::opfs::navigate(&target).await;
            });
        }
        Action::OpfsOpen(name) => {
            wasm_bindgen_futures::spawn_local(async move {
                super::opfs::open_file(&name).await;
            });
        }
        Action::OpfsEdit(name) => {
            wasm_bindgen_futures::spawn_local(async move {
                super::opfs::edit_file(&name).await;
            });
        }
        Action::OpfsSave(name) => {
            wasm_bindgen_futures::spawn_local(async move {
                super::opfs::save_file(&name).await;
            });
        }
        Action::ApexClaim => {
            // Read + validate name, then run the full on-chain claim
            // flow async: faucet -> registry::claim_name -> redirect.
            let raw = dom::input_by_id("apex-input")
                .map(|i| i.value())
                .unwrap_or_default();
            let cleaned = super::tenant::sanitize(&raw);
            if cleaned.len() < 3 || cleaned.len() > 32 {
                dom::swap_inner(
                    "apex-msg",
                    "<span style=\"color:var(--error)\">name must be 3-32 chars, a-z 0-9 -</span>",
                );
                return;
            }
            wasm_bindgen_futures::spawn_local(async move {
                run_apex_claim(cleaned).await;
            });
        }
        Action::ClaimHere => {
            wasm_bindgen_futures::spawn_local(async move {
                match super::owner::claim().await {
                    Ok(id) => {
                        web_sys::console::log_1(&JsValue::from_str(&format!(
                            "claimed with owner id {id}"
                        )));
                        // Re-render the tenant chrome now that we own it.
                        if let super::tenant::Host::Tenant(name) = super::tenant::current() {
                            super::paint_tenant(super::tenant::Host::Tenant(name.clone()), name)
                                .await;
                        }
                    }
                    Err(err) => {
                        dom::swap_inner(
                            "claim-msg",
                            &format!(
                                "<span style=\"color:var(--error)\">claim failed: {err}</span>"
                            ),
                        );
                    }
                }
            });
        }
        Action::ClaimOnChain => {
            // Tenant-side first-claim: ensure apex wallet exists (without
            // overwriting an existing one — that would nuke other NFTs),
            // run the on-chain register tx via the signer iframe, then
            // set the local OPFS marker + re-paint as owner. This kills
            // the previous "bounce to apex first" interstitial.
            let Some(name) = (match super::tenant::current() {
                super::tenant::Host::Tenant(n) => Some(n),
                _ => None,
            }) else {
                return;
            };
            dom::swap_inner(
                "claim-msg",
                "<span style=\"color:var(--muted)\">ensuring identity at apex…</span>",
            );
            wasm_bindgen_futures::spawn_local(async move {
                if let Err(err) = super::verify::create_wallet_via_iframe(false).await {
                    dom::swap_inner(
                        "claim-msg",
                        &format!(
                            "<span style=\"color:var(--error)\">identity setup failed: {err}</span>"
                        ),
                    );
                    return;
                }
                dom::swap_inner(
                    "claim-msg",
                    "<span style=\"color:var(--muted)\">claiming on-chain…</span>",
                );
                match super::verify::claim_name_via_iframe(&name).await {
                    Ok((_owner, _tx)) => {
                        let _ = super::owner::claim().await;
                        super::paint_tenant(
                            super::tenant::Host::Tenant(name.clone()),
                            name,
                        )
                        .await;
                    }
                    Err(err) => {
                        dom::swap_inner(
                            "claim-msg",
                            &format!(
                                "<span style=\"color:var(--error)\">claim failed: {err}</span>"
                            ),
                        );
                    }
                }
            });
        }
        Action::ImportOwner => {
            let raw = dom::input_by_id("import-uuid")
                .map(|i| i.value().trim().to_string())
                .unwrap_or_default();
            if raw.len() < 32 {
                dom::swap_inner(
                    "claim-msg",
                    "<span style=\"color:var(--error)\">paste a full UUID (36 chars with dashes)</span>",
                );
                return;
            }
            wasm_bindgen_futures::spawn_local(async move {
                let fs = super::shared_opfs();
                if let Err(err) = fs.write_atomic(".lh_owner", raw.as_bytes()).await {
                    dom::swap_inner(
                        "claim-msg",
                        &format!(
                            "<span style=\"color:var(--error)\">import failed: {err}</span>"
                        ),
                    );
                    return;
                }
                if let super::tenant::Host::Tenant(name) = super::tenant::current() {
                    super::paint_tenant(super::tenant::Host::Tenant(name.clone()), name).await;
                }
            });
        }
        Action::RevealSeed => {
            // Apex: read mnemonic directly from cached wallet (sync).
            // Tenant: round-trip through the apex signer iframe so the
            // seed never leaves apex OPFS unannounced.
            match super::tenant::current() {
                super::tenant::Host::Apex => {
                    let phrase = super::APP.with(|cell| {
                        cell.borrow()
                            .wallet
                            .as_ref()
                            .map(|w| w.mnemonic.to_string())
                    });
                    if let Some(p) = phrase {
                        dom::swap_inner(
                            "seed-reveal",
                            &super::templates::seed_phrase(&p).into_string(),
                        );
                    }
                }
                _ => {
                    dom::swap_inner(
                        "seed-reveal",
                        "<span style=\"color:var(--muted)\">fetching…</span>",
                    );
                    wasm_bindgen_futures::spawn_local(async move {
                        match super::verify::reveal_seed_via_iframe().await {
                            Ok(phrase) => dom::swap_inner(
                                "seed-reveal",
                                &super::templates::seed_phrase(&phrase).into_string(),
                            ),
                            Err(err) => dom::swap_inner(
                                "seed-reveal",
                                &format!(
                                    "<span style=\"color:var(--error)\">reveal failed: {err}</span>\
                                     <button type=\"button\" data-action=\"reveal-seed\" class=\"ghost\">retry</button>"
                                ),
                            ),
                        }
                    });
                }
            }
        }
        Action::HideSeed => {
            dom::swap_inner(
                "seed-reveal",
                r#"<button type="button" data-action="reveal-seed">I have a pen and paper — reveal</button>"#,
            );
        }
        Action::CreateIdentity => {
            // Apex: generate locally + bootstrap-fund + re-paint.
            // Tenant: route through the apex signer iframe so the wallet
            // lands at apex OPFS, then re-paint tenant chrome so
            // verification picks up the new owner.
            dom::swap_inner(
                "identity-msg",
                "<span style=\"color:var(--muted)\">generating identity…</span>",
            );
            match super::tenant::current() {
                super::tenant::Host::Apex => {
                    wasm_bindgen_futures::spawn_local(async move {
                        let wallet = match super::wallet_store::create_and_persist().await {
                            Ok(w) => w,
                            Err(err) => {
                                dom::swap_inner(
                                    "identity-msg",
                                    &format!(
                                        "<span style=\"color:var(--error)\">create failed: {err}</span>"
                                    ),
                                );
                                return;
                            }
                        };
                        run_bootstrap_funding(wallet.signer.clone(), wallet.address_hex()).await;
                        super::paint_apex(super::tenant::Host::Apex).await;
                    });
                }
                host => {
                    // Explicit "create" button from tenant admin: pass
                    // overwrite=true because the user has clicked the
                    // create action with intent (just like at apex).
                    wasm_bindgen_futures::spawn_local(async move {
                        match super::verify::create_wallet_via_iframe(true).await {
                            Ok(_addr) => {
                                if let super::tenant::Host::Tenant(name) = &host {
                                    super::paint_tenant(host.clone(), name.clone()).await;
                                }
                            }
                            Err(err) => {
                                dom::swap_inner(
                                    "identity-msg",
                                    &format!(
                                        "<span style=\"color:var(--error)\">create failed: {err}</span>"
                                    ),
                                );
                            }
                        }
                    });
                }
            }
        }
        Action::ShowImport => {
            // Reveal the import textarea in place of the secondary
            // button — the ImportSeed action handler picks it up from
            // there.
            dom::swap_outer(
                "import-slot",
                &templates::import_seed_inline().into_string(),
            );
            if let Some(textarea) = dom::textarea_by_id("import-seed") {
                let _ = textarea.focus();
            }
        }
        Action::ImportSeed => {
            let phrase = dom::textarea_by_id("import-seed")
                .map(|t| t.value())
                .unwrap_or_default();
            if phrase.trim().split_whitespace().count() != 12 {
                dom::swap_inner(
                    "seed-msg",
                    "<span style=\"color:var(--error)\">expected exactly 12 words</span>",
                );
                return;
            }
            // Apex: write directly to apex OPFS, re-paint apex.
            // Tenant: route through signer iframe so the seed lands at
            // apex OPFS even though we're on a subdomain origin. This is
            // how cross-device pairing works — paste your desktop seed on
            // mobile and the master identity now lives on both devices.
            match super::tenant::current() {
                super::tenant::Host::Apex => {
                    wasm_bindgen_futures::spawn_local(async move {
                        match super::wallet_store::import(&phrase).await {
                            Ok(_) => {
                                super::paint_apex(super::tenant::Host::Apex).await;
                            }
                            Err(err) => {
                                dom::swap_inner(
                                    "seed-msg",
                                    &format!(
                                        "<span style=\"color:var(--error)\">import failed: {err}</span>"
                                    ),
                                );
                            }
                        }
                    });
                }
                host => {
                    wasm_bindgen_futures::spawn_local(async move {
                        match super::verify::import_seed_via_iframe(&phrase).await {
                            Ok(_addr) => {
                                if let super::tenant::Host::Tenant(name) = &host {
                                    super::paint_tenant(host.clone(), name.clone()).await;
                                }
                            }
                            Err(err) => {
                                dom::swap_inner(
                                    "seed-msg",
                                    &format!(
                                        "<span style=\"color:var(--error)\">import failed: {err}</span>"
                                    ),
                                );
                            }
                        }
                    });
                }
            }
        }
        Action::OpfsDelete(name) => {
            // Direct delete — no per-row confirm. Mistakes can be
            // recovered by re-creating the file; the wipe button is
            // the heavyweight "everything" confirm flow.
            wasm_bindgen_futures::spawn_local(async move {
                let fs = super::shared_opfs();
                if let Err(err) = fs.delete(&name).await {
                    web_sys::console::warn_1(&JsValue::from_str(&format!(
                        "delete({name}): {err}"
                    )));
                }
                super::opfs::refresh().await;
            });
        }
        Action::OpfsWipe => {
            // Arm the wipe — swap the button into an inline confirm
            // pair (yes / no). The actual wipe runs via OpfsWipeConfirm.
            dom::swap_outer(
                "opfs-wipe-slot",
                &templates::opfs_wipe_confirm_inline().into_string(),
            );
        }
        Action::OpfsWipeConfirm => {
            // Restore the slot first so the in-flight wipe doesn't
            // leave stale confirm buttons visible.
            dom::swap_outer(
                "opfs-wipe-slot",
                &templates::opfs_wipe_armed_inline().into_string(),
            );
            wasm_bindgen_futures::spawn_local(async move {
                super::opfs::wipe().await;
            });
        }
        Action::OpfsWipeCancel => {
            dom::swap_outer(
                "opfs-wipe-slot",
                &templates::opfs_wipe_armed_inline().into_string(),
            );
        }
        Action::CancelImport => {
            dom::swap_outer("import-slot", r#"<div id="import-slot"></div>"#);
        }
        Action::HeaderAdminToggle => header_admin_toggle(),
        Action::HeaderAdminClose => header_admin_close(),
        Action::ResetArm => {
            dom::swap_outer(
                "reset-confirm-slot",
                &templates::reset_confirm_inline().into_string(),
            );
        }
        Action::ResetCancel => {
            dom::swap_outer(
                "reset-confirm-slot",
                &templates::reset_armed_inline().into_string(),
            );
        }
        Action::ResetConfirm => reset_confirm_pressed(),
        Action::PricingSave => pricing_save_pressed(),
        Action::ToggleFiles => toggle_layout_class("files-collapsed"),
        Action::ToggleFinancial => toggle_layout_class("financial-collapsed"),
        Action::ToggleTerminal => toggle_layout_class("terminal-collapsed"),
        Action::ToggleView => toggle_layout_class("view-collapsed"),
        Action::ShowTab(name) => show_mobile_tab(&name),
        Action::FeedbackOpen => feedback_open(),
        Action::FeedbackClose => feedback_close(),
        Action::FeedbackSubmit => feedback_submit(),
        Action::LhTransfer => lh_transfer_pressed(),
    }
}

/// $localharness transfer from the visitor's apex wallet to a recipient.
/// Reads the recipient + amount from the financial-card form, builds
/// `transfer(address,uint256)` calldata, signs via the apex signer
/// iframe, submits to Tempo Moderato. Caller's gas paid by the apex
/// wallet (visitor) — Tempo allows contract calls, so no native-transfer
/// ban hits this path.
fn lh_transfer_pressed() {
    let to_raw = dom::input_by_id("lh-transfer-to")
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();
    let amount_raw = dom::input_by_id("lh-transfer-amount")
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();
    if !is_address_hex(&to_raw) {
        dom::swap_inner(
            "lh-transfer-msg",
            "<span style=\"color:var(--error)\">recipient must be a 0x… address</span>",
        );
        return;
    }
    let Some(amount_wei) = parse_token_amount(&amount_raw) else {
        dom::swap_inner(
            "lh-transfer-msg",
            "<span style=\"color:var(--error)\">amount: positive number, up to 18 decimals</span>",
        );
        return;
    };
    if amount_wei == 0 {
        dom::swap_inner(
            "lh-transfer-msg",
            "<span style=\"color:var(--error)\">amount must be greater than zero</span>",
        );
        return;
    }

    // Visitor's apex address. The verify state's address (verified or
    // visitor) is the master wallet that the iframe signer controls.
    let from_hex = super::APP.with(|cell| {
        use super::VerifyState;
        match &cell.borrow().verify_state {
            VerifyState::Verified { address } => Some(address.clone()),
            VerifyState::Visitor { visitor_address, .. } => Some(visitor_address.clone()),
            _ => None,
        }
    });
    let Some(from_hex) = from_hex else {
        dom::swap_inner(
            "lh-transfer-msg",
            "<span style=\"color:var(--error)\">no apex identity yet — open admin to create one</span>",
        );
        return;
    };

    dom::swap_inner(
        "lh-transfer-msg",
        "<span style=\"color:var(--muted)\">signing + submitting…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(err) = run_lh_transfer(from_hex, to_raw, amount_wei).await {
            dom::swap_inner(
                "lh-transfer-msg",
                &format!("<span style=\"color:var(--error)\">{err}</span>"),
            );
        }
    });
}

async fn run_lh_transfer(
    from_hex: String,
    to_hex: String,
    amount_wei: u128,
) -> Result<(), String> {
    let calldata = encode_transfer_calldata(&to_hex, amount_wei)?;
    let calldata_hex = bytes_to_hex_str(&calldata);

    let nonce = super::registry::next_nonce(&from_hex).await
        .map_err(|e| format!("nonce: {e}"))?;
    let gas_price = super::registry::current_gas_price().await
        .map_err(|e| format!("gas price: {e}"))?;
    let gas_limit = 100_000u128; // ERC-20 transfer is ~50k; double for safety.

    let raw_tx = super::verify::sign_tx_via_iframe(super::verify::SignTxRequest {
        to_hex: super::registry::LOCALHARNESS_TOKEN_ADDRESS,
        value_wei: 0,
        nonce,
        gas_limit,
        gas_price,
        chain_id: super::registry::CHAIN_ID,
        purpose: "send $localharness",
        data_hex: &calldata_hex,
    })
    .await
    .map_err(|e| format!("signer: {e}"))?;

    let tx_hash = super::registry::submit_and_wait_receipt(&raw_tx).await
        .map_err(|e| format!("submit: {e}"))?;

    let short = tx_short_hash(&tx_hash);
    dom::swap_inner(
        "lh-transfer-msg",
        &format!("<span style=\"color:var(--accent)\">✓ sent (tx {short})</span>"),
    );
    if let Some(input) = dom::input_by_id("lh-transfer-amount") {
        input.set_value("");
    }
    // Refresh balance shown in the card. Cheap re-read; if it fails,
    // the next paint_tenant will pick it up. Don't bubble errors.
    if let super::tenant::Host::Tenant(name) = super::tenant::current() {
        super::paint_tenant(super::tenant::Host::Tenant(name.clone()), name).await;
    }
    Ok(())
}

fn is_address_hex(s: &str) -> bool {
    let stripped = s.trim_start_matches("0x").trim_start_matches("0X");
    stripped.len() == 40 && stripped.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Parse a human-typed amount like `1.5` or `0.000001` into 18-decimal
/// token wei. Returns None on garbage input. Accepts up to 18 fractional
/// digits; truncates anything finer.
fn parse_token_amount(raw: &str) -> Option<u128> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (whole_s, frac_s) = match raw.split_once('.') {
        Some((w, f)) => (w, f),
        None => (raw, ""),
    };
    let whole: u128 = if whole_s.is_empty() {
        0
    } else {
        whole_s.parse().ok()?
    };
    if frac_s.bytes().any(|b| !b.is_ascii_digit()) {
        return None;
    }
    let mut frac: u128 = 0;
    let mut scale: u128 = 1_000_000_000_000_000_000;
    for ch in frac_s.chars().take(18) {
        let d = ch.to_digit(10)? as u128;
        scale /= 10;
        frac = frac.checked_add(d.checked_mul(scale)?)?;
    }
    let whole_wei = whole.checked_mul(1_000_000_000_000_000_000)?;
    whole_wei.checked_add(frac)
}

/// ABI-encode `transfer(address,uint256)` — selector + padded address +
/// padded amount. Keccak the signature here so we don't depend on a
/// constant elsewhere.
fn encode_transfer_calldata(to_hex: &str, amount_wei: u128) -> Result<Vec<u8>, String> {
    use sha3::{Digest, Keccak256};
    let mut hasher = Keccak256::new();
    hasher.update(b"transfer(address,uint256)");
    let digest = hasher.finalize();
    let mut selector = [0u8; 4];
    selector.copy_from_slice(&digest[..4]);

    let to_bytes = parse_address(to_hex)?;
    let mut to_padded = [0u8; 32];
    to_padded[12..].copy_from_slice(&to_bytes);
    let mut amount_padded = [0u8; 32];
    amount_padded[16..].copy_from_slice(&amount_wei.to_be_bytes());

    let mut out = Vec::with_capacity(4 + 32 + 32);
    out.extend_from_slice(&selector);
    out.extend_from_slice(&to_padded);
    out.extend_from_slice(&amount_padded);
    Ok(out)
}

fn parse_address(hex: &str) -> Result<[u8; 20], String> {
    let stripped = hex.trim_start_matches("0x").trim_start_matches("0X");
    if stripped.len() != 40 {
        return Err(format!("address must be 40 hex chars, got {}", stripped.len()));
    }
    let mut out = [0u8; 20];
    let bytes = stripped.as_bytes();
    for i in 0..20 {
        let hi = hex_nibble(bytes[i * 2])?;
        let lo = hex_nibble(bytes[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(format!("non-hex byte {b}")),
    }
}

fn bytes_to_hex_str(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn tx_short_hash(tx_hash: &str) -> String {
    let stripped = tx_hash.trim_start_matches("0x");
    if stripped.len() < 12 {
        return tx_hash.to_string();
    }
    format!("{}…{}", &stripped[..6], &stripped[stripped.len() - 4..])
}

/// Toggle the header admin dropdown. Origin determines content —
/// apex shows seed reveal + import + reset, tenant has the gemini
/// api key input + reset. After opening, pre-fill the api key from
/// sessionStorage / OPFS so the user sees their existing key
/// (admin opens and closes constantly; the input is fresh DOM each time).
fn header_admin_toggle() {
    let body = match super::tenant::current() {
        super::tenant::Host::Apex => templates::admin_dropdown_apex().into_string(),
        super::tenant::Host::Tenant(_) | super::tenant::Host::Other(_) => {
            templates::admin_dropdown_tenant().into_string()
        }
    };
    dom::swap_outer("header-admin-panel", &body);

    // Pre-fill api key from sessionStorage (sync) then refresh from
    // OPFS (async). Same pattern as the old in-chrome key restore.
    if matches!(
        super::tenant::current(),
        super::tenant::Host::Tenant(_) | super::tenant::Host::Other(_)
    ) {
        if let Ok(Some(storage)) = dom::session_storage() {
            if let Ok(Some(cached)) = storage.get_item("gemini_api_key") {
                if let Some(input) = dom::input_by_id("key") {
                    input.set_value(&cached);
                    refresh_keymeta();
                }
            }
        }
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(persisted) = super::key_store::load().await {
                if let Some(input) = dom::input_by_id("key") {
                    input.set_value(&persisted);
                    refresh_keymeta();
                }
            }
        });
    }
}

fn header_admin_close() {
    dom::swap_outer(
        "header-admin-panel",
        r#"<div id="header-admin-panel" hidden></div>"#,
    );
}

/// Mobile-only: swap which `tab-<name>` class is on `#layout`.
/// CSS uses it to show exactly one panel at a time on narrow
/// viewports. Tab button styling syncs by toggling `.active`.
fn show_mobile_tab(name: &str) {
    let Some(layout) = dom::by_id("layout") else { return };
    let parts: Vec<String> = layout
        .class_name()
        .split_whitespace()
        .filter(|c| !c.starts_with("tab-"))
        .map(String::from)
        .collect();
    let mut new_cls = parts.join(" ");
    if !new_cls.is_empty() {
        new_cls.push(' ');
    }
    new_cls.push_str(&format!("tab-{name}"));
    layout.set_class_name(&new_cls);

    // Reflect active state on each tab button by id — small fixed
    // set of tabs, no need for query_selector_all (which needs the
    // NodeList web-sys feature we don't enable).
    for tab in ["files", "edit", "chat", "agent"] {
        let id = format!("tab-btn-{tab}");
        let Some(el) = dom::by_id(&id) else { continue };
        let cls = el.class_name();
        let mut classes: Vec<&str> =
            cls.split_whitespace().filter(|c| *c != "active").collect();
        if tab == name {
            classes.push("active");
        }
        el.set_class_name(&classes.join(" "));
    }
}

/// Inject the feedback modal into the body (overlays everything).
fn feedback_open() {
    let Ok(doc) = dom::document() else { return };
    let Some(body) = doc.body() else { return };
    // If a modal already exists, focus the textarea instead of stacking.
    if let Some(_existing) = doc.get_element_by_id("feedback-modal") {
        if let Some(t) = dom::textarea_by_id("feedback-text") {
            let _ = t.focus();
        }
        return;
    }
    let _ = body.insert_adjacent_html(
        "beforeend",
        &templates::feedback_modal().into_string(),
    );
    if let Some(t) = dom::textarea_by_id("feedback-text") {
        let _ = t.focus();
    }
}

fn feedback_close() {
    if let Some(el) = dom::by_id("feedback-modal") {
        if let Some(parent) = el.parent_element() {
            let _ = parent.remove_child(&el);
        }
    }
}

fn feedback_submit() {
    let Some(textarea) = dom::textarea_by_id("feedback-text") else {
        return;
    };
    let text = textarea.value().trim().to_string();
    if text.is_empty() {
        dom::swap_inner(
            "feedback-msg",
            "<span style=\"color:var(--muted)\">type something first.</span>",
        );
        return;
    }
    dom::swap_inner(
        "feedback-msg",
        "<span style=\"color:var(--muted)\">saving…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        match append_feedback(&text).await {
            Ok(()) => {
                dom::swap_inner(
                    "feedback-msg",
                    "<span style=\"color:var(--accent)\">saved to .lh_feedback.txt</span>",
                );
                // Auto-close after a short beat so the user gets confirmation.
                if let Some(window) = web_sys::window() {
                    let cb = Closure::<dyn FnMut()>::new(|| {
                        feedback_close();
                    });
                    let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                        cb.as_ref().unchecked_ref(),
                        900,
                    );
                    cb.forget();
                }
            }
            Err(err) => {
                dom::swap_inner(
                    "feedback-msg",
                    &format!(
                        "<span style=\"color:var(--error)\">save failed: {err}</span>"
                    ),
                );
            }
        }
    });
}

/// Append a feedback entry to `.lh_feedback.txt` in this origin's
/// OPFS. Each entry is one line: `ISO-timestamp\\tTEXT\\n`. Caller
/// (the operator) can read the file off any device via the file
/// browser; eventually this routes to an on-chain `FeedbackFacet`
/// so devs can scrape events without per-device access.
async fn append_feedback(text: &str) -> Result<(), String> {
    use crate::filesystem::Filesystem;
    let fs = super::shared_opfs();
    let existing = fs.read(".lh_feedback.txt").await.unwrap_or_default();
    let now = js_sys::Date::new_0().to_iso_string().as_string().unwrap_or_default();
    let entry = format!("{now}\t{text}\n");
    let mut combined = existing;
    combined.extend_from_slice(entry.as_bytes());
    fs.write_atomic(".lh_feedback.txt", &combined)
        .await
        .map_err(|e| format!("{e}"))
}

/// Pure DOM class flip on `#layout` — used by the panel toggles
/// (files-collapsed, financial-collapsed) so a collapse + expand
/// doesn't lose any panel state (open file viewer, pricing edit
/// in-flight, etc.). CSS handles the actual hide/show.
fn toggle_layout_class(class: &str) {
    let Some(layout) = dom::by_id("layout") else { return };
    let current = layout.class_name();
    let trimmed = current.trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    let new_cls = if parts.iter().any(|c| *c == class) {
        parts.iter().filter(|c| **c != class).copied().collect::<Vec<_>>().join(" ")
    } else if parts.is_empty() {
        class.to_string()
    } else {
        format!("{} {class}", parts.join(" "))
    };
    layout.set_class_name(&new_cls);
}

/// Full bootstrap-funding sequence for a freshly-created wallet:
/// 1. `tempo_fundAddress` for the gas drip (so subsequent contract
///    calls can pay their own gas).
/// 2. Poll `eth_getBalance` until the gas lands (or timeout).
/// 3. If [`super::registry::BOOTSTRAP_FAUCET_ADDRESS`] is set (non-zero),
///    call `BootstrapFaucet.fund(self)` for the bigger drip so the
///    user can register a name + transact without re-hitting the public
///    faucet.
///
/// Status messages flow into `#identity-msg` so the user sees what's
/// happening. Errors short-circuit the rest of the sequence but the
/// identity itself is already saved — the user can retry funding
/// later via a (future) "top up" affordance, or just live with the
/// gas drip until the BootstrapFaucet is reachable.
async fn run_bootstrap_funding(
    signer: k256::ecdsa::SigningKey,
    addr_hex: String,
) {
    dom::swap_inner(
        "identity-msg",
        "<span style=\"color:var(--muted)\">funding wallet (gas drip)…</span>",
    );
    if let Err(err) = super::registry::request_faucet_funds(&addr_hex).await {
        // Faucet rate-limited or down — show but proceed; balance poll
        // below will catch the "actually 0" case and bail.
        web_sys::console::warn_1(&JsValue::from_str(&format!("faucet: {err}")));
    }

    dom::swap_inner(
        "identity-msg",
        "<span style=\"color:var(--muted)\">waiting for gas to land…</span>",
    );
    // 15-second window. Tempo blocks are ~1s.
    if let Err(err) = super::registry::wait_for_min_balance(&addr_hex, 1, 15).await {
        dom::swap_inner(
            "identity-msg",
            &format!(
                "<span style=\"color:var(--error)\">funding stalled: {err}. \
                 identity saved; try again later.</span>"
            ),
        );
        return;
    }

    // Mint $localharness tokens to the new wallet via the
    // LocalharnessToken self-faucet. This is what gives the user
    // actual spending power for paid agents — gas alone isn't useful.
    dom::swap_inner(
        "identity-msg",
        "<span style=\"color:var(--muted)\">claiming starter $localharness…</span>",
    );
    match super::registry::token_faucet_self(&signer).await {
        Ok(tx) => {
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "token_faucet_self tx: {tx}"
            )));
        }
        Err(err) => {
            web_sys::console::warn_1(&JsValue::from_str(&format!(
                "token_faucet_self: {err}"
            )));
            // Soft-fail: identity is saved + gas drip landed. User can
            // retry by re-creating identity (after admin reset) or wait
            // for a future top-up affordance.
        }
    }
}

/// Inline-confirmed reset: nuke every entry at OPFS root, reload.
/// Replaces the old `window.confirm()` flow per [[feedback-no-js-alerts]].
fn reset_confirm_pressed() {
    wasm_bindgen_futures::spawn_local(async move {
        let fs = super::shared_opfs();
        if let Ok(entries) = fs.read_dir("").await {
            for entry in entries {
                let _ = fs.delete(&entry.name).await;
            }
        }
        if let Ok(window) = dom::window() {
            let _ = window.location().reload();
        }
    });
}

/// Parse the pricing-input as a decimal test-ETH amount, convert to
/// wei, persist via `pricing::save`, and re-paint the card so the
/// new value shows. Owner-only — the input is only rendered when
/// the verifier confirmed this visitor is the owner — but we still
/// re-check `verify_state` here as belt-and-suspenders against a
/// stale DOM.
fn pricing_save_pressed() {
    let Some(input) = dom::input_by_id("pricing-input") else {
        return;
    };
    let raw = input.value().trim().to_string();
    let wei = match parse_eth_to_wei(&raw) {
        Ok(w) => w,
        Err(err) => {
            dom::swap_inner(
                "pricing-msg",
                &format!(
                    "<span style=\"color:var(--error)\">{err}</span>"
                ),
            );
            return;
        }
    };

    let is_owner = super::APP.with(|cell| {
        matches!(cell.borrow().verify_state, super::VerifyState::Verified { .. })
    });
    if !is_owner {
        dom::swap_inner(
            "pricing-msg",
            "<span style=\"color:var(--error)\">only the verified owner can change pricing</span>",
        );
        return;
    }

    dom::swap_inner(
        "pricing-msg",
        "<span style=\"color:var(--muted)\">saving…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        match super::pricing::save(wei).await {
            Ok(()) => {
                super::APP
                    .with(|cell| cell.borrow_mut().pricing_wei = Some(wei));
                let html = templates::pricing_card_body(wei, true).into_string();
                dom::swap_outer("pricing-body", &html);
            }
            Err(err) => {
                dom::swap_inner(
                    "pricing-msg",
                    &format!(
                        "<span style=\"color:var(--error)\">save failed: {err}</span>"
                    ),
                );
            }
        }
    });
}

/// Parse a decimal test-ETH amount ("0", "0.001", "1.5") into a wei
/// `u128`. Rejects negatives, NaN-shaped input, and values with more
/// than 18 fractional digits (wei is the precision floor).
fn parse_eth_to_wei(s: &str) -> Result<u128, String> {
    if s.is_empty() {
        return Ok(0);
    }
    let (whole_str, frac_str) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s, ""),
    };
    if !whole_str.bytes().all(|b| b.is_ascii_digit()) {
        return Err("price must be a positive decimal".into());
    }
    if !frac_str.bytes().all(|b| b.is_ascii_digit()) {
        return Err("price must be a positive decimal".into());
    }
    if frac_str.len() > 18 {
        return Err("price has more precision than wei (18 decimals max)".into());
    }
    let whole: u128 = whole_str.parse().map_err(|e| format!("whole: {e}"))?;
    // Right-pad fraction to 18 digits then parse.
    let mut padded = String::with_capacity(18);
    padded.push_str(frac_str);
    while padded.len() < 18 {
        padded.push('0');
    }
    let frac: u128 = if padded.is_empty() {
        0
    } else {
        padded.parse().map_err(|e| format!("frac: {e}"))?
    };
    whole
        .checked_mul(1_000_000_000_000_000_000)
        .and_then(|w| w.checked_add(frac))
        .ok_or_else(|| "price too large".into())
}


// --- Action handlers ---------------------------------------------------

fn reset_pressed() {
    super::APP.with(|cell| {
        let mut app = cell.borrow_mut();
        app.agent = None;
        app.session_key = None;
        app.turn_count = 0;
        app.pending_history = None;
    });
    dom::swap_inner("transcript", "");
    // No status text — clearing is silent.
    // Drop the persisted history too — a reload after reset starts
    // fresh, matching the user's expectation of "new conversation."
    wasm_bindgen_futures::spawn_local(async move {
        super::history::clear().await;
    });
    if let Some(prompt) = dom::textarea_by_id("prompt") {
        prompt.focus().ok();
    }
}

fn clear_key_pressed() {
    if let Some(input) = dom::input_by_id("key") {
        input.set_value("");
    }
    if let Ok(Some(storage)) = dom::session_storage() {
        let _ = storage.remove_item("gemini_api_key");
    }
    refresh_keymeta();
    if let Some(input) = dom::input_by_id("key") {
        input.focus().ok();
    }
    wasm_bindgen_futures::spawn_local(async move {
        super::key_store::clear().await;
    });
    dom::set_status("key cleared (sessionStorage + OPFS)", false);
}
