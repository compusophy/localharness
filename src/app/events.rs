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
    OpfsCloseViewer,
    OpfsNav(String),
    OpfsOpen(String),
    OpfsEdit(String),
    OpfsSave(String),
    ApexClaim,
    ClaimHere,
    ImportOwner,
    RevealSeed,
    HideSeed,
    ImportSeed,
}

impl Action {
    fn parse(name: &str, arg: Option<String>) -> Option<Action> {
        Some(match name {
            "send" => Action::Send,
            "reset" => Action::Reset,
            "clear-key" => Action::ClearKey,
            "opfs-refresh" => Action::OpfsRefresh,
            "opfs-wipe" => Action::OpfsWipe,
            "opfs-close-viewer" => Action::OpfsCloseViewer,
            "opfs-nav" => Action::OpfsNav(arg.unwrap_or_default()),
            "opfs-open" => Action::OpfsOpen(arg.unwrap_or_default()),
            "opfs-edit" => Action::OpfsEdit(arg.unwrap_or_default()),
            "opfs-save" => Action::OpfsSave(arg.unwrap_or_default()),
            "apex-claim" => Action::ApexClaim,
            "claim-here" => Action::ClaimHere,
            "import-owner" => Action::ImportOwner,
            "reveal-seed" => Action::RevealSeed,
            "hide-seed" => Action::HideSeed,
            "import-seed" => Action::ImportSeed,
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

    // Cmd/Ctrl+Enter inside the prompt textarea triggers send.
    let keydown = Closure::<dyn FnMut(_)>::new(move |event: KeyboardEvent| {
        if event.key() == "Enter" && (event.meta_key() || event.ctrl_key()) {
            // Only when focus is on the prompt — avoid hijacking globally.
            if let Some(target) = event.target() {
                if let Ok(el) = target.dyn_into::<Element>() {
                    if el.id() == "prompt" {
                        event.prevent_default();
                        dispatch(Action::Send);
                    }
                }
            }
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
            // Read the mnemonic out of the cached wallet (loaded in
            // paint_apex) and swap it into the reveal slot. No async
            // I/O needed — the wallet is in App state.
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
        Action::HideSeed => {
            dom::swap_inner(
                "seed-reveal",
                r#"<button type="button" data-action="reveal-seed">I have a pen and paper — reveal</button>"#,
            );
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
            wasm_bindgen_futures::spawn_local(async move {
                match super::wallet_store::import(&phrase).await {
                    Ok(_) => {
                        // Refresh the apex chrome — wallet field will
                        // show the imported address.
                        let host = super::tenant::current();
                        if matches!(host, super::tenant::Host::Apex) {
                            super::paint_apex(host).await;
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
        Action::OpfsWipe => {
            // Browser confirm() is sync — fine here since the click
            // handler is already a synchronous closure dispatching to
            // a future. Skip the deletion if the user backs out.
            let proceed = dom::window()
                .ok()
                .and_then(|w| w.confirm_with_message("Wipe all files in this tab's OPFS? This can't be undone.").ok())
                .unwrap_or(false);
            if proceed {
                wasm_bindgen_futures::spawn_local(async move {
                    super::opfs::wipe().await;
                });
            }
        }
    }
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
    dom::set_status("ready · new conversation", false);
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
