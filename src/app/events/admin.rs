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
    let registry_addr = crate::encoding::parse_address(crate::app::registry::REGISTRY_ADDRESS)?;
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
        use crate::filesystem::Filesystem;
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
    let registry_addr = crate::encoding::parse_address(crate::app::registry::REGISTRY_ADDRESS)?;
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

/// The admin "notifications" row: permission prompt (this click is the user
/// gesture browsers require) → Web Push subscription → sponsored on-chain
/// publish under `keccak256("localharness.push_sub")`. After this, the
/// proxy's scheduler worker can push job results with the tab CLOSED, and
/// the agent's `notify` tool fires without a mid-turn permission prompt.
/// The header notification bell — a DIRECT user gesture (unlike the cartridge
/// subscribe tap, whose worker→main postMessage loses user activation so its
/// permission prompt never fires on mobile). Enables Web Push for THIS device
/// keyed by its ADDRESS (works with no MAIN identity) and opens the panel. This
/// is the path that actually lets a phone register to be pinged.
pub(super) fn notif_bell_pressed() {
    // The bell is the notification LOG. Tap = open the log + clear the badge.
    let items = crate::app::notifications::bell_items();
    dom::swap_outer(
        "notif-bell-panel",
        &templates::notif_list_panel(&items, None, false).into_string(),
    );
    crate::app::notifications::clear_bell_badge();
    // Register this device for Web Push as a side effect of this real tap (the
    // gesture the permission prompt needs — the cartridge tap can't prompt).
    // On SUCCESS: silent (the user sees only their log). On FAILURE: surface the
    // reason in the panel + console so a broken link (permission denied / SW /
    // publish tx) is visible, not swallowed.
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(e) = crate::app::notifications::enable_device_push().await {
            web_sys::console::error_1(&wasm_bindgen::JsValue::from_str(&format!(
                "[push] device registration failed: {e}"
            )));
            let items = crate::app::notifications::bell_items();
            dom::swap_outer(
                "notif-bell-panel",
                &templates::notif_list_panel(&items, Some(&format!("⚠ {e}")), false).into_string(),
            );
        }
    });
}

pub(super) fn enable_notifications_pressed() {
    wasm_bindgen_futures::spawn_local(async move {
        let msg = "notify-msg";
        dom::swap_inner(msg, "<span style=\"color:var(--muted)\">enabling…</span>");
        match crate::app::notifications::enable_and_publish().await {
            Ok(_tx) => dom::swap_inner(
                msg,
                "<span style=\"color:var(--muted)\">notifications on — push subscription published on-chain</span>",
            ),
            Err(e) => dom::swap_inner(msg, &dom::msg_span(dom::Msg::Error, &e)),
        }
    });
}

/// Fire a LOCAL test notification (+ vibration) so the user verifies the
/// permission + service-worker render path in one tap, without scheduling
/// anything. This does NOT exercise the closed-tab Web Push leg (that needs
/// a real push from the proxy); it proves the device-side half.
pub(super) fn test_notification_pressed() {
    wasm_bindgen_futures::spawn_local(async move {
        let msg = "notify-msg";
        crate::app::notifications::vibrate(200);
        match crate::app::notifications::ensure_permission().await {
            Ok(true) => match crate::app::notifications::show(
                "localharness test",
                "notifications are working on this device",
            )
            .await
            {
                Ok(()) => dom::swap_inner(
                    msg,
                    "<span style=\"color:var(--muted)\">test notification sent — check your shade</span>",
                ),
                Err(e) => dom::swap_inner(msg, &dom::msg_span(dom::Msg::Error, &e)),
            },
            Ok(false) => dom::swap_inner(
                msg,
                &dom::msg_span(
                    dom::Msg::Error,
                    "notification permission is blocked — allow notifications for this site in the browser settings, then retry",
                ),
            ),
            Err(e) => dom::swap_inner(msg, &dom::msg_span(dom::Msg::Error, &e)),
        }
    });
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
    let body = match crate::app::tenant::current() {
        crate::app::tenant::Host::Apex => templates::admin_dropdown_apex().into_string(),
        crate::app::tenant::Host::Tenant(_) | crate::app::tenant::Host::Other(_) => {
            templates::admin_dropdown_tenant().into_string()
        }
    };
    dom::swap_outer("header-admin-panel", &body);

    // Inject the stashed agent card (folded in from the retired right rail)
    // into the Account tab's #financial-slot. Built by kick_verification.
    if let Some(card) = crate::app::APP.with(|c| c.borrow().financial_card_html.clone()) {
        if dom::by_id("financial-slot").is_some() {
            dom::swap_outer("financial-slot", &card);
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
    // Recurring jobs list (ScheduleFacet) — no-ops if the slot isn't mounted
    // (no wallet) or no identity exists yet. Same fire-and-forget shape as
    // the credits pill so the dropdown paints immediately.
    wasm_bindgen_futures::spawn_local(async move {
        super::schedule::refresh_jobs_list().await;
    });
    // Open bounties list (BountyFacet) — same fire-and-forget shape; no-ops if
    // the slot isn't mounted (no wallet).
    wasm_bindgen_futures::spawn_local(async move {
        super::bounty::refresh_bounty_list().await;
    });
    // The caller's guilds (GuildFacet) — same fire-and-forget shape; no-ops if
    // the slot isn't mounted (no wallet) or no identity exists yet.
    wasm_bindgen_futures::spawn_local(async move {
        super::guild::refresh_guild_list().await;
    });
    // Device/signer management lives at the apex only.
    if matches!(crate::app::tenant::current(), crate::app::tenant::Host::Apex) {
        wasm_bindgen_futures::spawn_local(async move {
            super::devices::refresh_signer_list().await;
        });
    }

    // Pre-fill api key from sessionStorage (sync) then refresh from
    // OPFS (async). Same pattern as the old in-chrome key restore.
    if matches!(
        crate::app::tenant::current(),
        crate::app::tenant::Host::Tenant(_) | crate::app::tenant::Host::Other(_)
    ) {
        if let Ok(Some(storage)) = dom::session_storage() {
            if let Ok(Some(cached)) = storage.get_item("gemini_api_key") {
                if let Some(input) = dom::input_by_id("key") {
                    input.set_value(&cached);
                    super::refresh_keymeta();
                }
            }
        }
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(persisted) = crate::app::key_store::load().await {
                if let Some(input) = dom::input_by_id("key") {
                    input.set_value(&persisted);
                    super::refresh_keymeta();
                }
            }
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
                use crate::filesystem::Filesystem;
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
            use crate::filesystem::Filesystem;
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
}

/// Switch the active admin tab by flipping the `tab-<name>` class on
/// `#admin-dialog` (CSS shows the matching `.panel-<name>`), and sync the
/// `.active` state on the tab buttons.
pub(super) fn show_admin_tab(name: &str) {
    let Some(dialog) = dom::by_id("admin-dialog") else { return };
    let mut cls: Vec<String> = dialog
        .class_name()
        .split_whitespace()
        .filter(|c| !c.starts_with("tab-"))
        .map(String::from)
        .collect();
    cls.push(format!("tab-{name}"));
    dialog.set_class_name(&cls.join(" "));

    for tab in ["agent", "account", "usage", "feedback"] {
        let Some(el) = dom::by_id(&format!("admin-tab-btn-{tab}")) else { continue };
        let c = el.class_name();
        let mut classes: Vec<&str> = c.split_whitespace().filter(|x| *x != "active").collect();
        if tab == name {
            classes.push("active");
        }
        el.set_class_name(&classes.join(" "));
    }
}
