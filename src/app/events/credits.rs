//! Credits / funding — fund banner, model access + selection, local-model
//! download, redeem codes, and invites.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::app::{dom, templates};
use crate::encoding::bytes_to_hex_str;


/// Show or hide the inline no-funds funding banner (`#fund-banner` in the
/// terminal). Credit access is now GATED — a session costs `$LH` and the
/// daily allowance is disabled — so an identity with zero `$LH` (zero wallet
/// balance + zero meter) can't reach the model and would otherwise hit a
/// silent proxy rejection on first send. When that's the case, surface a
/// one-click redeem CTA right above the prompt; once funded, clear it.
///
/// No-ops gracefully: if the banner slot isn't in the DOM (apex, public
/// face) there's nothing to fill, and a missing identity (no wallet/device
/// key yet) leaves the banner empty rather than nagging a marketing visit.
/// BYOK users (own key) aren't gated on `$LH`, so the banner stays hidden
/// for them too.
pub(crate) async fn refresh_fund_banner() {
    // Only meaningful where the terminal chrome exists.
    if dom::by_id("fund-banner").is_none() {
        return;
    }
    // BYOK reaches the model without `$LH` — don't show a funding nag.
    let is_credits = local_storage()
        .and_then(|s| s.get_item("lh_model_access").ok().flatten())
        .map(|m| m != "byok")
        .unwrap_or(true);
    if !is_credits {
        dom::swap_inner("fund-banner", "");
        return;
    }
    // Resolve the credit identity WITHOUT minting one (an unfunded marketing
    // visit shouldn't generate a device key just to be told it's broke).
    let Some(addr) = crate::app::chat::credit_address_existing().await else {
        dom::swap_inner("fund-banner", "");
        return;
    };
    let wallet = crate::app::registry::token_balance_of(&addr).await.unwrap_or(0);
    let meter = crate::app::registry::credit_balance_of(&addr).await.unwrap_or(0);
    if wallet == 0 && meter == 0 {
        dom::swap_inner("fund-banner", &templates::fund_banner_body().into_string());
    } else {
        dom::swap_inner("fund-banner", "");
    }
}

/// Flip platform-credits vs BYOK, persist it, and repaint the section.
pub(super) fn run_set_model_access(mode: String) {
    if let Some(storage) =
        web_sys::window().and_then(|w| w.local_storage().ok().flatten())
    {
        let _ = storage.set_item("lh_model_access", &mode);
    }
    // Repaint the admin credits section if it's open.
    if dom::by_id("credits-section").is_some() {
        dom::swap_outer(
            "credits-section",
            &crate::app::templates::admin_credits_section().into_string(),
        );
    }
    // If the api-key modal happens to be open (BYOK-without-key path),
    // switching to credits dismisses it. No terminal status text — credits
    // is the default and the account tab holds the controls.
    if mode == "credits" {
        if let Some(el) = dom::by_id("api-key-modal") {
            if let Some(parent) = el.parent_element() {
                let _ = parent.remove_child(&el);
            }
        }
    }
    wasm_bindgen_futures::spawn_local(async {
        super::refresh_credits_pill().await;
    });
}

/// Persist the chosen LLM model id (`.lh_model`) and reflect the active
/// button in the selector. The change takes effect on the NEXT session
/// start (`chat::start_session` reads `.lh_model`), so a turn already
/// streaming keeps its backend — note that in `#model-msg`.
pub(super) fn run_set_model(model: String) {
    wasm_bindgen_futures::spawn_local(async move {
        crate::app::model::save(&model).await;
        refresh_model_selector().await;
        let label = crate::app::model::MODELS
            .iter()
            .find(|(id, _)| *id == model)
            .map(|(_, l)| *l)
            .unwrap_or("model");
        dom::swap_inner(
            "model-msg",
            &format!("{label} — applies on your next message"),
        );
    });
}

/// Ungated HF CDN URLs for the local Gemma 3 270M model files (the `unsloth`
/// mirror — no license click-through, CORS-permissive across the
/// huggingface.co → cas-bridge.xethub.hf.co redirect chain).
const LOCAL_WEIGHTS_URL: &str =
    "https://huggingface.co/unsloth/gemma-3-270m/resolve/main/model.safetensors";
const LOCAL_TOKENIZER_URL: &str =
    "https://huggingface.co/unsloth/gemma-3-270m/resolve/main/tokenizer.json";

/// OPFS destinations for the downloaded files. MUST match the paths the local
/// backend reads (`backends::local::connection::{WEIGHTS_PATH, TOKENIZER_PATH}`)
/// — kept as literals here so the download works whether or not the heavy
/// `local` feature is compiled into this bundle.
const LOCAL_WEIGHTS_OPFS: &str = ".lh_local_model.safetensors";
const LOCAL_TOKENIZER_OPFS: &str = ".lh_local_tokenizer.json";

/// Download the in-browser local model (Gemma 3 270M weights + tokenizer) from
/// the HF CDN into OPFS, streaming with a byte-progress message. One-time opt-in
/// — once the files are in OPFS the local backend loads them on session start.
pub(super) fn run_download_local_model() {
    use futures_util::StreamExt as _;
    wasm_bindgen_futures::spawn_local(async move {
        let fs = crate::app::shared_opfs();

        // Fetch one URL, streaming chunks into a buffer and reporting progress
        // into `#local-model-msg`, then persist to OPFS via write_atomic.
        async fn fetch_to_opfs(
            fs: &crate::filesystem::SharedFilesystem,
            url: &str,
            opfs_path: &str,
            label: &str,
        ) -> Result<(), String> {
            let resp = reqwest::Client::new()
                .get(url)
                .send()
                .await
                .map_err(|e| format!("fetch {label}: {e}"))?;
            if !resp.status().is_success() {
                return Err(format!("fetch {label}: HTTP {}", resp.status().as_u16()));
            }
            let total = resp.content_length();
            let mut buf: Vec<u8> = Vec::with_capacity(total.unwrap_or(0) as usize);
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| format!("download {label}: {e}"))?;
                buf.extend_from_slice(&chunk);
                let got_mb = buf.len() / (1024 * 1024);
                let msg = match total {
                    Some(t) => {
                        let pct = (buf.len() as f64 / t as f64 * 100.0) as u32;
                        format!("downloading {label}: {got_mb} MB ({pct}%)")
                    }
                    None => format!("downloading {label}: {got_mb} MB"),
                };
                dom::swap_inner("local-model-msg", &msg);
            }
            fs.write_atomic(opfs_path, &buf)
                .await
                .map_err(|e| format!("save {label}: {e}"))?;
            Ok(())
        }

        dom::swap_inner("local-model-msg", "starting download…");
        let result = async {
            fetch_to_opfs(&fs, LOCAL_TOKENIZER_URL, LOCAL_TOKENIZER_OPFS, "tokenizer").await?;
            fetch_to_opfs(&fs, LOCAL_WEIGHTS_URL, LOCAL_WEIGHTS_OPFS, "weights").await?;
            Ok::<(), String>(())
        }
        .await;
        match result {
            Ok(()) => dom::swap_inner(
                "local-model-msg",
                "local model ready — select Local (Gemma) and send a message",
            ),
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("local model download: {e}")));
                dom::swap_inner("local-model-msg", &dom::msg_span(dom::Msg::Error, &e));
            }
        }
    });
}

/// Mark the persisted model's button `active` in `#model-selector-row`.
/// No-op when the selector isn't mounted. Mirrors `refresh_public_face_status`
/// (async-fill after the synchronous template paint).
pub(super) async fn refresh_model_selector() {
    if dom::by_id("model-selector-row").is_none() {
        return;
    }
    let chosen = crate::app::model::load().await;
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Ok(buttons) = doc.query_selector_all("#model-selector-row button[data-model]") {
            for i in 0..buttons.length() {
                if let Some(el) = buttons.get(i) {
                    let btn: web_sys::Element = JsCast::unchecked_into(el);
                    let is_active = btn.get_attribute("data-model").as_deref() == Some(&chosen);
                    btn.set_class_name(if is_active { "ghost active" } else { "ghost" });
                }
            }
        }
    }
}

/// Redeem a one-time code from the admin credits section (`#redeem-code`),
/// writing status into `#credits-msg`.
pub(super) fn redeem_code_pressed() {
    redeem_from("redeem-code", "credits-msg");
}

/// Redeem a one-time code from the inline no-funds banner
/// (`#fund-redeem-code`), writing status into `#fund-msg`. Same sponsored
/// `redeem` path as the admin field — just a different input/message slot.
pub(super) fn redeem_banner_pressed() {
    redeem_from("fund-redeem-code", "fund-msg");
}

/// Shared redeem flow — local key signs, sponsor pays. Reads the code from
/// `input_id`, reports into `msg_id`, then re-funds the meter + refreshes
/// the balance pill and the no-funds banner. Used by both the admin credits
/// field and the inline funding banner so there's ONE redeem path.
fn redeem_from(input_id: &'static str, msg_id: &'static str) {
    let Some(input) = dom::input_by_id(input_id) else { return };
    let code = input.value().trim().to_string();
    if code.is_empty() {
        return;
    }
    dom::swap_inner(
        msg_id,
        "<span style=\"color:var(--muted)\">redeeming…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            // No sponsor_rate_guard here: a redeem requires a valid, unused,
            // owner-loaded single-use code, so it's inherently un-spammable
            // (the guard was the one thing differing from the invite-link
            // path, which redeems fine). Keeps manual + invite identical.
            let (signer, _) = crate::app::chat::credit_signer()
                .await
                .ok_or_else(|| "no identity".to_string())?;
            let fee_payer = crate::app::sponsor::signer()?;
            crate::app::registry::redeem_sponsored(
                &signer,
                &fee_payer,
                &code,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
        }
        .await;
        match result {
            Ok(_) => {
                dom::swap_inner(
                    msg_id,
                    "<span style=\"color:var(--muted)\">redeemed</span>",
                );
                // Move the redeemed $LH straight into the per-request meter so
                // it's billable + the balance reflects it now (not next turn).
                crate::app::chat::ensure_credit_meter().await;
                super::refresh_credits_pill().await;
                // Now-funded → drop the no-funds banner (if it was up).
                refresh_fund_banner().await;
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("redeem: {e}")));
                dom::swap_inner(
                    msg_id,
                    &dom::msg_span(dom::Msg::Error, &format!("redeem failed: {e}")),
                );
            }
        }
    });
}

/// First-time onboarding redeem — the fresh-visitor `invite_onboarding`
/// surface. The user types an invite code; this ensures a credit identity
/// EXISTS (via `credit_signer`, which generates + persists a device key on
/// first use). That generation is the user's EXPLICIT redeem action — not
/// silent generation on a marketing visit — so the no-auto-create gate
/// holds: no wallet is conjured until the user deliberately clicks redeem
/// with a code. If one already exists it's reused (no second seed).
///
/// Accepts the invite escrow on-chain via the SAME `accept_invite_sponsored`
/// path the `?invite=` auto-redeem uses (bearer `inv-…` codes); a non-`inv-`
/// code falls through to `redeem_sponsored` (owner-minted) for symmetry with
/// `try_redeem_pending_invite`. On success it re-paints the apex so the
/// now-funded visitor sees the claim-a-name surface. Empty input is a silent
/// no-op (no explanatory-validation prose).
pub(super) fn redeem_invite_onboard_pressed() {
    let Some(input) = dom::input_by_id("invite-onboard-input") else {
        return;
    };
    let code = input.value().trim().to_string();
    if code.is_empty() {
        return;
    }
    // SINGLE-FLIGHT: a second press while a flow runs is ignored (mashing a
    // slow button must not spawn parallel identity creations).
    let Some(flow_guard) = super::onboard_flow_begin() else {
        return;
    };
    let msg_id = "invite-onboard-msg";
    let is_invite = code.starts_with("inv-");
    // STAGE-TAGGED progress + hard timeouts: every await below is bounded, so
    // a flaky mobile connection / a wedged storage API shows WHICH stage died
    // instead of "redeeming…" forever (the iPhone stuck-redeem report).
    dom::swap_inner(
        msg_id,
        "<span style=\"color:var(--muted)\">creating identity…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        let flow_guard = flow_guard; // released on every exit path
        let result = async {
            // Explicit user action → generating the device/credit key here is
            // ALLOWED (not silent). Reuses `credit_signer` (master wallet if
            // present, else load-or-generate the local key) so a returning
            // user with a seed doesn't get a second identity.
            let (signer, _) = crate::app::net::with_timeout(
                15_000,
                crate::app::chat::credit_signer(),
            )
            .await
            .map_err(|_| "identity setup timed out — reload and try again".to_string())?
            .ok_or_else(|| "no identity".to_string())?;
            let fee_payer = crate::app::sponsor::signer()?;
            crate::app::debuglog::log("onboard: identity ready — sending sponsored claim");
            dom::swap_inner(
                "invite-onboard-msg",
                "<span style=\"color:var(--muted)\">accepting on-chain…</span>",
            );
            let send = async {
                if is_invite {
                    crate::app::registry::accept_invite_sponsored(
                        &signer,
                        &fee_payer,
                        &code,
                        crate::app::registry::ALPHA_USD_ADDRESS,
                    )
                    .await
                } else {
                    crate::app::registry::redeem_sponsored(
                        &signer,
                        &fee_payer,
                        &code,
                        crate::app::registry::ALPHA_USD_ADDRESS,
                    )
                    .await
                }
            };
            crate::app::net::with_timeout(45_000, send)
                .await
                .map_err(|_| {
                    "network timed out — check your connection and tap redeem again".to_string()
                })?
        }
        .await;
        match result {
            Ok(_) => {
                // Land them on platform credits (the default) so the new $LH
                // is the model-access path, and move it into the meter now.
                if let Some(s) = local_storage() {
                    let _ = s.set_item("lh_model_access", "credits");
                }
                dom::swap_inner(
                    msg_id,
                    &dom::msg_span(dom::Msg::Accent, "redeemed — $LH added"),
                );
                crate::app::chat::ensure_credit_meter().await;
                // Re-paint the apex on a FRESH executor tick (not inline): the
                // visitor now has an identity, so `paint_apex` renders the
                // claim-a-name surface (+ agents list). Deferring avoids the iOS
                // re-entrant-`Task::run` "RefCell already borrowed" panic — see
                // `super::defer_onboard_repaint`. The guard rides along so a
                // re-press stays blocked until the surface is up.
                super::defer_onboard_repaint(flow_guard, async {
                    crate::app::paint_apex(crate::app::tenant::Host::Apex).await;
                });
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("invite redeem: {e}")));
                dom::swap_inner(
                    msg_id,
                    &dom::msg_span(
                        dom::Msg::Error,
                        "couldn't redeem (it may be used or expired)",
                    ),
                );
            }
        }
    });
}

/// Buy `$LH` with a card via an in-app Stripe **Elements** modal (no redirect).
/// Reads a USD amount from `#buy-usd`, builds the SAME proxy auth token the model
/// path uses (local key personal-signs `localharness-proxy:<addr>:<ts>`), fetches
/// a PaymentIntent `client_secret` + id from the proxy, swaps in the branded
/// modal, and mounts the compact Stripe Elements form into it
/// (`web/stripe-embed.js` → Express Checkout + Payment Element). On payment the
/// shim POSTs `/stripe/finalize` for an instant mint; the proxy webhook is the
/// durable backstop. Both mint `$LH` to THIS identity. Empty/invalid amount is a
/// silent no-op.
pub(super) fn buy_lh_pressed(onboarding: bool) {
    // Amount: the admin field if present, else a fixed $2 — the onboarding
    // "create agent · $2" path has no `#buy-usd` input. $2 = 200 $LH at the
    // $1 = 100 $LH rate (200 starter messages; claiming a name costs 1 $LH).
    // `onboarding` re-paints the apex on a successful mint so a fresh buyer
    // lands on the (now-funded) name-claim input instead of staying put.
    let cents = match dom::input_by_id("buy-usd") {
        Some(input) => match parse_usd_cents(input.value().trim()) {
            Some(c) => c,
            None => return,
        },
        None => 200,
    };
    if onboarding {
        // INSTANT FEEDBACK (the core fix for "nothing happens"): synchronously,
        // BEFORE any await, replace the "create agent · $2" button in place with
        // the inline checkout card showing "preparing secure checkout…". The
        // card already carries the `#lh-pay-region` mount ids, so there's no
        // modal to open later — the shim mounts straight into it.
        dom::swap_outer("apex-onboard", &templates::onboard_checkout().into_string());
        wasm_bindgen_futures::spawn_local(async move {
            match start_checkout_embedded(cents).await {
                Ok((client_secret, payment_intent)) => {
                    // Mount Stripe's native elements into the INLINE region (same
                    // ids as the modal). Minting is driven by the JS status
                    // watcher (`lhWatchPayment`) which calls back into wasm
                    // (`lh_payment_succeeded` → `finalize_after_payment`) ONLY on
                    // success — no pre-payment wasm async loop (the iOS WebKit
                    // BorrowError fix). The freshly signed token is minted there.
                    let opts = serde_json::json!({ "clientSecret": client_secret }).to_string();
                    call_js("lhBuyLh", Some(&opts));
                    // Form is up → clear the interstitial line.
                    dom::swap_inner("onboard-checkout-msg", "");
                    call_js(
                        "lhWatchPayment",
                        Some(
                            &serde_json::json!({
                                "payment_intent": payment_intent,
                                "onboarding": true,
                                "lh_label": lh_label(cents),
                            })
                            .to_string(),
                        ),
                    );
                }
                Err(e) => {
                    web_sys::console::warn_1(&JsValue::from_str(&format!("buy $LH: {e}")));
                    // Swap the onboarding card back so the user can retry.
                    dom::swap_outer("apex-onboard", &crate::landing::create_wallet_cta().into_string());
                    dom::swap_inner(
                        "onboard-msg",
                        &dom::msg_span(dom::Msg::Error, "couldn't start checkout — try again"),
                    );
                }
            }
        });
        return;
    }
    // ADMIN "buy $LH" path (modal) — unchanged.
    // Status slot: the admin `#buy-msg`, falling back to the pre-claim
    // `#fund-msg` so the affordance shows "opening checkout…" too.
    let msg_id = if dom::by_id("buy-msg").is_some() { "buy-msg" } else { "fund-msg" };
    dom::swap_inner(
        msg_id,
        "<span style=\"color:var(--muted)\">opening checkout…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        match start_checkout_embedded(cents).await {
            Ok((client_secret, payment_intent)) => {
                open_buy_modal(&lh_label(cents));
                // The shim only needs the client_secret (mounts the native
                // Stripe elements). Minting is driven by the JS status watcher
                // (`lhWatchPayment`) which calls back into wasm only on success
                // (`lh_payment_succeeded` → `finalize_after_payment`) with a
                // freshly signed token — no pre-payment wasm async loop (iOS fix).
                let opts = serde_json::json!({ "clientSecret": client_secret }).to_string();
                call_js("lhBuyLh", Some(&opts));
                dom::swap_inner(msg_id, "");
                // Watch the PaymentIntent in JS; on success, mint via the wasm
                // export. No wasm loop remains; this spawned task can end.
                call_js(
                    "lhWatchPayment",
                    Some(
                        &serde_json::json!({
                            "payment_intent": payment_intent,
                            "onboarding": false,
                            "lh_label": lh_label(cents),
                        })
                        .to_string(),
                    ),
                );
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("buy $LH: {e}")));
                dom::swap_inner(
                    msg_id,
                    &dom::msg_span(dom::Msg::Error, "couldn't start checkout"),
                );
            }
        }
    });
}

/// Close + tear down the buy modal (drop the Stripe Elements handle first), then
/// refresh the balance pill + no-funds banner — a just-settled purchase may have
/// already minted by the time the user closes the modal.
pub(super) fn close_buy_modal() {
    call_js("lhUnmountCheckout", None);
    if let Some(el) = dom::by_id("buy-modal") {
        if let Some(p) = el.parent_element() {
            let _ = p.remove_child(&el);
        }
    }
    wasm_bindgen_futures::spawn_local(async {
        super::refresh_credits_pill().await;
        refresh_fund_banner().await;
    });
}

/// Insert the branded buy modal once (mirrors `show_api_key_modal`).
fn open_buy_modal(lh_label: &str) {
    let Ok(doc) = dom::document() else { return };
    if doc.get_element_by_id("buy-modal").is_some() {
        return;
    }
    if let Some(body) = doc.body() {
        let _ = body
            .insert_adjacent_html("beforeend", &templates::buy_modal(lh_label).into_string());
    }
}

/// `$LH` minted for `cents` USD, for the modal preview. $LH is decoupled from $
/// and minted on the GROSS at $1 = 100 $LH (fees absorbed), so 1 cent = 1 $LH
/// exactly — a $2 buy shows "200 $LH". Round, no decimals.
fn lh_label(cents: u64) -> String {
    format!("{cents} $LH")
}

/// Call a global JS function from the Stripe shim (`window.<name>`); no-op if
/// absent. Keeps the imperative Stripe.js wiring in the JS glue layer.
fn call_js(name: &str, arg: Option<&str>) {
    let Some(w) = web_sys::window() else { return };
    let Ok(f) = js_sys::Reflect::get(&w, &JsValue::from_str(name)) else { return };
    if let Some(func) = f.dyn_ref::<js_sys::Function>() {
        let _ = match arg {
            Some(a) => func.call1(&w, &JsValue::from_str(a)),
            None => func.call0(&w),
        };
    }
}

/// Parse a USD amount ("5", "$5", "5.50") into integer cents. `None` on
/// empty / invalid / non-positive.
fn parse_usd_cents(raw: &str) -> Option<u64> {
    let s = raw.trim().trim_start_matches('$').trim();
    if s.is_empty() {
        return None;
    }
    let dollars: f64 = s.parse().ok()?;
    if !dollars.is_finite() || dollars <= 0.0 {
        return None;
    }
    let cents = (dollars * 100.0).round();
    if cents < 1.0 {
        return None;
    }
    Some(cents as u64)
}

/// POST an authenticated embedded buy request to the credit proxy; returns
/// `(client_secret, payment_intent_id)` for the Stripe Elements form. The
/// `client_secret` mounts the native elements; the `payment_intent_id` is what
/// the reactivity poll finalizes once the payment succeeds.
async fn start_checkout_embedded(cents: u64) -> Result<(String, String), String> {
    let (signer, addr) = crate::app::chat::credit_signer()
        .await
        .ok_or_else(|| "no identity".to_string())?;
    let addr_hex = bytes_to_hex_str(&addr); // lowercase 0x — matches the proxy
    let ts = (js_sys::Date::now() / 1000.0) as u64;
    let msg = format!("localharness-proxy:{addr_hex}:{ts}");
    let sig = crate::wallet::personal_sign(&signer, msg.as_bytes());
    let token = format!("{addr_hex}:{ts}:{}", bytes_to_hex_str(&sig));
    let url = format!("{}stripe/checkout", crate::registry::CREDIT_PROXY_URL);
    let resp = reqwest::Client::new()
        .post(&url)
        .header("x-goog-api-key", token)
        .header("content-type", "application/json")
        .body(format!("{{\"usd_cents\":{cents},\"embedded\":true}}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let ok = resp.status().is_success();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !ok {
        return Err(text);
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let client_secret = v
        .get("client_secret")
        .and_then(|u| u.as_str())
        .ok_or_else(|| "no client_secret in response".to_string())?
        .to_string();
    let payment_intent = v
        .get("payment_intent")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    Ok((client_secret, payment_intent))
}

/// Finalize a SUCCEEDED PaymentIntent: mint, confirm, persist. Driven from JS —
/// `web/stripe-embed.js`'s `lhWatchPayment` runs the status poll in JS (the shim
/// already holds the Stripe instance) and calls `window.lh_payment_succeeded`
/// (→ `lh_payment_succeeded` wasm export → here) ONLY once the PaymentIntent is
/// `succeeded`. There is NO pre-payment wasm async loop: the repeated JsFuture +
/// timer churn re-entered the wasm-bindgen single-thread executor on iOS WebKit
/// ("already mutably borrowed: BorrowError"), killing the app mid-checkout.
///
/// Mints via `/stripe/finalize` with a FRESHLY signed token, so a slow payer
/// can't outrun the 300s token-freshness window and silently fail the mint (the
/// bug that charged a card but credited no `$LH`). On a confirmed mint it flips
/// the modal to the done state and refreshes the balance. `finalize_mint` is
/// idempotent (the on-chain mint + the webhook backstop are too), so a
/// double-fire of `lh_payment_succeeded` is safe. Stops early if the user closed
/// the modal; the proxy webhook stays a backstop for tab-close / 3DS.
pub(crate) async fn finalize_after_payment(
    payment_intent: String,
    lh_label: String,
    onboarding: bool,
) {
    if payment_intent.is_empty() {
        return;
    }
    // Mint. Retry a few times: finalize fails closed (minted:false)
    // in the brief window before Stripe's NET-settled amount is available.
    for _ in 0..6 {
        if checkout_gone() {
            return;
        }
        if let Ok(true) = finalize_mint(&payment_intent).await {
            call_js("lhBuySuccess", Some(&format!("✓ {lh_label} added")));
            crate::app::chat::ensure_credit_meter().await;
            super::refresh_credits_pill().await;
            refresh_fund_banner().await;
            // Onboarding: the fresh buyer is now funded. PAY-FIRST RULE — the
            // seed has been held IN MEMORY ONLY until this moment; now that the
            // payment confirmed, persist it to disk. This is the FIRST thing
            // after the mint, so the pay→persist window is tiny (the inline
            // checkout keeps the page open, so the only residual risk is a reload
            // between the mint confirming and this write — narrow, and the webhook
            // still minted to the address). If persist FAILS, surface it loudly:
            // the user paid and the seed is still in memory (reveal/retry
            // possible), so we must NOT swallow it — the seed MUST land.
            if onboarding {
                if let Err(err) = crate::app::wallet_store::persist_current_seed().await {
                    call_js(
                        "lhBuyError",
                        Some(&format!(
                            "paid ✓ but saving your identity failed: {err} — do NOT close \
                             this tab; reveal & back up your seed phrase, then reload"
                        )),
                    );
                    return;
                }
                // Seed safely on disk → BACK IT UP at this safest moment (owner
                // request): show the recovery phrase with copy/download so a
                // device loss / OPFS wipe / the narrow reload-before-persist
                // window can't strand the just-paid identity. [continue]
                // (onboard-continue) proceeds to the name-claim. Tear down the
                // Stripe Elements instance first; the inline checkout card lives
                // inside `#root`, so the seed-backup swap below removes it.
                let words = crate::app::APP
                    .with(|c| c.borrow().wallet.as_ref().map(|w| w.mnemonic.to_string()))
                    .unwrap_or_default();
                call_js("lhUnmountCheckout", None);
                if let Some(root) = dom::by_id("root") {
                    root.set_inner_html(&templates::onboard_seed_backup(&words).into_string());
                }
            }
            return;
        }
        crate::runtime::sleep_ms(4000).await;
    }
    // Couldn't confirm the mint in-page; the webhook backstop will mint and the
    // "minting shortly" line stays up.
}

/// The checkout surface is gone → stop polling. True only when NEITHER the
/// admin `#buy-modal` NOR the inline onboarding `#lh-pay-region` is in the DOM,
/// so one check covers both paths (a closed modal or a navigated-away inline
/// card). The inline card lives inside `#apex-onboard`; `#lh-pay-region` is its
/// unique marker (the modal carries the same id, hence the OR).
fn checkout_gone() -> bool {
    dom::by_id("buy-modal").is_none() && dom::by_id("lh-pay-region").is_none()
}

/// Mint a settled PaymentIntent via `/stripe/finalize` with a FRESH auth token
/// (signed now, never the stale modal-open one). `Ok(true)` once the on-chain
/// mint lands (idempotent), `Ok(false)` while pending/not-yet-settled, `Err` on
/// transport failure.
async fn finalize_mint(payment_intent: &str) -> Result<bool, String> {
    let (signer, addr) = crate::app::chat::credit_signer()
        .await
        .ok_or_else(|| "no identity".to_string())?;
    let addr_hex = bytes_to_hex_str(&addr);
    let ts = (js_sys::Date::now() / 1000.0) as u64;
    let msg = format!("localharness-proxy:{addr_hex}:{ts}");
    let sig = crate::wallet::personal_sign(&signer, msg.as_bytes());
    let token = format!("{addr_hex}:{ts}:{}", bytes_to_hex_str(&sig));
    let url = format!("{}stripe/finalize", crate::registry::CREDIT_PROXY_URL);
    let resp = reqwest::Client::new()
        .post(&url)
        .header("x-goog-api-key", token)
        .header("content-type", "application/json")
        .body(format!("{{\"payment_intent\":\"{payment_intent}\"}}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let text = resp.text().await.map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(v.get("minted").and_then(|b| b.as_bool()).unwrap_or(false))
}

/// localStorage handle (best-effort).
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// The redeem code stashed from an `?invite=CODE` link, if any. `pub(crate)`:
/// the apex hero reads it to PREFILL the invite input (an invitee must never
/// have to re-copy a code that already rode in on the URL).
pub(crate) fn pending_invite_code() -> Option<String> {
    local_storage()?
        .get_item("lh_pending_invite")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
}

/// Auto-claim a pending invite code (captured from an `?invite=CODE`
/// link) into the visitor's credit identity, so an invitee lands with a
/// credited `$LH` balance instead of typing a code.
///
/// TWO code shapes share the one `?invite=` router (distinguished by
/// prefix — `design/invites.md` §5.1):
/// - **`inv-…`** → an InviteFacet BEARER invite: the on-chain `$LH` was
///   ESCROWED by another HOLDER; `acceptInvite(code)` pays it out to the
///   newcomer (`accept_invite_sponsored`). This is the growth primitive.
/// - **anything else** (`lh-…` etc.) → an owner-minted RedeemFacet code:
///   `redeem(code)` MINTS `$LH` to the caller (`redeem_sponsored`). The
///   pre-existing path, untouched.
///
/// `allow_generate`: on the apex (identity hub) we pass `false` so we
/// wait for the visitor to create/import their MAIN before crediting it
/// (the code stays pending across the repaint); on tenant/other origins
/// we pass `true` and credit the local device key. Idempotent: the code
/// is cleared after any committed attempt so a refresh can't double-spend.
pub(crate) async fn try_redeem_pending_invite(allow_generate: bool) {
    let Some(code) = pending_invite_code() else {
        return;
    };
    // On the apex, only redeem once an identity actually exists — don't
    // silently mint a device key on a marketing-style visit. Leave the
    // code pending; the post-create `paint_apex` re-fires this.
    if !allow_generate && crate::app::chat::credit_address_existing().await.is_none() {
        return;
    }
    let Some((signer, _)) = crate::app::chat::credit_signer().await else {
        return;
    };
    let Ok(fee_payer) = crate::app::sponsor::signer() else {
        return;
    };
    // Commit: clear the pending code first so a concurrent repaint or a
    // refresh can't fire a second (double-spend) accept/redeem of the same
    // code.
    if let Some(s) = local_storage() {
        let _ = s.remove_item("lh_pending_invite");
    }
    // Bearer InviteFacet invite (escrow payout) vs owner-minted redeem code.
    let is_invite = code.starts_with("inv-");
    dom::set_status(
        if is_invite { "accepting invite…" } else { "redeeming invite…" },
        false,
    );
    let claim = async {
        if is_invite {
            crate::app::registry::accept_invite_sponsored(
                &signer,
                &fee_payer,
                &code,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
        } else {
            crate::app::registry::redeem_sponsored(
                &signer,
                &fee_payer,
                &code,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
        }
    };
    // Bounded: a stalled mobile connection must surface, not hang the status
    // line forever.
    let result = crate::app::net::with_timeout(45_000, claim)
        .await
        .map_err(|_| "timed out".to_string())
        .and_then(|r| r);
    match result {
        Ok(_) => {
            // Land them on platform credits (the default) and refresh the
            // balance pill so the new $LH shows immediately.
            if let Some(s) = local_storage() {
                let _ = s.set_item("lh_model_access", "credits");
            }
            dom::set_status(
                if is_invite {
                    "invite accepted — $LH added"
                } else {
                    "invite redeemed — platform credits added"
                },
                false,
            );
            super::refresh_credits_pill().await;
            // Now funded → drop the no-funds banner if the chrome is up.
            refresh_fund_banner().await;
        }
        Err(e) => {
            web_sys::console::warn_1(&JsValue::from_str(&format!("invite claim: {e}")));
            dom::set_status(
                "invite couldn't be claimed (it may be used or expired)",
                true,
            );
            // Claim failed (e.g. used/expired code) → the visitor may still be
            // unfunded; surface the manual redeem CTA so they have a recovery path.
            refresh_fund_banner().await;
        }
    }
}

/// Default invite lifetime when the owner doesn't specify one: 7 days
/// (matches the CLI's `INVITE_DEFAULT_TTL_SECS`). After it expires
/// unclaimed the funder reclaims the escrow (`invite reclaim`).
const INVITE_DEFAULT_TTL_SECS: u64 = 7 * 24 * 3600;

/// Generate a fresh, link-safe bearer invite code: `inv-<amount>-<10
/// base32 chars>`. Mirrors the CLI's `gen_invite_code` EXACTLY (same
/// Crockford-ish alphabet, same 10-char CSPRNG tail) so a browser-minted
/// code is indistinguishable from a CLI one — both hash via
/// `registry::invite_code_hash` and both route through the `inv-` `?invite=`
/// branch. The plaintext is the bearer secret; only its keccak hash is
/// stored on-chain.
fn gen_invite_code(amount_label: &str) -> String {
    // Crockford base32 minus the visually-ambiguous 0/1/i/l/o/u.
    const ALPHABET: &[u8; 32] = b"abcdefghjkmnpqrstvwxyz23456789ab";
    let bytes = crate::app::registry::random_x402_nonce(); // 32 CSPRNG bytes (getrandom/js)
    let mut tail = String::with_capacity(10);
    for &b in bytes.iter().take(10) {
        tail.push(ALPHABET[(b & 0x1f) as usize] as char);
    }
    format!("inv-{amount_label}-{tail}")
}

/// Escrow the owner's `$LH` behind a fresh bearer code and surface the
/// `?invite=` share link (InviteFacet `createInvite`). The funder is the
/// credit identity (local key) — same signing path as redeem/deposit, so
/// `create_invite_sponsored` is called directly (sponsor pays the fee).
/// Silent no-op on empty/invalid amount (no explanatory-validation text);
/// success swaps `#invite-result` for the share-link panel.
pub(super) fn create_invite_pressed() {
    let Some(input) = dom::input_by_id("invite-amount") else {
        return;
    };
    let raw = input.value().trim().to_string();
    // Silent no-op on empty/invalid/zero (no explanatory-validation text).
    let Some(amount_wei) = crate::encoding::parse_token_amount(&raw) else {
        return;
    };
    if amount_wei == 0 {
        return;
    }
    // Link-safe label for the human-readable middle of the code: keep only
    // digits + the decimal point (the `?invite=` router keys ONLY on the
    // `inv-` prefix, so this part is cosmetic but must stay URL-clean).
    let amount_label: String = raw
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let code = gen_invite_code(&amount_label);
    let code_hash = crate::app::registry::invite_code_hash(&code);
    dom::swap_inner(
        "invite-result",
        "<span style=\"color:var(--muted)\">creating invite…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            super::sponsor_rate_guard()?;
            let (signer, addr) = crate::app::chat::credit_signer()
                .await
                .ok_or_else(|| "no identity".to_string())?;
            // Escrow auto-bridge (feedback #63): a wallet shortfall covered by
            // unspent chat-meter credits rides as a withdrawCredits call in the
            // SAME atomic tx as approve+createInvite.
            let from_hex = crate::encoding::bytes_to_hex_str(&addr);
            let bridge_wei =
                crate::app::chat::escrow_bridge_wei(&from_hex, amount_wei).await?;
            let fee_payer = crate::app::sponsor::signer()?;
            crate::app::registry::create_invite_sponsored_bridged(
                &signer,
                &fee_payer,
                code_hash,
                amount_wei,
                INVITE_DEFAULT_TTL_SECS,
                crate::app::registry::ALPHA_USD_ADDRESS,
                bridge_wei,
            )
            .await
        }
        .await;
        match result {
            Ok(_) => {
                // The escrow left the funder's spendable balance — reflect it.
                super::refresh_credits_pill().await;
                // The apex is the canonical landing origin for `?invite=` links
                // (standalone `…/?invite=CODE`), so share that.
                let link = format!("https://localharness.xyz/?invite={code}");
                dom::swap_inner(
                    "invite-result",
                    &templates::invite_result_panel(&code, &link).into_string(),
                );
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("create invite: {e}")));
                dom::swap_inner(
                    "invite-result",
                    &dom::msg_span(dom::Msg::Error, "invite couldn't be created (need $LH to escrow)"),
                );
            }
        }
    });
}
