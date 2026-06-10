//! Guilds — create + fund pooled-treasury orgs (GuildFacet).

use wasm_bindgen::prelude::*;

use crate::app::{dom, templates};

/// How many of the caller's guilds the board lists.
const GUILD_LIST_LIMIT: usize = 25;

/// Create a guild from the admin panel (mirrors `post_bounty_pressed`). Reads
/// the name input, mints the guild (caller = founding Admin) in ONE sponsored
/// tx, swaps `#guild-result` for the success panel, then refreshes the guild
/// list. Bad/empty input is a SILENT no-op (no explanatory-validation text).
/// Reuses `registry::create_guild_sponsored`.
pub(super) fn create_guild_pressed() {
    let name = dom::input_by_id("guild-name")
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();
    if name.is_empty() {
        return;
    }
    dom::swap_inner(
        "guild-result",
        "<span style=\"color:var(--muted)\">creating…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            super::sponsor_rate_guard()?;
            let (signer, _) = crate::app::chat::credit_signer()
                .await
                .ok_or_else(|| "no identity".to_string())?;
            let fee_payer = crate::app::sponsor::signer()?;
            crate::app::registry::create_guild_sponsored(
                &signer,
                &fee_payer,
                &name,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
        }
        .await;
        match result {
            Ok(_) => {
                // New guild id = the caller's last entry in guilds_of.
                let new_id = match crate::app::chat::credit_address_existing().await {
                    Some(addr) => crate::app::registry::guilds_of(&addr)
                        .await
                        .ok()
                        .and_then(|ids| ids.last().copied())
                        .unwrap_or(0),
                    None => 0,
                };
                dom::swap_inner(
                    "guild-result",
                    &templates::guild_result_panel(new_id, &name).into_string(),
                );
                refresh_guild_list().await;
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("create guild: {e}")));
                dom::swap_inner(
                    "guild-result",
                    &dom::msg_span(dom::Msg::Error, "guild couldn't be created"),
                );
            }
        }
    });
}

/// Fund a guild's pooled treasury from its per-row input (GuildFacet
/// `fundGuild`). The `data-arg` is the guild id; the amount is read from the
/// row's `#guild-fund-<id>` field. Bad/empty input is a SILENT no-op. Reuses
/// `registry::fund_guild_sponsored`.
pub(super) fn fund_guild_pressed(guild_id_raw: String) {
    let Ok(guild_id) = guild_id_raw.trim().parse::<u64>() else {
        return;
    };
    let amt_raw = dom::input_by_id(&format!("guild-fund-{guild_id}"))
        .map(|i| i.value())
        .unwrap_or_default();
    let Some(amount_wei) = crate::encoding::parse_token_amount(&amt_raw) else {
        return;
    };
    if amount_wei == 0 {
        return;
    }
    dom::swap_inner(
        "guild-result",
        "<span style=\"color:var(--muted)\">funding…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            super::sponsor_rate_guard()?;
            let (signer, _) = crate::app::chat::credit_signer()
                .await
                .ok_or_else(|| "no identity".to_string())?;
            let fee_payer = crate::app::sponsor::signer()?;
            crate::app::registry::fund_guild_sponsored(
                &signer,
                &fee_payer,
                guild_id,
                amount_wei,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
        }
        .await;
        match result {
            Ok(_) => {
                super::refresh_credits_pill().await;
                dom::swap_inner(
                    "guild-result",
                    &dom::msg_span(dom::Msg::Muted, "funded the guild treasury"),
                );
                refresh_guild_list().await;
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("fund guild: {e}")));
                dom::swap_inner(
                    "guild-result",
                    &dom::msg_span(dom::Msg::Error, "couldn't fund (need $LH to contribute)"),
                );
            }
        }
    });
}

/// Read `guilds_of(caller)` + paint the caller's guilds into `#guild-list` (per
/// guild: id, name, treasury balance + a fund field/button). Soft-fails to a
/// quiet line. Called on admin open + after every create/fund. No-op if the slot
/// isn't mounted or no identity exists yet. Reuses `registry::{guilds_of,
/// guild_name, treasury_balance_of}`.
pub(crate) async fn refresh_guild_list() {
    if dom::by_id("guild-list").is_none() {
        return;
    }
    let Some(addr) = crate::app::chat::credit_address_existing().await else {
        dom::swap_inner("guild-list", "");
        return;
    };
    let ids = match crate::app::registry::guilds_of(&addr).await {
        Ok(v) => v,
        Err(_) => {
            dom::swap_inner("guild-list", "");
            return;
        }
    };
    if ids.is_empty() {
        dom::swap_inner("guild-list", &dom::msg_span(dom::Msg::Muted, "no guilds yet"));
        return;
    }
    let mut rows: Vec<maud::Markup> = Vec::new();
    for id in ids.into_iter().take(GUILD_LIST_LIMIT) {
        let name = crate::app::registry::guild_name(id)
            .await
            .ok()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("guild#{id}"));
        let treasury_wei = crate::app::registry::treasury_balance_of(id).await.unwrap_or(0);
        let whole = treasury_wei / 1_000_000_000_000_000_000u128;
        let cents = (treasury_wei % 1_000_000_000_000_000_000u128) / 10_000_000_000_000_000u128;
        // Inline monochrome styles — same self-contained convention as
        // `refresh_bounty_list`. maud `(…)` escapes the RPC-sourced name text.
        rows.push(maud::html! {
            div style="border-top:1px solid var(--border);padding:6px 0;font-size:11px;color:var(--fg)" {
                div style="display:flex;align-items:center;gap:8px" {
                    code style="color:var(--muted)" { "#" (id) }
                    span style="flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" { (name) }
                    span style="color:var(--muted)" { (whole) "." (format!("{cents:02}")) " LH" }
                }
                div style="display:flex;align-items:center;gap:8px;margin-top:4px" {
                    input id=(format!("guild-fund-{id}")) .redeem-input type="text"
                        inputmode="decimal" aria-label="fund amount in $LH"
                        placeholder="$LH" style="flex:1";
                    button type="button" data-action="fund-guild" data-arg=(id.to_string())
                        .ghost style="padding:0 6px" { "fund" }
                }
            }
        });
    }
    let html = maud::html! {
        div style="margin-top:8px" { @for r in &rows { (r) } }
    }
    .into_string();
    dom::swap_inner("guild-list", &html);
}
