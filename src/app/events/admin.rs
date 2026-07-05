//! Admin panel — config handlers (prompt / allowlist / api key / x402 price)
//! plus the header dropdown shell, tabs, and usage slot.

use wasm_bindgen::JsCast;

use crate::app::{dom, templates};

/// Persist the textarea content as the per-origin custom system
/// prompt AND publish it as the on-chain persona. The local file drives
/// THIS tab's sessions; the on-chain persona slot is what the hosted
/// x402 `ask_agent` path answers from — saving only locally let owners
/// believe callers would see their prompt when they never did. Off a
/// registered subdomain (localhost/preview) the publish is skipped.
/// Empty/whitespace-only content deletes the file, reverting to the
/// bundle's default (the on-chain slot is left as-is).
pub(super) fn save_prompt_pressed() {
    let Some(textarea) = dom::textarea_by_id("prompt-input") else { return };
    let content = textarea.value();
    dom::swap_inner(
        "prompt-msg",
        "<span style=\"color:var(--muted)\">saving…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        match crate::app::system_prompt::save(&content).await {
            Ok(()) => {
                let trimmed = content.trim().to_string();
                if trimmed.is_empty() {
                    dom::swap_inner(
                        "prompt-msg",
                        &dom::msg_span(dom::Msg::Accent, "✓ saved · using default on next session"),
                    );
                    return;
                }
                let summary = match publish_persona_onchain(&trimmed).await {
                    Ok(true) => {
                        "✓ saved + published on-chain · takes effect on next session".to_string()
                    }
                    Ok(false) => "✓ saved · takes effect on next session".to_string(),
                    Err(e) => format!("✓ saved locally · on-chain publish failed: {e}"),
                };
                dom::swap_inner(
                    "prompt-msg",
                    &dom::msg_span(dom::Msg::Accent, &summary),
                );
            }
            Err(err) => {
                dom::swap_inner(
                    "prompt-msg",
                    &dom::msg_span(dom::Msg::Error, &err.to_string()),
                );
            }
        }
    });
}

/// Publish `text` to this subdomain's on-chain persona slot (the same
/// sponsored `setMetadata` path as the `set_persona` self-edit tool).
/// `Ok(false)` = not on a registered subdomain, publish skipped.
async fn publish_persona_onchain(text: &str) -> Result<bool, String> {
    let Some(tenant) = crate::app::tenant::current_name() else {
        return Ok(false);
    };
    let token_id = match crate::app::registry::id_of_name(&tenant).await {
        Ok(id) if id != 0 => id,
        Ok(_) => return Ok(false),
        Err(e) => return Err(format!("id_of_name: {e}")),
    };
    let (_, owner) = crate::app::tenant::current_tenant_owner().await?;
    let registry_addr = crate::encoding::parse_address(crate::app::registry::REGISTRY_ADDRESS())?;
    let call = crate::tempo_tx::TempoCall {
        to: registry_addr,
        value_wei: 0,
        input: crate::app::registry::encode_set_persona(token_id, text),
    };
    let gas = crate::app::gas::set_metadata_gas(text.len());
    super::run_sponsored_tempo_call(&owner, vec![call], gas, "publish persona")
        .await
        .map(|_| true)
}

pub(super) fn save_tool_allowlist_pressed() {
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
        match crate::app::tool_allowlist::save(&enabled).await {
            Ok(()) => {
                let summary = crate::app::tool_allowlist::summary(&enabled);
                dom::swap_inner(
                    "tool-allowlist-msg",
                    &dom::msg_span(dom::Msg::Accent, &format!("✓ saved · {summary} · takes effect on next session")),
                );
            }
            Err(err) => {
                dom::swap_inner(
                    "tool-allowlist-msg",
                    &dom::msg_span(dom::Msg::Error, &err.to_string()),
                );
            }
        }
    });
}

pub(super) fn reset_tool_allowlist_pressed() {
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
        match crate::app::tool_allowlist::save(&[]).await {
            Ok(()) => {
                dom::swap_inner(
                    "tool-allowlist-msg",
                    "<span style=\"color:var(--accent)\">✓ reset · all tools enabled · takes effect on next session</span>",
                );
            }
            Err(err) => {
                dom::swap_inner(
                    "tool-allowlist-msg",
                    &dom::msg_span(dom::Msg::Error, &err.to_string()),
                );
            }
        }
    });
}

/// Save the API key from the centered modal, then dismiss the modal.
pub(super) fn save_api_key_pressed() {
    // BYOK is owner/admin-only (on-chain #60.2): refuse a key from a public
    // visitor on someone else's agent. (The modal is also withheld from
    // visitors; this is the matching dispatch-layer guard.)
    if crate::app::is_visitor() {
        dom::swap_inner(
            "api-key-msg",
            "<span style=\"color:var(--error)\">only the owner can set a key</span>",
        );
        return;
    }
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
        crate::app::key_store::save(&value).await;
        crate::app::opfs::refresh().await;
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
        if let Some(name) = crate::app::tenant::current_name() {
            super::key_sync::auto_sync_gemini_key(name, value).await;
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

/// Save this agent's per-call x402 price (decimal `$LH` → wei in
/// `.lh_x402_price`) AND publish it on-chain — the hosted `ask_agent`
/// gate enforces the on-chain price, so a local-only save would be a
/// price nobody pays. Empty / 0 clears both (callers then pay the
/// platform default, `registry::DEFAULT_ASK_PRICE_WEI`).
pub(super) fn save_x402_price_pressed() {
    let Some(input) = dom::input_by_id("x402-price-input") else {
        return;
    };
    let raw = input.value().trim().to_string();
    wasm_bindgen_futures::spawn_local(async move {
        let fs = crate::app::shared_opfs();
        let local: Result<u128, String> = async {
            let wei = if raw.is_empty() {
                0
            } else {
                crate::encoding::parse_token_amount(&raw)
                    .ok_or_else(|| format!("'{raw}' is not a $LH amount"))?
            };
            if wei == 0 {
                let _ = fs.delete(".lh_x402_price").await;
            } else {
                fs.write_atomic(".lh_x402_price", wei.to_string().as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Ok(wei)
        }
        .await;
        let wei = match local {
            Ok(wei) => wei,
            Err(e) => {
                dom::swap_inner(
                    "x402-price-msg",
                    &dom::msg_span(dom::Msg::Error, &format!("save failed: {e}")),
                );
                return;
            }
        };
        // Local state is already written; a publish failure leaves the old
        // on-chain price live for callers, so the message must say PARTIAL
        // (mirrors save_prompt_pressed) — "save failed" here would hide a
        // local/on-chain divergence the prefill then displays as saved.
        match publish_x402_price_onchain(wei).await {
            Ok(true) => dom::swap_inner(
                "x402-price-msg",
                "<span style=\"color:var(--muted)\">saved + published on-chain</span>",
            ),
            Ok(false) => dom::swap_inner(
                "x402-price-msg",
                "<span style=\"color:var(--muted)\">saved</span>",
            ),
            Err(e) => dom::swap_inner(
                "x402-price-msg",
                &dom::msg_span(
                    dom::Msg::Error,
                    &format!("saved locally · on-chain publish failed: {e}"),
                ),
            ),
        }
    });
}

/// Publish the advertised per-call price to the on-chain slot the hosted
/// `ask_agent` gate reads. `Ok(false)` = not on a registered subdomain.
async fn publish_x402_price_onchain(wei: u128) -> Result<bool, String> {
    let Some(tenant) = crate::app::tenant::current_name() else {
        return Ok(false);
    };
    let token_id = match crate::app::registry::id_of_name(&tenant).await {
        Ok(id) if id != 0 => id,
        Ok(_) => return Ok(false),
        Err(e) => return Err(format!("id_of_name: {e}")),
    };
    let (_, owner) = crate::app::tenant::current_tenant_owner().await?;
    let registry_addr = crate::encoding::parse_address(crate::app::registry::REGISTRY_ADDRESS())?;
    let call = crate::tempo_tx::TempoCall {
        to: registry_addr,
        value_wei: 0,
        input: crate::app::registry::encode_set_x402_price(token_id, wei),
    };
    let gas = crate::app::gas::set_metadata_gas(40);
    super::run_sponsored_tempo_call(&owner, vec![call], gas, "publish x402 price")
        .await
        .map(|_| true)
}

/// The header notification bell — a DIRECT user gesture (unlike the cartridge
/// subscribe tap, whose worker→main postMessage loses user activation so its
/// permission prompt never fires on mobile): permission prompt → Web Push
/// subscription → enrollment in the proxy's OFF-CHAIN push store (POST
/// /api/push-sub, keyed by this device's ADDRESS — works with no MAIN
/// identity, NO on-chain write). After this, the proxy's notify/broadcast/
/// scheduler workers can push with the tab CLOSED, and the agent's `notify`
/// tool fires without a mid-turn permission prompt. This is the path that
/// actually lets a phone register to be pinged.
///
/// Whether the bell dropdown is currently showing.
pub(super) fn notif_panel_open() -> bool {
    dom::by_id("notif-bell-panel")
        .map(|e| !e.has_attribute("hidden"))
        .unwrap_or(false)
}

/// Close the bell dropdown (second bell tap, ESC, or any outside click).
pub(super) fn close_notif_panel() {
    dom::swap_outer(
        "notif-bell-panel",
        &templates::notif_list_panel(&crate::app::notifications::bell_items(), None, true, false)
            .into_string(),
    );
}

/// Whether the header feedback-bug dropdown (#36) is currently showing.
pub(super) fn feedback_panel_open() -> bool {
    dom::by_id("feedback-panel")
        .map(|e| !e.has_attribute("hidden"))
        .unwrap_or(false)
}

/// Close the feedback dropdown (second bug tap, ESC, or any outside click) —
/// re-render it hidden. Mirrors `close_notif_panel`.
pub(super) fn close_feedback_panel() {
    dom::swap_outer("feedback-panel", &templates::feedback_panel(true).into_string());
}

/// Toggle the feedback dropdown: open it (and pull focus to the textarea) or, if
/// already open, close it. Same toggle archetype as the notif bell.
pub(super) fn toggle_feedback_panel() {
    if feedback_panel_open() {
        close_feedback_panel();
        return;
    }
    close_all_header_overlays(); // mutually exclusive — close notif/admin/brand first
    dom::swap_outer("feedback-panel", &templates::feedback_panel(false).into_string());
    dom::focus_first_in("feedback-panel");
}

/// Whether the top-left `localharness` brand menu is open. It is a native
/// `<details class="brand-menu">` disclosure, so its open state IS the `open`
/// attribute the browser toggles on summary-click.
pub(super) fn brand_menu_open() -> bool {
    dom::document()
        .ok()
        .and_then(|d| d.query_selector(".brand-menu[open]").ok().flatten())
        .is_some()
}

/// Close the brand menu (ESC or any outside click), mirroring the bell/admin
/// dropdowns. Native `<details>` does NOT auto-dismiss, so we clear `open`
/// ourselves — a plain attribute mutation, no new listener / DOM tree.
pub(super) fn close_brand_menu() {
    if let Some(el) = dom::document()
        .ok()
        .and_then(|d| d.query_selector(".brand-menu[open]").ok().flatten())
    {
        let _ = el.remove_attribute("open");
    }
}

/// Whether the header admin (cog) panel is currently open — true iff its dialog
/// is in the DOM (`header_admin_toggle` swaps it in, `header_admin_close` out).
pub(super) fn header_admin_open() -> bool {
    dom::by_id("admin-dialog").is_some()
}

/// The header overlays (notif bell · feedback · admin cog · brand menu) are
/// MUTUALLY EXCLUSIVE — exactly one open at a time. Each open path calls this
/// first so a bell tap never opens BEHIND an already-open admin panel and two
/// panels never stack. Every close is an idempotent hidden-swap, so closing an
/// already-closed panel is a harmless no-op.
pub(super) fn close_all_header_overlays() {
    close_notif_panel();
    close_feedback_panel();
    header_admin_close();
    close_brand_menu();
}

/// Neutralize every LIVE inline admin card (#36 chat-native admin): swap each
/// `#admin-card-<slug>` in the transcript for an id-free "superseded" note, so
/// the fixed ids the section handlers target (`#model-msg`,
/// `#public-face-status`, `#redeem-code`, …) exist at most ONCE. Runs before
/// the settings sheet opens AND before a new card mounts — whichever admin
/// surface opened LAST owns the ids. Idempotent (a missing card is a no-op).
pub(super) fn retire_admin_cards() {
    for topic in crate::router::AdminTopic::ALL {
        let id = format!("admin-card-{}", topic.slug());
        if dom::by_id(&id).is_some() {
            dom::swap_outer(
                &id,
                &templates::admin_chat_card_retired(topic.title()).into_string(),
            );
        }
    }
}

/// [clear all] tapped: re-render the OPEN panel with the inline yes/cancel
/// confirm (no JS alert — on-chain feedback). Nothing is cleared yet.
pub(super) fn notif_clear_all_pressed() {
    dom::swap_outer(
        "notif-bell-panel",
        &templates::notif_list_panel(&crate::app::notifications::bell_items(), None, false, true)
            .into_string(),
    );
}

/// [cancel] in the clear confirm: re-render the OPEN panel without the confirm.
pub(super) fn notif_clear_cancelled() {
    dom::swap_outer(
        "notif-bell-panel",
        &templates::notif_list_panel(&crate::app::notifications::bell_items(), None, false, false)
            .into_string(),
    );
}

/// [yes] in the clear confirm: actually empty the inbox.
pub(super) fn notif_clear_confirmed() {
    crate::app::notifications::clear_all();
}

pub(super) fn notif_bell_pressed() {
    // TOGGLE: a second tap closes the log (ESC and outside clicks do too —
    // see the delegated listeners). Closing must NOT re-run the push
    // registration side effect below.
    if notif_panel_open() {
        close_notif_panel();
        return;
    }
    close_all_header_overlays(); // mutually exclusive — close feedback/admin/brand first
    // The bell is the notification LOG. Tap = open the log + clear the badge.
    // The status line surfaces the push enrolled/not-enrolled state up front
    // (telemetry #40: silent enrollment left users with no way to see that
    // closed-tab pushes were never going to arrive).
    let items = crate::app::notifications::bell_items();
    dom::swap_outer(
        "notif-bell-panel",
        &templates::notif_list_panel(
            &items,
            Some(crate::app::notifications::bell_status_line()),
            false,
            false,
        )
        .into_string(),
    );
    crate::app::notifications::clear_bell_badge();
    // Register this device for Web Push as a side effect of this real tap (the
    // gesture the permission prompt needs — the cartridge tap can't prompt).
    // The enroll now VERIFIES the sub landed in the store; surface the verified
    // outcome (enrolled ✓ / ⚠ reason) in the panel if it's still open.
    wasm_bindgen_futures::spawn_local(async move {
        let note = match crate::app::notifications::enable_device_push().await {
            Ok(msg) => msg,
            Err(e) => {
                web_sys::console::error_1(&wasm_bindgen::JsValue::from_str(&format!(
                    "[push] device registration failed: {e}"
                )));
                format!("⚠ {e}")
            }
        };
        if notif_panel_open() {
            let items = crate::app::notifications::bell_items();
            dom::swap_outer(
                "notif-bell-panel",
                &templates::notif_list_panel(&items, Some(&note), false, false).into_string(),
            );
        }
    });
}

/// Flip off-chain telemetry (auto error reports) on/off for this device and
/// reflect it in the button label. Reports are redacted on-device; default on.
pub(super) fn toggle_telemetry_pressed() {
    let now_on = !crate::app::telemetry::enabled();
    crate::app::telemetry::set_enabled(now_on);
    dom::swap_inner(
        "telemetry-toggle",
        if now_on { "telemetry: on" } else { "telemetry: off" },
    );
}

/// Trigger the browser's PWA install prompt from INSIDE the app: boot.js
/// stashes `beforeinstallprompt` on `window.__lhInstall`; this click (a user
/// gesture) calls `.prompt()` on it. When the stash is empty the app is
/// either already installed or the browser doesn't expose the prompt
/// (iOS Safari) — say which path applies instead of failing silently.
pub(super) fn install_app_pressed() {
    wasm_bindgen_futures::spawn_local(async move {
        let msg = "install-msg";
        let window = web_sys::window().expect("window");
        let stash = js_sys::Reflect::get(&window, &"__lhInstall".into()).ok();
        let evt = stash.filter(|v| !v.is_null() && !v.is_undefined());
        match evt {
            Some(evt) => {
                let prompt = js_sys::Reflect::get(&evt, &"prompt".into()).ok();
                match prompt.and_then(|p| p.dyn_into::<js_sys::Function>().ok()) {
                    Some(f) => {
                        let _ = f.call0(&evt);
                        dom::swap_inner(
                            msg,
                            "<span style=\"color:var(--muted)\">follow the browser's install dialog</span>",
                        );
                    }
                    None => dom::swap_inner(
                        msg,
                        &dom::msg_span(dom::Msg::Error, "install prompt unavailable"),
                    ),
                }
            }
            None => {
                // Either already installed (Chrome won't re-offer) or the
                // browser never exposes the prompt (iOS Safari).
                dom::swap_inner(
                    msg,
                    "<span style=\"color:var(--muted)\">already installed, or this \
                     browser hides the prompt — use the browser menu's install / \
                     add-to-home-screen entry</span>",
                );
            }
        }
    });
}

/// Toggle the header admin dropdown. Origin determines content —
/// apex shows seed reveal + import + reset, tenant has the gemini
/// api key input + reset. After opening, pre-fill the api key from
/// sessionStorage / OPFS so the user sees their existing key
/// (admin opens and closes constantly; the input is fresh DOM each time).
pub(super) fn header_admin_toggle() {
    // Real toggle: a second cog tap closes it (matches the bell/feedback).
    if header_admin_open() {
        header_admin_close();
        return;
    }
    close_all_header_overlays(); // mutually exclusive — close notif/feedback/brand first
    retire_admin_cards(); // inline admin cards yield their fixed ids to the sheet
    let body = match crate::app::tenant::current() {
        crate::app::tenant::Host::Apex => templates::admin_dropdown_apex().into_string(),
        crate::app::tenant::Host::Tenant(_) | crate::app::tenant::Host::Other(_) => {
            templates::admin_dropdown_tenant().into_string()
        }
    };
    dom::remember_focus();
    dom::swap_outer("header-admin-panel", &body);
    dom::focus_first_in("header-admin-panel");

    // Inject the stashed agent card (folded in from the retired right rail)
    // into the Account tab's #financial-slot. Built by kick_verification.
    if let Some(card) = crate::app::APP.with(|c| c.borrow().financial_card_html.clone()) {
        if dom::by_id("financial-slot").is_some() {
            dom::swap_outer("financial-slot", &card);
            // The card was stashed pre-session (builtin-only count); if a
            // session has since started, swap in the live tool count.
            if let Some(n) = crate::app::APP.with(|c| c.borrow().agent_tool_count) {
                dom::swap_outer("tools-count", &templates::tools_count_span(n));
            }
        }
    }

    // Credit balance — on EVERY host (apex AND subdomains). Credits are
    // master-EOA-scoped, so a subdomain shows the SAME balance as the apex; the
    // old apex-only gate is exactly why subdomains showed a blank "…" / "—".
    // Fire-and-forget so the dropdown paints immediately; the pill resolves from
    // "…" to "N LH".
    wasm_bindgen_futures::spawn_local(async move {
        super::refresh_credits_pill().await;
    });
    // Device/signer management lives at the apex only.
    if matches!(crate::app::tenant::current(), crate::app::tenant::Host::Apex) {
        wasm_bindgen_futures::spawn_local(async move {
            super::devices::refresh_signer_list().await;
        });
    }

    // On a tenant/other host, prefill the agent-config fields (prompt, x402
    // price, tool allowlist) + the model / public-face status into the (still
    // collapsed) `agent` group so they're ready the moment the user expands it.
    // The collapsed `<details>` keeps its children in the DOM, so every id below
    // still resolves. (BYOK key prefill is gone — there's no `#key` field in the
    // sheet; the gemini key lives in its own api-key modal.)
    if matches!(
        crate::app::tenant::current(),
        crate::app::tenant::Host::Tenant(_) | crate::app::tenant::Host::Other(_)
    ) {
        wasm_bindgen_futures::spawn_local(async move {
            // Restore the saved custom prompt into the textarea so the
            // user can edit instead of re-typing.
            if let Some(prompt) = crate::app::system_prompt::load().await {
                if let Some(textarea) = dom::textarea_by_id("prompt-input") {
                    textarea.set_value(&prompt);
                }
            }
            // Prefill the x402 price (stored as wei → shown as decimal LH).
            // Must round-trip what `save_x402_price_pressed` parses — the old
            // integer division showed a saved 0.1 as "0", and re-saving that
            // "0" would silently delete the price.
            {
                if let Ok(bytes) = crate::app::shared_opfs().read(".lh_x402_price").await {
                    if let Some(wei) = String::from_utf8(bytes)
                        .ok()
                        .and_then(|s| s.trim().parse::<u128>().ok())
                    {
                        if let Some(input) = dom::input_by_id("x402-price-input") {
                            input.set_value(&crate::app::format_wei_as_test_eth(wei));
                        }
                    }
                }
            }
            if let Some(allowed) = crate::app::tool_allowlist::load().await {
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
                let summary = crate::app::tool_allowlist::summary(&allowed);
                dom::swap_inner("tool-allowlist-status", &summary);
            } else {
                dom::swap_inner("tool-allowlist-status", "all tools enabled");
            }
            refresh_public_face_status().await;
            super::credits::refresh_model_selector().await;
        });
    }
}

/// Read the subdomain's current on-chain public-face choice and reflect it
/// in the `#public-face-status` slot. No-op off a tenant or if the slot
/// isn't mounted.
pub(super) async fn refresh_public_face_status() {
    let Some(name) = crate::app::tenant::current_name() else { return };
    if dom::by_id("public-face-status").is_none() {
        return;
    }
    // Timeout-capped so a dead RPC resolves to the directory-default label
    // instead of leaving the placeholder text up forever.
    let face = match crate::app::net::read(crate::app::registry::id_of_name(&name)).await {
        Ok(Ok(id)) if id != 0 => crate::app::net::read(crate::app::registry::public_face_of(id))
            .await
            .ok()
            .and_then(Result::ok)
            .flatten(),
        _ => None,
    };
    // Surface local-only working copies: an `app.rl`/`index.html` on this
    // device that visitors can't see until published. Binary state
    // (published vs local only) — no byte-level staleness diffing.
    let label: String = match face.as_deref() {
        Some("app") => "currently: app · published ✓".into(),
        Some("html") => "currently: html · published ✓".into(),
        _ => {
            let fs = crate::app::shared_opfs();
            let has_app = fs.read("app.rl").await.map(|v| !v.is_empty()).unwrap_or(false);
            let has_html =
                fs.read("index.html").await.map(|v| !v.is_empty()).unwrap_or(false);
            if has_app {
                "currently: directory · app.rl local only — publish to share".into()
            } else if has_html {
                "currently: directory · index.html local only — publish to share".into()
            } else {
                "currently: directory (default)".into()
            }
        }
    };
    dom::swap_inner("public-face-status", &label);
}

pub(super) fn header_admin_close() {
    dom::swap_outer(
        "header-admin-panel",
        r#"<div id="header-admin-panel" hidden></div>"#,
    );
    dom::restore_focus();
}
