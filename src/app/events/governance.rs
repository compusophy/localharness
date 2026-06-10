//! DAO governance — propose / vote / execute guild treasury spends (VotingFacet).

use wasm_bindgen::prelude::*;

use crate::app::{dom, templates};

// =============================================================================
// DAO governance board (VotingFacet) — propose a treasury spend from a guild,
// vote, and execute once it passes past its deadline. Same sponsored path as the
// guild/bounty handlers (the owner's credit key signs, the embedded sponsor pays
// gas). Reuses the SIBLING-OWNED registry helpers propose_sponsored /
// vote_sponsored / execute_proposal_sponsored + reads proposals_of / get_proposal
// / tally_of. Bad/empty input is a SILENT no-op (no explanatory-validation text).
// =============================================================================

/// How many of a guild's proposals the board lists.
const PROPOSAL_LIST_LIMIT: u64 = 50;
/// Default proposal voting window (hours) when the field is blank.
const PROPOSAL_DEFAULT_PERIOD_HOURS: u64 = 48;

/// Read the guild id from `#governance-guild` + paint that guild's proposals.
/// Bad/empty id is a silent no-op. Reuses `refresh_governance_list`.
pub(super) fn load_proposals_pressed() {
    let Some(guild_id) = dom::input_by_id("governance-guild")
        .map(|i| i.value().trim().to_string())
        .and_then(|s| s.parse::<u64>().ok())
    else {
        return;
    };
    wasm_bindgen_futures::spawn_local(async move {
        refresh_governance_list(guild_id).await;
    });
}

/// Open a treasury-spend governance proposal from the admin panel (mirrors
/// `post_bounty_pressed`). Reads the guild/to/amount/period fields, opens the
/// proposal in ONE sponsored tx, swaps `#governance-result` for the success
/// panel, then refreshes the proposal list. Bad/empty input is a SILENT no-op.
/// Reuses `registry::propose_sponsored`.
pub(super) fn propose_measure_pressed() {
    let guild_raw = dom::input_by_id("governance-guild")
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();
    let to = dom::input_by_id("governance-to")
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();
    let amount_raw = dom::input_by_id("governance-amount")
        .map(|i| i.value())
        .unwrap_or_default();
    let period_raw = dom::input_by_id("governance-period")
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();

    // Silent no-ops on missing/invalid fields (no explanatory text).
    let Ok(guild_id) = guild_raw.parse::<u64>() else {
        return;
    };
    if to.is_empty() {
        return;
    }
    let Some(amount_wei) = crate::encoding::parse_token_amount(&amount_raw) else {
        return;
    };
    if amount_wei == 0 {
        return;
    }
    // Optional voting period (hours): blank → default; garbage/zero → no-op.
    let period_secs = if period_raw.is_empty() {
        PROPOSAL_DEFAULT_PERIOD_HOURS * 3600
    } else {
        match period_raw.parse::<u64>() {
            Ok(h) if h > 0 => h * 3600,
            _ => return,
        }
    };
    let amount_label: String = amount_raw
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();

    dom::swap_inner(
        "governance-result",
        "<span style=\"color:var(--muted)\">proposing…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            super::sponsor_rate_guard()?;
            // Resolve `to` (address or subdomain name) to a 0x address.
            let to_hex = resolve_governance_recipient(&to).await?;
            let (signer, _) = crate::app::chat::credit_signer()
                .await
                .ok_or_else(|| "no identity".to_string())?;
            let fee_payer = crate::app::sponsor::signer()?;
            crate::app::registry::propose_sponsored(
                &signer,
                &fee_payer,
                guild_id,
                &to_hex,
                amount_wei,
                &[],
                period_secs,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
        }
        .await;
        match result {
            Ok(_) => {
                // New proposal id = the guild's last entry in proposals_of.
                let new_id = crate::app::registry::proposals_of(guild_id, 0, PROPOSAL_LIST_LIMIT)
                    .await
                    .ok()
                    .and_then(|ids| ids.last().copied())
                    .unwrap_or(0);
                dom::swap_inner(
                    "governance-result",
                    &templates::governance_result_panel(new_id, &amount_label).into_string(),
                );
                refresh_governance_list(guild_id).await;
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("propose measure: {e}")));
                dom::swap_inner(
                    "governance-result",
                    &dom::msg_span(dom::Msg::Error, "proposal couldn't be opened"),
                );
            }
        }
    });
}

/// Cast a vote on an open proposal from the board. `data-arg` is
/// `"<proposal_id>:<for|against>"`. Refreshes the list (re-reading the guild id
/// from the field) after voting. Reuses `registry::vote_sponsored`.
pub(super) fn vote_pressed(arg: String) {
    let mut parts = arg.splitn(2, ':');
    let Some(proposal_id) = parts.next().and_then(|s| s.trim().parse::<u64>().ok()) else {
        return;
    };
    let support = matches!(parts.next().map(|s| s.trim()), Some("for"));
    let guild_id = dom::input_by_id("governance-guild")
        .map(|i| i.value().trim().to_string())
        .and_then(|s| s.parse::<u64>().ok());
    dom::swap_inner(
        "governance-result",
        "<span style=\"color:var(--muted)\">voting…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            super::sponsor_rate_guard()?;
            let (signer, _) = crate::app::chat::credit_signer()
                .await
                .ok_or_else(|| "no identity".to_string())?;
            let fee_payer = crate::app::sponsor::signer()?;
            crate::app::registry::vote_sponsored(
                &signer,
                &fee_payer,
                proposal_id,
                support,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
        }
        .await;
        match result {
            Ok(_) => {
                dom::swap_inner(
                    "governance-result",
                    &dom::msg_span(dom::Msg::Muted, "vote recorded"),
                );
                if let Some(g) = guild_id {
                    refresh_governance_list(g).await;
                }
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("vote: {e}")));
                dom::swap_inner(
                    "governance-result",
                    &dom::msg_span(dom::Msg::Error, "couldn't record that vote"),
                );
            }
        }
    });
}

/// Execute a passed proposal (past its deadline) from the board. `data-arg` is
/// the proposal id; refreshes the list (re-reading the guild id) after. Reuses
/// `registry::execute_proposal_sponsored`.
pub(super) fn execute_proposal_pressed(proposal_id_raw: String) {
    let Ok(proposal_id) = proposal_id_raw.trim().parse::<u64>() else {
        return;
    };
    let guild_id = dom::input_by_id("governance-guild")
        .map(|i| i.value().trim().to_string())
        .and_then(|s| s.parse::<u64>().ok());
    dom::swap_inner(
        "governance-result",
        "<span style=\"color:var(--muted)\">executing…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            super::sponsor_rate_guard()?;
            let (signer, _) = crate::app::chat::credit_signer()
                .await
                .ok_or_else(|| "no identity".to_string())?;
            let fee_payer = crate::app::sponsor::signer()?;
            crate::app::registry::execute_proposal_sponsored(
                &signer,
                &fee_payer,
                proposal_id,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
        }
        .await;
        match result {
            Ok(_) => {
                super::refresh_credits_pill().await;
                dom::swap_inner(
                    "governance-result",
                    &dom::msg_span(dom::Msg::Muted, "executed — treasury spend paid out"),
                );
                if let Some(g) = guild_id {
                    refresh_governance_list(g).await;
                }
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("execute proposal: {e}")));
                dom::swap_inner(
                    "governance-result",
                    &dom::msg_span(
                        dom::Msg::Error,
                        "couldn't execute (not passed yet, or before the deadline)",
                    ),
                );
            }
        }
    });
}

/// Resolve a free-form governance recipient (a raw `0x…` address OR a subdomain
/// name → its on-chain owner) to a 0x-hex address. Mirrors `send_lh`'s split.
async fn resolve_governance_recipient(arg: &str) -> Result<String, String> {
    use crate::encoding::Recipient;
    match crate::encoding::classify_recipient(arg)? {
        Recipient::Address(addr) => Ok(addr),
        Recipient::Name(name) => crate::app::registry::owner_of_name(&name)
            .await?
            .ok_or_else(|| format!("no on-chain owner for subdomain \"{name}\"")),
    }
}

/// Read `proposals_of(guild_id, 0, LIMIT)` + paint the guild's proposals into
/// `#governance-list` (per proposal: id, recipient, amount, status, for/against
/// tally + vote/execute buttons). An open proposal shows [for]/[against]; a
/// passed-status proposal shows [execute]. Soft-fails to a quiet line. No-op if
/// the slot isn't mounted. Reuses `registry::{proposals_of, get_proposal,
/// tally_of}`.
pub(crate) async fn refresh_governance_list(guild_id: u64) {
    if dom::by_id("governance-list").is_none() {
        return;
    }
    let ids = match crate::app::registry::proposals_of(guild_id, 0, PROPOSAL_LIST_LIMIT).await {
        Ok(v) => v,
        Err(_) => {
            dom::swap_inner("governance-list", "");
            return;
        }
    };
    if ids.is_empty() {
        dom::swap_inner(
            "governance-list",
            &dom::msg_span(dom::Msg::Muted, "no proposals"),
        );
        return;
    }
    let mut rows: Vec<maud::Markup> = Vec::new();
    for id in ids {
        let Ok(p) = crate::app::registry::get_proposal(id).await else {
            continue;
        };
        let (votes_for, votes_against) = crate::app::registry::tally_of(id)
            .await
            .map(|t| (t.for_votes, t.against_votes))
            .unwrap_or((0, 0));
        let whole = p.amount / 1_000_000_000_000_000_000u128;
        let cents = (p.amount % 1_000_000_000_000_000_000u128) / 10_000_000_000_000_000u128;
        let status = p.status_label();
        let is_open = p.status == 0;
        // Inline monochrome styles — same self-contained convention as
        // `refresh_bounty_list`. maud `(…)` escapes the RPC-sourced address text.
        rows.push(maud::html! {
            div style="border-top:1px solid var(--border);padding:6px 0;font-size:11px;color:var(--fg)" {
                div style="display:flex;align-items:center;gap:8px" {
                    code style="color:var(--muted)" { "#" (id) }
                    span style="flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" { (p.to) }
                    span style="color:var(--muted)" { (whole) "." (format!("{cents:02}")) " LH" }
                    span style="color:var(--muted)" { (status) }
                }
                div style="display:flex;align-items:center;gap:8px;margin-top:4px;color:var(--muted)" {
                    span { "for " (votes_for) " · against " (votes_against) }
                    span style="flex:1" {}
                    @if is_open {
                        button type="button" data-action="vote" data-arg=(format!("{id}:for"))
                            .ghost style="padding:0 6px" { "vote for" }
                        button type="button" data-action="vote" data-arg=(format!("{id}:against"))
                            .ghost style="padding:0 6px" { "vote against" }
                        button type="button" data-action="execute-proposal" data-arg=(id.to_string())
                            .ghost style="padding:0 6px" { "execute" }
                    }
                }
            }
        });
    }
    let html = maud::html! {
        div style="margin-top:8px" { @for r in &rows { (r) } }
    }
    .into_string();
    dom::swap_inner("governance-list", &html);
}
