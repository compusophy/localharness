//! Agent-wallet (TBA) actions — the "act FROM your agent's token-bound
//! account" panel in the tenant admin's account tab.
//!
//! Every registered name owns an ERC-6551 `MultiSignerAccount` whose `$LH`
//! (bounty rewards, x402 earnings) was previously read-only in the UI. This
//! module lets the OWNER spend it: the panel shows the TBA address + balance
//! and sends `$LH` from the TBA to a `0x…` address or another agent's name
//! (paid to that agent's own TBA — the same place bounty/x402 payouts land).
//!
//! Tx path: the owner's EOA signs the sender hash (local-first off
//! `APP.wallet`, iframe fallback — `verify::sign_tempo_tx_via_iframe` inside
//! `events::run_sponsored_tempo_call`), authorizing
//! `[createTokenBoundAccount (idempotent), tba.execute($LH, 0, transfer)]`
//! in ONE sponsored Tempo tx; authorization is enforced ON-CHAIN by
//! `MultiSignerAccount.execute` (reverts unless the signer is the NFT holder
//! or an enrolled device). The calldata batch is the native-tested
//! `registry::tba_send_lh_calls`.
//!
//! Sending value is irreversible, so the flow follows the destructive-action
//! convention: [send] only ARMS a confirmation; the user must TYPE the
//! amount (never auto-filled) before the tx fires.

use crate::encoding::{classify_recipient, parse_token_amount, short_addr, tx_short_hash, Recipient};

use crate::app::{dom, templates};

/// Fill the act panel's address + balance rows from the chain. Fired on
/// admin-open (and after a successful send); no-ops when the slot isn't
/// mounted (apex) or the host isn't a registered tenant. Timeout-capped so
/// a dead RPC degrades to a dash instead of a stuck placeholder.
pub(super) async fn refresh_tba_panel() {
    if dom::by_id("tba-act-address").is_none() {
        return;
    }
    let Some(name) = crate::app::tenant::current_name() else { return };
    let tba = crate::app::net::read(crate::app::registry::tba_of_name(&name))
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
    let Some(tba) = tba else {
        dom::swap_inner("tba-act-address", "—");
        dom::swap_inner("tba-act-balance", "—");
        return;
    };
    // The full address matters here (it's where bounty/x402 payouts land, and
    // the user may want to fund it externally) — show it whole, like the
    // financial card's wallet line, not abbreviated.
    dom::swap_inner("tba-act-address", &maud::html! { (tba) }.into_string());
    let balance = crate::app::net::read(crate::app::registry::token_balance_of(&tba))
        .await
        .ok()
        .and_then(Result::ok);
    match balance {
        Some(wei) => dom::swap_inner(
            "tba-act-balance",
            &format!("{} LH", crate::app::format_wei_as_test_eth(wei)),
        ),
        None => dom::swap_inner("tba-act-balance", "—"),
    }
}

/// [send] on the act panel — ARM only, never submits. Reads the recipient +
/// amount inputs, resolves a name to that agent's TBA, and swaps in the
/// typed-amount confirmation. Empty / unparsable input is a silent no-op
/// (per the no-explanatory-validation rule); genuine failures (unknown name,
/// zero address, dead RPC) surface as error messages.
pub(super) fn tba_send_pressed() {
    let recipient_raw = dom::input_by_id("tba-send-recipient")
        .map(|i| i.value())
        .unwrap_or_default();
    let amount_raw = dom::input_by_id("tba-send-amount")
        .map(|i| i.value())
        .unwrap_or_default();
    if recipient_raw.trim().is_empty() {
        return;
    }
    let Some(amount_wei) = parse_token_amount(&amount_raw) else { return };
    if amount_wei == 0 {
        return;
    }
    // classify_recipient also refuses the funds-burning zero address —
    // surface that one (it's an outcome, not a validation rule).
    let recipient = match classify_recipient(&recipient_raw) {
        Ok(r) => r,
        Err(e) => {
            dom::swap_inner("tba-send-msg", &dom::msg_span(dom::Msg::Error, &e));
            return;
        }
    };
    dom::swap_inner("tba-send-msg", "");
    wasm_bindgen_futures::spawn_local(async move {
        // A raw address is used as-is; a NAME pays that agent's own TBA —
        // the agent-economy convention (bounty + x402 settle to TBAs), not
        // the owner's EOA like the wallet-side send_lh.
        let resolved = match &recipient {
            Recipient::Address(addr) => Ok(addr.clone()),
            Recipient::Name(name) => {
                match crate::app::registry::tba_of_name(name).await {
                    Ok(Some(tba)) => Ok(tba),
                    Ok(None) => Err(format!("\"{name}\" is not a registered name")),
                    Err(e) => Err(format!("lookup failed: {e}")),
                }
            }
        };
        match resolved {
            Ok(to_hex) => {
                let label = match &recipient {
                    Recipient::Address(_) => short_addr(&to_hex),
                    Recipient::Name(name) => format!("{name} ({})", short_addr(&to_hex)),
                };
                dom::swap_inner(
                    "tba-send-confirm-slot",
                    &templates::tba_send_confirm_panel(&label, &to_hex, amount_wei)
                        .into_string(),
                );
            }
            Err(e) => {
                dom::swap_inner("tba-send-msg", &dom::msg_span(dom::Msg::Error, &e));
            }
        }
    });
}

/// Abort an armed TBA send — clear the confirmation.
pub(super) fn tba_send_cancel_pressed() {
    dom::swap_inner("tba-send-confirm-slot", "");
    dom::swap_inner("tba-send-msg", "");
}

/// Execute the armed send IFF the typed amount matches. `arg` is
/// `"<resolved 0x…>:<amount wei>"` (stamped by the confirm panel from the
/// SAME values it displayed, so editing the original inputs after arming
/// can't desync what's shown from what's sent). The typed value is compared
/// in WEI ("1.5" and "1.50" both confirm 1.5) but must be present — an
/// empty confirmation never passes.
pub(super) fn tba_send_confirm_pressed(arg: String) {
    let Some((to_hex, amount_str)) = arg.split_once(':') else { return };
    let Ok(amount_wei) = amount_str.parse::<u128>() else { return };
    if amount_wei == 0 {
        return;
    }
    let typed = dom::input_by_id("tba-send-confirm-input")
        .map(|i| i.value())
        .unwrap_or_default();
    if typed.trim().is_empty() || parse_token_amount(&typed) != Some(amount_wei) {
        dom::swap_inner(
            "tba-send-msg",
            &dom::msg_span(dom::Msg::Error, "type the amount to confirm"),
        );
        return;
    }
    let to_hex = to_hex.to_string();
    dom::swap_inner("tba-send-msg", &dom::msg_span(dom::Msg::Accent, "sending…"));
    wasm_bindgen_futures::spawn_local(async move {
        match run_tba_send(&to_hex, amount_wei).await {
            Ok(tx_hash) => {
                dom::swap_inner("tba-send-confirm-slot", "");
                if let Some(input) = dom::input_by_id("tba-send-recipient") {
                    input.set_value("");
                }
                if let Some(input) = dom::input_by_id("tba-send-amount") {
                    input.set_value("");
                }
                dom::swap_inner(
                    "tba-send-msg",
                    &dom::msg_span(
                        dom::Msg::Muted,
                        &format!("sent — tx {}", tx_short_hash(&tx_hash)),
                    ),
                );
                refresh_tba_panel().await;
            }
            Err(e) => {
                dom::swap_inner(
                    "tba-send-msg",
                    &dom::msg_span(dom::Msg::Error, &format!("send failed: {e}")),
                );
            }
        }
    });
}

/// Resolve this tenant's (token id, TBA), pre-check the TBA balance, and
/// submit the `tba_send_lh_calls` batch as ONE sponsored Tempo tx signed by
/// the on-chain OWNER (the chain re-checks authorization in
/// `MultiSignerAccount.execute`).
async fn run_tba_send(to_hex: &str, amount_wei: u128) -> Result<String, String> {
    let (name, owner) = crate::app::tenant::current_tenant_owner().await?;
    let token_id = crate::app::registry::id_of_name(&name)
        .await
        .map_err(|e| format!("id: {e}"))?;
    if token_id == 0 {
        return Err("name not registered".into());
    }
    let tba = crate::app::registry::tba_of_name(&name)
        .await
        .map_err(|e| format!("tba: {e}"))?
        .ok_or_else(|| "no agent wallet for this name".to_string())?;
    // Pre-check the TBA balance so an underfunded send fails with the real
    // number instead of an opaque on-chain revert (the chain still enforces).
    let balance = crate::app::registry::token_balance_of(&tba)
        .await
        .map_err(|e| format!("balance: {e}"))?;
    if balance < amount_wei {
        return Err(format!(
            "agent wallet holds {} $LH",
            crate::app::format_wei_as_test_eth(balance)
        ));
    }
    let calls = crate::app::registry::tba_send_lh_calls(token_id, &tba, to_hex, amount_wei)?;
    let purpose = format!(
        "send {} $LH from {name}'s agent wallet",
        crate::app::format_wei_as_test_eth(amount_wei)
    );
    crate::app::events::run_sponsored_tempo_call(
        &owner,
        calls,
        crate::app::registry::TBA_SEND_LH_GAS,
        &purpose,
    )
    .await
}
