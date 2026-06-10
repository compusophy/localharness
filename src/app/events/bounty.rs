//! Bounty board — post / claim open bounties (BountyFacet).

use wasm_bindgen::prelude::*;

use crate::app::{dom, templates};

/// Default bounty lifetime when the owner leaves the TTL blank: 24 hours.
const BOUNTY_DEFAULT_TTL_HOURS: u64 = 24;
/// How many open bounties the board lists (the `open_bounties` page size).
const BOUNTY_LIST_LIMIT: u64 = 25;

/// Post a bounty from the admin panel (mirrors `schedule_job_pressed`). Reads
/// the task/reward/ttl inputs, escrows the reward behind `postBounty` in ONE
/// sponsored tx, swaps `#bounty-result` for the success panel, then refreshes
/// the open-bounties list + credits pill. Bad/empty input is a SILENT no-op
/// (no explanatory-validation text). Reuses `registry::post_bounty_sponsored`.
pub(super) fn post_bounty_pressed() {
    let task = dom::input_by_id("bounty-task")
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();
    let reward_raw = dom::input_by_id("bounty-reward")
        .map(|i| i.value())
        .unwrap_or_default();
    let ttl_raw = dom::input_by_id("bounty-ttl")
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();

    // Silent no-ops on missing/invalid fields (no explanatory text).
    if task.is_empty() {
        return;
    }
    let Some(reward_wei) = crate::encoding::parse_token_amount(&reward_raw) else {
        return;
    };
    if reward_wei == 0 {
        return;
    }
    // Optional TTL (hours): blank → default; garbage/zero → silent no-op.
    let ttl_secs = if ttl_raw.is_empty() {
        BOUNTY_DEFAULT_TTL_HOURS * 3600
    } else {
        match ttl_raw.parse::<u64>() {
            Ok(h) if h > 0 => h * 3600,
            _ => return,
        }
    };
    let reward_label: String = reward_raw
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();

    dom::swap_inner(
        "bounty-result",
        "<span style=\"color:var(--muted)\">posting…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            super::sponsor_rate_guard()?;
            let (signer, _) = crate::app::chat::credit_signer()
                .await
                .ok_or_else(|| "no identity".to_string())?;
            let fee_payer = crate::app::sponsor::signer()?;
            crate::app::registry::post_bounty_sponsored(
                &signer,
                &fee_payer,
                task.as_bytes(),
                reward_wei,
                ttl_secs,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
        }
        .await;
        match result {
            Ok(_) => {
                super::refresh_credits_pill().await;
                // New bounty id = the caller's last entry in bounties_of.
                let new_id = match crate::app::chat::credit_address_existing().await {
                    Some(addr) => crate::app::registry::bounties_of(&addr)
                        .await
                        .ok()
                        .and_then(|ids| ids.last().copied())
                        .unwrap_or(0),
                    None => 0,
                };
                dom::swap_inner(
                    "bounty-result",
                    &templates::bounty_result_panel(new_id, &reward_label).into_string(),
                );
                refresh_bounty_list().await;
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("post bounty: {e}")));
                dom::swap_inner(
                    "bounty-result",
                    &dom::msg_span(dom::Msg::Error, "bounty couldn't be posted (need $LH to escrow)"),
                );
            }
        }
    });
}

/// Claim an open bounty from the board (BountyFacet `claimBounty`). The
/// claimant is THIS subdomain's own on-chain tokenId. Then refresh the list.
/// Reuses `registry::claim_bounty_sponsored`.
pub(super) fn claim_bounty_pressed(bounty_id_raw: String) {
    let Ok(bounty_id) = bounty_id_raw.trim().parse::<u64>() else {
        return;
    };
    dom::swap_inner(
        "bounty-result",
        "<span style=\"color:var(--muted)\">claiming…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            super::sponsor_rate_guard()?;
            // Claimant = this subdomain's own tokenId.
            let tenant = crate::app::tenant::require_tenant()?;
            let claimant_token_id = crate::app::registry::id_of_name(&tenant).await?;
            if claimant_token_id == 0 {
                return Err("this subdomain isn't registered on-chain yet".to_string());
            }
            let (signer, _) = crate::app::chat::credit_signer()
                .await
                .ok_or_else(|| "no identity".to_string())?;
            let fee_payer = crate::app::sponsor::signer()?;
            crate::app::registry::claim_bounty_sponsored(
                &signer,
                &fee_payer,
                bounty_id,
                claimant_token_id,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
        }
        .await;
        match result {
            Ok(_) => {
                dom::swap_inner(
                    "bounty-result",
                    &dom::msg_span(
                        dom::Msg::Muted,
                        "claimed — work the task, then submit_result via chat",
                    ),
                );
                refresh_bounty_list().await;
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("claim bounty: {e}")));
                dom::swap_inner(
                    "bounty-result",
                    &dom::msg_span(dom::Msg::Error, "couldn't claim that bounty"),
                );
            }
        }
    });
}

/// Read `open_bounties(0, LIMIT)` + paint the open-bounties list into
/// `#bounty-list` (per bounty: id, reward, task preview + a claim button).
/// Soft-fails to a quiet line. Called on admin open + after every post/claim.
/// No-op if the slot isn't mounted. Reuses `registry::{open_bounties,
/// get_bounty, task_of_bounty}`.
pub(crate) async fn refresh_bounty_list() {
    if dom::by_id("bounty-list").is_none() {
        return;
    }
    let ids = match crate::app::registry::open_bounties(0, BOUNTY_LIST_LIMIT).await {
        Ok(v) => v,
        Err(_) => {
            dom::swap_inner("bounty-list", "");
            return;
        }
    };
    if ids.is_empty() {
        dom::swap_inner(
            "bounty-list",
            &dom::msg_span(dom::Msg::Muted, "no open bounties"),
        );
        return;
    }
    let mut rows: Vec<maud::Markup> = Vec::new();
    for id in ids {
        // get_bounty → (poster, reward_wei, expiry, status, claimant). We only
        // surface id / reward / task / claimed-flag here. A failed read skips
        // the row rather than failing the whole list.
        let Ok(b) = crate::app::registry::get_bounty(id).await else {
            continue;
        };
        let reward_wei = b.reward_wei;
        let claimant = b.claimant_token_id;
        let task = crate::app::registry::task_of_bounty(id)
            .await
            .ok()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| format!("bounty#{id}"));
        let reward_whole = reward_wei / 1_000_000_000_000_000_000u128;
        let reward_cents =
            (reward_wei % 1_000_000_000_000_000_000u128) / 10_000_000_000_000_000u128;
        // An unclaimed bounty (claimant tokenId 0) can still be claimed here.
        let claimable = claimant == 0;
        // Inline monochrome styles — same self-contained convention as
        // `refresh_jobs_list`. maud `(…)` escapes the RPC-sourced task text.
        rows.push(maud::html! {
            div style="border-top:1px solid var(--border);padding:6px 0;font-size:11px;color:var(--fg)" {
                div style="display:flex;align-items:center;gap:8px" {
                    code style="color:var(--muted)" { "#" (id) }
                    span style="flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" { (task) }
                    span style="color:var(--muted)" { (reward_whole) "." (format!("{reward_cents:02}")) " LH" }
                    @if claimable {
                        button type="button" data-action="claim-bounty" data-arg=(id.to_string())
                            .ghost style="padding:0 6px" { "claim" }
                    } @else {
                        span style="color:var(--muted)" { "claimed" }
                    }
                }
            }
        });
    }
    let html = maud::html! {
        div style="margin-top:8px" { @for r in &rows { (r) } }
    }
    .into_string();
    dom::swap_inner("bounty-list", &html);
}
