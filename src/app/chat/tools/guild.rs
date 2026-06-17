// =============================================================================
// Guild tools — the in-tab agent founds + runs an on-chain GUILD (GuildFacet):
// a durable org with members, roles, and a pooled `$LH` treasury. Same sponsored
// path as the bounty tools (owner's credit key signs, the embedded sponsor pays
// gas). Registry helpers reused: create_guild_sponsored / invite_to_guild_sponsored
// / fund_guild_sponsored / spend_treasury_sponsored + reads guilds_of / guild_name
// / treasury_balance_of. Never re-encoded here.
// =============================================================================

use crate::app::chat::access::credit_address_existing;
use crate::tools::ClosureTool;

use super::bounty::bounty_signers;

/// `create_guild(name)` — found an on-chain guild (members + roles + a pooled
/// `$LH` treasury); the caller becomes its founding Admin. Reuses
/// `registry::create_guild_sponsored`; reads the new id back from
/// `guilds_of(caller)`'s last entry.
pub(crate) fn create_guild_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Display name for the guild (a short label for the org)."
            }
        },
        "required": ["name"]
    });
    ClosureTool::new(
        "create_guild",
        "Found an on-chain GUILD: a durable org with members, roles, and a pooled $LH \
         treasury. You become its founding Admin. Use this to organize a standing team \
         of agents (as opposed to a one-off bounty). Returns { guild_id, name, treasury, \
         tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            if name.is_empty() {
                return Err(crate::error::Error::other("name cannot be empty"));
            }
            let (signer, fee_payer) = bounty_signers().await?;
            let tx_hash = crate::app::registry::create_guild_sponsored(
                &signer,
                &fee_payer,
                name,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
            .map_err(|e| crate::error::Error::other(format!("create_guild failed: {e}")))?;
            // New guild id = the caller's last entry in guilds_of (best-effort).
            let (guild_id, treasury) = match credit_address_existing().await {
                Some(addr) => match crate::app::registry::guilds_of(&addr).await.ok().and_then(|ids| {
                    ids.last().copied()
                }) {
                    Some(id) => {
                        let t = crate::app::registry::guild_address(id).await.unwrap_or_default();
                        (Some(id), t)
                    }
                    None => (None, String::new()),
                },
                None => (None, String::new()),
            };
            let mut result = serde_json::json!({
                "name": name,
                "tx_hash": tx_hash,
            });
            if let Some(id) = guild_id {
                result["guild_id"] = serde_json::json!(id);
            }
            if !treasury.is_empty() {
                result["treasury"] = serde_json::json!(treasury);
            }
            Ok(result)
        },
    )
}

/// `invite_to_guild(guild_id, member)` — invite an address or a subdomain name
/// (its on-chain owner) into a guild the caller administers. Admin-gated
/// on-chain. Reuses `registry::invite_to_guild_sponsored`.
pub(crate) fn invite_to_guild_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "guild_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of the guild you administer."
            },
            "member": {
                "type": "string",
                "description": "Who to invite — a raw 0x… address OR a subdomain name \
                    (resolved to that name's on-chain owner)."
            }
        },
        "required": ["guild_id", "member"]
    });
    ClosureTool::new(
        "invite_to_guild",
        "Invite an address or subdomain name (its on-chain owner) into a guild you \
         administer; they join by accepting. Admin-gated on-chain (only a guild Admin \
         can invite). Returns { guild_id, member, resolved_member, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let guild_id = args
                .get("guild_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("guild_id is required"))?;
            let member_arg = args
                .get("member")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let member_hex = resolve_account(&member_arg).await?;
            let (signer, fee_payer) = bounty_signers().await?;
            let tx_hash = crate::app::registry::invite_to_guild_sponsored(
                &signer,
                &fee_payer,
                guild_id,
                &member_hex,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
            .map_err(|e| crate::error::Error::other(format!("invite_to_guild failed: {e}")))?;
            Ok(serde_json::json!({
                "guild_id": guild_id,
                "member": member_arg,
                "resolved_member": member_hex,
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `fund_guild(guild_id, amount_lh)` — contribute `$LH` from the caller's wallet
/// into a guild's shared treasury. Reuses `registry::fund_guild_sponsored`.
pub(crate) fn fund_guild_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "guild_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of the guild to fund."
            },
            "amount_lh": {
                "type": "string",
                "description": "Amount of $LH to contribute, as a decimal string \
                    (\"5\", \"1.5\"). Pulled from YOUR wallet into the shared treasury. \
                    Must be > 0."
            }
        },
        "required": ["guild_id", "amount_lh"]
    });
    ClosureTool::new(
        "fund_guild",
        "Contribute $LH from your wallet into a guild's pooled treasury. Anyone can \
         fund; spending the treasury is Admin-gated. Moves value: confirm the amount \
         with the owner before calling. Returns { guild_id, amount_lh, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let guild_id = args
                .get("guild_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("guild_id is required"))?;
            let amount_arg = args
                .get("amount_lh")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let amount_wei = crate::encoding::parse_token_amount(&amount_arg).ok_or_else(|| {
                crate::error::Error::other(format!(
                    "could not parse amount_lh \"{amount_arg}\" — pass a decimal $LH \
                     figure like \"5\" or \"1.5\""
                ))
            })?;
            if amount_wei == 0 {
                return Err(crate::error::Error::other("amount_lh must be greater than 0"));
            }
            let (signer, fee_payer) = bounty_signers().await?;
            // Escrow auto-bridge (feedback #63): a wallet shortfall covered by
            // unspent chat-meter credits rides as a withdrawCredits call in the
            // SAME atomic tx as approve+fundGuild.
            let from_hex =
                crate::encoding::bytes_to_hex_str(&crate::wallet::address(&signer));
            let bridge_wei = crate::app::chat::escrow_bridge_wei(&from_hex, amount_wei)
                .await
                .map_err(crate::error::Error::other)?;
            let tx_hash = crate::app::registry::fund_guild_sponsored_bridged(
                &signer,
                &fee_payer,
                guild_id,
                amount_wei,
                crate::app::registry::ALPHA_USD_ADDRESS,
                bridge_wei,
            )
            .await
            .map_err(|e| crate::error::Error::other(format!("fund_guild failed: {e}")))?;
            Ok(serde_json::json!({
                "guild_id": guild_id,
                "amount_lh": amount_arg,
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `spend_treasury(guild_id, to, amount_lh, memo?)` — pay `$LH` OUT of a guild's
/// pooled treasury. Admin-gated ON-CHAIN. Reuses
/// `registry::spend_treasury_sponsored`.
pub(crate) fn spend_treasury_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "guild_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of the guild whose treasury to spend from."
            },
            "to": {
                "type": "string",
                "description": "Recipient — a raw 0x… address OR a subdomain name \
                    (resolved to that name's on-chain owner)."
            },
            "amount_lh": {
                "type": "string",
                "description": "Amount of $LH to pay out, as a decimal string. Must be > 0."
            },
            "memo": {
                "type": "string",
                "description": "OPTIONAL note recorded with the payment (what it's for)."
            },
            "confirmation": {
                "type": "string",
                "description": "Single-use confirmation code. OMIT (or pass \"\") on the \
                    first call — it returns a challenge code shown to the owner. Relay \
                    it, wait for the owner to TYPE the code in chat, then retry with it. \
                    Never invent it; only the platform issues it."
            }
        },
        "required": ["guild_id", "to", "amount_lh"]
    });
    ClosureTool::new(
        "spend_treasury",
        "Pay $LH OUT of a guild's pooled treasury to an address or subdomain name, with \
         an optional memo. Admin-gated ON-CHAIN: only a guild Admin can spend (the call \
         reverts otherwise). MOVES VALUE (non-refundable, arbitrary recipient) — the \
         first call does NOT execute: it returns a single-use confirmation code (also \
         shown to the owner in the UI). State the recipient + amount, ask the owner to \
         TYPE the code, then retry with `confirmation` set to it. Returns \
         { guild_id, to, resolved_to, amount_lh, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let guild_id = args
                .get("guild_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("guild_id is required"))?;
            let to_arg = args
                .get("to")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let amount_arg = args
                .get("amount_lh")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let memo = args.get("memo").and_then(|v| v.as_str()).unwrap_or("").trim();
            let amount_wei = crate::encoding::parse_token_amount(&amount_arg).ok_or_else(|| {
                crate::error::Error::other(format!(
                    "could not parse amount_lh \"{amount_arg}\" — pass a decimal $LH \
                     figure like \"5\" or \"1.5\""
                ))
            })?;
            if amount_wei == 0 {
                return Err(crate::error::Error::other("amount_lh must be greater than 0"));
            }
            let to_hex = resolve_account(&to_arg).await?;
            let (signer, fee_payer) = bounty_signers().await?;
            let tx_hash = crate::app::registry::spend_treasury_sponsored(
                &signer,
                &fee_payer,
                guild_id,
                &to_hex,
                amount_wei,
                memo.as_bytes(),
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
            .map_err(|e| crate::error::Error::other(format!("spend_treasury failed: {e}")))?;
            Ok(serde_json::json!({
                "guild_id": guild_id,
                "to": to_arg,
                "resolved_to": to_hex,
                "amount_lh": amount_arg,
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `list_my_guilds()` — read-only: every guild the caller belongs to, each with
/// its name + pooled treasury balance. Reuses `registry::{guilds_of, guild_name,
/// treasury_balance_of}`.
pub(crate) fn list_my_guilds_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    ClosureTool::new(
        "list_my_guilds",
        "List every guild you belong to — each with its id, name, and pooled $LH \
         treasury balance. Read-only. Use when asked about your guilds/orgs. Returns \
         { guilds: [ { guild_id, name, treasury_lh } ], count }.",
        serde_json::json!({ "type": "object", "properties": {}, "required": [] }),
        |_args: serde_json::Value, _ctx| async move {
            let addr = credit_address_existing()
                .await
                .ok_or_else(|| crate::error::Error::other("no identity — claim a subdomain first"))?;
            let ids = crate::app::registry::guilds_of(&addr)
                .await
                .map_err(crate::error::Error::other)?;
            let mut guilds = Vec::new();
            for id in ids {
                let name = crate::app::registry::guild_name(id).await.unwrap_or_default();
                let treasury_wei = crate::app::registry::treasury_balance_of(id).await.unwrap_or(0);
                guilds.push(serde_json::json!({
                    "guild_id": id,
                    "name": name,
                    "treasury_lh": format_lh(treasury_wei),
                }));
            }
            Ok(serde_json::json!({
                "count": guilds.len(),
                "guilds": guilds,
            }))
        },
    )
}

/// Resolve a free-form account argument (a raw `0x…` address OR a subdomain
/// name) to a 0x-hex address — names map to their on-chain owner. Shared by the
/// guild invite/spend tools (mirrors `send_lh`'s `classify_recipient` branch).
pub(crate) async fn resolve_account(arg: &str) -> Result<String, crate::error::Error> {
    use crate::encoding::Recipient;
    let kind = crate::encoding::classify_recipient(arg).map_err(crate::error::Error::other)?;
    match kind {
        Recipient::Address(addr) => Ok(addr),
        Recipient::Name(name) => crate::app::registry::owner_of_name(&name)
            .await
            .map_err(crate::error::Error::other)?
            .ok_or_else(|| {
                crate::error::Error::other(format!(
                    "no on-chain owner for subdomain \"{name}\" — is it registered?"
                ))
            }),
    }
}

/// Resolve THIS subdomain's own on-chain tokenId (the claimant id for
/// `claim_bounty`). Errors clearly off-subdomain or pre-registration.
pub(crate) async fn own_token_id() -> Result<u64, crate::error::Error> {
    let tenant = crate::app::tenant::current_name().ok_or_else(|| {
        crate::error::Error::other("not running on a subdomain — no agent identity to claim as")
    })?;
    match crate::app::registry::id_of_name(&tenant).await {
        Ok(id) if id != 0 => Ok(id),
        Ok(_) => Err(crate::error::Error::other(
            "this subdomain isn't registered on-chain yet — claim it first",
        )),
        Err(e) => Err(crate::error::Error::other(format!("id_of_name: {e}"))),
    }
}

/// Render 18-decimal `$LH` wei as a compact decimal string for tool output
/// (whole + 2 fractional digits), matching the bounty board's display.
pub(crate) fn format_lh(wei: u128) -> String {
    let whole = wei / 1_000_000_000_000_000_000u128;
    let cents = (wei % 1_000_000_000_000_000_000u128) / 10_000_000_000_000_000u128;
    format!("{whole}.{cents:02}")
}
