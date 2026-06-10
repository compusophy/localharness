//! Apex claim flow — live availability check + the sponsored first-claim tx.

use wasm_bindgen::prelude::*;

use crate::app::{dom, templates};

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

/// Full apex claim flow: faucet → registration tx → confirm → redirect.
/// Silent except for the button itself — disabled with text "creating…"
/// while in flight, redirects on success, reverts to the input-driven
/// state on failure. All status/error chatter goes to `console.warn`
/// for debuggability. Per [[feedback-no-explanatory-validation]].
pub(super) async fn run_apex_claim(name: String, create_if_missing: bool) {
    set_create_button_busy(true);

    // Trap fix: a device with NO wallet that claims a name used to silently
    // mint a brand-new seed (see the `None` branch below) — which is how a
    // returning user on a second device ended up owning a *different* EOA's
    // subdomains, splitting their identity. Now we refuse to mint silently:
    // if there's no wallet and the user hasn't explicitly chosen "create a
    // new identity", show the choice (create new / adopt existing) instead.
    let has_wallet = crate::app::APP.with(|cell| cell.borrow().wallet.is_some());
    if !has_wallet && !create_if_missing {
        set_create_button_busy(false);
        dom::swap_outer("agents-list", &templates::identity_choice(&name).into_string());
        return;
    }

    let result: Result<String, String> = async {
        // 1. Re-confirm availability — the user might have been
        //    overtaken between live-check and submit.
        match crate::app::registry::check_name(&name).await {
            Ok(crate::app::registry::Status::Available) => {}
            Ok(other) => return Err(format!("name not available: {other:?}")),
            Err(err) => return Err(format!("check_name: {err}")),
        }

        // 2. Pull the wallet out of App state — or generate one in
        //    place. The subdomain IS the identity primitive: a visitor
        //    arriving at apex without a wallet is just one who hasn't
        //    claimed yet. Roll wallet creation into this submit so we
        //    never end up with a wallet that doesn't own anything
        //    on-chain.
        let cached = crate::app::APP.with(|cell| {
            cell.borrow()
                .wallet
                .as_ref()
                .map(|w| (w.signer.clone(), wallet_address_hex(&w.address)))
        });
        let (signer, addr_hex) = match cached {
            Some(pair) => pair,
            None => match crate::app::wallet_store::create_and_persist().await {
                Ok(wallet) => {
                    let pair = (wallet.signer.clone(), wallet.address_hex());
                    crate::app::APP.with(|cell| cell.borrow_mut().wallet = Some(wallet));
                    pair
                }
                Err(err) => return Err(format!("wallet: {err}")),
            },
        };

        // 2.5. Cost-gate pre-check. Registration is currently free
        //      (on-chain `registrationCost` is 0), so this is a no-op
        //      today. It stays as a guard in case the cost gate is ever
        //      re-enabled: if the registry charges LH and the user can't
        //      cover it, bail before burning sponsor gas on a guaranteed
        //      revert.
        let cost = crate::app::registry::registration_cost().await.unwrap_or(0);
        if cost > 0 {
            let bal = crate::app::registry::token_balance_of(&addr_hex).await.unwrap_or(0);
            if bal < cost {
                let deficit_lh = (cost - bal) / 1_000_000_000_000_000_000u128;
                return Err(format!("__NEED_LH__{deficit_lh}"));
            }
        }

        // 3. Submit the claim as a sponsored Tempo tx. The bundle's
        //    sponsor wallet pays the fees in AlphaUSD; the user's
        //    fresh apex wallet signs as sender and never needs any
        //    native gas or any TIP-20 stablecoin. No faucet step.
        let fee_payer = crate::app::sponsor::signer()
            .map_err(|e| format!("sponsor key: {e}"))?;
        crate::app::registry::claim_and_maybe_set_main_sponsored(
            &signer,
            &fee_payer,
            &name,
            crate::app::registry::ALPHA_USD_ADDRESS,
        )
        .await
        .map_err(|e| format!("claim_name: {e}"))
    }
    .await;

    match result {
        Ok(tx_hash) => {
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "claimed {name} (tx {})",
                short_hash(&tx_hash)
            )));
            let target = format!("https://{name}.localharness.xyz/?claim=1");
            if let Ok(window) = dom::window() {
                let _ = window.location().assign(&target);
            }
        }
        Err(err) => {
            web_sys::console::warn_1(&JsValue::from_str(&format!("apex claim failed: {err}")));
            // Surface failure on the button itself so the user knows
            // the click had an effect — a silent reset to disabled
            // looks indistinguishable from "nothing happened" and
            // invites frustrated re-clicking. `on_apex_input` clears
            // the failed state on the next keystroke.
            //
            // Specific case: insufficient credits. Pre-check encodes
            // the deficit in the error string with a sentinel prefix
            // so we can show "need N more LH" instead of a generic
            // "✗ failed".
            if let Some(rest) = err.strip_prefix("__NEED_LH__") {
                set_create_button_failed_with(&format!("need {rest} more LH"));
            } else {
                set_create_button_state(CreateBtnState::Failed);
            }
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

fn wallet_address_hex(addr: &[u8; 20]) -> String {
    let mut s = String::with_capacity(42);
    s.push_str("0x");
    for b in addr {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn short_hash(tx_hash: &str) -> String {
    let stripped = tx_hash.trim_start_matches("0x");
    if stripped.len() < 12 {
        return tx_hash.to_string();
    }
    format!("{}…{}", &stripped[..6], &stripped[stripped.len() - 4..])
}
