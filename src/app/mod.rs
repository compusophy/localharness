//! Browser-resident IDE for the localharness SDK.
//!
//! Compiled into the crate only when both `feature = "browser-app"`
//! and `target_arch = "wasm32"` are active (see `lib.rs`). The shipping
//! bundle is built by `scripts/build-web.{sh,ps1}` running
//! `wasm-pack build --features browser-app --no-default-features`.
//!
//! Design rule: **no imperative DOM manipulation**. All HTML is built
//! by [`maud`] templates and shipped into the document via
//! `set_inner_html` or `insert_adjacent_html` swaps targeted by `id=`.
//! Event handling uses one delegated click + one delegated keydown
//! listener at the document level — UI elements declare intent through
//! `data-action="..."` attributes, the way HTMX does. There is no
//! per-element `Closure::wrap` chain anywhere in this module.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use wasm_bindgen::prelude::*;

use crate::filesystem::OpfsFilesystem;
use crate::Agent;

mod chat;
mod dom;
mod events;
mod history;
mod key_store;
mod opfs;
mod owner;
mod registry;
mod signer;
mod templates;
mod tenant;
mod verify;
mod wallet_store;

/// Per-tab state. One instance lives in [`APP`] for the lifetime of the
/// page. Nothing here is `Send`/`Sync` — wasm32 is single-threaded.
pub(crate) struct App {
    pub(crate) agent: Option<Rc<Agent>>,
    /// API key the current `agent` was started with. Used to detect
    /// "user pasted a new key" and reset the session.
    pub(crate) session_key: Option<String>,
    pub(crate) turn_count: u32,
    /// Monotonic id used for unique DOM ids on turns, segments, tool
    /// blocks. Never reused across resets so stale event targets are
    /// safe to drop.
    pub(crate) next_id: u32,
    /// Current working directory for the OPFS panel, as a sequence of
    /// directory names from the OPFS root. Empty means root.
    pub(crate) opfs_cwd: Vec<String>,
    /// Shared OPFS handle used by the panel. Built lazily so a missing
    /// browser-OPFS just leaves the panel idle rather than panicking.
    pub(crate) opfs: Option<Arc<OpfsFilesystem>>,
    /// Restored-from-OPFS history bytes from a previous session. Set
    /// once on mount (if the marker file exists) and consumed by the
    /// next `start_session`. None after first use so it doesn't get
    /// re-applied on subsequent key changes.
    pub(crate) pending_history: Option<Vec<u8>>,
    /// Master wallet at the apex origin. Cached after first load so
    /// the "reveal seed" affordance can read it without re-touching
    /// OPFS. `None` everywhere except the apex chrome path.
    pub(crate) wallet: Option<wallet_store::MasterWallet>,
    /// Result of the most-recent on-chain owner verification on a
    /// tenant subdomain. Updated asynchronously after `paint_tenant`
    /// renders the chrome; the UI pill reflects whatever's here.
    pub(crate) verify_state: VerifyState,
}

/// Surface-level summary of the cross-origin verification flow,
/// mirrored into the chrome via a status pill.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum VerifyState {
    #[default]
    Pending,
    /// On-chain owner exists AND the iframe signer's signature
    /// recovered to that address.
    Verified {
        address: String,
    },
    /// On-chain owner exists but the visitor's wallet signed with a
    /// different address — they're browsing someone else's space.
    Visitor {
        owner_address: String,
    },
    /// Name has no on-chain owner; legacy local-OPFS marker is the
    /// only source of truth.
    Unregistered,
    /// Verification flow itself failed (RPC down, iframe failed,
    /// signer didn't respond). Treat as legacy-trust mode but show
    /// the user that verification didn't complete.
    Failed {
        reason: String,
    },
}

impl App {
    fn new() -> Self {
        Self {
            agent: None,
            session_key: None,
            turn_count: 0,
            next_id: 0,
            opfs_cwd: Vec::new(),
            opfs: None,
            pending_history: None,
            wallet: None,
            verify_state: VerifyState::Pending,
        }
    }

    pub(crate) fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }
}

thread_local! {
    pub(crate) static APP: RefCell<App> = RefCell::new(App::new());
}

/// Shared `OpfsFilesystem` handle — one per tab. Lazily initialised on
/// first use so the rest of the app doesn't have to care whether the
/// browser supports OPFS until something actually touches it.
pub(crate) fn shared_opfs() -> Arc<OpfsFilesystem> {
    APP.with(|cell| {
        let mut app = cell.borrow_mut();
        if app.opfs.is_none() {
            app.opfs = Some(Arc::new(OpfsFilesystem::new()));
        }
        app.opfs.as_ref().unwrap().clone()
    })
}

/// Auto-runs at module load. Renders the initial chrome into `#root`
/// and attaches the delegated event listeners. Everything else is
/// driven by user events from here on.
#[wasm_bindgen(start)]
fn start() {
    console_error_panic_hook::set_once();

    if let Err(err) = mount() {
        web_sys::console::error_1(&JsValue::from_str(&format!(
            "localharness app failed to mount: {err:?}"
        )));
    }
}

fn mount() -> Result<(), JsValue> {
    let doc = dom::document()?;
    let root = doc
        .get_element_by_id("root")
        .ok_or_else(|| JsValue::from_str("missing <div id=\"root\"> in the host page"))?;

    // Resolve which tenant we're being served as. On apex, we paint a
    // marketing chrome with a single "claim a subdomain" CTA. On a
    // tenant subdomain, we check the OPFS ownership marker and paint
    // either the unclaimed-prompt or the full app. On unknown hosts
    // (localhost, Vercel preview) we paint the full app for testing.
    let host = tenant::current();
    let host_for_listeners = host.clone();

    // Delegated listeners are installed first so the apex / unclaimed
    // templates' buttons work even before we hit the async branches.
    events::install_delegated_listeners(&doc)?;

    // Signer mode short-circuit. When apex is loaded with ?signer=1
    // (typically in a hidden iframe from a subdomain doing owner
    // verification), skip the marketing chrome entirely and just turn
    // this tab into a postMessage signing service.
    if matches!(&host, tenant::Host::Apex) && has_signer_hint() {
        root.set_inner_html("<main style=\"padding:48px;text-align:center;color:#7a8493;font:14px ui-monospace,Menlo,Consolas,monospace\">localharness signer · loading…</main>");
        signer::install_signer_listener()?;
        wasm_bindgen_futures::spawn_local(async move {
            // Loading the wallet warms it into App state so the
            // postMessage handler can pull it synchronously.
            paint_signer().await;
        });
        return Ok(());
    }

    match &host {
        tenant::Host::Apex => {
            // Wallet load is async (OPFS); paint a placeholder first
            // and refresh once we know the address.
            let host_for_apex = host.clone();
            root.set_inner_html(
                &templates::apex(&host_for_apex, "loading…").into_string(),
            );
            wasm_bindgen_futures::spawn_local(async move {
                paint_apex(host_for_apex).await;
            });
            return Ok(());
        }
        tenant::Host::Tenant(name) => {
            // Tenant subdomain — defer the chrome choice until we've
            // peeked at the ownership marker (async).
            let placeholder = format!(
                "<main style=\"padding:48px;text-align:center;color:#7a8493;\
                 font:14px ui-monospace,Menlo,Consolas,monospace\">\
                 resolving {name}…</main>"
            );
            root.set_inner_html(&placeholder);
            let name = name.clone();
            wasm_bindgen_futures::spawn_local(async move {
                paint_tenant(host_for_listeners, name).await;
            });
            return Ok(());
        }
        tenant::Host::Other(_) => {
            // Fall through to the existing chrome path.
        }
    }

    // Full-app chrome (localhost, Vercel preview, etc.).
    root.set_inner_html(&templates::chrome(&host).into_string());

    // sessionStorage is the synchronous fallback for the input field's
    // initial value. The OPFS-stored key (async) takes over once it
    // resolves; if both exist, OPFS wins.
    if let Some(storage) = dom::session_storage()? {
        if let Ok(Some(cached)) = storage.get_item("gemini_api_key") {
            if let Some(input) = dom::input_by_id("key") {
                input.set_value(&cached);
                events::refresh_keymeta();
            }
        }
    }

    dom::set_status("ready · type a prompt", false);

    // Initial OPFS panel paint + history restore + key restore. All
    // async; the key loader populates the input field if a persisted
    // key exists (overriding sessionStorage).
    wasm_bindgen_futures::spawn_local(async move {
        if let Some(persisted_key) = key_store::load().await {
            if let Some(input) = dom::input_by_id("key") {
                input.set_value(&persisted_key);
                events::refresh_keymeta();
            }
        }
        history::load_into_pending().await;
        opfs::refresh().await;
    });
    Ok(())
}

/// Render a tenant subdomain after we know whether it's claimed. If
/// no `.lh_owner` marker exists in this device's OPFS, paint the
/// claim flow; otherwise paint the full app and run the usual restore
/// path. Called once on mount and again after a successful claim.
pub(crate) async fn paint_tenant(host: tenant::Host, name: String) {
    let Ok(doc) = dom::document() else { return };
    let Some(root) = doc.get_element_by_id("root") else { return };

    let mut owner = owner::current_owner().await;
    // Apex sends users here with ?claim=1 to skip the
    // "claim this name?" interstitial — the user has already expressed
    // intent on the previous page. Auto-claim, then strip the param so
    // a refresh doesn't trigger anything weird.
    if owner.is_none() && has_claim_hint() {
        if let Ok(id) = owner::claim().await {
            owner = Some(id);
            strip_claim_hint();
        }
    }
    if owner.is_none() {
        // Unclaimed on this device — paint the prompt.
        root.set_inner_html(&templates::unclaimed(&host, &name).into_string());
        return;
    }

    // Claimed — paint the full app.
    root.set_inner_html(&templates::chrome(&host).into_string());

    if let Ok(Some(storage)) = dom::session_storage() {
        if let Ok(Some(cached)) = storage.get_item("gemini_api_key") {
            if let Some(input) = dom::input_by_id("key") {
                input.set_value(&cached);
                events::refresh_keymeta();
            }
        }
    }
    dom::set_status("ready · type a prompt", false);

    if let Some(persisted_key) = key_store::load().await {
        if let Some(input) = dom::input_by_id("key") {
            input.set_value(&persisted_key);
            events::refresh_keymeta();
        }
    }
    history::load_into_pending().await;
    opfs::refresh().await;

    // Background: try to verify the visitor against the on-chain
    // owner via the apex iframe signer. Fire-and-forget so the
    // chrome paint doesn't block on a ~1-5s roundtrip; the pill
    // updates when the result lands.
    wasm_bindgen_futures::spawn_local(async move {
        kick_verification(name).await;
    });
}

/// Run `verify::verify_owner` for `name` and stash the result. The
/// pill in the chrome reflects whatever lands here. Falls back to
/// the legacy local-OPFS marker if the on-chain check fails.
async fn kick_verification(name: String) {
    let outcome = match verify::verify_owner(&name).await {
        Ok(verify::VerifyResult::VerifiedOwner { address }) => {
            VerifyState::Verified { address }
        }
        Ok(verify::VerifyResult::Visitor { owner_address }) => {
            VerifyState::Visitor { owner_address }
        }
        Ok(verify::VerifyResult::Unregistered) => VerifyState::Unregistered,
        Err(err) => VerifyState::Failed { reason: err },
    };
    APP.with(|cell| cell.borrow_mut().verify_state = outcome.clone());
    let html = templates::verify_pill(&outcome).into_string();
    dom::swap_outer("verify-pill", &html);

    // Visitor mode: replace the input region with a read-only banner.
    // The transcript + OPFS panel stay visible (they live outside
    // `#input-region`), but the visitor can't send messages or save
    // anything new in the chat session.
    if let VerifyState::Visitor { owner_address } = &outcome {
        let html = templates::visitor_banner(owner_address).into_string();
        dom::swap_outer("input-region", &html);
    }

    // When the name is on-chain (Verified or Visitor), look up its
    // ERC-6551 token-bound account and surface it as a TBA pill in
    // the header. This is the agent's wallet — receives funds,
    // signs messages, settles payments. Counterfactual; address
    // exists whether the account has been deployed yet or not.
    let on_chain = matches!(
        outcome,
        VerifyState::Verified { .. } | VerifyState::Visitor { .. }
    );
    if on_chain {
        if let Ok(Some(tba)) = registry::tba_of_name(&name).await {
            let html = templates::tba_pill(&tba).into_string();
            dom::swap_outer("tba-pill", &html);
        }
    }
}

/// Load (or generate) the master wallet, stash it in `App`, then
/// re-paint the apex chrome with the real address. Called once on
/// mount and again after an `import-seed` action.
pub(crate) async fn paint_apex(host: tenant::Host) {
    let Ok(doc) = dom::document() else { return };
    let Some(root) = doc.get_element_by_id("root") else { return };

    match wallet_store::load_or_create().await {
        Ok(wallet) => {
            let addr = wallet.address_hex();
            APP.with(|cell| cell.borrow_mut().wallet = Some(wallet));
            root.set_inner_html(&templates::apex(&host, &addr).into_string());
            // Pre-fill the claim input + trigger the live-check if the
            // user landed here via `?prefill=<name>` (e.g. from a
            // tenant subdomain's "claim on-chain" CTA).
            if let Some(prefill) = read_query_param("prefill") {
                let cleaned = tenant::sanitize(&prefill);
                if !cleaned.is_empty() {
                    if let Some(input) = dom::input_by_id("apex-input") {
                        input.set_value(&cleaned);
                        // Dispatch an input event so the existing
                        // delegated listener kicks off the live check.
                        if let Ok(event) = web_sys::Event::new("input") {
                            let _ = input.dispatch_event(&event);
                        }
                        let _ = input.focus();
                    }
                }
            }
        }
        Err(err) => {
            web_sys::console::error_1(&JsValue::from_str(&format!("wallet: {err}")));
            root.set_inner_html(
                &templates::apex(&host, "(wallet unavailable — see console)").into_string(),
            );
        }
    }
}

/// Paint the minimal signer chrome once the wallet has loaded.
pub(crate) async fn paint_signer() {
    let Ok(doc) = dom::document() else { return };
    let Some(root) = doc.get_element_by_id("root") else { return };
    match wallet_store::load_or_create().await {
        Ok(wallet) => {
            let addr = wallet.address_hex();
            APP.with(|cell| cell.borrow_mut().wallet = Some(wallet));
            root.set_inner_html(&templates::signer_chrome(&addr).into_string());
        }
        Err(err) => {
            web_sys::console::error_1(&JsValue::from_str(&format!("signer wallet: {err}")));
            root.set_inner_html(
                "<main style=\"padding:48px;text-align:center;color:#ff8b8b;font:14px ui-monospace,Menlo,Consolas,monospace\">signer wallet failed — see console</main>",
            );
        }
    }
}

/// Read a `?key=value` query parameter from the current URL, naive
/// implementation that avoids pulling a URL crate. Returns `None` if
/// the param is missing or empty.
fn read_query_param(key: &str) -> Option<String> {
    let window = dom::window().ok()?;
    let search = window.location().search().ok()?;
    let stripped = search.trim_start_matches('?');
    for pair in stripped.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key && !v.is_empty() {
                return Some(decode_uri_component(v));
            }
        }
    }
    None
}

fn decode_uri_component(s: &str) -> String {
    js_sys::decode_uri_component(s)
        .map(|js| js.as_string().unwrap_or_else(|| s.to_string()))
        .unwrap_or_else(|_| s.to_string())
}

/// `true` iff `?signer=1` is in the URL.
fn has_signer_hint() -> bool {
    let Ok(window) = dom::window() else { return false };
    let Ok(search) = window.location().search() else { return false };
    search.contains("signer=1")
}

/// `true` iff `?claim=1` (or `?claim=anything`) is in the URL.
fn has_claim_hint() -> bool {
    let Ok(window) = dom::window() else { return false };
    let Ok(search) = window.location().search() else { return false };
    // search is like "?claim=1" or "" — naive contains() is fine for our flag.
    search.contains("claim=1") || search.contains("claim=true")
}

/// Remove the claim-hint query param without reloading the page. Used
/// once auto-claim succeeds so the URL looks clean.
fn strip_claim_hint() {
    let Ok(window) = dom::window() else { return };
    let Ok(history) = window.history() else { return };
    let url = window.location().pathname().unwrap_or_else(|_| "/".into());
    let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&url));
}
