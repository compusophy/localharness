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
mod templates;
mod tenant;

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

    match &host {
        tenant::Host::Apex => {
            root.set_inner_html(&templates::apex(&host).into_string());
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

    let owner = owner::current_owner().await;
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
}
