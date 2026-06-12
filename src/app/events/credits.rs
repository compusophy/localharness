//! Credits / funding — fund banner, model access + selection, local-model
//! download, redeem codes, and invites.

use wasm_bindgen::prelude::*;

use crate::app::{dom, templates};


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
    let msg_id = "invite-onboard-msg";
    let is_invite = code.starts_with("inv-");
    dom::swap_inner(
        msg_id,
        "<span style=\"color:var(--muted)\">redeeming…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            // Explicit user action → generating the device/credit key here is
            // ALLOWED (not silent). Reuses `credit_signer` (master wallet if
            // present, else load-or-generate the local key) so a returning
            // user with a seed doesn't get a second identity.
            let (signer, _) = crate::app::chat::credit_signer()
                .await
                .ok_or_else(|| "no identity".to_string())?;
            let fee_payer = crate::app::sponsor::signer()?;
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
                // Re-paint the apex: the visitor now has an identity, so
                // `paint_apex` renders the claim-a-name surface (+ agents list).
                crate::app::paint_apex(crate::app::tenant::Host::Apex).await;
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

/// localStorage handle (best-effort).
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// The redeem code stashed from an `?invite=CODE` link, if any.
fn pending_invite_code() -> Option<String> {
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
    let result = if is_invite {
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
    };
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
