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
    ShowAdminTab(String),
    SyncKey,
    RestoreKey,
    RevealSecurity,
    HideSecurity,
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
    PairStart,
    PairCancel,
    PairJoin,
    AgentActToggle(String),
    AgentSendLh(String),
    SavePrompt,
    SaveToolAllowlist,
    ResetToolAllowlist,
    SaveApiKey,
    ToggleDisplay,
    StopTurn,
    PublishApp,
}

impl Action {
    fn parse(name: &str, arg: Option<String>) -> Option<Action> {
        Some(match name {
            "send" => Action::Send,
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
            "show-admin-tab" => Action::ShowAdminTab(arg.unwrap_or_default()),
            "sync-key" => Action::SyncKey,
            "restore-key" => Action::RestoreKey,
            "reveal-security" => Action::RevealSecurity,
            "hide-security" => Action::HideSecurity,
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
            "pair-start" => Action::PairStart,
            "pair-cancel" => Action::PairCancel,
            "pair-join" => Action::PairJoin,
            "agent-act-toggle" => Action::AgentActToggle(arg.unwrap_or_default()),
            "agent-send-lh" => Action::AgentSendLh(arg.unwrap_or_default()),
            "save-prompt" => Action::SavePrompt,
            "save-tool-allowlist" => Action::SaveToolAllowlist,
            "reset-tool-allowlist" => Action::ResetToolAllowlist,
            "save-api-key" => Action::SaveApiKey,
            "toggle-display" => Action::ToggleDisplay,
            "stop-turn" => Action::StopTurn,
            "publish-app" => Action::PublishApp,
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

    // Delegated pointer tracking for the DISPLAY canvas. The display
    // cartridge ABI is poll-model (Orbclient-style): the cartridge reads
    // pointer_x/pointer_y each frame, so we just keep the latest cursor
    // position fresh. No-op when the canvas isn't mounted.
    let mousemove = Closure::<dyn FnMut(_)>::new(move |event: MouseEvent| {
        if dom::by_id("display-canvas").is_some() {
            super::display::set_pointer(event.client_x() as f64, event.client_y() as f64);
        }
    });
    doc.add_event_listener_with_callback("mousemove", mousemove.as_ref().unchecked_ref())?;
    mousemove.forget();

    // Primary-button state for the display. Press counts only when it
    // starts on the canvas; release clears regardless of where it lands.
    let mousedown = Closure::<dyn FnMut(_)>::new(move |event: MouseEvent| {
        if let Some(target) = event.target() {
            if let Ok(el) = target.dyn_into::<Element>() {
                if el.id() == "display-canvas" {
                    super::display::set_pointer(event.client_x() as f64, event.client_y() as f64);
                    super::display::set_pointer_down(true);
                }
            }
        }
    });
    doc.add_event_listener_with_callback("mousedown", mousedown.as_ref().unchecked_ref())?;
    mousedown.forget();

    let mouseup = Closure::<dyn FnMut(_)>::new(move |_event: MouseEvent| {
        super::display::set_pointer_down(false);
    });
    doc.add_event_listener_with_callback("mouseup", mouseup.as_ref().unchecked_ref())?;
    mouseup.forget();

    // Touch input — map the first touch to the same display pointer state
    // as the mouse, so drag-based cartridges (drawing) work on phones.
    // The canvas sets `touch-action: none` in CSS, so these don't need
    // non-passive preventDefault to stop the page scrolling under a draw.
    let touchstart = Closure::<dyn FnMut(_)>::new(move |event: web_sys::TouchEvent| {
        if let Some(target) = event.target() {
            if let Ok(el) = target.dyn_into::<Element>() {
                if el.id() == "display-canvas" {
                    if let Some(t) = event.touches().get(0) {
                        super::display::set_pointer(t.client_x() as f64, t.client_y() as f64);
                        super::display::set_pointer_down(true);
                    }
                }
            }
        }
    });
    doc.add_event_listener_with_callback("touchstart", touchstart.as_ref().unchecked_ref())?;
    touchstart.forget();

    let touchmove = Closure::<dyn FnMut(_)>::new(move |event: web_sys::TouchEvent| {
        if dom::by_id("display-canvas").is_some() {
            if let Some(t) = event.touches().get(0) {
                super::display::set_pointer(t.client_x() as f64, t.client_y() as f64);
            }
        }
    });
    doc.add_event_listener_with_callback("touchmove", touchmove.as_ref().unchecked_ref())?;
    touchmove.forget();

    let touchend = Closure::<dyn FnMut(_)>::new(move |_event: web_sys::TouchEvent| {
        super::display::set_pointer_down(false);
    });
    doc.add_event_listener_with_callback("touchend", touchend.as_ref().unchecked_ref())?;
    touchend.forget();

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

/// States for the submit button. `Disabled` = grey, not clickable
/// (length out of range, registry check pending, name taken).
/// `Ready` = accent-green, clickable. `Failed` = red, disabled,
/// label swapped to "✗ failed" so a chain-reverted claim doesn't
/// silently look like nothing happened. The next keystroke into the
/// input clears the failed state via `on_apex_input`.
enum CreateBtnState {
    Disabled,
    Ready,
    Failed,
}

fn set_create_button_state(state: CreateBtnState) {
    match state {
        CreateBtnState::Disabled => set_create_button_classes(false, false, "create"),
        CreateBtnState::Ready => set_create_button_classes(true, false, "create"),
        CreateBtnState::Failed => set_create_button_classes(false, true, "✗ failed"),
    }
}

/// Set the create button to the failed state with a custom label
/// (e.g., "need 30 more LH"). Same red styling + disabled attribute
/// as `CreateBtnState::Failed`, but a more specific message.
/// Cleared on the next keystroke by `on_apex_input`.
fn set_create_button_failed_with(label: &str) {
    set_create_button_classes(false, true, label);
}

fn set_create_button_classes(enabled: bool, failed: bool, label: &str) {
    let Some(btn) = dom::by_id("create-btn") else { return };
    let stripped: String = btn
        .class_name()
        .split_whitespace()
        .filter(|c| *c != "ready" && *c != "failed")
        .collect::<Vec<_>>()
        .join(" ");
    if enabled {
        let _ = btn.remove_attribute("disabled");
    } else {
        let _ = btn.set_attribute("disabled", "");
    }
    let class = if enabled {
        format!("{stripped} ready")
    } else if failed {
        format!("{stripped} failed")
    } else {
        stripped
    };
    btn.set_class_name(&class);
    btn.set_inner_html(label);
}

/// Live registry check as the user types a subdomain name. Sanitises
/// to the same charset the contract enforces. The ONLY visible output
/// is the submit button's state — disabled (default) or ready (the
/// accent-green CTA). No status text under the input, no error
/// messages. Per [[feedback-no-explanatory-validation]].
fn on_apex_input() {
    let Some(input) = dom::input_by_id("apex-input") else { return };
    let raw = input.value();
    let cleaned = super::tenant::sanitize(&raw);
    if cleaned != raw {
        // Reflect the canonical form so the user sees the live filter.
        input.set_value(&cleaned);
    }

    // Length check first — short-circuit before hitting the registry
    // for input we already know won't pass on-chain validation.
    if cleaned.len() < 3 || cleaned.len() > 32 {
        set_create_button_state(CreateBtnState::Disabled);
        return;
    }

    // Disable while the registry roundtrip is in flight, then enable
    // (with the .ready style) only on Status::Available.
    set_create_button_state(CreateBtnState::Disabled);
    let pending = cleaned.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let result = super::registry::check_name(&pending).await;
        // Only act on the result if the input still matches what we
        // queried — otherwise the user typed more chars and a fresh
        // check is already in flight.
        let still_pending = dom::input_by_id("apex-input")
            .map(|i| super::tenant::sanitize(&i.value()) == pending)
            .unwrap_or(false);
        if !still_pending {
            return;
        }
        match result {
            Ok(super::registry::Status::Available) => {
                set_create_button_state(CreateBtnState::Ready);
            }
            _ => {
                // Taken, registry-not-deployed, or RPC error all map to
                // "not currently claimable" — no text, just keep
                // the button disabled.
                set_create_button_state(CreateBtnState::Disabled);
            }
        }
    });
}

/// Full apex claim flow: faucet → registration tx → confirm → redirect.
/// Silent except for the button itself — disabled with text "creating…"
/// while in flight, redirects on success, reverts to the input-driven
/// state on failure. All status/error chatter goes to `console.warn`
/// for debuggability. Per [[feedback-no-explanatory-validation]].
async fn run_apex_claim(name: String) {
    set_create_button_busy(true);

    let result: Result<String, String> = async {
        // 1. Re-confirm availability — the user might have been
        //    overtaken between live-check and submit.
        match super::registry::check_name(&name).await {
            Ok(super::registry::Status::Available) => {}
            Ok(other) => return Err(format!("name not available: {other:?}")),
            Err(err) => return Err(format!("check_name: {err}")),
        }

        // 2. Pull the wallet out of App state — or generate one in
        //    place. The subdomain IS the identity primitive: a visitor
        //    arriving at apex without a wallet is just one who hasn't
        //    claimed yet. Roll wallet creation into this submit so we
        //    never end up with a wallet that doesn't own anything
        //    on-chain.
        let cached = super::APP.with(|cell| {
            cell.borrow()
                .wallet
                .as_ref()
                .map(|w| (w.signer.clone(), wallet_address_hex(&w.address)))
        });
        let (signer, addr_hex) = match cached {
            Some(pair) => pair,
            None => match super::wallet_store::create_and_persist().await {
                Ok(wallet) => {
                    let pair = (wallet.signer.clone(), wallet.address_hex());
                    super::APP.with(|cell| cell.borrow_mut().wallet = Some(wallet));
                    pair
                }
                Err(err) => return Err(format!("wallet: {err}")),
            },
        };

        // 2.5. Cost-gate pre-check. Registration is currently free
        //      (on-chain `registrationCost` is 0), so this is a no-op
        //      today. It stays as a guard in case the cost gate is ever
        //      re-enabled: if the registry charges LH and the user can't
        //      cover it, bail before burning sponsor gas on a guaranteed
        //      revert.
        let cost = super::registry::registration_cost().await.unwrap_or(0);
        if cost > 0 {
            let bal = super::registry::token_balance_of(&addr_hex).await.unwrap_or(0);
            if bal < cost {
                let deficit_lh = (cost - bal) / 1_000_000_000_000_000_000u128;
                return Err(format!("__NEED_LH__{deficit_lh}"));
            }
        }

        // 3. Submit the claim as a sponsored Tempo tx. The bundle's
        //    sponsor wallet pays the fees in AlphaUSD; the user's
        //    fresh apex wallet signs as sender and never needs any
        //    native gas or any TIP-20 stablecoin. No faucet step.
        let fee_payer = super::sponsor::signer()
            .map_err(|e| format!("sponsor key: {e}"))?;
        super::registry::claim_and_maybe_set_main_sponsored(
            &signer,
            &fee_payer,
            &name,
            super::registry::ALPHA_USD_ADDRESS,
        )
        .await
        .map_err(|e| format!("claim_name: {e}"))
    }
    .await;

    match result {
        Ok(tx_hash) => {
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "claimed {name} (tx {})",
                short_hash(&tx_hash)
            )));
            let target = format!("https://{name}.localharness.xyz/?claim=1");
            if let Ok(window) = dom::window() {
                let _ = window.location().assign(&target);
            }
        }
        Err(err) => {
            web_sys::console::warn_1(&JsValue::from_str(&format!("apex claim failed: {err}")));
            // Surface failure on the button itself so the user knows
            // the click had an effect — a silent reset to disabled
            // looks indistinguishable from "nothing happened" and
            // invites frustrated re-clicking. `on_apex_input` clears
            // the failed state on the next keystroke.
            //
            // Specific case: insufficient credits. Pre-check encodes
            // the deficit in the error string with a sentinel prefix
            // so we can show "need N more LH" instead of a generic
            // "✗ failed".
            if let Some(rest) = err.strip_prefix("__NEED_LH__") {
                set_create_button_failed_with(&format!("need {rest} more LH"));
            } else {
                set_create_button_state(CreateBtnState::Failed);
            }
        }
    }
}

/// Swap the create button between its idle state (whatever `.ready` /
/// `disabled` it had) and the in-flight "creating…" state. The
/// in-flight state is always disabled + label-swapped so the user
/// can't double-submit and can see something is happening without a
/// separate status string.
fn set_create_button_busy(busy: bool) {
    let Some(btn) = dom::by_id("create-btn") else { return };
    if busy {
        btn.set_inner_html("creating…");
        let _ = btn.set_attribute("disabled", "");
    } else {
        btn.set_inner_html("create");
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
        Action::ClearKey => clear_key_pressed(),
        Action::OpfsRefresh => {
            wasm_bindgen_futures::spawn_local(async move {
                super::opfs::refresh().await;
            });
        }
        Action::OpfsCloseViewer => super::opfs::close_viewer(),
        Action::ToggleDisplay => super::opfs::toggle_display(),
        Action::StopTurn => super::chat::request_stop_turn(),
        Action::PublishApp => {
            wasm_bindgen_futures::spawn_local(async move {
                run_publish_app().await;
            });
        }
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
            // Silent no-op on invalid input — the create button is
            // disabled by `on_apex_input` when length is out of range,
            // so this branch only ever fires for valid names. Per
            // [[feedback-no-explanatory-validation]].
            let raw = dom::input_by_id("apex-input")
                .map(|i| i.value())
                .unwrap_or_default();
            let cleaned = super::tenant::sanitize(&raw);
            if cleaned.len() < 3 || cleaned.len() > 32 {
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
                            &dom::msg_span(dom::Msg::Error, &format!("claim failed: {err}")),
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
                        &dom::msg_span(dom::Msg::Error, &format!("identity setup failed: {err}")),
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
                            &dom::msg_span(dom::Msg::Error, &format!("claim failed: {err}")),
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
                        &dom::msg_span(dom::Msg::Error, &format!("import failed: {err}")),
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
                                &maud::html! {
                                    span style="color:var(--error)" { "reveal failed: " (err) }
                                    button type="button" data-action="reveal-seed" class="ghost" { "retry" }
                                }
                                .into_string(),
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
                        if let Err(err) = super::wallet_store::create_and_persist().await {
                            dom::swap_inner(
                                "identity-msg",
                                &dom::msg_span(dom::Msg::Error, &format!("create failed: {err}")),
                            );
                            return;
                        }
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
                                    &dom::msg_span(dom::Msg::Error, &format!("create failed: {err}")),
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
            if phrase.split_whitespace().count() != 12 {
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
                                    &dom::msg_span(dom::Msg::Error, &format!("import failed: {err}")),
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
                                    &dom::msg_span(dom::Msg::Error, &format!("import failed: {err}")),
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
                // Deleting the conversation history wipes the on-screen
                // transcript instantly — no page refresh. (on-chain feedback)
                if name == ".lh_history.json" {
                    dom::swap_inner("transcript", "");
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
        Action::ShowAdminTab(name) => show_admin_tab(&name),
        Action::SyncKey => {
            wasm_bindgen_futures::spawn_local(async move { run_sync_key().await });
        }
        Action::RestoreKey => {
            wasm_bindgen_futures::spawn_local(async move { run_restore_key().await });
        }
        Action::RevealSecurity => {
            dom::swap_outer(
                "security-slot",
                &templates::admin_security_expanded().into_string(),
            );
        }
        Action::HideSecurity => {
            dom::swap_outer(
                "security-slot",
                &templates::admin_security_collapsed().into_string(),
            );
        }
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
        Action::PairStart => pair_start_pressed(),
        Action::PairCancel => pair_cancel_pressed(),
        Action::PairJoin => {
            if let Some(code) = super::read_query_param("pair") {
                pair_join_pressed(code);
            }
        }
        Action::AgentActToggle(token_id) => agent_act_toggle_pressed(token_id),
        Action::AgentSendLh(token_id) => agent_send_lh_pressed(token_id),
        Action::SavePrompt => save_prompt_pressed(),
        Action::SaveToolAllowlist => save_tool_allowlist_pressed(),
        Action::ResetToolAllowlist => reset_tool_allowlist_pressed(),
        Action::SaveApiKey => save_api_key_pressed(),
    }
}

/// Persist the textarea content as the per-origin custom system
/// prompt. Empty/whitespace-only content deletes the file, reverting
/// to the bundle's default. The change takes effect on the next
/// session start — surfaced inline so the user knows what to expect.
fn save_prompt_pressed() {
    let Some(textarea) = dom::textarea_by_id("prompt-input") else { return };
    let content = textarea.value();
    dom::swap_inner(
        "prompt-msg",
        "<span style=\"color:var(--muted)\">saving…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        match super::system_prompt::save(&content).await {
            Ok(()) => {
                let trimmed = content.trim();
                let summary = if trimmed.is_empty() {
                    "✓ saved · using default on next session"
                } else {
                    "✓ saved · takes effect on next session"
                };
                dom::swap_inner(
                    "prompt-msg",
                    &dom::msg_span(dom::Msg::Accent, &format!("{summary}")),
                );
            }
            Err(err) => {
                dom::swap_inner(
                    "prompt-msg",
                    &dom::msg_span(dom::Msg::Error, &format!("{err}")),
                );
            }
        }
    });
}

fn save_tool_allowlist_pressed() {
    use crate::types::BuiltinTool;
    let mut enabled = Vec::new();
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Ok(checkboxes) = doc.query_selector_all(".tool-checkbox") {
            for i in 0..checkboxes.length() {
                if let Some(el) = checkboxes.get(i) {
                    let input: web_sys::HtmlInputElement = JsCast::unchecked_into(el);
                    if input.checked() {
                        if let Some(name) = input.get_attribute("data-tool") {
                            if let Some(tool) = BuiltinTool::ALL.iter().find(|t| t.wire_name() == name) {
                                enabled.push(*tool);
                            }
                        }
                    }
                }
            }
        }
    }
    dom::swap_inner(
        "tool-allowlist-msg",
        "<span style=\"color:var(--muted)\">saving…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        match super::tool_allowlist::save(&enabled).await {
            Ok(()) => {
                let summary = super::tool_allowlist::summary(&enabled);
                dom::swap_inner(
                    "tool-allowlist-msg",
                    &dom::msg_span(dom::Msg::Accent, &format!("✓ saved · {summary} · takes effect on next session")),
                );
            }
            Err(err) => {
                dom::swap_inner(
                    "tool-allowlist-msg",
                    &dom::msg_span(dom::Msg::Error, &format!("{err}")),
                );
            }
        }
    });
}

fn reset_tool_allowlist_pressed() {
    dom::swap_inner(
        "tool-allowlist-msg",
        "<span style=\"color:var(--muted)\">resetting…</span>",
    );
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Ok(checkboxes) = doc.query_selector_all(".tool-checkbox") {
            for i in 0..checkboxes.length() {
                if let Some(el) = checkboxes.get(i) {
                    let input: web_sys::HtmlInputElement = JsCast::unchecked_into(el);
                    input.set_checked(true);
                }
            }
        }
    }
    wasm_bindgen_futures::spawn_local(async move {
        match super::tool_allowlist::save(&[]).await {
            Ok(()) => {
                dom::swap_inner(
                    "tool-allowlist-msg",
                    "<span style=\"color:var(--accent)\">✓ reset · all tools enabled · takes effect on next session</span>",
                );
            }
            Err(err) => {
                dom::swap_inner(
                    "tool-allowlist-msg",
                    &dom::msg_span(dom::Msg::Error, &format!("{err}")),
                );
            }
        }
    });
}

/// Save the API key from the centered modal, then dismiss the modal.
fn save_api_key_pressed() {
    let Some(input) = dom::input_by_id("api-key-input") else { return };
    let value = input.value().trim().to_string();
    if value.is_empty() {
        return;
    }
    if let Ok(Some(storage)) = dom::session_storage() {
        let _ = storage.set_item("gemini_api_key", &value);
    }
    dom::swap_inner(
        "api-key-msg",
        "<span style=\"color:var(--muted)\">checking…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        super::key_store::save(&value).await;
        super::opfs::refresh().await;
        // Validate against Gemini so a bad key is caught here, not
        // mid-turn. A definitive rejection keeps the modal open; a valid
        // key OR an inconclusive check (network/CORS) closes it — we
        // never block the user on a flaky probe.
        if let Some(false) = gemini_key_is_valid(&value).await {
            dom::swap_inner(
                "api-key-msg",
                "<span style=\"color:var(--error)\">key rejected — check it</span>",
            );
            return;
        }
        if let Some(el) = dom::by_id("api-key-modal") {
            if let Some(parent) = el.parent_element() {
                let _ = parent.remove_child(&el);
            }
        }
        // Auto-sync to the MAIN slot on-chain (best-effort, seed-bearing
        // devices only) so other subdomains + linked devices pick it up
        // without re-entry. Fire-and-forget after the modal closes.
        if let super::tenant::Host::Tenant(name) = super::tenant::current() {
            auto_sync_gemini_key(name, value).await;
        }
    });
}

/// Probe whether a Gemini API key works via a cheap `models.list` GET
/// (no token cost). `Some(true/false)` is definitive; `None` means the
/// check was inconclusive (network/CORS) and the caller should not block
/// on it. Browser→Gemini CORS is already proven by the chat path.
async fn gemini_key_is_valid(key: &str) -> Option<bool> {
    let url = format!("https://generativelanguage.googleapis.com/v1beta/models?key={key}");
    match reqwest::Client::new().get(&url).send().await {
        Ok(resp) => Some(resp.status().is_success()),
        Err(_) => None,
    }
}

/// Expand or collapse the inline act-panel under an agent row.
/// First open fetches TBA balance + paints the panel; subsequent
/// toggles just flip the `hidden` attribute on the existing DOM.
fn agent_act_toggle_pressed(token_id_str: String) {
    let Ok(token_id) = token_id_str.parse::<u64>() else { return };
    let panel_id = format!("agent-act-{token_id}");
    let Some(panel) = dom::by_id(&panel_id) else { return };
    let was_hidden = panel.has_attribute("hidden");
    if was_hidden {
        // First-paint flow: fetch TBA + balance, render the form.
        panel.set_inner_html(
            "<div class=\"admin-msg-slot\"><span style=\"color:var(--muted)\">loading…</span></div>",
        );
        let _ = panel.remove_attribute("hidden");
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(err) = paint_agent_act_panel(token_id).await {
                let panel_id = format!("agent-act-{token_id}");
                dom::swap_inner(
                    &panel_id,
                    &maud::html! {
                        div class="admin-msg-slot" {
                            span style="color:var(--error)" { (err) }
                        }
                    }
                    .into_string(),
                );
            }
        });
    } else {
        let _ = panel.set_attribute("hidden", "");
    }
}

async fn paint_agent_act_panel(token_id: u64) -> Result<(), String> {
    let tba = super::registry::tba_of_token_id(token_id)
        .await
        .map_err(|e| format!("tba: {e}"))?
        .ok_or_else(|| "no TBA".to_string())?;
    let balance = super::registry::token_balance_of(&tba).await.unwrap_or(0);
    let html = templates::agent_act_panel(token_id, &tba, balance).into_string();
    let panel_id = format!("agent-act-{token_id}");
    dom::swap_inner(&panel_id, &html);
    Ok(())
}

/// User clicked "send" in an inline act-panel. Reads the recipient
/// and amount inputs scoped to this token_id, fires a sponsored
/// `tba.execute(credits, 0, transfer(...), 0)` tempo tx. The user's
/// apex wallet signs as one of the TBA's authorized signers (it IS
/// the NFT owner). Sponsor pays AlphaUSD.
fn agent_send_lh_pressed(token_id_str: String) {
    let Ok(token_id) = token_id_str.parse::<u64>() else { return };
    let msg_id = format!("agent-act-msg-{token_id}");

    let to_raw = dom::input_by_id(&format!("agent-send-to-{token_id}"))
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();
    let amt_raw = dom::input_by_id(&format!("agent-send-amt-{token_id}"))
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();
    if !is_address_hex(&to_raw) {
        return; // silent no-op per [[feedback-no-explanatory-validation]]
    }
    let Some(amount_wei) = parse_token_amount(&amt_raw) else { return };
    if amount_wei == 0 {
        return;
    }

    let signer = super::APP.with(|cell| {
        cell.borrow().wallet.as_ref().map(|w| w.signer.clone())
    });
    let Some(signer) = signer else { return };

    dom::swap_inner(
        &msg_id,
        "<span style=\"color:var(--muted)\">signing + submitting…</span>",
    );

    wasm_bindgen_futures::spawn_local(async move {
        let msg_id = format!("agent-act-msg-{token_id}");
        let result = async {
            let tba = super::registry::tba_of_token_id(token_id)
                .await
                .map_err(|e| format!("tba: {e}"))?
                .ok_or_else(|| "no TBA".to_string())?;
            let fee_payer = super::sponsor::signer()?;
            super::registry::tba_transfer_lh_sponsored(
                &signer,
                &fee_payer,
                token_id,
                &tba,
                &to_raw,
                amount_wei,
                super::registry::ALPHA_USD_ADDRESS,
            )
            .await
        }
        .await;
        match result {
            Ok(tx_hash) => {
                let short = tx_short_hash(&tx_hash);
                dom::swap_inner(
                    &msg_id,
                    &dom::msg_span(dom::Msg::Accent, &format!("✓ sent (tx {short})")),
                );
                // Re-paint to refresh the balance line.
                let _ = paint_agent_act_panel(token_id).await;
            }
            Err(err) => {
                dom::swap_inner(
                    &msg_id,
                    &dom::msg_span(dom::Msg::Error, &format!("{err}")),
                );
            }
        }
    });
}

/// Fetch the credit balance for the apex wallet and write it into
/// `#credits-balance`. Called on admin-open. Soft-fail — leaves the
/// placeholder on error so the UI stays clean.
pub(crate) async fn refresh_credits_pill() {
    let addr = super::APP.with(|cell| {
        cell.borrow().wallet.as_ref().map(|w| w.address_hex())
    });
    let Some(addr) = addr else { return };
    let Ok(balance_wei) = super::registry::token_balance_of(&addr).await else { return };
    let lh = balance_wei / 1_000_000_000_000_000_000u128;
    dom::swap_inner("credits-balance", &format!("{lh} LH"));
}

async fn refresh_signer_list() {
    let addr = super::APP.with(|cell| {
        cell.borrow().wallet.as_ref().map(|w| w.address_hex())
    });
    let Some(addr) = addr else { return };

    let main_id = match super::registry::main_of(&addr).await {
        Ok(id) if id > 0 => id,
        _ => {
            dom::swap_inner("signer-list", "no MAIN set");
            return;
        }
    };

    let main_name = match super::registry::name_of_id(main_id).await {
        Ok(name) if !name.is_empty() => name,
        _ => {
            dom::swap_inner("signer-list", "");
            return;
        }
    };

    let tba = match super::registry::tba_of_name(&main_name).await {
        Ok(Some(tba)) => tba,
        _ => {
            dom::swap_inner("signer-list", "no TBA");
            return;
        }
    };

    // Fetch signers
    match super::registry::tba_signers(&tba).await {
        Ok(signers) if signers.is_empty() => {
            dom::swap_inner("signer-list", "owner only (no extra signers)");
        }
        Ok(signers) => {
            let mut html = String::new();
            for s in &signers {
                let short = if s.len() > 10 {
                    format!("{}…{}", &s[..6], &s[s.len()-4..])
                } else {
                    s.clone()
                };
                html.push_str(&format!(
                    "<div style=\"color:var(--fg);font-size:11px;margin:2px 0\">\
                     <code>{short}</code></div>"
                ));
            }
            dom::swap_inner("signer-list", &html);
        }
        Err(_) => {
            dom::swap_inner("signer-list", "");
        }
    }
}

/// Desktop side of device pairing. Generate a one-time code, show it +
/// the deep link to open on the other device, then poll the on-chain
/// pairing rendezvous. When the phone announces (its fresh device key as
/// `msg.sender` of a sponsored tx), we learn its address from the log
/// and enroll it via `addSigner` — no 0x ever copied between machines.
fn pair_start_pressed() {
    // The user's MAIN name is what the phone will open (its own
    // subdomain). Resolve it from the apex wallet's MAIN.
    wasm_bindgen_futures::spawn_local(async move {
        let owner_hex = super::APP
            .with(|cell| cell.borrow().wallet.as_ref().map(|w| w.address_hex()));
        let Some(owner_hex) = owner_hex else {
            dom::swap_inner("pair-msg", &dom::msg_span(dom::Msg::Error, "no identity"));
            return;
        };
        let token_id = match super::registry::main_of(&owner_hex).await {
            Ok(id) if id != 0 => id,
            _ => {
                dom::swap_inner(
                    "pair-msg",
                    &dom::msg_span(dom::Msg::Error, "claim a subdomain first"),
                );
                return;
            }
        };
        let name = match super::registry::name_of_id(token_id).await {
            Ok(n) if !n.is_empty() => n,
            _ => {
                dom::swap_inner("pair-msg", &dom::msg_span(dom::Msg::Error, "no MAIN name"));
                return;
            }
        };

        // One-time code: 6 uppercase base32-ish chars from CSPRNG.
        let code = generate_pair_code();
        let pair_url = format!("https://{name}.localharness.xyz/?pair={code}");
        dom::swap_outer(
            "pair-slot",
            &templates::pair_panel(&code, &pair_url).into_string(),
        );
        dom::swap_inner("pair-msg", "");

        // Poll the rendezvous: ~5 min at 3s intervals.
        let code_hash = super::registry::pairing_code_hash(&code);
        let mut found: Option<(String, Vec<u8>)> = None;
        for _ in 0..100 {
            // Stop if the user cancelled (panel swapped away).
            if dom::by_id("pair-slot")
                .map(|el| !el.class_name().contains("pair-active"))
                .unwrap_or(true)
            {
                return;
            }
            match super::registry::find_pairing_device(&code_hash).await {
                Ok(Some(pair)) => {
                    found = Some(pair);
                    break;
                }
                _ => {}
            }
            super::registry::sleep_ms(3000).await;
        }

        let Some((device, device_pubkey)) = found else {
            dom::swap_inner(
                "pair-msg",
                &dom::msg_span(dom::Msg::Error, "timed out — try again"),
            );
            dom::swap_outer(
                "pair-slot",
                &r#"<div id="pair-slot" class="pair-slot"><button id="pair-btn" type="button" data-action="pair-start" class="ghost">link a device</button></div>"#,
            );
            return;
        };

        dom::swap_inner(
            "pair-msg",
            &dom::msg_span(dom::Msg::Accent, "device found — enrolling…"),
        );
        match run_add_device(device.clone()).await {
            Ok(tx_hash) => {
                let short = tx_short_hash(&tx_hash);
                dom::swap_inner(
                    "pair-msg",
                    &dom::msg_span(
                        dom::Msg::Accent,
                        &format!("✓ linked {} (tx {short})", short_addr(&device)),
                    ),
                );
                dom::swap_outer(
                    "pair-slot",
                    &r#"<div id="pair-slot" class="pair-slot"><button id="pair-btn" type="button" data-action="pair-start" class="ghost">link another device</button></div>"#,
                );
                // ECIES-wrap the MAIN Gemini key to this device's pubkey and
                // post it on-chain, so the phone gets the key WITHOUT ever
                // importing the seed. Best-effort: silently skipped if no key
                // is synced or the seed isn't on this device.
                if !device_pubkey.is_empty() {
                    if wrap_and_post_key_to_device(token_id, &owner_hex, &device, &device_pubkey)
                        .await
                        .is_ok()
                    {
                        dom::swap_inner(
                            "pair-msg",
                            &dom::msg_span(
                                dom::Msg::Accent,
                                &format!(
                                    "✓ linked {} + shared your key — it's ready to use",
                                    short_addr(&device)
                                ),
                            ),
                        );
                    }
                }
                refresh_signer_list().await;
            }
            Err(err) => {
                dom::swap_inner("pair-msg", &dom::msg_span(dom::Msg::Error, &format!("{err}")));
            }
        }
    });
}

/// Cancel an in-progress pairing — swap the panel back to the button.
/// The poll loop notices the missing `.pair-active` class and exits.
fn pair_cancel_pressed() {
    dom::swap_outer(
        "pair-slot",
        &r#"<div id="pair-slot" class="pair-slot"><button id="pair-btn" type="button" data-action="pair-start" class="ghost">link a device</button></div>"#,
    );
    dom::swap_inner("pair-msg", "");
}

/// Phone side. The device opened `<name>.localharness.xyz/?pair=CODE`.
/// Generate a fresh device keypair, persist it as this origin's wallet,
/// and announce on-chain (sponsored) so the desktop can enroll it.
pub(crate) fn pair_join_pressed(code: String) {
    dom::swap_inner(
        "pair-join-msg",
        "<span style=\"color:var(--muted)\">generating device key + announcing…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        match run_pair_join(&code).await {
            Ok(addr) => {
                dom::swap_inner(
                    "pair-join-msg",
                    &dom::msg_span(
                        dom::Msg::Accent,
                        &format!(
                            "✓ announced {} — finish on your other device.",
                            short_addr(&addr)
                        ),
                    ),
                );
            }
            Err(err) => {
                dom::swap_inner(
                    "pair-join-msg",
                    &dom::msg_span(dom::Msg::Error, &format!("{err}")),
                );
            }
        }
    });
}

/// Generate the device key, persist it to this origin's OPFS so the
/// device can keep acting as a signer, and announce `keccak(code)`
/// on-chain via a sponsored tx.
async fn run_pair_join(code: &str) -> Result<String, String> {
    // A fresh device identity for THIS subdomain origin. Persist it so
    // future visits reuse the same enrolled key (the apex flow stores a
    // mnemonic; here a per-device random key is enough — it's a signer,
    // not the master seed).
    let wallet = crate::wallet::generate();
    let device_hex = wallet.address_hex();
    super::wallet_store::persist_device_key(&wallet.private_key_hex)
        .await
        .map_err(|e| format!("save device key: {e}"))?;

    let code_hash = super::registry::pairing_code_hash(code);
    // Announce our compressed pubkey so the desktop can ECIES-wrap the
    // Gemini key directly to us — we never need the master seed.
    let pubkey = crate::wallet::pubkey_compressed(&wallet.signer);
    let fee_payer = super::sponsor::signer()?;
    super::registry::announce_pairing_sponsored(
        &wallet.signer,
        &fee_payer,
        &code_hash,
        &pubkey,
        super::registry::ALPHA_USD_ADDRESS,
    )
    .await?;

    // Background: poll for the desktop's ECIES-wrapped Gemini key, decrypt
    // it with our device key, and save it locally — so this device never
    // prompts for an API key. Best-effort; the device still works as a
    // signer even if the desktop has no key to share.
    let device_signer = wallet.signer.clone();
    let device_addr = device_hex.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let name = match super::tenant::current() {
            super::tenant::Host::Tenant(n) => n,
            _ => return,
        };
        let slot_id = match gemini_key_slot_id(&name).await {
            Ok(id) => id,
            Err(_) => return,
        };
        for _ in 0..40 {
            if let Ok(Some(blob)) =
                super::registry::wrapped_device_key_of(slot_id, &device_addr).await
            {
                if let Some(pt) = super::encryption::ecies_open(&device_signer, &blob).await {
                    if let Ok(key) = String::from_utf8(pt) {
                        super::key_store::save(&key).await;
                        if let Ok(Some(storage)) = dom::session_storage() {
                            let _ = storage.set_item("gemini_api_key", &key);
                        }
                        dom::swap_inner(
                            "pair-join-msg",
                            &dom::msg_span(
                                dom::Msg::Accent,
                                "✓ linked + key received — open this subdomain to use it",
                            ),
                        );
                        return;
                    }
                }
            }
            super::registry::sleep_ms(3000).await;
        }
    });

    Ok(device_hex)
}

/// Derive the seed-sync AES key from the LOCAL apex master wallet (no
/// iframe — this runs on the apex origin where the seed lives). Must
/// match `signer::seed_sync_key` byte-for-byte so a blob sealed by the
/// iframe path opens here. `None` if no wallet is loaded.
fn apex_seed_sync_key() -> Option<[u8; 32]> {
    use sha3::{Digest, Keccak256};
    super::APP.with(|cell| {
        let app = cell.borrow();
        let wallet = app.wallet.as_ref()?;
        let entropy = wallet.mnemonic.to_entropy();
        let mut hasher = Keccak256::new();
        hasher.update(b"localharness/v0/keysync");
        hasher.update(&entropy);
        let mut out = [0u8; 32];
        out.copy_from_slice(&hasher.finalize());
        Some(out)
    })
}

/// Desktop side. Read the MAIN's seed-sealed Gemini key from chain,
/// decrypt it with the local seed, ECIES-wrap it to the freshly-paired
/// device's pubkey, and post that blob under the device's per-device slot.
/// Errors (no key synced, no seed here) are returned so the caller can
/// silently skip — the device still enrolls as a signer regardless.
async fn wrap_and_post_key_to_device(
    main_id: u64,
    owner_hex: &str,
    device_hex: &str,
    device_pubkey: &[u8],
) -> Result<(), String> {
    let ct = super::registry::gemini_key_of(main_id)
        .await
        .map_err(|e| format!("read key: {e}"))?
        .ok_or_else(|| "no synced key".to_string())?;
    let seed_key = apex_seed_sync_key().ok_or_else(|| "no seed on this device".to_string())?;
    let plaintext = super::encryption::open_with_raw_key(&seed_key, &ct)
        .await
        .ok_or_else(|| "decrypt failed".to_string())?;
    let blob = super::encryption::ecies_seal(device_pubkey, &plaintext)
        .await
        .ok_or_else(|| "wrap failed".to_string())?;
    let signer = super::APP
        .with(|cell| cell.borrow().wallet.as_ref().map(|w| w.signer.clone()))
        .ok_or_else(|| "no wallet".to_string())?;
    let fee_payer = super::sponsor::signer()?;
    super::registry::set_device_wrapped_key_sponsored(
        &signer,
        &fee_payer,
        main_id,
        device_hex,
        &blob,
        super::registry::ALPHA_USD_ADDRESS,
    )
    .await?;
    let _ = owner_hex; // owner is implied by the local signer
    Ok(())
}

/// 6-char one-time pairing code (Crockford-ish base32, no ambiguous
/// chars) from the browser CSPRNG. Short enough to read aloud / type.
fn generate_pair_code() -> String {
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let mut bytes = [0u8; 6];
    let _ = getrandom::getrandom(&mut bytes);
    bytes
        .iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect()
}

/// Resolve the current user's MAIN's TBA address, then submit a
/// sponsored Tempo tx that creates the TBA (if needed) + adds the new
/// device as an authorized signer.
async fn run_add_device(new_signer_hex: String) -> Result<String, String> {
    // Apex wallet — must be present (the button is only rendered when
    // the apex has a wallet, so failing here is an unusual race).
    let (signer, owner_hex) = super::APP
        .with(|cell| {
            cell.borrow()
                .wallet
                .as_ref()
                .map(|w| (w.signer.clone(), w.address_hex()))
        })
        .ok_or_else(|| "no apex identity".to_string())?;

    // Identify the user's MAIN. `mainOf` returns the tokenId or 0.
    let token_id = super::registry::main_of(&owner_hex)
        .await
        .map_err(|e| format!("mainOf: {e}"))?;
    if token_id == 0 {
        return Err("claim a subdomain first — it becomes your MAIN".into());
    }

    // Derive the MAIN's TBA address (counterfactual or already-deployed).
    let tba_addr = super::registry::tba_of_token_id(token_id)
        .await
        .map_err(|e| format!("tba lookup: {e}"))?
        .ok_or_else(|| "no TBA for MAIN".to_string())?;

    let fee_payer = super::sponsor::signer()?;
    super::registry::add_signer_sponsored(
        &signer,
        &fee_payer,
        token_id,
        &tba_addr,
        &new_signer_hex,
        super::registry::ALPHA_USD_ADDRESS,
    )
    .await
}

fn short_addr(addr: &str) -> String {
    let stripped = addr.trim_start_matches("0x");
    if stripped.len() < 8 {
        return addr.to_string();
    }
    format!("0x{}…{}", &stripped[..4], &stripped[stripped.len() - 4..])
}

/// Sponsored Tempo tx orchestrator. Apex iframe signs `sender_hash`,
/// the bundle sponsor key signs `fee_payer_hash`, raw tx assembled
/// locally and submitted. User holds zero of anything — `fee_payer`
/// pays fees in AlphaUSD.
///
/// `from_hex` is the sender's EOA — it must own whatever balance the
/// calls touch (e.g. $LH for a `transfer`), but does NOT need native
/// gas or the fee_token.
pub(crate) async fn run_sponsored_tempo_call(
    from_hex: &str,
    calls: Vec<crate::tempo_tx::TempoCall>,
    gas_limit: u128,
    purpose: &str,
) -> Result<String, String> {
    let sender_address = parse_address(from_hex)?;
    let fee_token_addr = parse_address(super::registry::ALPHA_USD_ADDRESS)?;
    let nonce = super::registry::next_nonce(from_hex).await
        .map_err(|e| format!("nonce: {e}"))?;
    let gas_price = super::registry::current_gas_price().await
        .map_err(|e| format!("gas price: {e}"))?;

    let tx = crate::tempo_tx::TempoTxBuilder::new(super::registry::CHAIN_ID)
        .max_priority_fee_per_gas(gas_price)
        .max_fee_per_gas(gas_price)
        .gas_limit(gas_limit)
        .nonce(nonce)
        .calls(calls)
        .fee_token(fee_token_addr)
        .sponsored()
        .build();

    let sender_hash = tx.sender_hash();
    let (claimed_addr, sender_sig) =
        super::verify::sign_tempo_tx_via_iframe(&tx, purpose)
            .await
            .map_err(|e| format!("signer: {e}"))?;

    // Defensive: the recovered address must match the expected sender
    // EOA. If it doesn't, the iframe signed with a different wallet
    // (XSS, race with a wallet swap, etc.) and submitting would burn
    // sponsor funds on a tx that doesn't even authorize the call.
    let recovered = crate::wallet::recover_address(&sender_sig, &sender_hash)
        .map_err(|e| format!("recover: {e}"))?;
    if recovered != sender_address {
        return Err(format!(
            "sender sig recovered 0x{} but expected {claimed_addr} ({from_hex})",
            recovered.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        ));
    }

    let fee_payer = super::sponsor::signer()?;
    let fp_hash = tx.fee_payer_hash(&sender_address);
    let fp_sig = crate::wallet::sign_hash(&fee_payer, &fp_hash);
    let raw = tx.serialize_signed(&sender_sig, Some(&fp_sig));
    let raw_hex = bytes_to_hex_str(&raw);
    super::registry::submit_and_wait_receipt(&raw_hex).await
        .map_err(|e| format!("submit: {e}"))
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

    // Inject the stashed agent card (folded in from the retired right rail)
    // into the Account tab's #financial-slot. Built by kick_verification.
    if let Some(card) = super::APP.with(|c| c.borrow().financial_card_html.clone()) {
        if dom::by_id("financial-slot").is_some() {
            dom::swap_outer("financial-slot", &card);
        }
    }

    // Fill the Usage tab's subdomain count + token total. No-ops if the
    // slots aren't there.
    wasm_bindgen_futures::spawn_local(async move {
        refresh_usage_slot().await;
    });

    // Apex shows a credit balance pill. Fire-and-forget the on-chain
    // read so the rest of the dropdown paints immediately; the pill
    // updates from "…" to "N LH" when the call resolves.
    if matches!(super::tenant::current(), super::tenant::Host::Apex) {
        wasm_bindgen_futures::spawn_local(async move {
            refresh_credits_pill().await;
            refresh_signer_list().await;
        });
    }

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
            // Restore the saved custom prompt into the textarea so the
            // user can edit instead of re-typing.
            if let Some(prompt) = super::system_prompt::load().await {
                if let Some(textarea) = dom::textarea_by_id("prompt-input") {
                    textarea.set_value(&prompt);
                }
            }
            if let Some(allowed) = super::tool_allowlist::load().await {
                if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                    if let Ok(checkboxes) = doc.query_selector_all(".tool-checkbox") {
                        for i in 0..checkboxes.length() {
                            if let Some(el) = checkboxes.get(i) {
                                let input: web_sys::HtmlInputElement = JsCast::unchecked_into(el);
                                if let Some(name) = input.get_attribute("data-tool") {
                                    let is_allowed = allowed.iter().any(|t| t.wire_name() == name);
                                    input.set_checked(is_allowed);
                                }
                            }
                        }
                    }
                }
                let summary = super::tool_allowlist::summary(&allowed);
                dom::swap_inner("tool-allowlist-status", &summary);
            } else {
                dom::swap_inner("tool-allowlist-status", "all tools enabled");
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

/// Switch the active admin tab by flipping the `tab-<name>` class on
/// `#admin-dialog` (CSS shows the matching `.panel-<name>`), and sync the
/// `.active` state on the tab buttons. Mirrors `show_mobile_tab`.
fn show_admin_tab(name: &str) {
    let Some(dialog) = dom::by_id("admin-dialog") else { return };
    let mut cls: Vec<String> = dialog
        .class_name()
        .split_whitespace()
        .filter(|c| !c.starts_with("tab-"))
        .map(String::from)
        .collect();
    cls.push(format!("tab-{name}"));
    dialog.set_class_name(&cls.join(" "));

    for tab in ["agent", "account", "usage"] {
        let Some(el) = dom::by_id(&format!("admin-tab-btn-{tab}")) else { continue };
        let c = el.class_name();
        let mut classes: Vec<&str> = c.split_whitespace().filter(|x| *x != "active").collect();
        if tab == name {
            classes.push("active");
        }
        el.set_class_name(&classes.join(" "));
    }
}

/// Fill the admin Usage tab's `#usage-subdomains` slot with the on-chain
/// registered-subdomain count. Soft-fail (leaves a dash). No-op if the
/// slot isn't present.
pub(crate) async fn refresh_usage_slot() {
    // Tokens — synchronous read from App state (updated after each turn).
    if dom::by_id("usage-tokens").is_some() {
        let total = super::APP.with(|c| c.borrow().total_tokens);
        dom::swap_inner("usage-tokens", &format!("{total}"));
    }
    if dom::by_id("usage-subdomains").is_none() {
        return;
    }
    match super::registry::subdomain_count().await {
        Ok(n) => dom::swap_inner("usage-subdomains", &format!("{n}")),
        Err(_) => dom::swap_inner("usage-subdomains", "—"),
    }
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

    // The display tab shows the framebuffer; mount an idle surface if
    // nothing is already on it so the canvas exists when the tab opens.
    if name == "display" && dom::by_id("display-canvas").is_none() {
        dom::swap_inner(
            "view-content",
            &super::templates::display_surface().into_string(),
        );
    }

    // Reflect active state on each tab button by id — small fixed
    // set of tabs, no need for query_selector_all (which needs the
    // NodeList web-sys feature we don't enable).
    for tab in ["files", "chat", "display", "agent"] {
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
    // Feedback is write-only: we submit on-chain but don't show a list of
    // everyone's submissions (it's a public, ever-growing log — surfacing
    // the whole thing to every visitor isn't wanted). Harvest off-chain
    // via scripts/harvest-feedback when triaging.
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
        return; // silent no-op per [[feedback-no-explanatory-validation]]
    }
    if text.len() > 2048 {
        dom::swap_inner(
            "feedback-msg",
            &format!(
                "<span style=\"color:var(--error)\">feedback too long: {} bytes (max 2048) — please shorten</span>",
                text.len()
            ),
        );
        return;
    }

    // Client-side rate limit: one submission per 60 seconds.
    thread_local! {
        static LAST_FEEDBACK_MS: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
    }
    let now = js_sys::Date::now();
    let elapsed = LAST_FEEDBACK_MS.with(|c| now - c.get());
    if elapsed < 60_000.0 {
        let remaining = ((60_000.0 - elapsed) / 1000.0).ceil() as u32;
        dom::swap_inner(
            "feedback-msg",
            &dom::msg_span(dom::Msg::Muted, &format!("wait {remaining}s")),
        );
        return;
    }
    LAST_FEEDBACK_MS.with(|c| c.set(now));

    // Need an apex wallet to sign. The visitor address from verify
    // state is what the iframe signer controls.
    let from_hex = super::APP.with(|cell| {
        use super::VerifyState;
        match &cell.borrow().verify_state {
            VerifyState::Verified { address } => Some(address.clone()),
            VerifyState::Visitor { visitor_address, .. } => Some(visitor_address.clone()),
            _ => cell.borrow().wallet.as_ref().map(|w| w.address_hex()),
        }
    });
    let Some(from_hex) = from_hex else {
        dom::swap_inner(
            "feedback-msg",
            "<span style=\"color:var(--error)\">claim an identity first</span>",
        );
        return;
    };

    dom::swap_inner(
        "feedback-msg",
        "<span style=\"color:var(--muted)\">signing…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        // Mirror to local OPFS first so the user always has a copy
        // even if the on-chain leg fails. Best-effort — log and
        // continue on error.
        if let Err(err) = append_feedback_local(&text).await {
            web_sys::console::warn_1(&JsValue::from_str(&format!(
                "feedback local copy: {err}"
            )));
        }
        match submit_feedback_onchain(&from_hex, &text).await {
            Ok(tx_hash) => {
                let short = tx_short_hash(&tx_hash);
                dom::swap_inner(
                    "feedback-msg",
                    &dom::msg_span(dom::Msg::Accent, &format!("✓ on-chain (tx {short})")),
                );
                if let Some(window) = web_sys::window() {
                    let cb = Closure::<dyn FnMut()>::new(|| {
                        feedback_close();
                    });
                    let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                        cb.as_ref().unchecked_ref(),
                        1200,
                    );
                    cb.forget();
                }
            }
            Err(err) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!(
                    "feedback on-chain: {err}"
                )));
                dom::swap_inner(
                    "feedback-msg",
                    "<span style=\"color:var(--error)\">on-chain submit failed (saved locally)</span>",
                );
            }
        }
    });
}

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
async fn run_sync_key() {
    let msg = "key-sync-msg";
    let set_err = |m: &str| dom::swap_inner(msg, &dom::msg_span(dom::Msg::Error, &format!("{m}")));

    let name = match super::tenant::current() {
        super::tenant::Host::Tenant(n) => n,
        _ => {
            set_err("only on a subdomain");
            return;
        }
    };
    let owner_hex = super::APP.with(|cell| {
        use super::VerifyState;
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
    let ct_hex = match super::verify::seal_key_via_iframe(&key).await {
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
    let registry_addr = match parse_address(super::registry::REGISTRY_ADDRESS) {
        Ok(a) => a,
        Err(e) => {
            set_err(&e);
            return;
        }
    };
    let call = crate::tempo_tx::TempoCall {
        to: registry_addr,
        value_wei: 0,
        input: super::registry::encode_set_gemini_key(id, &ct),
    };
    let words = (ct.len() / 32 + 1) as u128;
    let gas = 1_200_000 + words * 40_000;
    dom::swap_inner(msg, "<span style=\"color:var(--muted)\">syncing on-chain…</span>");
    match run_sponsored_tempo_call(&owner_hex, vec![call], gas, "sync key").await {
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
async fn run_restore_key() {
    let msg = "key-sync-msg";
    let set_err = |m: &str| dom::swap_inner(msg, &dom::msg_span(dom::Msg::Error, &format!("{m}")));

    let name = match super::tenant::current() {
        super::tenant::Host::Tenant(n) => n,
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
    let ct = match super::registry::gemini_key_of(id).await {
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
    let plaintext = match super::verify::open_key_via_iframe(&ct_hex).await {
        Ok(p) => p,
        Err(e) => {
            set_err(&format!("open: {e} — import your seed on this device first"));
            return;
        }
    };
    if let Some(input) = dom::input_by_id("key") {
        input.set_value(&plaintext);
    }
    super::key_store::save(&plaintext).await;
    refresh_keymeta();
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
async fn gemini_key_slot_id(name: &str) -> Result<u64, String> {
    let owner = super::registry::owner_of_name(name)
        .await
        .map_err(|e| format!("owner: {e}"))?
        .ok_or_else(|| "name not registered on-chain".to_string())?;
    let main_id = super::registry::main_of(&owner).await.unwrap_or(0);
    if main_id != 0 {
        return Ok(main_id);
    }
    match super::registry::id_of_name(name).await {
        Ok(id) if id != 0 => Ok(id),
        _ => Err("no token id for name".into()),
    }
}

/// Best-effort: seal the Gemini key with the seed-derived key (via the
/// apex iframe) and store it on-chain under the owner's MAIN slot, so any
/// other subdomain / seed-bearing device auto-restores it. No-op on any
/// failure — most importantly when the seed isn't on this device (the
/// iframe seal fails), which is fine: nothing to sync from here.
async fn auto_sync_gemini_key(name: String, key: String) {
    let owner = match super::registry::owner_of_name(&name).await {
        Ok(Some(o)) => o,
        _ => return,
    };
    let slot_id = match gemini_key_slot_id(&name).await {
        Ok(id) => id,
        Err(_) => return,
    };
    // Seal with the seed-derived key via the apex iframe. Fails (and we
    // bail) on a device that doesn't hold the seed.
    let ct_hex = match super::verify::seal_key_via_iframe(&key).await {
        Ok(h) => h,
        Err(_) => return,
    };
    let Some(ct) = decode_hex_local(&ct_hex) else { return };
    let Ok(registry_addr) = parse_address(super::registry::REGISTRY_ADDRESS) else { return };
    let call = crate::tempo_tx::TempoCall {
        to: registry_addr,
        value_wei: 0,
        input: super::registry::encode_set_gemini_key(slot_id, &ct),
    };
    let words = (ct.len() / 32 + 1) as u128;
    let gas = 1_200_000 + words * 40_000;
    let _ = run_sponsored_tempo_call(&owner, vec![call], gas, "auto-sync key").await;
}

/// Try to pull the owner's MAIN Gemini key from chain and decrypt it
/// with this device's seed (via the apex iframe). On success the key is
/// saved to this origin's OPFS + sessionStorage and `true` is returned,
/// so the caller can skip the api-key modal. Returns `false` (silently)
/// when there's no synced key OR this device lacks the seed (e.g. a phone
/// linked by device key only — that path will use the wrapped-key blob).
pub(crate) async fn try_auto_restore_gemini_key(name: &str) -> bool {
    if super::key_store::load().await.is_some() {
        return true;
    }
    let slot_id = match gemini_key_slot_id(name).await {
        Ok(id) => id,
        Err(_) => return false,
    };
    let ct = match super::registry::gemini_key_of(slot_id).await {
        Ok(Some(b)) => b,
        _ => return false,
    };
    let ct_hex = format!(
        "0x{}",
        ct.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let plaintext = match super::verify::open_key_via_iframe(&ct_hex).await {
        Ok(p) => p,
        Err(_) => return false,
    };
    super::key_store::save(&plaintext).await;
    if let Ok(Some(storage)) = dom::session_storage() {
        let _ = storage.set_item("gemini_api_key", &plaintext);
    }
    true
}

/// Publish the device's local `app.rl` as the subdomain's on-chain app:
/// compile it to wasm and store the bytes under the app metadata key via
/// a sponsored `setMetadata` call (owner-signed through the apex iframe).
/// Once published, ANY visitor to the subdomain boots into the app.
async fn run_publish_app() {
    let msg = "publish-app-msg";
    let set_err = |m: &str| {
        dom::swap_inner(msg, &dom::msg_span(dom::Msg::Error, &format!("{m}")));
    };

    let name = match super::tenant::current() {
        super::tenant::Host::Tenant(n) => n,
        _ => {
            set_err("only on a subdomain");
            return;
        }
    };

    // Only the verified owner can write metadata on-chain.
    let owner_hex = super::APP.with(|cell| {
        use super::VerifyState;
        match &cell.borrow().verify_state {
            VerifyState::Verified { address } => Some(address.clone()),
            _ => None,
        }
    });
    let Some(owner_hex) = owner_hex else {
        set_err("verify as owner first");
        return;
    };

    let fs = super::shared_opfs();
    let src = match fs.read("app.rl").await {
        Ok(b) if !b.is_empty() => String::from_utf8_lossy(&b).into_owned(),
        _ => {
            set_err("no app.rl to publish");
            return;
        }
    };
    let wasm = match crate::rustlite::compile(&src) {
        Ok(w) => w,
        Err(e) => {
            set_err(&format!("compile: {e}"));
            return;
        }
    };
    if wasm.len() > 16_384 {
        set_err("app wasm too large to publish (max 16 KB)");
        return;
    }

    let id = match super::registry::id_of_name(&name).await {
        Ok(id) if id != 0 => id,
        _ => {
            set_err("name isn't registered on-chain");
            return;
        }
    };

    dom::swap_inner(msg, "<span style=\"color:var(--muted)\">publishing…</span>");
    let registry_addr = match parse_address(super::registry::REGISTRY_ADDRESS) {
        Ok(a) => a,
        Err(e) => {
            set_err(&e);
            return;
        }
    };
    let call = crate::tempo_tx::TempoCall {
        to: registry_addr,
        value_wei: 0,
        input: super::registry::encode_set_app_wasm(id, &wasm),
    };
    // Storing `bytes` costs ~20k gas per 32-byte word (cold SSTORE) on
    // top of the ~275k Tempo sponsorship + base call.
    let words = (wasm.len() / 32 + 1) as u128;
    let gas = 1_200_000 + words * 40_000;
    match run_sponsored_tempo_call(&owner_hex, vec![call], gas, "publish app").await {
        Ok(_tx) => dom::swap_inner(
            msg,
            &maud::html! {
                span style="color:var(--fg)" {
                    "published ✓ — live at "
                    a href=(format!("https://{name}.localharness.xyz/"))
                      target="_blank" rel="noopener" style="color:var(--accent)" {
                        (format!("{name}.localharness.xyz →"))
                    }
                    " (share it — anyone can open the app)"
                }
            }
            .into_string(),
        ),
        Err(e) => set_err(&format!("publish failed: {e}")),
    }
}

/// Sign + submit `FeedbackFacet.submitFeedback(text)` on the diamond
/// via the apex iframe signer. The event log on the registry is the
/// canonical store; the developer harvests via `eth_getLogs`. Caller's
/// gas paid by the apex wallet — Tempo allows contract calls.
pub(crate) async fn submit_feedback_onchain(from_hex: &str, text: &str) -> Result<String, String> {
    let calldata = encode_submit_feedback_calldata(text);
    let registry_addr = parse_address(super::registry::REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: registry_addr,
        value_wei: 0,
        input: calldata,
    };
    // String-emitting events scale with byte length (~12k base +
    // ~200 gas/byte log data). 2048-byte upper bound + base ~150k
    // for the inner call. Tempo sponsorship overhead adds ~275k.
    // 800k is generous headroom for any reasonable feedback length.
    run_sponsored_tempo_call(from_hex, vec![call], 800_000, "submit feedback").await
}

/// ABI-encode `submitFeedback(string)`. Layout: selector + offset(0x20)
/// + length + bytes (right-padded to 32-byte multiple).
fn encode_submit_feedback_calldata(text: &str) -> Vec<u8> {
    use sha3::{Digest, Keccak256};
    let mut hasher = Keccak256::new();
    hasher.update(b"submitFeedback(string)");
    let digest = hasher.finalize();
    let mut selector = [0u8; 4];
    selector.copy_from_slice(&digest[..4]);

    let bytes = text.as_bytes();
    let len = bytes.len();
    let padded_len = len.div_ceil(32) * 32;

    let mut out = Vec::with_capacity(4 + 32 + 32 + padded_len);
    out.extend_from_slice(&selector);
    // offset to dynamic head — 0x20 (one dynamic arg after a 32-byte slot)
    let mut offset = [0u8; 32];
    offset[31] = 0x20;
    out.extend_from_slice(&offset);
    // length
    let mut len_bytes = [0u8; 32];
    len_bytes[24..].copy_from_slice(&(len as u64).to_be_bytes());
    out.extend_from_slice(&len_bytes);
    // payload + zero-pad
    out.extend_from_slice(bytes);
    out.resize(4 + 32 + 32 + padded_len, 0);
    out
}

/// Append a feedback entry to `.lh_feedback.txt` in this origin's OPFS
/// as a local-first mirror. The canonical store is the on-chain event
/// log; this file is a per-device safety net for when the on-chain leg
/// is unreachable (offline, rate-limited, etc.). One line per entry:
/// `ISO-timestamp\tTEXT\n`.
async fn append_feedback_local(text: &str) -> Result<(), String> {
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
    let new_cls = if parts.contains(&class) {
        parts.iter().filter(|c| **c != class).copied().collect::<Vec<_>>().join(" ")
    } else if parts.is_empty() {
        class.to_string()
    } else {
        format!("{} {class}", parts.join(" "))
    };
    layout.set_class_name(&new_cls);
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
                &dom::msg_span(dom::Msg::Error, &format!("{err}")),
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
                    &dom::msg_span(dom::Msg::Error, &format!("save failed: {err}")),
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
