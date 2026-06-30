// =============================================================================
// Company tools — read a "company" (an on-chain GUILD: org identity + pooled $LH
// treasury + ranked members) as ONE snapshot, composing EXISTING registry reads
// only (guilds_of / guild_name / members_of_guild / role_of_guild /
// treasury_balance_of / guild_address). No new on-chain surface. The honest
// reduction (design/autonomous-business/COMPANY-FEATURE.md): a company is not a
// new object — it's a named composition of a guild + role members + a treasury.
// `found_company` (the write half) is a later slice; this ships the read half.
// =============================================================================

use crate::app::chat::access::credit_address_existing;
use crate::tools::ClosureTool;

use super::guild::format_lh;

/// `company_status(company)` — READ-ONLY snapshot of a company (a guild): its
/// members with their on-chain roles and its pooled `$LH` treasury. `company` is
/// a numeric guild id OR a guild display name (matched, case-insensitively, among
/// the guilds the caller belongs to). Composes existing reads only — no write, no
/// `$LH`, not confirm-gated.
pub(crate) fn company_status_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "company": {
                "type": "string",
                "description": "Which company/guild to report on — a numeric guild id \
                    (e.g. \"67\") OR a guild display name you belong to."
            }
        },
        "required": ["company"]
    });
    ClosureTool::new(
        "company_status",
        "Read-only snapshot of a COMPANY (an on-chain guild): its members with their \
         roles (admin / officer / member) and its pooled $LH treasury (the guild's \
         token-bound account). `company` is a numeric guild id OR a guild name you \
         belong to. Use it to inspect an org's roster + treasury before acting on it. \
         Returns { guild_id, name, treasury_address, treasury_lh, member_count, \
         members: [ { address, role } ] }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let company = args
                .get("company")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if company.is_empty() {
                return Err(crate::error::Error::other("company cannot be empty"));
            }
            let guild_id = resolve_guild(&company).await?;
            // Read the org snapshot from EXISTING views. The name/treasury reads are
            // best-effort (a transient RPC miss shouldn't sink the whole report); the
            // member roster is the load-bearing read, so its failure surfaces.
            let name = crate::app::registry::guild_name(guild_id).await.unwrap_or_default();
            let treasury_address = crate::app::registry::guild_address(guild_id)
                .await
                .unwrap_or_default();
            let treasury_wei = crate::app::registry::treasury_balance_of(guild_id)
                .await
                .unwrap_or(0);
            let addrs = crate::app::registry::members_of_guild(guild_id)
                .await
                .map_err(|e| crate::error::Error::other(format!("members_of_guild: {e}")))?;
            let mut members = Vec::with_capacity(addrs.len());
            for addr in &addrs {
                let role = crate::app::registry::role_of_guild(guild_id, addr)
                    .await
                    .map(|r| r.label())
                    .unwrap_or("unknown");
                members.push(serde_json::json!({
                    "address": addr,
                    "role": role,
                }));
            }
            Ok(serde_json::json!({
                "guild_id": guild_id,
                "name": name,
                "treasury_address": treasury_address,
                "treasury_lh": format_lh(treasury_wei),
                "member_count": members.len(),
                "members": members,
            }))
        },
    )
}

/// Resolve a free-form company argument — a numeric guild id OR a guild display
/// name (matched, case-insensitively, among the guilds the caller belongs to) —
/// to a concrete guild id. A bare integer is taken as the id directly; otherwise
/// the caller's `guilds_of` roster is scanned by name.
async fn resolve_guild(arg: &str) -> Result<u64, crate::error::Error> {
    if let Ok(id) = arg.parse::<u64>() {
        return Ok(id);
    }
    let addr = credit_address_existing()
        .await
        .ok_or_else(|| crate::error::Error::other("no identity — claim a subdomain first"))?;
    let ids = crate::app::registry::guilds_of(&addr)
        .await
        .map_err(crate::error::Error::other)?;
    let want = arg.to_ascii_lowercase();
    for id in ids {
        let name = crate::app::registry::guild_name(id).await.unwrap_or_default();
        if name.to_ascii_lowercase() == want {
            return Ok(id);
        }
    }
    Err(crate::error::Error::other(format!(
        "no guild named \"{arg}\" among the guilds you belong to — pass a numeric guild id, \
         or use list_my_guilds to find it"
    )))
}
