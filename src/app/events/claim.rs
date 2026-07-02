//! Apex claim flow — live availability check + the sponsored first-claim tx.

use wasm_bindgen::prelude::*;

use crate::app::{dom, templates};
use crate::encoding::{bytes_to_hex_str, tx_short_hash};

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
pub(super) fn on_apex_input() {
    let Some(input) = dom::input_by_id("apex-input") else { return };
    let raw = input.value();
    let cleaned = crate::app::tenant::sanitize(&raw);
    if cleaned != raw {
        // Reflect the canonical form so the user sees the live filter.
        input.set_value(&cleaned);
    }
    // A keystroke dismisses a stale "need N LH"/buy affordance, same as it
    // clears the failed-button state (no-op when the slot's already empty).
    dom::swap_inner("claim-fund-slot", "");

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
        let result = crate::app::registry::check_name(&pending).await;
        // Only act on the result if the input still matches what we
        // queried — otherwise the user typed more chars and a fresh
        // check is already in flight.
        let still_pending = dom::input_by_id("apex-input")
            .map(|i| crate::app::tenant::sanitize(&i.value()) == pending)
            .unwrap_or(false);
        if !still_pending {
            return;
        }
        match result {
            Ok(crate::app::registry::Status::Available) => {
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

/// The on-chain first-claim core, with NO DOM/UI — callers own their own
/// surface (`run_apex_claim` drives the create button; `onboard_claim` shows a
/// post-payment interstitial). Re-confirms availability, ensures the wallet
/// exists (create+persist if missing), runs a pot-aware cost pre-check, then
/// submits the sponsored register tx and returns the tx hash. On a missing
/// wallet + `!create_if_missing` it returns the `__NO_WALLET__` sentinel so the
/// caller can show the identity choice instead of silently minting a new seed.
async fn submit_claim(name: &str, create_if_missing: bool) -> Result<String, String> {
    // 1. Re-confirm availability — the user might have been overtaken between
    //    the live-check and submit.
    match crate::app::registry::check_name(name).await {
        Ok(crate::app::registry::Status::Available) => {}
        Ok(other) => return Err(format!("name not available: {other:?}")),
        Err(err) => return Err(format!("check_name: {err}")),
    }

    // 2. Pull the wallet out of App state — or generate one in place. The
    //    subdomain IS the identity primitive: a visitor at apex without a wallet
    //    is just one who hasn't claimed yet. Refuse to silently mint a NEW seed
    //    when the caller hasn't opted in (the second-device identity-split trap).
    let cached = crate::app::APP.with(|cell| {
        cell.borrow()
            .wallet
            .as_ref()
            .map(|w| (w.signer.clone(), bytes_to_hex_str(&w.address)))
    });
    let (signer, addr_hex) = match cached {
        Some(pair) => pair,
        None if !create_if_missing => return Err("__NO_WALLET__".to_string()),
        None => match crate::app::wallet_store::create_and_persist().await {
            Ok(wallet) => {
                let pair = (wallet.signer.clone(), wallet.address_hex());
                crate::app::APP.with(|cell| cell.borrow_mut().wallet = Some(wallet));
                pair
            }
            Err(err) => return Err(format!("wallet: {err}")),
        },
    };

    // 2.5. Cost-gate pre-check. Registration costs `registrationCost()` $LH,
    //      charged via `transferFrom` from the WALLET. A fiat buyer's $LH is in
    //      the METER, so count BOTH pots: the sponsored claim bridges the wallet
    //      shortfall out of the (now-unlocked) meter credits in the same atomic
    //      tx. Only bail — before burning sponsor gas on a guaranteed revert —
    //      when neither pot, nor both together, can cover the cost.
    let cost = crate::app::registry::registration_cost().await.unwrap_or(0);
    if cost > 0 {
        let wallet = crate::app::registry::token_balance_of(&addr_hex).await.unwrap_or(0);
        if wallet < cost {
            let meter = crate::app::registry::withdrawable_credit_of(&addr_hex)
                .await
                .unwrap_or(0);
            if wallet + meter < cost {
                // Round UP: a fractional shortfall (e.g. 0.5 LH short) must show
                // "need 1 more LH", not floor-divide to a confusing "need 0 more".
                let deficit_lh = (cost - wallet - meter).div_ceil(1_000_000_000_000_000_000u128);
                return Err(format!("__NEED_LH__{deficit_lh}"));
            }
        }
    }

    // 3. Submit the claim as a sponsored Tempo tx. The sponsor (testnet key /
    //    mainnet relay, resolved inside `registry::`) pays the fees; the user's
    //    apex wallet signs as sender and needs no native gas / stablecoin.
    crate::app::registry::claim_and_maybe_set_main_sponsored(&signer, name)
    .await
    .map_err(|e| format!("claim_name: {e}"))
}

/// Redirect into the just-claimed agent's chat.
fn redirect_to_agent(name: &str) {
    let target = format!("https://{name}.localharness.xyz/?claim=1");
    if let Ok(window) = dom::window() {
        let _ = window.location().assign(&target);
    }
}

/// Apex claim flow driven by the create button: "creating…" while in flight,
/// redirects on success, surfaces failure ON the button (a silent reset looks
/// like "nothing happened" and invites re-clicking). `on_apex_input` clears the
/// failed state on the next keystroke. Per [[feedback-no-explanatory-validation]].
pub(super) async fn run_apex_claim(name: String, create_if_missing: bool) {
    set_create_button_busy(true);
    match submit_claim(&name, create_if_missing).await {
        Ok(tx_hash) => {
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "claimed {name} (tx {})",
                tx_short_hash(&tx_hash)
            )));
            redirect_to_agent(&name);
        }
        Err(err) if err == "__NO_WALLET__" => {
            // No wallet + the user hasn't chosen "create a new identity": show the
            // choice (create new / adopt existing) instead of splitting identity.
            set_create_button_busy(false);
            dom::swap_outer("agents-list", &templates::identity_choice(&name).into_string());
        }
        Err(err) => {
            web_sys::console::warn_1(&JsValue::from_str(&format!("apex claim failed: {err}")));
            // Insufficient credits: the pre-check encodes the deficit behind a
            // sentinel so we can show "need N more LH" + a buy affordance instead
            // of a generic "✗ failed".
            if let Some(rest) = err.strip_prefix("__NEED_LH__") {
                set_create_button_failed_with(&format!("need {rest} more LH"));
                dom::swap_inner("claim-fund-slot", &templates::buy_to_claim().into_string());
            } else {
                set_create_button_state(CreateBtnState::Failed);
            }
        }
    }
}

/// [`Action::ApexClaim`](super::Action::ApexClaim).
///
/// Silent no-op on invalid input — the create button is
/// disabled by `on_apex_input` when length is out of range,
/// so this branch only ever fires for valid names. Per
/// [[feedback-no-explanatory-validation]].
pub(super) fn apex_claim_pressed() {
    let raw = dom::input_by_id("apex-input")
        .map(|i| i.value())
        .unwrap_or_default();
    let cleaned = crate::app::tenant::sanitize(&raw);
    if cleaned.len() < 3 || cleaned.len() > 32 {
        return;
    }
    wasm_bindgen_futures::spawn_local(async move {
        run_apex_claim(cleaned, false).await;
    });
}

/// [`Action::ClaimOnChain`](super::Action::ClaimOnChain).
///
/// Tenant-side first-claim: ensure apex wallet exists (without
/// overwriting an existing one — that would nuke other NFTs),
/// run the on-chain register tx via the signer iframe, then
/// set the local OPFS marker + re-paint as owner. This kills
/// the previous "bounce to apex first" interstitial.
pub(super) fn claim_on_chain_pressed() {
    let Some(name) = crate::app::tenant::current_name() else {
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
        if let Err(err) = crate::app::verify::create_wallet_via_iframe(false).await {
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
        match crate::app::verify::claim_name_via_iframe(&name).await {
            Ok((owner_addr, _tx)) => {
                // Remember the just-registered owner address as the
                // local first-paint hint (the chain stays authority).
                let _ = crate::app::owner::remember(&owner_addr).await;
                crate::app::paint_tenant(
                    crate::app::tenant::Host::Tenant(name.clone()),
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

/// Post-payment onboarding claim: the wallet is persisted + funded, so claim the
/// name the visitor chose on the front door and drop them into its chat. Shows a
/// brief "creating…" interstitial (the checkout card was just unmounted). On
/// failure — e.g. the name was taken during checkout — it falls back to the
/// funded name-claim apex so the just-paid user can pick another name instead of
/// being stranded; their $LH is safe in the meter.
pub(super) async fn onboard_claim(name: String) {
    if let Some(root) = dom::by_id("root") {
        root.set_inner_html(&templates::onboard_claiming(&name).into_string());
    }
    match submit_claim(&name, true).await {
        Ok(tx_hash) => {
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "onboard-claimed {name} (tx {})",
                tx_short_hash(&tx_hash)
            )));
            redirect_to_agent(&name);
        }
        Err(err) => {
            web_sys::console::warn_1(&JsValue::from_str(&format!(
                "onboard claim failed: {err}"
            )));
            crate::app::paint_apex(crate::app::tenant::Host::Apex).await;
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

