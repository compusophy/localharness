//! Agent act-panel — per-agent TBA balance panel + send-$LH action.

use crate::encoding::{parse_token_amount, tx_short_hash};

use crate::app::{dom, templates};

/// Expand or collapse the inline act-panel under an agent row.
/// First open fetches TBA balance + paints the panel; subsequent
/// toggles just flip the `hidden` attribute on the existing DOM.
pub(super) fn agent_act_toggle_pressed(token_id_str: String) {
    let Ok(token_id) = token_id_str.parse::<u64>() else { return };
    let panel_id = format!("agent-act-{token_id}");
    let Some(panel) = dom::by_id(&panel_id) else { return };
    let was_hidden = panel.has_attribute("hidden");
    if was_hidden {
        // First-paint flow: fetch TBA + balance, render the form.
        panel.set_inner_html(
            "<div class=\"admin-msg-slot\"><span style=\"color:var(--muted)\">loading…</span></div>",
        );
        let _ = panel.remove_attribute("hidden");
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(err) = paint_agent_act_panel(token_id).await {
                let panel_id = format!("agent-act-{token_id}");
                dom::swap_inner(
                    &panel_id,
                    &maud::html! {
                        div class="admin-msg-slot" {
                            span style="color:var(--error)" { (err) }
                        }
                    }
                    .into_string(),
                );
            }
        });
    } else {
        let _ = panel.set_attribute("hidden", "");
    }
}

async fn paint_agent_act_panel(token_id: u64) -> Result<(), String> {
    let tba = crate::app::registry::tba_of_token_id(token_id)
        .await
        .map_err(|e| format!("tba: {e}"))?
        .ok_or_else(|| "no TBA".to_string())?;
    let balance = crate::app::registry::token_balance_of(&tba).await.unwrap_or(0);
    let html = templates::agent_act_panel(token_id, &tba, balance).into_string();
    let panel_id = format!("agent-act-{token_id}");
    dom::swap_inner(&panel_id, &html);
    Ok(())
}

/// User clicked "send" in an inline act-panel. Reads the recipient
/// and amount inputs scoped to this token_id, fires a sponsored
/// `tba.execute(credits, 0, transfer(...), 0)` tempo tx. The user's
/// apex wallet signs as one of the TBA's authorized signers (it IS
/// the NFT owner). Sponsor pays AlphaUSD.
pub(super) fn agent_send_lh_pressed(token_id_str: String) {
    let Ok(token_id) = token_id_str.parse::<u64>() else { return };
    let msg_id = format!("agent-act-msg-{token_id}");

    let to_raw = dom::input_by_id(&format!("agent-send-to-{token_id}"))
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();
    let amt_raw = dom::input_by_id(&format!("agent-send-amt-{token_id}"))
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();
    // Route the recipient through the SAME hardened choke point as send_lh /
    // the per-turn payment (`classify_recipient`), which refuses the zero
    // address (a transfer there burns the TBA's $LH irrecoverably). The act
    // panel only accepts a raw address, so anything that classifies as a Name
    // (wrong length, non-hex) is a silent no-op too — per
    // [[feedback-no-explanatory-validation]].
    let Ok(crate::encoding::Recipient::Address(to_raw)) =
        crate::encoding::classify_recipient(&to_raw)
    else {
        return; // empty, zero-address, or not a 40-hex address → silent no-op
    };
    let Some(amount_wei) = parse_token_amount(&amt_raw) else { return };
    if amount_wei == 0 {
        return;
    }

    let signer = crate::app::APP.with(|cell| {
        cell.borrow().wallet.as_ref().map(|w| w.signer.clone())
    });
    let Some(signer) = signer else { return };

    dom::swap_inner(
        &msg_id,
        "<span style=\"color:var(--muted)\">signing + submitting…</span>",
    );

    wasm_bindgen_futures::spawn_local(async move {
        let msg_id = format!("agent-act-msg-{token_id}");
        let result = async {
            let tba = crate::app::registry::tba_of_token_id(token_id)
                .await
                .map_err(|e| format!("tba: {e}"))?
                .ok_or_else(|| "no TBA".to_string())?;
            let fee_payer = crate::app::sponsor::signer()?;
            crate::app::registry::tba_transfer_lh_sponsored(
                &signer,
                &fee_payer,
                token_id,
                &tba,
                &to_raw,
                amount_wei,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
        }
        .await;
        match result {
            Ok(tx_hash) => {
                let short = tx_short_hash(&tx_hash);
                dom::swap_inner(
                    &msg_id,
                    &dom::msg_span(dom::Msg::Accent, &format!("✓ sent (tx {short})")),
                );
                // Re-paint to refresh the balance line.
                let _ = paint_agent_act_panel(token_id).await;
            }
            Err(err) => {
                dom::swap_inner(
                    &msg_id,
                    &dom::msg_span(dom::Msg::Error, &format!("{err}")),
                );
            }
        }
    });
}
