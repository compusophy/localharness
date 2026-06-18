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

use crate::filesystem::{EncryptedFilesystem, OpfsFilesystem, SharedFilesystem};
use crate::Agent;

mod chat;
mod compose;
mod debuglog;
// pub(crate) so the `run_cartridge` builtin tool can hand a compiled
// cartridge to the framebuffer (the agent→display loop).
pub(crate) mod display;
pub(crate) mod agent_config;
mod dom;
mod embed;
mod events;
mod feedback;
mod gas;
mod history;
mod key_store;
mod lessons;
mod model;
mod net;
mod notifications;
mod opfs;
mod owner;
mod pricing;
mod remote_call;
mod seed_pull;
// Cross-subdomain secure folder — apex-side encrypted store + data types
// (scaffold). Items are unused until the deferred round-trip wiring lands,
// so silence dead-code until then. See `shared_fs` module doc.
#[allow(dead_code)]
mod shared_fs;
/// WebRTC P2P transport (Layer 3) for cross-device shared-folder sync —
/// `RtcPeerConnection` over STUN, signaling carried by the on-chain
/// SignalingFacet. Compile-verified only (needs two browsers to exercise);
/// dead-code-allowed until the Layer 4 sync protocol wires it.
#[allow(dead_code)]
mod webrtc;
/// Cross-device shared-folder sync protocol (Layer 4) over the WebRTC channel +
/// the apex store. Compile-verified only; dead-code-allowed until the Layer 5
/// orchestration (SignalingFacet driver + peer discovery + UI) wires it.
#[allow(dead_code)]
mod sharedfs_sync;
/// Layer-5 orchestration: ephemeral keys + on-chain signaling drive a WebRTC
/// connect + shared-folder sync between the owner's devices (or team members).
/// Compile-verified only; dead-code-allowed until the teams/sync UI wires it.
#[allow(dead_code)]
mod teams_sync;
mod self_docs;
mod signer;
mod signer_protocol;
mod sponsor;
mod style;
mod agent_rpc;
mod encryption;
mod system_prompt;
mod templates;
mod tool_allowlist;
mod tenant;
mod verify;
mod wallet_store;

// Re-export the crate-level public registry module as `app::registry`
// so the existing intra-app imports keep working unchanged.
pub(crate) use crate::registry;

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
    /// Seed-keyed at-rest encryption wrapper over `opfs`
    /// ([`crate::filesystem::EncryptedFilesystem`]). Installed by
    /// [`install_at_rest_encryption`] the moment a master wallet
    /// materializes (`wallet_store::{load, create_and_persist, import}`);
    /// while `None` (pre-identity, seedless origins) `shared_opfs` serves
    /// the raw handle and everything stays plaintext, exactly as before.
    pub(crate) opfs_at_rest: Option<SharedFilesystem>,
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
    /// Agent's ERC-6551 token-bound account, populated by
    /// `kick_verification` after the on-chain TBA lookup. Read by
    /// the payment flow so the visitor pays the right address.
    pub(crate) tba_address: Option<String>,
    /// Per-turn payment price in wei for this tenant. Populated by
    /// the same load step that reads the verify state; consulted by
    /// `chat::run_send` to decide whether to gate the next turn.
    /// `None` means "haven't checked yet"; `Some(0)` means "free".
    pub(crate) pricing_wei: Option<u128>,
    /// The agent's on-chain card (name/owner/wallet/balance/…) rendered by
    /// `kick_verification`. Stashed here because the card now lives in the
    /// admin Account tab (folded in from the old right rail), which isn't
    /// in the DOM until the admin opens — `header_admin_toggle` injects it.
    pub(crate) financial_card_html: Option<String>,
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
    /// `visitor_address` is the recovered signer; payment flow uses it
    /// as the `from` of the on-chain payment tx.
    Visitor {
        owner_address: String,
        visitor_address: String,
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
            opfs_at_rest: None,
            pending_history: None,
            wallet: None,
            verify_state: VerifyState::Pending,
            tba_address: None,
            pricing_wei: None,
            financial_card_html: None,
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

/// Shared filesystem handle — one per tab. Lazily initialised on first
/// use so the rest of the app doesn't have to care whether the browser
/// supports OPFS until something actually touches it.
///
/// Once [`install_at_rest_encryption`] has run (a master wallet exists),
/// this returns the seed-keyed [`EncryptedFilesystem`] wrapper instead of
/// the raw OPFS handle — every caller transparently gets at-rest sealing
/// on write and magic-sniffed decrypt-or-passthrough on read.
pub(crate) fn shared_opfs() -> SharedFilesystem {
    APP.with(|cell| {
        let mut app = cell.borrow_mut();
        if let Some(enc) = app.opfs_at_rest.as_ref() {
            return enc.clone();
        }
        if app.opfs.is_none() {
            app.opfs = Some(Arc::new(OpfsFilesystem::new()));
        }
        let raw: SharedFilesystem = app.opfs.as_ref().unwrap().clone();
        raw
    })
}

/// Install the at-rest encryption layer over the shared OPFS handle.
/// Idempotent — the first install wins for the tab's lifetime (the key is
/// deterministic from the seed, so repeated wallet loads derive the same
/// key anyway). Called from `wallet_store::{load, create_and_persist,
/// import}`, the three places a [`wallet_store::MasterWallet`]
/// materializes; never called on seedless origins, which keep today's
/// plaintext behavior.
pub(crate) fn install_at_rest_encryption(key: [u8; 32]) {
    APP.with(|cell| {
        let mut app = cell.borrow_mut();
        if app.opfs_at_rest.is_some() {
            return;
        }
        if app.opfs.is_none() {
            app.opfs = Some(Arc::new(OpfsFilesystem::new()));
        }
        let raw: SharedFilesystem = app.opfs.as_ref().unwrap().clone();
        app.opfs_at_rest = Some(Arc::new(EncryptedFilesystem::new(raw, &key)));
    });
}

/// Auto-runs at module load. Renders the initial chrome into `#root`
/// and attaches the delegated event listeners. Everything else is
/// driven by user events from here on.
#[wasm_bindgen(start)]
fn start() {
    console_error_panic_hook::set_once();
    // AFTER the console hook (it chains the previous hook): paint panics into
    // a visible banner — iOS has no console, and a wasm panic otherwise looks
    // like a silently frozen app (every spawned future dies, timeouts included).
    debuglog::install_panic_banner();

    if let Err(err) = mount() {
        web_sys::console::error_1(&JsValue::from_str(&format!(
            "localharness app failed to mount: {err:?}"
        )));
    }
}

/// Web Push → in-app inbox bridge. `web/sw.js` relays an arriving push to
/// open pages as a `{type:'lh-push'}` message; `web/boot.js` (the project's
/// one JS file — the no-per-element-closure rule stays intact in Rust) hands
/// it here so the header-bell inbox + unread badge update live.
#[wasm_bindgen]
pub fn push_arrived(title: String, body: String) {
    notifications::push_arrived(&title, &body);
}

/// Stripe payment → mint bridge. `web/stripe-embed.js`'s `lhWatchPayment` polls
/// the PaymentIntent status IN JS (the shim holds the Stripe instance) and calls
/// `window.lh_payment_succeeded` (wired in `web/boot.js`) ONLY when it reaches
/// `succeeded`. We then finalize the mint off the wasm executor — moving the
/// status poll out of wasm fixed an iOS WebKit "already mutably borrowed"
/// BorrowError that the repeated pre-payment JsFuture + timer loop triggered.
/// Idempotent: `finalize_after_payment` mints at most once, so a double-fire is
/// safe.
#[wasm_bindgen]
pub fn lh_payment_succeeded(payment_intent: String, onboarding: bool, lh_label: String) {
    wasm_bindgen_futures::spawn_local(events::finalize_after_payment(
        payment_intent,
        lh_label,
        onboarding,
    ));
}

/// Inject the Rust-owned design tokens (`style::root_tokens_css`) into
/// `<head>` as `<style id="lh-tokens">`, once. Idempotent: re-running the
/// mount (or a paint that re-enters) won't stack duplicate blocks. The
/// static `web/styles.css` consumes the emitted `var()`s; CSS custom
/// properties resolve at use-time, so injecting before or after the
/// stylesheet link is equivalent.
fn inject_token_styles(doc: &web_sys::Document) {
    if doc.get_element_by_id("lh-tokens").is_some() {
        return;
    }
    // `Document::head()` lives on HtmlDocument; query the element instead so
    // this stays on the plain `web_sys::Document` we already hold. Fall back
    // to <html> (or skip) if a host page somehow lacks a <head>.
    let Some(parent) = doc
        .query_selector("head")
        .ok()
        .flatten()
        .or_else(|| doc.document_element())
    else {
        return;
    };
    if let Ok(style_el) = doc.create_element("style") {
        let _ = style_el.set_attribute("id", "lh-tokens");
        style_el.set_text_content(Some(&style::root_tokens_css()));
        let _ = parent.append_child(&style_el);
    }
}

fn mount() -> Result<(), JsValue> {
    debuglog::log("mount (page load / reload)");
    let doc = dom::document()?;
    let root = doc
        .get_element_by_id("root")
        .ok_or_else(|| JsValue::from_str("missing <div id=\"root\"> in the host page"))?;

    // Design tokens are the Rust source of truth. Inject the `:root { … }`
    // block (from `style::root_tokens_css`) into <head> ahead of the static
    // `styles.css` — the stylesheet's component rules read these `var()`s.
    // One-shot + idempotent (`#lh-tokens`), and it precedes every short-
    // circuit return below so the signer / rpc / embed chromes get tokens
    // too. Best-effort: a missing <head> never blocks the mount.
    inject_token_styles(&doc);

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

    // Install the x402 signing hook so the backend `call_agent` tool can
    // pay a callee that demands `$LH` — signs with the local credit key
    // (never the iframe). See [[x402_hook]].
    crate::x402_hook::install(std::rc::Rc::new(|ch: crate::x402_hook::X402Challenge|
        -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<crate::x402_hook::X402Payment, String>>>> {
        Box::pin(async move {
            let (signer, from) = chat::credit_signer()
                .await
                .ok_or_else(|| "no identity to pay from".to_string())?;
            // Sign exactly the caller-decided fields (to/value/window/nonce
            // are validated + chosen by call_agent before we get here).
            let sig = crate::registry::sign_x402(
                &signer,
                &from,
                &ch.to,
                ch.value_wei,
                ch.valid_after,
                ch.valid_before,
                &ch.nonce,
            )?;
            Ok(crate::x402_hook::X402Payment { from, signature: sig })
        })
    }));

    // Install the proxy-mediated agent-call route: when the local `?rpc=1`
    // iframe can't serve (a foreign agent has no state on this machine),
    // `call_agent` falls back to the hosted x402 `ask_agent` endpoint — the
    // local credit key pays the target's TBA in $LH. See [[remote_call]].
    crate::x402_hook::install_remote_call(std::rc::Rc::new(|target: String, message: String|
        -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>>>> {
        Box::pin(async move { remote_call::ask_via_proxy(&target, &message).await })
    }));

    // Compose mode short-circuit (?compose=name1,name2,...). The iframe-free
    // host::compose path (roadmap Track A): composite each named subdomain's
    // PUBLISHED app.wasm into one framebuffer via `display::mount_composition` —
    // no iframes, one shared canvas, focus-gated pointer routing, budget-capped.
    // Runs in the cartridge WORKER + watchdog (issue #77): a composed child is
    // untrusted wasm too, so it must be contained off the main thread. The worker
    // resolves each name's published app.wasm through the same compose round-trip
    // a recursive spawn uses (an unpublished name just stays a black cell). Works
    // on any origin.
    if let Some(names) = compose::compose_names() {
        root.set_inner_html(&templates::app_fullscreen(false).into_string());
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(err) = display::mount_composition(names).await {
                web_sys::console::warn_1(&JsValue::from_str(&format!("compose failed: {err:?}")));
            }
        });
        return Ok(());
    }

    // Embed mode short-circuit (?embed=1). Paints just the identity
    // card sized for inclusion in a parent iframe. Activated on any
    // host so apex and tenants alike can present themselves as
    // modules.
    if embed::has_embed_hint() {
        root.set_inner_html(
            "<main style=\"padding:24px;color:#7a8493;font:14px ui-monospace,Menlo,Consolas,monospace\">embed · loading…</main>",
        );
        let host_for_embed = host.clone();
        wasm_bindgen_futures::spawn_local(async move {
            embed::paint_embed(host_for_embed).await;
        });
        return Ok(());
    }

    // Explore mode short-circuit (?explore=1). A public directory of
    // every agent on the registry — works on any host.
    if has_explore_hint() {
        let host_for_explore = host.clone();
        root.set_inner_html(&templates::explore_chrome(&host_for_explore).into_string());
        dom::mark_ready();
        wasm_bindgen_futures::spawn_local(async move {
            paint_explore().await;
        });
        return Ok(());
    }

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

    // RPC endpoint mode. Any subdomain loaded with ?rpc=1 becomes a
    // headless agent that accepts lh-agent-call postMessages from other
    // agents. Starts the same agent session as the chat UI but renders
    // no transcript — inter-agent communication only.
    if agent_rpc::has_rpc_hint() {
        root.set_inner_html("<main style=\"padding:24px;color:#7a8493;font:14px ui-monospace,Menlo,Consolas,monospace\">rpc · loading…</main>");
        agent_rpc::install_rpc_listener()?;
        wasm_bindgen_futures::spawn_local(async move {
            agent_rpc::paint_rpc().await;
        });
        return Ok(());
    }

    // Invite links: stash any `?invite=CODE` now so it survives identity
    // creation + repaints; the paint paths redeem it once an identity is
    // resolvable. Skipped for signer/rpc modes (returned above).
    capture_invite_code();

    match &host {
        tenant::Host::Apex => {
            // Linked-device hand-off: a device that just paired redirects here
            // with `?link_device=<owner>` so the apex (this origin) records
            // which identity the device belongs to — without it the apex has
            // no master wallet to key on. Store the pointer, then continue to
            // `?then` (the subdomain) if present.
            if let Some(owner) = read_query_param("link_device") {
                root.set_inner_html(
                    "<main style=\"padding:48px;text-align:center;color:#7a8493;font:14px ui-monospace,Menlo,Consolas,monospace\">linking…</main>",
                );
                wasm_bindgen_futures::spawn_local(async move {
                    let _ = wallet_store::persist_linked_owner(&owner).await;
                    // Validate `then` is a bare DNS label before building the
                    // redirect URL. An unvalidated value would be an OPEN
                    // REDIRECT: `?then=evil.com%23` →
                    // `https://evil.com#.localharness.xyz/` navigates to evil.com.
                    if let Some(then) = read_query_param("then") {
                        let valid_label = !then.is_empty()
                            && then.len() <= 63
                            && then.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
                        if valid_label {
                            if let Some(window) = web_sys::window() {
                                let _ = window
                                    .location()
                                    .set_href(&format!("https://{then}.localharness.xyz/"));
                                return;
                            }
                        }
                    }
                    paint_apex(tenant::Host::Apex).await;
                });
                return Ok(());
            }
            // Seed-pull (local-seed-per-origin): a subdomain with no local
            // seed sent its top-level tab here to fetch the master seed
            // (the iframe path is dead on mobile). Seal it to the supplied
            // ephemeral key and navigate back. See `seed_pull`.
            if read_query_param("seed_export").is_some() {
                root.set_inner_html(
                    "<main style=\"padding:48px;text-align:center;color:#7a8493;font:14px ui-monospace,Menlo,Consolas,monospace\">linking this device…</main>",
                );
                wasm_bindgen_futures::spawn_local(async move {
                    seed_pull::handle_apex_export().await;
                });
                return Ok(());
            }
            // Option A device adoption: `localharness.xyz/?adopt=1#s=<ct>`.
            // A device scanning the "add a device" QR lands here. Render the
            // code-entry form; the encrypted seed rides in the URL fragment
            // (never sent to a server) and is read back from a hidden input
            // when the user submits the one-time code. Delegated listeners
            // are already installed above, so the form works.
            if read_query_param("adopt").is_some() {
                let ct_hex = read_fragment_param("s").unwrap_or_default();
                root.set_inner_html(&templates::adopt_join(&ct_hex).into_string());
                dom::mark_ready();
                return Ok(());
            }

            // Wallet load is async (OPFS). Show a single-line placeholder
            // rather than the full chrome so we don't flash the
            // pre-identity sidecar before we know whether a wallet exists.
            let host_for_apex = host.clone();
            root.set_inner_html(
                "<main style=\"padding:48px;text-align:center;color:#7a8493;font:14px ui-monospace,Menlo,Consolas,monospace\">localharness · loading…</main>",
            );
            wasm_bindgen_futures::spawn_local(async move {
                paint_apex(host_for_apex).await;
            });
            return Ok(());
        }
        tenant::Host::Tenant(name) => {
            // Seed-pull return leg: apex sealed the master seed to this
            // origin's ephemeral key. Import it into THIS origin's OPFS, then
            // paint the tenant normally — now with a LOCAL seed, so every
            // seed op runs locally and the iframe (dead on mobile) is unused.
            if read_query_param("seed_import").is_some() {
                root.set_inner_html(
                    "<main style=\"padding:48px;text-align:center;color:#7a8493;\
                     font:14px ui-monospace,Menlo,Consolas,monospace\">\
                     setting up this device…</main>",
                );
                let name = name.clone();
                let host_for_import = host_for_listeners.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    seed_pull::handle_tenant_import().await;
                    paint_tenant(host_for_import, name).await;
                });
                return Ok(());
            }
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

    // Full-app chrome (localhost, Vercel preview, etc.). Defer so the
    // async app-mode check (OPFS read) can take over before we paint the
    // workshop.
    root.set_inner_html(
        "<main style=\"padding:48px;text-align:center;color:#7a8493;font:14px ui-monospace,Menlo,Consolas,monospace\">localharness · loading…</main>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        // `Other` hosts (localhost / preview) have no on-chain name, so
        // only the local `app.rl` path applies. Dev/preview keeps the
        // [studio] escape on the fullscreen surface.
        if try_paint_app(true).await {
            return;
        }
        paint_workshop(&host).await;
    });
    Ok(())
}

/// Paint the full workshop chrome and run the usual key/history setup.
/// Shared by the `Other` host path (and any fallback that wants the IDE
/// without tenant verification).
async fn paint_workshop(host: &tenant::Host) {
    let Ok(doc) = dom::document() else { return };
    let Some(root) = doc.get_element_by_id("root") else { return };
    root.set_inner_html(&templates::chrome(host).into_string());
    dom::mark_ready();

    // Auto-redeem a pending `?invite=CODE` into the local credit identity.
    wasm_bindgen_futures::spawn_local(events::try_redeem_pending_invite(true));
    // No-funds onboarding CTA above the prompt (self-clears once funded).
    wasm_bindgen_futures::spawn_local(events::refresh_fund_banner());

    let has_key = if let Some(persisted_key) = key_store::load().await {
        if let Ok(Some(storage)) = dom::session_storage() {
            let _ = storage.set_item("gemini_api_key", &persisted_key);
        }
        true
    } else if let Ok(Some(storage)) = dom::session_storage() {
        storage.get_item("gemini_api_key").ok().flatten().is_some()
    } else {
        false
    };
    history::load_into_pending().await;
    notifications::load_inbox().await;
    opfs::refresh().await;
    // No onboarding key prompt: new accounts default to platform credits
    // (no Gemini key needed). BYOK is opt-in via admin → account.
    let _ = has_key;
}

/// Render a tenant subdomain. Three branches:
/// 1. Local `.lh_owner` marker → device thinks it owns the name. Paint
///    full chat app, kick verification in the background.
/// 2. No local marker but the name IS on-chain owned (by anyone) →
///    paint full chat app as a visitor; verification will recover the
///    visitor's address and the pricing/payment loop takes it from there.
/// 3. No local marker AND no on-chain owner → genuinely unclaimed;
///    paint the "claim this name?" prompt.
pub(crate) async fn paint_tenant(host: tenant::Host, name: String) {
    debuglog::log("paint_tenant");
    let Ok(doc) = dom::document() else { return };
    let Some(root) = doc.get_element_by_id("root") else { return };

    // Auto-redeem a pending `?invite=CODE` into the local credit identity.
    wasm_bindgen_futures::spawn_local(events::try_redeem_pending_invite(true));

    // Local-seed-per-origin: if this subdomain origin holds the master seed
    // (pulled in via `seed_pull`), load it into App state. With it present,
    // `verify.rs` runs every seed op locally and the cross-origin iframe
    // (dead on mobile) is never touched — and `chat::credit_signer` uses the
    // master wallet, so credits no longer fragment per origin.
    let local_wallet = wallet_store::load().await;
    APP.with(|cell| cell.borrow_mut().wallet = local_wallet);

    // The local hint = the on-chain owner address this device last PROVED
    // it controls. Authority is the chain (every load re-verifies below);
    // the hint only decides which face paints FIRST and is deleted the
    // moment the chain disagrees (`kick_verification` → `owner::forget`).
    let mut owner = owner::current_owner().await;
    // Apex sends users here with ?claim=1 to skip the
    // "claim this name?" interstitial — the user has already expressed
    // intent on the previous page. The name was just registered there, so
    // the chain already knows the owner; remember that address as the hint
    // (best-effort), then strip the param so a refresh does nothing weird.
    if owner.is_none() && has_claim_hint() {
        let registered = registry::owner_of_name(&name).await.ok().flatten();
        if let Some(addr) = registered {
            let _ = owner::remember(&addr).await;
            owner = Some(addr);
            strip_claim_hint();
        }
    }
    // Resolve the on-chain owner once — used for the unclaimed check AND
    // the owner-by-signer check below. (Only when there's no local claim.)
    let on_chain = if owner.is_none() {
        registry::owner_of_name(&name).await.ok().flatten()
    } else {
        None
    };
    if owner.is_none() && on_chain.is_none() {
        root.set_inner_html(&templates::unclaimed(&host, &name).into_string());
        dom::mark_ready();
        return;
    }

    // Owner-by-signer (linked device): if this device's LOCAL key is an
    // authorized signer of this name's TBA, treat the device as an owner.
    // Devices enrolled as signers on the name's MultiSigner TBA (under the
    // retired pairing flow) live there, so we must check the TBA — NOT the
    // NFT owner, which is normally an EOA with no isAuthorizedSigner (that
    // always returned false, so paired phones were wrongly treated as
    // visitors). Falls back to the on-chain owner for the consolidation case
    // where the owner itself is a TBA. ONE on-load on-chain read = the
    // source of truth; no polling.
    let signer_owner = if owner.is_none() {
        match chat::credit_address_existing().await {
            Some(my_addr) => {
                let mut ok = false;
                if let Ok(Some(tba)) = registry::tba_of_name(&name).await {
                    ok = registry::is_authorized_signer(&tba, &my_addr)
                        .await
                        .unwrap_or(false);
                }
                if !ok {
                    if let Some(oc) = on_chain.as_deref() {
                        ok = registry::is_authorized_signer(oc, &my_addr)
                            .await
                            .unwrap_or(false);
                    }
                }
                ok
            }
            None => false,
        }
    } else {
        false
    };

    // Two surfaces per subdomain:
    //  - PUBLIC FACE (fullscreen cartridge) — the visitor surface.
    //  - STUDIO (the workshop chrome below) — the owner surface.
    // The owner lands in the Studio by default and previews their public
    // face with `?view=public`; a visitor only ever sees the public face.
    // `owner.is_some()` is this device's local claim; `signer_owner` is the
    // on-chain consolidation case (a linked device controlling a TBA-owned
    // subdomain).
    let is_owner_device = owner.is_some() || signer_owner;
    let show_public_face = !is_owner_device || has_view_public_hint();
    if show_public_face {
        // Resolve the on-chain public-face choice (directory / app / html)
        // and paint it. Directory is the universal fallback, so this always
        // takes over.
        paint_public_face(&host, &name, is_owner_device).await;
        // A seed-bearing owner visiting from a device without the local
        // `.lh_owner` marker (e.g. a second device) lands on the public
        // face like a visitor. Verify in the background and, if the apex
        // signer proves ownership, send them to their studio (`?edit=1`).
        // Skipped when this device already claims ownership (a deliberate
        // `?view=public` preview must not bounce back).
        if !is_owner_device {
            let n = name.clone();
            wasm_bindgen_futures::spawn_local(async move {
                redirect_to_studio_if_owner(n).await;
            });
        }
        return;
    }

    // Local-seed-per-origin for the OWNER device too. We hold `.lh_owner`
    // for this name but, on desktop, OPFS is per-origin: the master seed
    // lives in the apex OPFS, not in this subdomain's. Without it,
    // `chat::credit_signer` / `credit_address_existing` fall back to a
    // per-origin device key (0 $LH) — so the Usage tab shows 0 and the
    // proxy gates the chat turn against an empty EOA even though the
    // owner's master EOA holds credits (correctly shown on apex/MAIN).
    // Pull the seed via the top-level apex round-trip so credits resolve to
    // the ONE master EOA on every subdomain. `maybe_auto_kick` is a no-op
    // when the seed is already local or already attempted this tab session,
    // so this navigates at most once and is self-healing (the round-trip
    // re-enters `paint_tenant` with the seed present). Skip when this
    // device has no master seed anywhere (a linked device acting via a
    // device key): the apex side returns `seed_import=none` and the studio
    // paints with the device key — at most one guarded redirect.
    if APP.with(|cell| cell.borrow().wallet.is_none())
        && seed_pull::maybe_auto_kick(&name).await
    {
        return; // navigating to the apex round-trip; it re-enters paint_tenant
    }

    // Paint the Studio — we own this name on this device (or a deliberate
    // preview fell through with nothing published).
    root.set_inner_html(&templates::chrome(&host).into_string());
    dom::mark_ready();

    // No-funds onboarding: surface the inline redeem CTA above the prompt
    // when this credit identity holds zero `$LH` (gated access means an
    // unfunded send would silently fail at the proxy). Fire-and-forget so
    // the chrome paints immediately; self-clears once funded.
    wasm_bindgen_futures::spawn_local(events::refresh_fund_banner());

    let has_key = if let Some(persisted_key) = key_store::load().await {
        if let Ok(Some(storage)) = dom::session_storage() {
            let _ = storage.set_item("gemini_api_key", &persisted_key);
        }
        true
    } else if let Ok(Some(storage)) = dom::session_storage() {
        storage.get_item("gemini_api_key").ok().flatten().is_some()
    } else {
        false
    };
    history::load_into_pending().await;
    notifications::load_inbox().await;
    opfs::refresh().await;

    if !has_key {
        // Best-effort: restore the owner's MAIN Gemini key from chain for
        // BYOK users that hold the seed (a new subdomain on the same device
        // reuses the MAIN's key — "the subdomain IS the primary owner").
        // No modal on miss: new accounts default to platform credits and
        // BYOK is opt-in via admin → account.
        let _ = events::try_auto_restore_gemini_key(&name).await;
        // Self-heal a STALE push subscription (PWA reinstall invalidates the
        // old endpoint; the chain kept serving it → every push died with an
        // FCM 410). Background, best-effort, no prompt.
        wasm_bindgen_futures::spawn_local(async {
            notifications::refresh_subscription_if_stale().await;
        });
        // HEADLESS: once permission is granted (one-time bell tap), keep THIS
        // device's address-keyed push sub published on every load so a READY-UP
        // broadcast always reaches it — no button needed after the first grant.
        wasm_bindgen_futures::spawn_local(async {
            notifications::auto_register_device_push().await;
        });
    }

    // Background: try to verify the visitor against the on-chain
    // owner via the apex iframe signer. Fire-and-forget so the
    // chrome paint doesn't block on a ~1-5s roundtrip; the pill
    // updates when the result lands. `painted_from_hint` is true when the
    // studio above was painted optimistically off `.lh_owner` — if the
    // chain now disagrees (Visitor/Unregistered), `kick_verification`
    // demotes: deletes the hint and repaints the public face.
    let painted_from_hint = owner.is_some();
    let host_for_verify = host.clone();
    wasm_bindgen_futures::spawn_local(async move {
        kick_verification(host_for_verify, name, painted_from_hint).await;
    });
}

/// Run `verify::verify_owner` for `name` and stash the result. The
/// pill in the chrome reflects whatever lands here.
///
/// The chain is authority. `painted_from_hint` means the studio was
/// painted optimistically off the `.lh_owner` hint:
/// - a `VerifiedOwner` proof refreshes the hint (`owner::remember`) so the
///   next load is fast;
/// - a `Visitor`/`Unregistered` verdict means ownership was lost or
///   transferred, so we DEMOTE — delete the hint (`owner::forget`) and
///   repaint the public face. A `Failed` (RPC/iframe hiccup) is NOT a
///   demotion: keep the optimistic studio rather than punish a transient.
async fn kick_verification(host: tenant::Host, name: String, painted_from_hint: bool) {
    // Cap the whole verification — its first step is an on-chain
    // `owner_of_name` read with NO transport timeout (browser fetch never
    // times out), so a black-holed RPC would otherwise leave the verify pill
    // stuck on "pending" forever. A timeout degrades to a `Failed` verdict,
    // which is treated as a transient (the optimistic studio stays painted).
    let verify_result = match net::with_timeout(verify::VERIFY_BUDGET_MS, verify::verify_owner(&name)).await {
        Ok(r) => r,
        Err(_) => Err("verification timed out (RPC unreachable)".to_string()),
    };

    // Self-correct the on-chain-derived hint before anything else.
    match &verify_result {
        Ok(verify::VerifyResult::VerifiedOwner { address }) => {
            let _ = owner::remember(address).await;
        }
        Ok(verify::VerifyResult::Visitor { .. })
        | Ok(verify::VerifyResult::Unregistered) => {
            if painted_from_hint {
                // The hint lied — the chain says we're not the owner.
                // Forget it and repaint the visitor surface in place.
                owner::forget().await;
                paint_public_face(&host, &name, false).await;
                return;
            }
        }
        Err(_) => {}
    }

    let outcome = match verify_result {
        Ok(verify::VerifyResult::VerifiedOwner { address }) => {
            VerifyState::Verified { address }
        }
        Ok(verify::VerifyResult::Visitor {
            owner_address,
            visitor_address,
        }) => VerifyState::Visitor {
            owner_address,
            visitor_address,
        },
        Ok(verify::VerifyResult::Unregistered) => VerifyState::Unregistered,
        Err(err) => VerifyState::Failed { reason: err },
    };
    APP.with(|cell| cell.borrow_mut().verify_state = outcome.clone());
    let html = templates::verify_pill(&outcome).into_string();
    dom::swap_outer("verify-pill", &html);

    // Surface the failure reason somewhere visible — the pill's
    // title tooltip alone isn't discoverable. Log to console too
    // for inspection across reloads.
    if let VerifyState::Failed { reason } = &outcome {
        dom::set_status(&format!("verify failed: {reason}"), true);
        web_sys::console::warn_1(&JsValue::from_str(&format!(
            "lh verify_owner failed: {reason}"
        )));
    }

    // Pricing — per-tenant OPFS, loaded once + stashed for chat send.
    let price = pricing::load().await.unwrap_or(0);
    APP.with(|cell| cell.borrow_mut().pricing_wei = Some(price));
    let is_owner = matches!(outcome, VerifyState::Verified { .. });

    // TBA + owner address — both needed for the agent tab.
    let on_chain = matches!(
        outcome,
        VerifyState::Verified { .. } | VerifyState::Visitor { .. }
    );
    let owner_addr: Option<String> = match &outcome {
        VerifyState::Verified { address } => Some(address.clone()),
        VerifyState::Visitor { owner_address, .. } => Some(owner_address.clone()),
        _ => None,
    };
    let mut tba_opt: Option<String> = None;
    if on_chain {
        if let Ok(Some(tba)) = registry::tba_of_name(&name).await {
            APP.with(|cell| cell.borrow_mut().tba_address = Some(tba.clone()));
            tba_opt = Some(tba);
        }
    }

    // Agent card: owner + TBA + $LH balance + pricing. Lives in the admin
    // Account tab now (folded in from the retired right rail), so stash the
    // HTML in App state and only swap the live DOM if the slot happens to
    // be present (admin already open). `header_admin_toggle` injects the
    // stash when the admin opens later.
    let card_html = if let (Some(tba), Some(owner)) = (&tba_opt, &owner_addr) {
        let lh_balance = registry::token_balance_of(tba).await.unwrap_or(0);
        templates::financial_card(&name, tba, owner, lh_balance, price, is_owner).into_string()
    } else {
        r#"<div id="financial-slot" class="financial-empty"></div>"#.to_string()
    };
    APP.with(|cell| cell.borrow_mut().financial_card_html = Some(card_html.clone()));
    if dom::by_id("financial-slot").is_some() {
        dom::swap_outer("financial-slot", &card_html);
    }
}

/// Paint the apex chrome. Reads (never creates) the master wallet,
/// then renders the unified claim flow. Fresh visitors and returning
/// visitors see the same surface — a name input + create button. The
/// wallet, if it doesn't exist yet, gets generated as a side effect of
/// the user's first claim submit (handled in `run_apex_claim`).
pub(crate) async fn paint_apex(host: tenant::Host) {
    debuglog::log("paint_apex (re-render apex → CTA)");
    let Ok(doc) = dom::document() else { return };
    let Some(root) = doc.get_element_by_id("root") else { return };

    let wallet = wallet_store::load().await;
    // Effective identity for the read-only on-chain views (agents list,
    // linked devices, MAIN): the master wallet if this device holds the
    // seed, else the linked-owner pointer a paired device recorded. The
    // claim form stays seed-gated (a linked device can't mint names).
    let mut addr_hex = wallet.as_ref().map(|w| w.address_hex());
    if addr_hex.is_none() {
        addr_hex = wallet_store::load_linked_owner().await;
    }
    APP.with(|cell| cell.borrow_mut().wallet = wallet);

    // Auto-redeem a pending `?invite=CODE` once an identity exists — on the
    // apex we never mint a device key for it (allow_generate=false), so the
    // post-create repaint of paint_apex is what actually credits the MAIN.
    wasm_bindgen_futures::spawn_local(events::try_redeem_pending_invite(false));

    root.set_inner_html(
        &templates::apex(&host, addr_hex.as_deref()).into_string(),
    );
    dom::mark_ready();

    // Volatile-storage (incognito) warning — the seed lives in OPFS, which a
    // private window can wipe on tab close. Detect async (a `persist()` request
    // round-trip) and fill the warning slot so a fresh visitor is warned BEFORE
    // minting a key they could silently lose (kit-qa #). Non-blocking. No seed
    // WRITE is in flight on a repaint, and `OpfsFilesystem::root_handle` now
    // never holds a borrow across an await, so this can't race the borrow.
    wasm_bindgen_futures::spawn_local(events::warn_if_storage_volatile());

    // Pre-fill the claim input + trigger the live-check if the user
    // landed here via `?prefill=<name>` (e.g. from a tenant subdomain's
    // "claim on-chain" CTA, or any external link).
    if let Some(prefill) = read_query_param("prefill") {
        let cleaned = tenant::sanitize(&prefill);
        if !cleaned.is_empty() {
            if let Some(input) = dom::input_by_id("apex-input") {
                input.set_value(&cleaned);
                if let Ok(event) = web_sys::Event::new("input") {
                    let _ = input.dispatch_event(&event);
                }
                let _ = input.focus();
            }
        }
    }

    // Fetch the "your agents" list only when there's an identity to
    // fetch for. On fresh visits the list stays empty — that's expected
    // and the placeholder div in the template covers it.
    if let Some(owner_addr) = addr_hex {
        // Show a loading placeholder while the on-chain list resolves —
        // the lookup can take a beat and an empty gap above the claim form
        // looked broken (per feedback). Only identity-having visitors hit
        // this path, so fresh visitors still see nothing.
        dom::swap_outer(
            "agents-list",
            r#"<div id="agents-list" class="agents-list"><p class="apex-fine">loading agents…</p></div>"#,
        );
        wasm_bindgen_futures::spawn_local(async move {
            // Timeout-capped: the "loading agents…" placeholder is up; a hung
            // RPC (browser fetch never times out) would freeze it there. A
            // timeout maps to the same error branch as a read failure.
            let listed = match net::read(registry::list_owned_tokens(&owner_addr)).await {
                Ok(r) => r,
                Err(_) => Err("on-chain read timed out".to_string()),
            };
            match listed {
                Ok(mut agents) => {
                    // MAIN lookup is best-effort — facet might not be
                    // cut on a given diamond, in which case the badge
                    // simply doesn't appear.
                    let main_id = registry::main_of(&owner_addr).await.unwrap_or(0);
                    // Pin the MAIN to the top — it's the owner's primary
                    // identity, so it leads the list regardless of mint
                    // order. The rest stay newest-first (list_owned_tokens
                    // already reverses).
                    if main_id != 0 {
                        if let Some(pos) =
                            agents.iter().position(|a| a.token_id == main_id)
                        {
                            let main = agents.remove(pos);
                            agents.insert(0, main);
                        }
                    }
                    let html = templates::agents_list(&agents, main_id).into_string();
                    dom::swap_outer("agents-list", &html);
                }
                Err(err) => {
                    // maud escapes `err` — an RPC node's error string is
                    // attacker-influencable, so never interpolate it raw.
                    dom::swap_outer(
                        "agents-list",
                        &maud::html! {
                            div #agents-list .agents-list {
                                p .apex-fine style="color:var(--error)" {
                                    "couldn't list agents: " (err)
                                }
                            }
                        }
                        .into_string(),
                    );
                }
            }
        });
    }
}

/// Paint the minimal signer chrome once we've checked for a wallet.
/// If the apex origin has no wallet yet, render a "no identity" notice
/// instead of conjuring one — the parent subdomain will see signing
/// requests rejected by [`signer::handle_message`] in that case.
pub(crate) async fn paint_signer() {
    let Ok(doc) = dom::document() else { return };
    let Some(root) = doc.get_element_by_id("root") else { return };
    match wallet_store::load().await {
        Some(wallet) => {
            let addr = wallet.address_hex();
            APP.with(|cell| cell.borrow_mut().wallet = Some(wallet));
            root.set_inner_html(&templates::signer_chrome(&addr).into_string());
        }
        None => {
            root.set_inner_html(&templates::signer_no_identity().into_string());
        }
    }

    // Tell the parent we're ready to receive sign requests.
    // The verify-side waits for this ping before posting the challenge
    // — avoids the race where parent posted before the wasm bundle
    // had finished loading + installed its postMessage listener.
    // Sent regardless of wallet presence so the parent can detect
    // "no identity" via the challenge's response error instead of
    // a generic timeout.
    if let Ok(window) = dom::window() {
        if let Ok(Some(parent)) = window.parent() {
            let ready = js_sys::Object::new();
            let _ = js_sys::Reflect::set(
                &ready,
                &JsValue::from_str("type"),
                &JsValue::from_str(signer_protocol::MSG_SIGNER_READY),
            );
            // Target "*" — the message carries no sensitive data, only
            // a presence ping. The PARENT enforces origin matching on
            // its receive side (it only accepts replies from
            // SIGNER_ORIGIN).
            let _ = parent.post_message(&ready.into(), "*");
        }
    }
}

/// Show the API key modal if it isn't already in the DOM.
pub(crate) fn show_api_key_modal() {
    let Ok(doc) = dom::document() else { return };
    if doc.get_element_by_id("api-key-modal").is_some() {
        return;
    }
    let Some(body) = doc.body() else { return };
    let _ = body.insert_adjacent_html(
        "beforeend",
        &templates::api_key_modal().into_string(),
    );
    if let Some(input) = dom::input_by_id("api-key-input") {
        let _ = input.focus();
    }
}

/// Format a wei value as a human-readable test-ETH string with up to
/// 6 decimals trimmed. Cosmetic only — the wei integer is what gets
/// signed and submitted on-chain.
pub(crate) fn format_wei_as_test_eth(wei: u128) -> String {
    const WEI_PER_ETH: u128 = 1_000_000_000_000_000_000;
    let whole = wei / WEI_PER_ETH;
    let frac_wei = wei % WEI_PER_ETH;
    if frac_wei == 0 {
        return whole.to_string();
    }
    let frac = (frac_wei * 1_000_000) / WEI_PER_ETH;
    format!("{whole}.{:06}", frac)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

/// Read a `?key=value` query parameter from the current URL, naive
/// implementation that avoids pulling a URL crate. Returns `None` if
/// the param is missing or empty.
pub(crate) fn read_query_param(key: &str) -> Option<String> {
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

/// Read a `key=value` pair out of the URL fragment (`#key=value&…`). Used
/// by Option A device adoption to recover the encrypted seed that rides in
/// `#s=<ciphertext>` — the fragment never leaves the browser, so it's the
/// right channel for the transport blob. No URI-decode: the value is hex.
pub(crate) fn read_fragment_param(key: &str) -> Option<String> {
    let window = dom::window().ok()?;
    let hash = window.location().hash().ok()?;
    let stripped = hash.trim_start_matches('#');
    for pair in stripped.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key && !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Capture an `?invite=CODE` redeem code into localStorage and strip it
/// from the URL (so a refresh can't replay it). Redemption itself fires
/// from each paint path via `events::try_redeem_pending_invite` once a
/// credit identity is resolvable. No-op when `?invite` is absent.
fn capture_invite_code() {
    let Some(code) = read_query_param("invite") else {
        return;
    };
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item("lh_pending_invite", &code);
    }
    // Invite links are standalone (`…/?invite=CODE`), so reducing the URL
    // to its pathname is the simplest clean-up.
    if let Some(window) = web_sys::window() {
        if let Ok(history) = window.history() {
            let path = window
                .location()
                .pathname()
                .unwrap_or_else(|_| "/".to_string());
            let _ = history.replace_state_with_url(
                &wasm_bindgen::JsValue::NULL,
                "",
                Some(&path),
            );
        }
    }
}

pub(crate) fn decode_uri_component(s: &str) -> String {
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

/// `true` iff `?explore=1` is in the URL — the public agent directory.
fn has_explore_hint() -> bool {
    let Ok(window) = dom::window() else { return false };
    let Ok(search) = window.location().search() else { return false };
    search.contains("explore=1")
}

/// Fetch the recent agents and swap them into the directory grid.
async fn paint_explore() {
    // Timeout-capped: the directory paints a "loading…" grid placeholder;
    // a hung RPC would otherwise freeze it there. A timeout maps to the same
    // error branch as a read failure (visible message, not a permanent spinner).
    let listed = match net::read(registry::list_recent_agents(60)).await {
        Ok(r) => r,
        Err(_) => Err("on-chain read timed out".to_string()),
    };
    match listed {
        Ok(agents) => {
            // Batch-fetch every agent's on-chain persona in ONE eth_call so
            // each card shows what the agent actually DOES (not just a name).
            // `personas_of` is index-aligned with `ids`, degrading any unset /
            // failed slot to None — the template then renders name-only.
            let ids: Vec<u64> = agents.iter().map(|(id, _)| *id).collect();
            let personas = registry::personas_of(&ids).await;
            dom::swap_outer(
                "explore-grid",
                &templates::explore_grid(&agents, &personas).into_string(),
            );
        }
        Err(err) => {
            // maud escapes `err` — an RPC node's error string is
            // attacker-influencable, so never interpolate it raw.
            dom::swap_inner(
                "explore-grid",
                &maud::html! {
                    span style="color:var(--muted)" { "couldn't load agents: " (err) }
                }
                .into_string(),
            );
        }
    }
}

/// `true` iff `?edit=1` is in the URL — forces the workshop chrome even
/// when an `app.rl` exists, so the owner can always get back to editing.
fn has_edit_hint() -> bool {
    let Ok(window) = dom::window() else { return false };
    let Ok(search) = window.location().search() else { return false };
    search.contains("edit=1")
}

/// Paint the **default public-face landing** for `name` — the surface a
/// visitor sees when no cartridge has been published. Fetches the owner,
/// the agent's TBA, the owner's MAIN name, and the owner's other agents
/// (siblings) from the registry, then renders `templates::public_landing`.
/// All reads are best-effort; missing data just omits that row.
async fn paint_public_landing(host: &tenant::Host, name: &str, owner_overlay: bool) {
    let _ = host;
    let owner = registry::owner_of_name(name).await.ok().flatten();
    let tba = registry::tba_of_name(name).await.ok().flatten();

    let mut main_name: Option<String> = None;
    let mut is_main = false;
    let mut siblings: Vec<registry::OwnedToken> = Vec::new();
    if let Some(addr) = owner.as_deref() {
        let main_id = registry::main_of(addr).await.unwrap_or(0);
        if main_id != 0 {
            if let Ok(m) = registry::name_of_id(main_id).await {
                if !m.is_empty() {
                    is_main = m == name;
                    // Only surface the MAIN as the owner link when it's a
                    // *different* subdomain — linking a name to itself is noise.
                    if !is_main {
                        main_name = Some(m);
                    }
                }
            }
        }
        siblings = registry::list_owned_tokens(addr).await.unwrap_or_default();
        siblings.retain(|t| t.name != name);
    }

    // Fetch every sibling's on-chain persona in ONE batched RPC POST (not N
    // serial round-trips). Aligned 1:1 with `siblings`; a missing/failed slot
    // is `None` and the card degrades to name-only. Cheap when empty.
    let sibling_ids: Vec<u64> = siblings.iter().map(|t| t.token_id).collect();
    let personas = registry::personas_of(&sibling_ids).await;

    let html = templates::public_landing(
        name,
        owner.as_deref(),
        tba.as_deref(),
        main_name.as_deref(),
        is_main,
        &siblings,
        &personas,
        owner_overlay,
    )
    .into_string();

    if let Ok(doc) = dom::document() {
        if let Some(root) = doc.get_element_by_id("root") {
            root.set_inner_html(&html);
        }
    }
}

/// Background check: if the apex signer proves this device controls the
/// on-chain owner of `name`, navigate to the studio (`?edit=1`). Lets a
/// seed-bearing owner reach their workshop from a device that has no
/// local `.lh_owner` marker, without exposing a studio door to visitors.
async fn redirect_to_studio_if_owner(name: String) {
    // Capped like `kick_verification` — a hung on-chain read inside
    // `verify_owner` would otherwise leave this background task pending
    // forever. A timeout maps to `Err`, which falls through to the
    // seed-pull recovery (the same as a genuine iframe failure).
    let verdict = match net::with_timeout(verify::VERIFY_BUDGET_MS, verify::verify_owner(&name)).await {
        Ok(r) => r,
        Err(_) => Err("verification timed out".to_string()),
    };
    match verdict {
        Ok(verify::VerifyResult::VerifiedOwner { address }) => {
            // Proven owner on a device without the local hint (e.g. a second
            // device): remember the proven address so the next load paints
            // the studio first instead of flashing the public face, then
            // bounce to the studio.
            let _ = owner::remember(&address).await;
            if let Ok(window) = dom::window() {
                let _ = window.location().set_search("edit=1");
            }
        }
        // The apex iframe couldn't prove ownership. On mobile this is the
        // PERSISTENT failure (partitioned cross-origin storage), not a
        // transient — so if this origin has no local seed, fetch it from
        // apex via the top-level round-trip. The apex only hands the seed
        // back if it actually owns this name, so a genuine visitor costs at
        // most one guarded redirect and learns nothing. A clean `Visitor`/
        // `Unregistered` verdict (iframe worked) does NOT land here.
        Err(_) => {
            seed_pull::maybe_auto_kick(&name).await;
        }
        Ok(_) => {}
    }
}

/// `true` iff `?view=public` is in the URL — the owner asking to preview
/// their public face from the studio.
fn has_view_public_hint() -> bool {
    let Ok(window) = dom::window() else { return false };
    let Ok(search) = window.location().search() else { return false };
    search.contains("view=public")
}

/// What a subdomain's public face resolves to right now.
enum PublicFace {
    /// The default profile/directory landing.
    Directory,
    /// A rustlite cartridge (compiled wasm) run fullscreen.
    Cartridge(Vec<u8>),
    /// An HTML page rasterized fullscreen.
    Html(String),
}

/// Read the device's local `app.rl` working copy and compile it. `None`
/// if absent/empty; logs + `None` on compile error (so the caller can
/// fall back rather than wedge).
async fn local_cartridge_wasm() -> Option<Vec<u8>> {
    let fs = shared_opfs();
    let bytes = fs.read("app.rl").await.ok().filter(|b| !b.is_empty())?;
    let src = String::from_utf8_lossy(&bytes).into_owned();
    match crate::rustlite::compile(&src) {
        Ok(w) => Some(w),
        Err(err) => {
            web_sys::console::warn_1(&JsValue::from_str(&format!(
                "app.rl compile failed, falling back: {err}"
            )));
            None
        }
    }
}

/// Read the device's local `index.html` working copy. `None` if absent/empty.
async fn local_public_html() -> Option<String> {
    let fs = shared_opfs();
    let bytes = fs.read("index.html").await.ok().filter(|b| !b.is_empty())?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Fetch the PUBLISHED on-chain `app.wasm` for another subdomain `name` — no
/// local working-copy fallback, since composing means showing what that
/// subdomain publishes to the world. `None` if the name is unregistered or has
/// published no app. Backs the iframe-free `?compose=` compositor.
async fn compose_module_wasm(name: &str) -> Option<Vec<u8>> {
    let id = registry::id_of_name(name).await.ok().filter(|&i| i != 0)?;
    registry::app_wasm_of(id).await.ok().flatten()
}

/// Cartridge bytes for the public face. When `prefer_local` (owner preview
/// only), the device's unpublished `app.rl` working copy wins so the owner
/// sees their edits; for a VISITOR `prefer_local` is false, so only the
/// PUBLISHED on-chain wasm is shown — never the owner-device's local draft.
async fn resolve_cartridge(id: Option<u64>, prefer_local: bool) -> Option<Vec<u8>> {
    if prefer_local {
        if let Some(w) = local_cartridge_wasm().await {
            return Some(w);
        }
    }
    match id {
        Some(i) => registry::app_wasm_of(i).await.ok().flatten(),
        None => None,
    }
}

/// Resolve the public face for tenant `name`. Reads the on-chain choice
/// (`directory` / `app` / `html`) and gathers content. `is_owner_preview`
/// (the owner viewing their own face via `?view=public`) prefers the device's
/// local working copy so unpublished edits show; a VISITOR (false) only ever
/// gets the PUBLISHED on-chain copy — the device-local OPFS draft must never
/// leak to visitors. An explicit choice with no content available falls back
/// to the directory; an UNSET choice infers "app if a published cartridge
/// exists, else directory" so subdomains that published a cartridge before the
/// picker shipped keep showing it.
async fn resolve_public_face(name: &str, is_owner_preview: bool) -> PublicFace {
    let id = registry::id_of_name(name).await.ok().filter(|&i| i != 0);
    let choice = match id {
        Some(i) => registry::public_face_of(i).await.ok().flatten(),
        None => None,
    };
    match choice.as_deref() {
        Some("directory") => PublicFace::Directory,
        Some("html") => {
            if is_owner_preview {
                if let Some(h) = local_public_html().await {
                    return PublicFace::Html(h);
                }
            }
            if let Some(i) = id {
                if let Ok(Some(bytes)) = registry::public_html_of(i).await {
                    return PublicFace::Html(String::from_utf8_lossy(&bytes).into_owned());
                }
            }
            PublicFace::Directory
        }
        // "app" or unset/legacy — prefer a cartridge, fall back to directory.
        _ => match resolve_cartridge(id, is_owner_preview).await {
            Some(w) => PublicFace::Cartridge(w),
            None => PublicFace::Directory,
        },
    }
}

/// Paint `app_fullscreen` chrome and run a cartridge into the root canvas.
/// `true` once it has taken over the page.
async fn paint_cartridge_fullscreen(wasm: &[u8], owner_overlay: bool) -> bool {
    let Ok(doc) = dom::document() else { return false };
    let Some(root) = doc.get_element_by_id("root") else { return false };
    root.set_inner_html(&templates::app_fullscreen(owner_overlay).into_string());
    if let Err(err) = display::run_in_root_canvas(wasm).await {
        web_sys::console::warn_1(&JsValue::from_str(&format!("app run failed: {err:?}")));
    }
    true
}

/// Paint `app_fullscreen` chrome and rasterize an HTML page into the root
/// canvas. `true` once it has taken over the page.
fn paint_html_fullscreen(html: &str, owner_overlay: bool) -> bool {
    let Ok(doc) = dom::document() else { return false };
    let Some(root) = doc.get_element_by_id("root") else { return false };
    root.set_inner_html(&templates::app_fullscreen(owner_overlay).into_string());
    if let Err(err) = display::render_html_in_root_canvas(html) {
        web_sys::console::warn_1(&JsValue::from_str(&format!("html render failed: {err:?}")));
    }
    true
}

/// Paint the resolved public face for tenant `name`. Always takes over the
/// page (directory is the universal fallback). `owner_overlay` shows the
/// `[studio]` escape AND signals an owner preview (`?view=public`), so the
/// resolver may prefer the device's local working copy; a visitor
/// (`owner_overlay == false`) only ever sees the published on-chain copy.
async fn paint_public_face(host: &tenant::Host, name: &str, owner_overlay: bool) {
    match resolve_public_face(name, owner_overlay).await {
        PublicFace::Cartridge(w) => {
            paint_cartridge_fullscreen(&w, owner_overlay).await;
        }
        PublicFace::Html(h) => {
            paint_html_fullscreen(&h, owner_overlay);
        }
        PublicFace::Directory => {
            paint_public_landing(host, name, owner_overlay).await;
        }
    }
    dom::mark_ready();
}

/// Local-only fullscreen cartridge for `Host::Other` (localhost / preview),
/// which has no on-chain name — only the device's `app.rl` working copy
/// applies. Tenants go through `paint_public_face` (choice-aware) instead.
/// `true` if it took over; a compile error / missing file → `false` so the
/// caller falls through to the workshop.
async fn try_paint_app(owner_overlay: bool) -> bool {
    if has_edit_hint() {
        return false;
    }
    let Some(wasm) = local_cartridge_wasm().await else {
        return false;
    };
    paint_cartridge_fullscreen(&wasm, owner_overlay).await
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
