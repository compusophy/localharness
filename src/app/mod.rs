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

use crate::filesystem::{Filesystem, OpfsFilesystem};
use crate::Agent;

mod chat;
mod compose;
// pub(crate) so the `run_cartridge` builtin tool can hand a compiled
// cartridge to the framebuffer (the agent→display loop).
pub(crate) mod display;
pub(crate) mod agent_config;
mod dom;
mod embed;
mod events;
mod history;
mod key_store;
mod opfs;
mod owner;
mod pricing;
mod signer;
mod sponsor;
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
    /// Total Gemini tokens used this session (cumulative across turns),
    /// updated by `chat::run_send` after each turn; shown in the Usage tab.
    pub(crate) total_tokens: u64,
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
            pending_history: None,
            wallet: None,
            verify_state: VerifyState::Pending,
            tba_address: None,
            pricing_wei: None,
            financial_card_html: None,
            total_tokens: 0,
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

    // Install the x402 signing hook so the backend `call_agent` tool can
    // pay a callee that demands `$LH` — signs with the local credit key
    // (never the iframe). See [[x402_hook]].
    crate::x402_hook::install(std::rc::Rc::new(|ch: crate::x402_hook::X402Challenge|
        -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<crate::x402_hook::X402Payment, String>>>> {
        Box::pin(async move {
            let (signer, from) = chat::credit_signer()
                .await
                .ok_or_else(|| "no identity to pay from".to_string())?;
            let sig = crate::registry::sign_x402(
                &signer,
                &from,
                &ch.to,
                ch.value_wei,
                0,
                ch.valid_before,
                &ch.nonce,
            )?;
            Ok(crate::x402_hook::X402Payment {
                from,
                valid_after: 0,
                valid_before: ch.valid_before,
                signature: sig,
            })
        })
    }));

    // Compose mode short-circuit (?compose=name1,name2,...). Renders a
    // grid of embed-mode iframes — the minimal host harness for the
    // composable-subdomain primitive. Works on any origin.
    if let Some(names) = compose::compose_names() {
        compose::paint_compose(names)?;
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

    match &host {
        tenant::Host::Apex => {
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
            // Device-pairing short-circuit: `?pair=CODE` means a second
            // device is enrolling itself as a signer for this subdomain.
            // Paint the minimal join chrome — no identity, no chat.
            if read_query_param("pair").is_some() {
                root.set_inner_html(&templates::pair_join(name).into_string());
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
    opfs::refresh().await;
    if !has_key {
        show_api_key_modal();
    }
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
        // No local-device claim. Check on-chain — if someone owns this
        // name in the registry, the visitor isn't claiming, they're
        // browsing. Paint the chat chrome and let verification figure
        // out whether they're the owner or a paying visitor.
        let on_chain = registry::owner_of_name(&name).await.ok().flatten();
        if on_chain.is_none() {
            root.set_inner_html(&templates::unclaimed(&host, &name).into_string());
            return;
        }
        // Fall through to chrome paint as visitor.
    }

    // Two surfaces per subdomain:
    //  - PUBLIC FACE (fullscreen cartridge) — the visitor surface.
    //  - STUDIO (the workshop chrome below) — the owner surface.
    // The owner lands in the Studio by default and previews their public
    // face with `?view=public`; a visitor only ever sees the public face.
    // So we only paint the public face for a visitor, OR for the owner
    // when they explicitly asked to preview it. `owner.is_some()` is this
    // device's local ownership claim (refined later by verification).
    let is_owner_device = owner.is_some();
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

    // Paint the Studio — we own this name on this device (or a deliberate
    // preview fell through with nothing published).
    root.set_inner_html(&templates::chrome(&host).into_string());

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
    opfs::refresh().await;

    if !has_key {
        // Before prompting, try to auto-restore the owner's MAIN Gemini
        // key from chain (works on any device that holds the seed). A new
        // subdomain on the same device reuses the MAIN's key with no
        // prompt — "the subdomain IS the primary owner". Falls through to
        // the modal on a device without the seed (e.g. a phone linked by
        // device key only).
        if !events::try_auto_restore_gemini_key(&name).await {
            show_api_key_modal();
        }
    }

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
    let Ok(doc) = dom::document() else { return };
    let Some(root) = doc.get_element_by_id("root") else { return };

    let wallet = wallet_store::load().await;
    let addr_hex = wallet.as_ref().map(|w| w.address_hex());
    APP.with(|cell| cell.borrow_mut().wallet = wallet);

    root.set_inner_html(
        &templates::apex(&host, addr_hex.as_deref()).into_string(),
    );

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
            match registry::list_owned_tokens(&owner_addr).await {
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
                    dom::swap_outer(
                        "agents-list",
                        &format!(
                            r#"<div id="agents-list" class="agents-list"><p class="apex-fine" style="color:var(--error)">couldn't list agents: {err}</p></div>"#
                        ),
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
                &JsValue::from_str("lh-signer-ready"),
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
    match registry::list_recent_agents(60).await {
        Ok(agents) => {
            dom::swap_outer("explore-grid", &templates::explore_grid(&agents).into_string());
        }
        Err(err) => {
            dom::swap_inner(
                "explore-grid",
                &format!("<span style=\"color:var(--muted)\">couldn't load agents: {err}</span>"),
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

    let html = templates::public_landing(
        name,
        owner.as_deref(),
        tba.as_deref(),
        main_name.as_deref(),
        is_main,
        &siblings,
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
    if let Ok(verify::VerifyResult::VerifiedOwner { .. }) = verify::verify_owner(&name).await {
        if let Ok(window) = dom::window() {
            let _ = window.location().set_search("edit=1");
        }
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

/// Cartridge bytes for `name`: the local `app.rl` working copy first (so
/// the owner previews unpublished edits), else the on-chain published wasm.
async fn resolve_cartridge(id: Option<u64>) -> Option<Vec<u8>> {
    if let Some(w) = local_cartridge_wasm().await {
        return Some(w);
    }
    match id {
        Some(i) => registry::app_wasm_of(i).await.ok().flatten(),
        None => None,
    }
}

/// Resolve the public face for tenant `name`. Reads the on-chain choice
/// (`directory` / `app` / `html`) and gathers content (local working copy
/// first, else the published copy). An explicit choice with no content
/// available falls back to the directory; an UNSET choice infers "app if a
/// cartridge exists, else directory" so subdomains that published a
/// cartridge before the picker shipped keep showing it.
async fn resolve_public_face(name: &str) -> PublicFace {
    let id = registry::id_of_name(name).await.ok().filter(|&i| i != 0);
    let choice = match id {
        Some(i) => registry::public_face_of(i).await.ok().flatten(),
        None => None,
    };
    match choice.as_deref() {
        Some("directory") => PublicFace::Directory,
        Some("html") => {
            if let Some(h) = local_public_html().await {
                return PublicFace::Html(h);
            }
            if let Some(i) = id {
                if let Ok(Some(bytes)) = registry::public_html_of(i).await {
                    return PublicFace::Html(String::from_utf8_lossy(&bytes).into_owned());
                }
            }
            PublicFace::Directory
        }
        // "app" or unset/legacy — prefer a cartridge, fall back to directory.
        _ => match resolve_cartridge(id).await {
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
/// `[studio]` escape (owner preview only).
async fn paint_public_face(host: &tenant::Host, name: &str, owner_overlay: bool) {
    match resolve_public_face(name).await {
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
