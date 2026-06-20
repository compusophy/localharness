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
use web_sys::{Document, Element, HtmlElement, KeyboardEvent, MouseEvent};

use crate::encoding::{bytes_to_hex_str, parse_address};

use super::dom;
use super::templates;

mod admin;
mod claim;
mod credits;
mod devices;
mod key_sync;
mod layout;
mod public_face;
// `pub(crate)` so the schedule_task chat tool can reuse submit_schedule_job /
// parse_schedule_interval (the same escrow core the chat tool uses).
pub(crate) mod schedule;
mod subdomains;

pub(crate) use credits::{finalize_after_payment, refresh_fund_banner, try_redeem_pending_invite};

thread_local! {
    /// One identity/onboarding flow at a time. Mashing a (perceived-stuck)
    /// button spawned PARALLEL identity creations — concurrent OPFS writes
    /// to the same key files plus a pile of racing timers, implicated in the
    /// iOS executor `RefCell already borrowed` panic. Guarded flows: the
    /// onboarding redeem, create-identity, import-seed.
    static ONBOARD_BUSY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };

    /// The name the visitor chose on the consolidated front door, stashed across
    /// the async $2 checkout (the inline card replaces the input, so the value
    /// can't be re-read from the DOM). `credits::persist_seed_and_claim` takes it
    /// after payment to claim the name + redirect into the new agent's chat.
    static ONBOARD_NAME: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// Stash the front-door name for the post-payment claim. See [`ONBOARD_NAME`].
pub(super) fn set_onboard_name(name: &str) {
    ONBOARD_NAME.with(|c| *c.borrow_mut() = Some(name.to_string()));
}

/// Take (and clear) the stashed front-door name. `None` = the buyer reached
/// checkout without a name (e.g. the admin top-up path), so claim is skipped.
pub(super) fn take_onboard_name() -> Option<String> {
    ONBOARD_NAME.with(|c| c.borrow_mut().take())
}

/// Begin an exclusive onboarding flow. `None` = one is already running
/// (caller silently ignores the press). Hold the guard for the WHOLE async
/// flow — dropping it (any exit path) releases the lock.
pub(super) fn onboard_flow_begin() -> Option<OnboardFlowGuard> {
    ONBOARD_BUSY.with(|b| {
        if b.get() {
            None
        } else {
            b.set(true);
            Some(OnboardFlowGuard)
        }
    })
}

pub(super) struct OnboardFlowGuard;

impl Drop for OnboardFlowGuard {
    fn drop(&mut self) {
        ONBOARD_BUSY.with(|b| b.set(false));
    }
}

/// Spawn `repaint` on a FRESH executor tick while keeping the onboarding
/// flow guard held until it finishes.
///
/// The post-onboarding repaint (`paint_apex` / `paint_tenant`) itself
/// `spawn_local`s sub-tasks and awaits OPFS + on-chain reads. Awaiting it
/// inline from the onboarding task deepens the wasm-bindgen single-thread
/// executor's poll chain; on iOS WebKit's microtask timing this can re-enter
/// `Task::run` for a task whose `RefCell` is still borrowed by the in-progress
/// poll → the `already mutably borrowed: BorrowError` panic that crashed the
/// "create wallet" front door. Returning from the onboarding task BEFORE the
/// repaint releases that executor borrow, so the repaint runs on a clean tick.
/// The guard moves into the new task so a re-press stays blocked until the
/// surface is actually up.
pub(super) fn defer_onboard_repaint<F>(guard: OnboardFlowGuard, repaint: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(async move {
        let _flow_guard = guard;
        repaint.await;
    });
}

/// Probe storage durability and, if volatile (incognito/private window), paint
/// the non-blocking "back up your seed" warning into `#storage-warn-slot`.
///
/// MUST be awaited SEQUENTIALLY — after the seed write, never concurrently with
/// it. `storage_is_volatile()` round-trips `navigator.storage`; racing it
/// against `create_and_persist`'s first OPFS write interleaved two futures on
/// the single-thread wasm executor and tripped the iOS "RefCell already
/// borrowed" panic at the seed write (the create-wallet front-door crash).
pub(super) async fn warn_if_storage_volatile() {
    if super::wallet_store::storage_is_volatile().await {
        dom::swap_inner(
            "storage-warn-slot",
            &templates::volatile_storage_warning().into_string(),
        );
    }
}
pub(crate) use key_sync::{sync_local_key_to_main, try_auto_restore_gemini_key};
pub(crate) use subdomains::{run_batch_create_subdomains, run_bulk_release, run_release_subdomain};

/// Every user interaction maps to one of these. The closed enum makes
/// it obvious from one file what the app actually does. Variants with
/// payloads pull the value from the element's `data-arg` attribute.
#[derive(Debug, Clone)]
enum Action {
    Send,
    SyncDevices,
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
    ClaimOnChain,
    RevealSeed,
    HideSeed,
    ImportSeed,
    CreateIdentity,
    /// Consolidated front door: the visitor picked a name + hit CREATE. Stash
    /// the name, create the identity, run the $2 checkout, then (once paid) claim
    /// the name and redirect into the new agent's chat — all one step.
    OnboardCreate,
    ShowImport,
    CancelImport,
    HeaderAdminToggle,
    HeaderAdminClose,
    /// Flip the light theme (`html.theme-light`) — live + persisted.
    ToggleTheme,
    /// Flip the mobile-preview frame (`html.preview-mobile`) — live + persisted.
    TogglePreview,
    ShowAdminTab(String),
    RevealSecurity,
    HideSecurity,
    ResetArm,
    ResetConfirm,
    ResetCancel,
    PricingSave,
    /// Open/close the OPFS file-browser modal (header [files] button +
    /// the modal's own ×).
    ToggleFiles,
    /// Open/close the header feedback-bug dropdown (#36) — the on-chain feedback
    /// widget, anchored under the bug button (between the bell and the cog).
    ToggleFeedback,
    FeedbackSubmit,
    /// Dismiss the QR seed-adoption panel back to the "add a device" button.
    PairCancel,
    /// Option A: desktop shows a QR carrying its seed (encrypted under a
    /// one-time code) so another device can adopt the same identity.
    AddDevice,
    /// Option A: phone submits the one-time code to decrypt + import the
    /// seed that rode in the URL fragment.
    AdoptDevice,
    /// Trap fix: explicit "create a genuinely new identity" confirmation
    /// from the no-wallet identity-choice interstitial, carrying the name
    /// the user was trying to claim.
    CreateNewClaim(String),
    SavePrompt,
    SaveToolAllowlist,
    ResetToolAllowlist,
    SaveApiKey,
    ToggleDisplay,
    /// Open/close the CLI-sandbox terminal overlay (feedback #6): the terminal
    /// card's [show] re-opens the last `run_wasm_cli` run; the overlay `×`
    /// closes it. Mirrors `ToggleDisplay`.
    ToggleTerminal,
    StopTurn,
    /// Broadcast-composer [send] — a cartridge's `broadcast_compose` opened
    /// a text input over the canvas; the payload is the notification TITLE
    /// (the typed body is read from `#broadcast-input` at dispatch).
    BroadcastSend(String),
    /// Broadcast-composer [cancel] / Escape — dismiss without sending.
    BroadcastCancel,
    /// Set this subdomain's public face: "directory", "app", or "html".
    /// "app"/"html" also publish the device's local app.rl/index.html.
    SetPublicFace(String),
    /// Copy the published share URL (the `data-arg`) to the clipboard —
    /// the [copy] button in the post-publish share fragment.
    CopyShareUrl(String),
    /// Copy the revealed seed phrase (the `data-arg`) to the clipboard —
    /// the [copy] button in the seed-reveal view. One tap banks the words
    /// before a mobile app-switch can refresh the tab away.
    CopySeed(String),
    /// Choose how the agent reaches the model: "credits" or "byok".
    SetModelAccess(String),
    /// Choose which LLM the in-tab agent uses (a `gemini-*` or `claude-*`
    /// model id). Persisted to `.lh_model`; read by `chat::start_session`.
    SetModel(String),
    /// Download the in-browser local model (Gemma 3 270M weights + tokenizer)
    /// from the HF CDN into OPFS — the one-time opt-in for on-device inference.
    DownloadLocalModel,
    /// First-time onboarding: redeem an invite code typed into the
    /// fresh-visitor `invite_onboarding` surface. Ensures a credit identity
    /// exists (the user's explicit redeem action — NOT silent generation),
    /// accepts the invite escrow, then re-paints the now-funded apex so the
    /// claim-a-name surface appears.
    RedeemInviteOnboard,
    /// Redeem a one-time code for `$LH` credits.
    RedeemCode,
    /// Buy `$LH` with a card via Stripe Checkout (proxy on-ramp).
    BuyLh,
    /// Cancel the inline buy-$LH checkout, restoring the buy control.
    CancelBuy,
    /// Redeem a one-time code from the inline no-funds banner above the prompt.
    RedeemBanner,
    /// Escrow the owner's `$LH` behind a fresh bearer code + surface the
    /// `?invite=` share link (InviteFacet `createInvite`).
    CreateInvite,
    /// Save this agent's per-call x402 price (`.lh_x402_price`).
    SaveX402Price,
    /// Unlink a device (remove its signer + index entry) — the X opens a
    /// typed confirmation; UnlinkConfirm performs it; UnlinkCancel aborts.
    UnlinkDevice(String),
    UnlinkConfirm(String),
    UnlinkCancel,
    /// Ask Notification permission (this click is the required user gesture),
    /// subscribe Web Push, and publish the subscription on-chain so the
    /// scheduler worker can notify the owner with the tab closed.
    EnableNotifications,
    /// Header notification bell: enable Web Push for THIS device (address-keyed,
    /// direct gesture) and open the in-app panel. The path a visitor uses to let
    /// their phone be pinged — the cartridge tap can't prompt for permission.
    NotifBell,
    /// [clear all] in the bell dropdown: show the inline yes/cancel confirm.
    NotifClearAll,
    /// [yes] in the clear confirm: empty the in-app notification inbox.
    NotifClearConfirm,
    /// [cancel] in the clear confirm: dismiss it, keep the notifications.
    NotifClearCancel,
    /// Fire a local test notification (+vibration) so the user can verify the
    /// permission + service-worker path without scheduling anything.
    TestNotification,
    /// Trigger the browser's PWA install prompt from inside the app (the
    /// stashed `beforeinstallprompt` in boot.js) instead of the browser menu.
    InstallApp,
}

impl Action {
    fn parse(name: &str, arg: Option<String>) -> Option<Action> {
        Some(match name {
            "send" => Action::Send,
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
            "claim-on-chain" => Action::ClaimOnChain,
            "reveal-seed" => Action::RevealSeed,
            "hide-seed" => Action::HideSeed,
            "import-seed" => Action::ImportSeed,
            "create-identity" => Action::CreateIdentity,
            "onboard-create" => Action::OnboardCreate,
            "show-import" => Action::ShowImport,
            "cancel-import" => Action::CancelImport,
            "header-admin-toggle" => Action::HeaderAdminToggle,
            "header-admin-close" => Action::HeaderAdminClose,
            "toggle-theme" => Action::ToggleTheme,
            "toggle-preview" => Action::TogglePreview,
            "show-admin-tab" => Action::ShowAdminTab(arg.unwrap_or_default()),
            "reveal-security" => Action::RevealSecurity,
            "hide-security" => Action::HideSecurity,
            "reset-arm" => Action::ResetArm,
            "reset-confirm" => Action::ResetConfirm,
            "reset-cancel" => Action::ResetCancel,
            "pricing-save" => Action::PricingSave,
            "toggle-files" => Action::ToggleFiles,
            "toggle-feedback" => Action::ToggleFeedback,
            "feedback-submit" => Action::FeedbackSubmit,
            "add-device" => Action::AddDevice,
            "sync-devices" => Action::SyncDevices,
            "adopt-device" => Action::AdoptDevice,
            "create-new-claim" => Action::CreateNewClaim(arg.unwrap_or_default()),
            "pair-cancel" => Action::PairCancel,
            "save-prompt" => Action::SavePrompt,
            "save-tool-allowlist" => Action::SaveToolAllowlist,
            "reset-tool-allowlist" => Action::ResetToolAllowlist,
            "save-api-key" => Action::SaveApiKey,
            "toggle-display" => Action::ToggleDisplay,
            "toggle-terminal" => Action::ToggleTerminal,
            "stop-turn" => Action::StopTurn,
            "broadcast-send" => Action::BroadcastSend(arg.unwrap_or_default()),
            "broadcast-cancel" => Action::BroadcastCancel,
            "set-public-face" => Action::SetPublicFace(arg.unwrap_or_default()),
            "copy-share-url" => Action::CopyShareUrl(arg.unwrap_or_default()),
            "copy-seed" => Action::CopySeed(arg.unwrap_or_default()),
            "set-model-access" => Action::SetModelAccess(arg.unwrap_or_default()),
            "set-model" => Action::SetModel(arg.unwrap_or_default()),
            "download-local-model" => Action::DownloadLocalModel,
            "redeem-invite-onboard" => Action::RedeemInviteOnboard,
            "redeem-code" => Action::RedeemCode,
            "buy-lh" => Action::BuyLh,
            "cancel-buy" => Action::CancelBuy,
            "redeem-banner" => Action::RedeemBanner,
            "create-invite" => Action::CreateInvite,
            "save-x402-price" => Action::SaveX402Price,
            "unlink-device" => Action::UnlinkDevice(arg.unwrap_or_default()),
            "unlink-confirm" => Action::UnlinkConfirm(arg.unwrap_or_default()),
            "unlink-cancel" => Action::UnlinkCancel,
            "enable-notifications" => Action::EnableNotifications,
            "notif-bell" => Action::NotifBell,
            "notif-clear-all" => Action::NotifClearAll,
            "notif-clear-confirm" => Action::NotifClearConfirm,
            "notif-clear-cancel" => Action::NotifClearCancel,
            "test-notification" => Action::TestNotification,
            "install-app" => Action::InstallApp,
            _ => return None,
        })
    }
}

pub(crate) fn install_delegated_listeners(doc: &Document) -> Result<(), JsValue> {
    let click = Closure::<dyn FnMut(_)>::new(move |event: MouseEvent| {
        let Some(target) = event.target() else { return };
        let Ok(mut node) = target.dyn_into::<Element>() else { return };

        // Standard dropdown dismissal: while the notification-bell panel is
        // open, ANY click outside the bell/panel closes it (the click still
        // dispatches normally below — e.g. opening ADMIN also shuts the bell).
        if admin::notif_panel_open() && node.closest(".notif-bell-wrap").ok().flatten().is_none() {
            admin::close_notif_panel();
        }

        // Same dismissal for the header feedback-bug dropdown (#36): any click
        // outside its wrap closes it (a click on the bug/panel IS inside the
        // wrap, so toggling + the textarea still work untouched).
        if admin::feedback_panel_open() && node.closest(".feedback-bug-wrap").ok().flatten().is_none() {
            admin::close_feedback_panel();
        }

        // Same treatment for the top-left `localharness` brand menu: a native
        // <details> won't dismiss on its own, so any click outside .brand-menu
        // closes it. A click on the summary/items IS inside .brand-menu, so the
        // native open-toggle / link-navigation still runs untouched.
        if admin::brand_menu_open() && node.closest(".brand-menu").ok().flatten().is_none() {
            admin::close_brand_menu();
        }

        // The admin (cog) panel is now a DROPDOWN (#36) anchored under the cog,
        // inside `.header-admin` — not a fullscreen backdrop. So dismiss it the
        // same way as the other header dropdowns: any click outside `.header-admin`
        // closes it. A click on the cog/panel IS inside `.header-admin`, so the
        // toggle + the panel's own controls still run untouched.
        if dom::by_id("header-admin-panel")
            .map(|e| !e.has_attribute("hidden"))
            .unwrap_or(false)
            && node.closest(".header-admin").ok().flatten().is_none()
        {
            dispatch(Action::HeaderAdminClose);
        }

        // Backdrop-click dismissal: a click whose RAW target IS the overlay
        // backdrop itself (the dark area, never a child inside the dialog —
        // those bubble up with a different target) closes the modal. Files modal
        // only; the display overlay is a fullscreen interactive surface (its × /
        // ESC close it), and admin is now a dropdown (handled above).
        match node.id().as_str() {
            "files-modal" => {
                event.prevent_default();
                dispatch(Action::ToggleFiles);
                return;
            }
            _ => {}
        }

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
            "apex-input" => claim::on_apex_input(),
            // Auto-grow the prompt textarea: reset to its single-row height,
            // then size to the content (CSS caps it at max-height + scrolls).
            "prompt" => autogrow_textarea(&el),
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
        let key = event.key();
        // ESC dismisses the topmost open overlay (display > files > admin) —
        // the universal "close this modal" gesture. Previously NO key closed
        // any overlay, so a keyboard user (or anyone) had to find the × to
        // escape. Reuses the wired close/toggle actions.
        if key == "Escape" {
            // An armed value-moving confirmation (the transaction "modal") is
            // the lightest, topmost layer — ESC dismisses IT first (a11y #75).
            // Its root carries `data-modal-cancel="<action>"`; route that so the
            // cancel handler also restores focus to the trigger.
            if let Some(id) = dom::open_modal_trap() {
                if let Some(cancel) = dom::by_id(&id)
                    .and_then(|el| el.get_attribute("data-modal-cancel"))
                    .and_then(|name| Action::parse(&name, None))
                {
                    event.prevent_default();
                    dispatch(cancel);
                    return;
                }
            }
            // The bell dropdown is the lightest layer — ESC takes it next.
            if admin::notif_panel_open() {
                event.prevent_default();
                admin::close_notif_panel();
            } else if admin::feedback_panel_open() {
                // The feedback-bug dropdown sits in the same chrome layer as the
                // bell; ESC dismisses it like the other dropdowns.
                event.prevent_default();
                admin::close_feedback_panel();
            } else if admin::brand_menu_open() {
                // The brand menu sits in the same chrome layer as the bell;
                // ESC dismisses it like the other dropdowns.
                event.prevent_default();
                admin::close_brand_menu();
            } else if super::display::broadcast_composer_open() {
                // The broadcast composer floats over the cartridge canvas —
                // dismiss IT, not the whole display surface beneath it.
                event.prevent_default();
                super::display::close_broadcast_composer();
            } else if dom::by_id("display-canvas").is_some() {
                event.prevent_default();
                dispatch(Action::ToggleDisplay);
            } else if dom::by_id("terminal-surface").is_some() {
                event.prevent_default();
                dispatch(Action::ToggleTerminal);
            } else if dom::by_id("fs-list").is_some() {
                event.prevent_default();
                dispatch(Action::ToggleFiles);
            } else if dom::by_id("header-admin-panel")
                .map(|e| !e.has_attribute("hidden"))
                .unwrap_or(false)
            {
                event.prevent_default();
                dispatch(Action::HeaderAdminClose);
            }
            return;
        }
        // Trap Tab / Shift+Tab inside an armed value-moving confirmation (a11y
        // #75). While `[data-modal-trap]` is in the DOM, Tab must cycle WITHIN
        // it — off the last element it wraps to the first (and a Tab from the
        // trigger still behind it is pulled in), so keyboard users don't have to
        // tab through the whole page to reach [confirm]. No new listener: this
        // rides the one delegated keydown, the way the rest of the app does.
        if key == "Tab" {
            if let Some(id) = dom::open_modal_trap() {
                if dom::trap_tab_in(&id, event.shift_key()) {
                    event.prevent_default();
                }
            }
            return;
        }
        if key != "Enter" && key != " " {
            return;
        }
        let Some(target) = event.target() else { return };
        let Ok(el) = target.dyn_into::<Element>() else { return };

        // a11y: Enter/Space activates a focused role="button" carrying a
        // data-action — the non-<button> clickables (OPFS file/dir rows,
        // breadcrumbs); real <button>s activate natively. Walk up for
        // data-action exactly like the click handler, then dispatch.
        if el.get_attribute("role").as_deref() == Some("button") {
            let mut node = el.clone();
            let action = loop {
                if let Some(name) = node.get_attribute("data-action") {
                    break Action::parse(&name, node.get_attribute("data-arg"));
                }
                match node.parent_element() {
                    Some(parent) => node = parent,
                    None => break None,
                }
            };
            if let Some(action) = action {
                event.prevent_default();
                dispatch(action);
                return;
            }
        }

        // Enter inside the broadcast composer's input sends — route through
        // the send BUTTON's click so the title rides its data-arg unchanged.
        if key == "Enter" && el.id() == "broadcast-input" {
            event.prevent_default();
            if let Some(btn) = dom::by_id("broadcast-send-btn")
                .and_then(|b| b.dyn_into::<HtmlElement>().ok())
            {
                btn.click();
            }
            return;
        }

        // Enter inside the prompt textarea sends; Shift+Enter inserts a
        // newline (default); Cmd/Ctrl+Enter still sends.
        if key != "Enter" || el.id() != "prompt" {
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
        if super::display::cartridge_canvas_present() {
            super::display::set_pointer(event.client_x() as f64, event.client_y() as f64);
        }
    });
    doc.add_event_listener_with_callback("mousemove", mousemove.as_ref().unchecked_ref())?;
    mousemove.forget();

    // Primary-button state for the display. Press counts only when it
    // starts on a cartridge canvas (the fullscreen overlay OR an inline
    // `embed_app` card); release clears regardless of where it lands.
    let mousedown = Closure::<dyn FnMut(_)>::new(move |event: MouseEvent| {
        if let Some(target) = event.target() {
            if let Ok(el) = target.dyn_into::<Element>() {
                if super::display::is_cartridge_canvas_id(&el.id()) {
                    super::display::set_pointer(event.client_x() as f64, event.client_y() as f64);
                    super::display::set_pointer_down(true);
                    // This tap HAS user activation — prime notification permission
                    // for a feed cartridge so its subscribe() can register the device.
                    super::display::prime_feed_permission_on_gesture();
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
                if super::display::is_cartridge_canvas_id(&el.id()) {
                    if let Some(t) = event.touches().get(0) {
                        super::display::set_pointer(t.client_x() as f64, t.client_y() as f64);
                        super::display::set_pointer_down(true);
                        // The tap that drives the cartridge's SUB also primes
                        // notification permission while the gesture is live.
                        super::display::prime_feed_permission_on_gesture();
                    }
                }
            }
        }
    });
    doc.add_event_listener_with_callback("touchstart", touchstart.as_ref().unchecked_ref())?;
    touchstart.forget();

    let touchmove = Closure::<dyn FnMut(_)>::new(move |event: web_sys::TouchEvent| {
        if super::display::cartridge_canvas_present() {
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

    install_keyboard_viewport_fix();


    Ok(())
}

/// Mobile soft-keyboard fix (FB#9). When the on-screen keyboard opens,
/// `dvh`/`vh` do NOT shrink (they track browser chrome, not the IME), so
/// the full-height `#root` grows taller than the visible area and the
/// browser scrolls the sticky header off the top. We listen on
/// `window.visualViewport` (the only viewport that tracks the keyboard)
/// and, while it is occluded, set CSS custom properties so `html,body,
/// #root` size to the VISIBLE height (`--lh-vh`) and the app is nudged
/// back into view (`--lh-vv-top`). A thin platform binding — it only
/// writes CSS variables / a class on <html>, building no DOM (same
/// spirit as the dom.rs helpers). No-op on desktop and when no
/// `visualViewport` exists. Chrome/Android is already handled by the
/// `interactive-widget=resizes-content` viewport meta; this covers iOS.
fn install_keyboard_viewport_fix() {
    let Some(win) = web_sys::window() else { return };
    let Some(vv) = win.visual_viewport() else { return };

    let apply = move || {
        let Some(win) = web_sys::window() else { return };
        let Some(vv) = win.visual_viewport() else { return };
        let Some(doc) = win.document() else { return };
        let Some(root) = doc.document_element() else { return };
        let Ok(html) = root.dyn_into::<HtmlElement>() else { return };
        let style = html.style();

        // Layout-viewport height to compare against (window.innerHeight).
        let layout_h = win
            .inner_height()
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let visible_h = vv.height();
        let offset_top = vv.offset_top();

        // Keyboard is "open" once it eats a meaningful slice of the
        // viewport. A small threshold avoids reacting to the URL-bar
        // collapse (already covered by dvh) or sub-pixel jitter.
        let occluded = layout_h - visible_h > 120.0;

        if occluded {
            let _ = style.set_property("--lh-vh", &format!("{visible_h}px"));
            let _ = style.set_property("--lh-vv-top", &format!("{offset_top}px"));
            let _ = html.class_list().add_1("lh-kb");
        } else {
            let _ = html.class_list().remove_1("lh-kb");
            let _ = style.remove_property("--lh-vh");
            let _ = style.remove_property("--lh-vv-top");
        }
    };

    // Run once now (in case the page loads with the keyboard already up)
    // and on every visualViewport resize/scroll.
    apply();
    let cb = Closure::<dyn FnMut()>::new(apply);
    let _ = vv.add_event_listener_with_callback("resize", cb.as_ref().unchecked_ref());
    let _ = vv.add_event_listener_with_callback("scroll", cb.as_ref().unchecked_ref());
    cb.forget(); // lives for the document lifetime, like the other listeners
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

/// Auto-grow the prompt textarea to its content: collapse to `auto` so the
/// scroll height reflects the real text height (not the last grown height),
/// then pin the element to that scroll height. CSS caps it (`max-height`) and
/// scrolls past the cap. Routed through the ONE delegated `input` listener — no
/// per-element closure (the app's no-imperative-DOM rule). No-op off a real
/// `HtmlElement`.
fn autogrow_textarea(el: &Element) {
    let Some(ta) = el.dyn_ref::<HtmlElement>() else { return };
    let style = ta.style();
    let _ = style.set_property("height", "auto");
    let _ = style.set_property("height", &format!("{}px", ta.scroll_height()));
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
        Action::OpfsCloseViewer => super::opfs::close_viewer(),
        Action::ToggleDisplay => super::opfs::toggle_display(),
        Action::ToggleTerminal => super::cli::toggle_terminal(),
        Action::BroadcastSend(title) => super::display::broadcast_send(title),
        Action::BroadcastCancel => super::display::close_broadcast_composer(),
        Action::StopTurn => super::chat::request_stop_turn(),
        Action::SetPublicFace(choice) => {
            wasm_bindgen_futures::spawn_local(async move {
                public_face::run_set_public_face(&choice).await;
            });
        }
        Action::CopyShareUrl(url) => {
            wasm_bindgen_futures::spawn_local(async move {
                public_face::run_copy_to_clipboard(&url, "share-copy").await;
            });
        }
        Action::CopySeed(phrase) => {
            wasm_bindgen_futures::spawn_local(async move {
                public_face::run_copy_to_clipboard(&phrase, "seed-copy").await;
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
                claim::run_apex_claim(cleaned, false).await;
            });
        }
        Action::CreateNewClaim(name) => {
            let cleaned = super::tenant::sanitize(&name);
            if cleaned.len() < 3 || cleaned.len() > 32 {
                return;
            }
            wasm_bindgen_futures::spawn_local(async move {
                claim::run_apex_claim(cleaned, true).await;
            });
        }
        Action::ClaimOnChain => {
            // Tenant-side first-claim: ensure apex wallet exists (without
            // overwriting an existing one — that would nuke other NFTs),
            // run the on-chain register tx via the signer iframe, then
            // set the local OPFS marker + re-paint as owner. This kills
            // the previous "bounce to apex first" interstitial.
            let Some(name) = super::tenant::current_name() else {
                return;
            };
            // Guard the routable-label invariant BEFORE spending sponsored gas
            // (juno-qa): an unroutable name (>63 chars / bad chars) would mint a
            // zombie the DNS gateway can't serve. The chat-tool + apex-form
            // paths already validate; this tenant-side claim was the gap.
            if !crate::subdomain::is_valid_subdomain_label(&name) {
                dom::swap_inner(
                    "claim-msg",
                    &dom::msg_span(dom::Msg::Error, "invalid name"),
                );
                return;
            }
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
                    Ok((owner_addr, _tx)) => {
                        // Remember the just-registered owner address as the
                        // local first-paint hint (the chain stays authority).
                        let _ = super::owner::remember(&owner_addr).await;
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
        Action::RevealSeed => {
            // Local-first (local-seed-per-origin, see `verify::local_master`):
            // when THIS origin holds the seed — apex always, a subdomain
            // after `seed_pull` — read the mnemonic straight off the cached
            // `APP.wallet`. The signer-iframe round-trip is only the
            // fallback for a seedless tenant origin: its `lh-reveal-seed`
            // handler is apex-origin-only and the iframe is partitioned
            // (dead) on mobile, so it must never be the primary path.
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
            } else if !matches!(super::tenant::current(), super::tenant::Host::Apex) {
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
        Action::HideSeed => {
            dom::swap_inner(
                "seed-reveal",
                r#"<button type="button" data-action="reveal-seed">I have a pen and paper — reveal</button>"#,
            );
        }
        Action::OnboardCreate => {
            // Consolidated front door: the visitor picked a name AND hit CREATE.
            // 1) Validate + STASH the name (the checkout card replaces the input,
            //    so it must survive the async flow); the live check already keeps
            //    the button disabled for invalid/taken names, so this is a guard.
            // 2) Generate the keypair IN MEMORY ONLY (persisted on confirmed
            //    payment, not here — the owner's rule + the iOS borrow-panic fix).
            // 3) Run the $2 checkout. Once paid, `credits::persist_seed_and_claim`
            //    claims the stashed name and redirects into the agent's chat — no
            //    second step, no shown-once seed page.
            let raw = dom::input_by_id("apex-input")
                .map(|i| i.value())
                .unwrap_or_default();
            let cleaned = super::tenant::sanitize(&raw);
            if cleaned.len() < 3 || cleaned.len() > 32 {
                return;
            }
            let Some(flow_guard) = onboard_flow_begin() else {
                return;
            };
            set_onboard_name(&cleaned);
            // INSTANT FEEDBACK: swap the form for the inline checkout card before
            // any await ($2 = 200 $LH at $1 = 100 $LH). The card carries the Stripe
            // mount ids; the spawned work below fills it.
            dom::swap_outer(
                "apex-onboard",
                &templates::onboard_checkout().into_string(),
            );
            wasm_bindgen_futures::spawn_local(async move {
                let _flow_guard = flow_guard;
                match super::wallet_store::generate_in_memory().await {
                    Err(err) => {
                        // Restore the form (with the typed name preserved) so the
                        // user can retry; drop the stale stash.
                        let _ = take_onboard_name();
                        dom::swap_outer(
                            "apex-onboard",
                            &crate::landing::create_wallet_cta().into_string(),
                        );
                        if let Some(input) = dom::input_by_id("apex-input") {
                            input.set_value(&cleaned);
                            claim::on_apex_input();
                        }
                        dom::swap_inner(
                            "onboard-msg",
                            &dom::msg_span(dom::Msg::Error, &format!("create failed: {err}")),
                        );
                    }
                    Ok(_) => {
                        // Identity in memory → drive the $2 checkout into the inline
                        // card (mints 200 $LH); on a confirmed mint the poll persists
                        // the seed, claims the stashed name, and redirects to chat.
                        credits::buy_lh_pressed(true);
                    }
                }
            });
        }
        Action::CreateIdentity => {
            // Apex: generate locally + bootstrap-fund + re-paint.
            // Tenant: route through the apex signer iframe so the wallet
            // lands at apex OPFS, then re-paint tenant chrome so
            // verification picks up the new owner.
            // SINGLE-FLIGHT: ignore re-presses while a flow runs.
            let Some(flow_guard) = onboard_flow_begin() else {
                return;
            };
            dom::swap_inner(
                "identity-msg",
                "<span style=\"color:var(--muted)\">generating identity…</span>",
            );
            // Volatile-storage (incognito) warning is surfaced AFTER the seed
            // write completes (see `warn_if_storage_volatile` calls below), NOT
            // concurrently: racing `storage_is_volatile()`'s `navigator.storage`
            // round-trip against `create_and_persist`'s first OPFS write
            // interleaved two tasks on the single-thread executor and tripped the
            // iOS "RefCell already borrowed" panic at the seed write. The warning
            // is about BACKING UP a freshly-minted seed, so showing it once the
            // seed exists is equally timely.
            match super::tenant::current() {
                super::tenant::Host::Apex => {
                    wasm_bindgen_futures::spawn_local(async move {
                        let _flow_guard = flow_guard;
                        // Bounded: a wedged storage write must surface as an
                        // error, not an eternal "generating identity…" (the
                        // iPhone stuck-create report).
                        match super::net::with_timeout(
                            15_000,
                            super::wallet_store::create_and_persist(),
                        )
                        .await
                        {
                            Err(_) => {
                                dom::swap_inner(
                                    "identity-msg",
                                    &dom::msg_span(
                                        dom::Msg::Error,
                                        "create timed out — reload and try again",
                                    ),
                                );
                                return;
                            }
                            Ok(Err(err)) => {
                                dom::swap_inner(
                                    "identity-msg",
                                    &dom::msg_span(dom::Msg::Error, &format!("create failed: {err}")),
                                );
                                return;
                            }
                            Ok(Ok(_)) => {}
                        }
                        // Seed is now safely persisted — only NOW probe storage
                        // durability (sequentially, never racing the write) and
                        // surface the incognito back-up warning if volatile.
                        warn_if_storage_volatile().await;
                        // Progress BEFORE the repaint: paint_apex does on-chain
                        // reads that can be slow on mobile — the identity is
                        // already safe at this point and the user should know.
                        dom::swap_inner(
                            "identity-msg",
                            "<span style=\"color:var(--muted)\">identity created — loading…</span>",
                        );
                        // Run the repaint on a FRESH executor tick, not inline.
                        // `paint_apex` itself `spawn_local`s several sub-tasks and
                        // awaits OPFS/RPC; awaiting it from inside THIS onboarding
                        // task deepens the wasm-bindgen single-thread executor's
                        // poll chain, and on iOS WebKit's microtask timing that
                        // re-enters `Task::run` for an already-borrowed task → the
                        // "RefCell already borrowed" panic. Deferring lets this
                        // task fully return (releasing the executor borrow) before
                        // the heavy paint runs. The flow guard rides along so a
                        // re-press is still blocked until the surface is up.
                        defer_onboard_repaint(_flow_guard, async {
                            super::paint_apex(super::tenant::Host::Apex).await;
                        });
                    });
                }
                host => {
                    wasm_bindgen_futures::spawn_local(async move {
                        let flow_guard = flow_guard;
                        match super::verify::create_wallet_via_iframe(false).await {
                            Ok(_addr) => {
                                // Seed persisted — probe durability now, never
                                // racing the write (the iOS borrow-panic class).
                                warn_if_storage_volatile().await;
                                if let super::tenant::Host::Tenant(name) = &host {
                                    // Defer the repaint to a fresh executor tick —
                                    // same iOS re-entrant-`Task::run` hazard as the
                                    // apex branch above (paint_tenant spawns + awaits
                                    // OPFS/RPC). The guard rides along.
                                    let host = host.clone();
                                    let name = name.clone();
                                    defer_onboard_repaint(flow_guard, async move {
                                        super::paint_tenant(host, name).await;
                                    });
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
            // Tenant: write THIS origin's OPFS directly too — a tenant
            // import intentionally affects only this origin (that IS the
            // local-seed-per-origin model; other origins adopt the seed
            // via the apex QR `?adopt=1` flow or `seed_pull`). The old
            // signer-iframe route always failed here: the iframe's
            // `lh-import-seed` handler is apex-origin-only and the iframe
            // itself is partitioned (dead) on mobile.
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
                        match super::wallet_store::import(&phrase).await {
                            Ok(wallet) => {
                                super::APP
                                    .with(|cell| cell.borrow_mut().wallet = Some(wallet));
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
            // Arm the wipe — swap the button into an inline confirm pair
            // (yes / no), then pull focus onto [wipe?] so a keyboard user lands
            // on the action (a11y #75). No focus-stack push: this lives inside
            // the already-trapped files modal, whose stack entry restores focus.
            dom::swap_outer(
                "opfs-wipe-slot",
                &templates::opfs_wipe_confirm_inline().into_string(),
            );
            dom::focus_first_in("opfs-wipe-slot");
        }
        Action::OpfsWipeConfirm => {
            // Restore the slot first so the in-flight wipe doesn't
            // leave stale confirm buttons visible.
            dom::swap_outer(
                "opfs-wipe-slot",
                &templates::opfs_wipe_armed_inline().into_string(),
            );
            // Return focus to the now-restored [wipe] trigger.
            dom::focus_first_in("opfs-wipe-slot");
            wasm_bindgen_futures::spawn_local(async move {
                super::opfs::wipe().await;
            });
        }
        Action::OpfsWipeCancel => {
            dom::swap_outer(
                "opfs-wipe-slot",
                &templates::opfs_wipe_armed_inline().into_string(),
            );
            // Return focus to the restored [wipe] trigger.
            dom::focus_first_in("opfs-wipe-slot");
        }
        Action::CancelImport => {
            dom::swap_outer("import-slot", r#"<div id="import-slot"></div>"#);
        }
        Action::HeaderAdminToggle => admin::header_admin_toggle(),
        Action::HeaderAdminClose => admin::header_admin_close(),
        Action::ToggleTheme => layout::toggle_theme(),
        Action::TogglePreview => layout::toggle_preview(),
        Action::ShowAdminTab(name) => admin::show_admin_tab(&name),
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
            // Remember the [reset…] trigger, swap in the typed-confirm dialog,
            // then pull focus INTO it (lands on the RESET input) — a11y #75.
            dom::remember_focus();
            dom::swap_outer(
                "reset-confirm-slot",
                &templates::reset_confirm_inline().into_string(),
            );
            dom::focus_first_in("reset-confirm-slot");
        }
        Action::ResetCancel => {
            dom::swap_outer(
                "reset-confirm-slot",
                &templates::reset_armed_inline().into_string(),
            );
            // Panel gone — return focus to the [reset…] trigger.
            dom::restore_focus();
        }
        Action::ResetConfirm => layout::reset_confirm_pressed(),
        Action::PricingSave => layout::pricing_save_pressed(),
        Action::ToggleFiles => {
            wasm_bindgen_futures::spawn_local(async move {
                super::opfs::toggle_files_modal().await;
            });
        }
        Action::ToggleFeedback => admin::toggle_feedback_panel(),
        Action::FeedbackSubmit => super::feedback::feedback_submit(),
        Action::AddDevice => devices::add_device_pressed(),
        Action::SyncDevices => devices::run_sync_devices(),
        Action::AdoptDevice => devices::adopt_device_pressed(),
        Action::PairCancel => devices::pair_cancel_pressed(),
        Action::SavePrompt => admin::save_prompt_pressed(),
        Action::SaveToolAllowlist => admin::save_tool_allowlist_pressed(),
        Action::ResetToolAllowlist => admin::reset_tool_allowlist_pressed(),
        Action::SaveApiKey => admin::save_api_key_pressed(),
        Action::SetModelAccess(mode) => credits::run_set_model_access(mode),
        Action::SetModel(model) => credits::run_set_model(model),
        Action::DownloadLocalModel => credits::run_download_local_model(),
        Action::RedeemInviteOnboard => credits::redeem_invite_onboard_pressed(),
        Action::RedeemCode => credits::redeem_code_pressed(),
        Action::BuyLh => credits::buy_lh_pressed(false),
        Action::CancelBuy => credits::cancel_buy_pressed(),
        Action::RedeemBanner => credits::redeem_banner_pressed(),
        Action::CreateInvite => credits::create_invite_pressed(),
        Action::SaveX402Price => admin::save_x402_price_pressed(),
        Action::UnlinkDevice(addr) => devices::unlink_device_prompt(addr),
        Action::UnlinkConfirm(addr) => devices::unlink_confirm_pressed(addr),
        Action::UnlinkCancel => devices::unlink_cancel_pressed(),
        Action::EnableNotifications => admin::enable_notifications_pressed(),
        Action::NotifBell => admin::notif_bell_pressed(),
        Action::NotifClearAll => admin::notif_clear_all_pressed(),
        Action::NotifClearConfirm => admin::notif_clear_confirmed(),
        Action::NotifClearCancel => admin::notif_clear_cancelled(),
        Action::TestNotification => admin::test_notification_pressed(),
        Action::InstallApp => admin::install_app_pressed(),
    }
}

/// Fetch the credit balance for the apex wallet and write it into
/// `#credits-balance`. Called on admin-open. Soft-fail — leaves the
/// placeholder on error so the UI stays clean.
pub(crate) async fn refresh_credits_pill() {
    // Use the credit identity (master wallet, else local device key) so
    // the balance + session reflect what the proxy will actually see.
    let Some(addr) = super::chat::credit_address_existing().await else { return };
    // "Credits" = total spendable $LH = wallet balance + the per-request meter
    // (the wallet auto-deposits into the meter on the next turn; the proxy
    // debits the meter per call). 2-decimal so a per-call debit (0.01–0.20 LH)
    // is visibly subtracted; goes up on redeem. (The session-status + separate
    // meter line are gone — metering is the only billing surface now.)
    // Timeout-capped: the browser-fetch transport has no timeout, so a dead
    // RPC would leave the pill stuck on its `…` placeholder forever. On a
    // timeout (or read error) show a dash rather than spinning.
    let wallet = super::net::read(super::registry::token_balance_of(&addr))
        .await
        .ok()
        .and_then(Result::ok);
    let meter = super::net::read(super::registry::credit_balance_of(&addr))
        .await
        .ok()
        .and_then(Result::ok);
    match (wallet, meter) {
        (Some(wallet), Some(meter)) => {
            let total = wallet + meter;
            let whole = total / 1_000_000_000_000_000_000u128;
            let cents = (total % 1_000_000_000_000_000_000u128) / 10_000_000_000_000_000u128;
            dom::swap_inner("credits-balance", &format!("{whole}.{cents:02} LH"));
        }
        _ => dom::swap_inner("credits-balance", "—"),
    }
    warn_if_sponsor_low().await;
}

/// Soft per-origin sponsored-tx rate cap — a testnet abuse guard. Rolling
/// window kept in localStorage; bypassable (clear storage) but bounds
/// runaway loops + casual drain. The real mainnet fix is the sponsor-key
/// rewrite. Returns Err when the window is saturated.
const SPONSOR_RL_WINDOW_SECS: u64 = 3600;
const SPONSOR_RL_MAX: usize = 60;

fn sponsor_rate_guard() -> Result<(), String> {
    let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
        return Ok(()); // no storage available → don't block the user
    };
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let prev = storage
        .get_item("lh_sponsor_rl")
        .ok()
        .flatten()
        .unwrap_or_default();
    let mut stamps: Vec<u64> = prev
        .split(',')
        .filter_map(|s| s.trim().parse::<u64>().ok())
        .filter(|t| now.saturating_sub(*t) < SPONSOR_RL_WINDOW_SECS)
        .collect();
    if stamps.len() >= SPONSOR_RL_MAX {
        return Err("too many sponsored actions in a short window — wait a bit".into());
    }
    stamps.push(now);
    let joined = stamps
        .iter()
        .map(|t| t.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let _ = storage.set_item("lh_sponsor_rl", &joined);
    Ok(())
}

/// Best-effort sponsor fee-token balance monitor — warns in the console
/// when the shared sponsor wallet runs low so the operator can refill.
/// Cheap single eth_call; never blocks.
pub(crate) async fn warn_if_sponsor_low() {
    // Mainnet has no embedded sponsor (the relay signs `fee_payer` + runs its own
    // float circuit-breaker), so there's no local key whose balance to warn on.
    if super::registry::is_mainnet() {
        return;
    }
    // AlphaUSD is a 6-DECIMAL TIP-20 (`decimals()` == 6), not 18 — the old
    // 5e18 threshold made every real balance read as "~0" on every load.
    const ALPHA_USD_UNIT: u128 = 1_000_000;
    const LOW_THRESHOLD: u128 = 5 * ALPHA_USD_UNIT; // ~5 AlphaUSD
    let Ok(signer) = super::sponsor::signer() else {
        return;
    };
    let addr = crate::wallet::address(&signer);
    let addr_hex = bytes_to_hex_str(&addr);
    if let Ok(bal) =
        super::registry::erc20_balance_of(super::registry::ALPHA_USD_ADDRESS(), &addr_hex).await
    {
        if bal < LOW_THRESHOLD {
            let whole = bal / ALPHA_USD_UNIT;
            web_sys::console::warn_1(&JsValue::from_str(&format!(
                "sponsor fee-token LOW: ~{whole} AlphaUSD at {addr_hex} — refill soon"
            )));
        }
    }
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
    sponsor_rate_guard()?;
    let sender_address = parse_address(from_hex)?;
    let fee_token_addr = parse_address(super::registry::ALPHA_USD_ADDRESS())?;
    let nonce = super::registry::next_nonce(from_hex).await
        .map_err(|e| format!("nonce: {e}"))?;
    let gas_price = super::registry::current_gas_price().await
        .map_err(|e| format!("gas price: {e}"))?;

    let tx = crate::tempo_tx::TempoTxBuilder::new(super::registry::CHAIN_ID())
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
            "sender sig recovered {} but expected {claimed_addr} ({from_hex})",
            bytes_to_hex_str(&recovered),
        ));
    }

    // fee_payer half. Mainnet: the bundle holds no money key — the server relay
    // signs it (authed by THIS identity's local signer, which must match the
    // sender). Testnet: the embedded sponsor signs locally. This is the
    // self-assembled twin of the `registry::tx` submit chokepoints.
    let fp_sig = if super::registry::is_mainnet() {
        let signer = super::APP
            .with(|c| c.borrow().wallet.as_ref().map(|w| w.signer.clone()))
            .ok_or_else(|| {
                "mainnet sponsorship needs your identity on this device".to_string()
            })?;
        if crate::wallet::address(&signer) != sender_address {
            return Err("local signer does not match the tx sender".to_string());
        }
        super::registry::request_fee_payer_signature(&signer, &tx, &sender_address, &sender_sig)
            .await?
    } else {
        let fee_payer = super::sponsor::signer()?;
        crate::wallet::sign_hash(&fee_payer, &tx.fee_payer_hash(&sender_address))
    };
    let raw = tx.serialize_signed(&sender_sig, Some(&fp_sig));
    let raw_hex = bytes_to_hex_str(&raw);
    super::registry::submit_and_wait_receipt(&raw_hex).await
        .map_err(|e| format!("submit: {e}"))
}
