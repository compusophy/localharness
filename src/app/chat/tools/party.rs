// =============================================================================
// Party economy tools — the in-tab agent participates in PartyFacet (rung 2 of
// the coordination ladder: bounty → PARTY → guild → DAO) via the SAME sponsored
// path as send_lh / post_bounty (owner authority: the owner's apex wallet signs,
// the bundle sponsor pays gas). A party is an EPHEMERAL squad of agent identities
// formed around one objective: the creator proposes members + a bps split (fixed
// at formation), each member's owner CONSENTS (join), anyone FUNDS the pot, and
// the creator COMPLETES — the pot splits to the member TBAs by shares, then the
// party dissolves. disband (creator any time; anyone after the ttl) refunds every
// funder exactly. The registry helpers (form_party_sponsored, join_party_sponsored,
// fund_party_sponsored_bridged, complete_party_sponsored, disband_party_sponsored,
// get_party, live_parties, parties_of, party_members_of, party_shares_of) are
// reused — never re-encoded here.
// =============================================================================

use crate::tools::ClosureTool;

use super::bounty::bounty_signer;
use super::guild::format_lh;

/// Resolve a `member` spec to its registry tokenId: a bare/`#`-prefixed number
/// is a tokenId used as-is (existence is checked on-chain by formParty); a name
/// resolves via `id_of_name` (0 = unregistered → a named error). Mirrors the
/// CLI's `party::resolve_member_token_id`.
async fn resolve_member_token_id(member: &str) -> Result<u64, crate::error::Error> {
    let trimmed = member.trim().trim_start_matches('#');
    if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
        return trimmed
            .parse::<u64>()
            .map_err(|_| crate::error::Error::other(format!("invalid member id \"{member}\"")));
    }
    match crate::app::registry::id_of_name(&member.trim().to_ascii_lowercase()).await {
        Ok(0) => Err(crate::error::Error::other(format!(
            "\"{member}\" is not registered"
        ))),
        Ok(id) => Ok(id),
        Err(e) => Err(crate::error::Error::other(format!(
            "RPC error resolving \"{member}\": {e}"
        ))),
    }
}

/// `form_party(members, shares?, ttl_hours?)` — propose a squad + a fixed bps
/// split, escrow-backed and settled to the member TBAs on complete. `members`
/// are subdomain names or token ids (#7). `shares` (optional, parallel array of
/// bps that MUST sum to 10000) fixes each member's cut; omit ALL of them for an
/// equal split (remainder to the first member). `ttl_hours` defaults to 168h
/// (7d). Reuses `registry::form_party_sponsored`.
pub(crate) fn form_party_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "members": {
                "type": "array",
                "items": { "type": "string" },
                "description": "The member identities — subdomain names (\"alice\") or \
                    token ids (\"#7\" / \"7\"). Each becomes a seat that must consent \
                    (via join_party) before the party can complete."
            },
            "shares": {
                "type": "array",
                "items": { "type": "integer", "minimum": 1, "maximum": 10000 },
                "description": "OPTIONAL parallel array of each member's share in basis \
                    points (1..10000), in the SAME order as `members`; MUST sum to \
                    10000. Omit entirely for an equal split (remainder to the first \
                    member). If given, its length must match `members`."
            },
            "ttl_hours": {
                "type": "string",
                "description": "OPTIONAL lifetime in hours before the party expires \
                    (decimal). Omit for the 168h (7d) default."
            }
        },
        "required": ["members"]
    });
    ClosureTool::new(
        "form_party",
        "Form an on-chain party: an ad-hoc squad of agent identities around one goal, \
         with a fixed bps split. Each member consents (join_party), anyone funds the pot \
         (fund_party), then you (the creator) complete_party to split the pot to the \
         members' TBAs by their shares. Use this to coordinate a paid collaboration. \
         Returns { party_id, members, shares, ttl_hours, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let members_arg: Vec<String> = args
                .get("members")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            if members_arg.is_empty() {
                return Err(crate::error::Error::other(
                    "members cannot be empty — pass at least one name or token id",
                ));
            }
            // Resolve every member to its tokenId BEFORE signing anything.
            let mut member_ids: Vec<u64> = Vec::with_capacity(members_arg.len());
            for m in &members_arg {
                member_ids.push(resolve_member_token_id(m).await?);
            }
            // Shares: explicit parallel bps array (must sum to 10000) or an equal
            // split with the remainder folded into the FIRST member (the CLI's
            // `parse_member_specs` semantics).
            let shares: Vec<u16> = match args.get("shares").and_then(|v| v.as_array()) {
                Some(arr) if !arr.is_empty() => {
                    if arr.len() != member_ids.len() {
                        return Err(crate::error::Error::other(format!(
                            "shares has {} entries but members has {} — give EVERY member \
                             a share or NONE (equal split)",
                            arr.len(),
                            member_ids.len()
                        )));
                    }
                    let mut out: Vec<u16> = Vec::with_capacity(arr.len());
                    for v in arr {
                        let bps = v.as_u64().filter(|&b| b > 0 && b <= 10_000).ok_or_else(|| {
                            crate::error::Error::other("each share must be 1..10000 bps")
                        })?;
                        out.push(bps as u16);
                    }
                    let sum: u32 = out.iter().map(|&b| b as u32).sum();
                    if sum != 10_000 {
                        return Err(crate::error::Error::other(format!(
                            "shares must sum to 10000 bps, got {sum}"
                        )));
                    }
                    out
                }
                _ => {
                    // Equal split; the FIRST member takes the rounding remainder.
                    let n = member_ids.len() as u16;
                    let base = 10_000 / n;
                    let remainder = 10_000 - base * n;
                    (0..n)
                        .map(|i| if i == 0 { base + remainder } else { base })
                        .collect()
                }
            };
            // TTL: hours → seconds. Default 168h (7d).
            let ttl_hours: f64 = match args.get("ttl_hours").and_then(|v| v.as_str()) {
                Some(s) if !s.trim().is_empty() => s
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| crate::error::Error::other("ttl_hours must be a number"))?,
                _ => 168.0,
            };
            if ttl_hours <= 0.0 {
                return Err(crate::error::Error::other("ttl_hours must be greater than 0"));
            }
            let ttl_secs = (ttl_hours * 3600.0) as u64;
            let signer = bounty_signer().await?;
            let tx_hash = crate::app::registry::form_party_sponsored(&signer, &member_ids, &shares, ttl_secs)
            .await
            .map_err(|e| crate::error::Error::other(format!("form_party failed: {e}")))?;
            // The new partyId = the creator's last entry in parties_of (best-effort).
            let from_hex = crate::encoding::bytes_to_hex_str(&crate::wallet::address(&signer));
            let party_id = crate::app::registry::parties_of(&from_hex)
                .await
                .ok()
                .and_then(|ids| ids.last().copied());
            let mut result = serde_json::json!({
                "members": member_ids,
                "shares": shares,
                "ttl_hours": ttl_hours,
                "tx_hash": tx_hash,
            });
            if let Some(id) = party_id {
                result["party_id"] = serde_json::json!(id);
            }
            Ok(result)
        },
    )
}

/// `join_party(party_id)` — consent to every member seat THIS agent's owner
/// holds. The last consent flips the party Active. Reuses
/// `registry::join_party_sponsored`.
pub(crate) fn join_party_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "party_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of the party to consent to (from \
                    discover_parties / get_party)."
            }
        },
        "required": ["party_id"]
    });
    ClosureTool::new(
        "join_party",
        "Consent to a party you've been added to as a member. This marks consented every \
         seat your owner holds; the LAST member's consent flips the party Active so it can \
         be funded and completed. Returns { party_id, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let party_id = args
                .get("party_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("party_id is required"))?;
            let signer = bounty_signer().await?;
            let tx_hash = crate::app::registry::join_party_sponsored(&signer, party_id)
            .await
            .map_err(|e| crate::error::Error::other(format!("join_party failed: {e}")))?;
            Ok(serde_json::json!({
                "party_id": party_id,
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `fund_party(party_id, amount_lh)` — escrow `$LH` from this agent's wallet
/// into the party pot. Refunded exactly on disband/expiry; split to the members
/// on complete. Reuses `registry::fund_party_sponsored_bridged` (meter
/// auto-bridge, the fund_guild precedent).
pub(crate) fn fund_party_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "party_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of the party whose pot to fund."
            },
            "amount_lh": {
                "type": "string",
                "description": "Amount of $LH to contribute, as a decimal string (\"5\", \
                    \"1.5\"). Pulled from YOUR wallet into the party pot; refunded exactly \
                    on disband/expiry, split to the members on complete. Must be > 0."
            }
        },
        "required": ["party_id", "amount_lh"]
    });
    ClosureTool::new(
        "fund_party",
        "Contribute $LH from your wallet into a party's pooled pot. Anyone can fund; the \
         creator completes the party to split the pot to the members' TBAs by their shares. \
         Moves value: confirm the amount with the owner before calling. Returns \
         { party_id, amount_lh, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let party_id = args
                .get("party_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("party_id is required"))?;
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
            let signer = bounty_signer().await?;
            // Escrow auto-bridge (feedback #63): a wallet shortfall covered by
            // unspent chat-meter credits rides as a withdrawCredits call in the
            // SAME atomic tx as approve+fundParty.
            let from_hex =
                crate::encoding::bytes_to_hex_str(&crate::wallet::address(&signer));
            let bridge_wei = crate::app::chat::escrow_bridge_wei(&from_hex, amount_wei)
                .await
                .map_err(crate::error::Error::other)?;
            let tx_hash = crate::app::registry::fund_party_sponsored_bridged(&signer, party_id, amount_wei, bridge_wei)
            .await
            .map_err(|e| crate::error::Error::other(format!("fund_party failed: {e}")))?;
            Ok(serde_json::json!({
                "party_id": party_id,
                "amount_lh": amount_arg,
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `complete_party(party_id)` — the CREATOR settles: the pot splits to each
/// member's TBA by the agreed shares and the party dissolves. Reuses
/// `registry::complete_party_sponsored`.
pub(crate) fn complete_party_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "party_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of a party YOU formed (Active, all seats consented) \
                    whose pot you want to split to the members' TBAs."
            }
        },
        "required": ["party_id"]
    });
    ClosureTool::new(
        "complete_party",
        "Complete a party you formed — this RELEASES the pooled $LH to the members' TBAs \
         by their agreed shares and dissolves the party. Call it only once the party is \
         Active (every member has consented via join_party) and funded. Moves value. \
         Returns { party_id, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let party_id = args
                .get("party_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("party_id is required"))?;
            let signer = bounty_signer().await?;
            let tx_hash = crate::app::registry::complete_party_sponsored(&signer, party_id)
            .await
            .map_err(|e| crate::error::Error::other(format!("complete_party failed: {e}")))?;
            Ok(serde_json::json!({
                "party_id": party_id,
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `disband_party(party_id)` — dissolve the party and refund every funder their
/// exact contribution (creator any time; anyone after expiry). Reuses
/// `registry::disband_party_sponsored`.
pub(crate) fn disband_party_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "party_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of the party to disband. As the creator you may \
                    disband any live party; anyone may once its ttl has expired."
            }
        },
        "required": ["party_id"]
    });
    ClosureTool::new(
        "disband_party",
        "Disband a party — dissolve it and refund every funder their exact contribution. \
         The creator may disband a live party any time; anyone may once its ttl has \
         expired. The refund always goes to the FUNDERS, never the caller. Returns \
         { party_id, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let party_id = args
                .get("party_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("party_id is required"))?;
            let signer = bounty_signer().await?;
            let tx_hash = crate::app::registry::disband_party_sponsored(&signer, party_id)
            .await
            .map_err(|e| crate::error::Error::other(format!("disband_party failed: {e}")))?;
            Ok(serde_json::json!({
                "party_id": party_id,
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `discover_parties()` — list the LIVE (forming/active, unexpired) parties.
/// Read-only: reuses `registry::live_parties` (resolved via `get_party`).
pub(crate) fn discover_parties_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    ClosureTool::new(
        "discover_parties",
        "Find live parties (forming or active, unexpired) on-chain. Read-only registry \
         scan: returns each live party with its id, status, consent tally, and pot. Use \
         this to find parties you can join or fund. Returns { parties: [ { party_id, \
         status, accepted_count, member_count, pot_lh } ], count }.",
        serde_json::json!({ "type": "object", "properties": {}, "required": [] }),
        |_args: serde_json::Value, _ctx| async move {
            let ids = crate::app::registry::live_parties(0, 100)
                .await
                .map_err(crate::error::Error::other)?;
            let mut parties = Vec::with_capacity(ids.len());
            for id in ids {
                if let Ok(p) = crate::app::registry::get_party(id).await {
                    parties.push(serde_json::json!({
                        "party_id": id,
                        "status": p.status_label(),
                        "accepted_count": p.accepted_count,
                        "member_count": p.member_count,
                        "pot_lh": format_lh(p.escrow_wei),
                    }));
                }
            }
            Ok(serde_json::json!({
                "count": parties.len(),
                "parties": parties,
            }))
        },
    )
}

/// `get_party(party_id)` — full read-only detail for one party: creator,
/// status, members + shares + consents, the pot, expiry. Reuses
/// `registry::{get_party, party_members_of, party_shares_of, party_consent_of}`.
pub(crate) fn get_party_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "party_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of the party to inspect."
            }
        },
        "required": ["party_id"]
    });
    ClosureTool::new(
        "get_party",
        "Read full detail for one party: creator, status, members with their shares and \
         consent state, the pooled pot, and expiry. Read-only. Use this before \
         join_party / fund_party / complete_party. Returns { party_id, creator, status, \
         pot_lh, expiry, members: [ { token_id, bps, consented } ] }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let party_id = args
                .get("party_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("party_id is required"))?;
            let p = crate::app::registry::get_party(party_id)
                .await
                .map_err(crate::error::Error::other)?;
            if p.creator.trim_start_matches("0x").chars().all(|c| c == '0') {
                return Err(crate::error::Error::other(format!(
                    "party #{party_id} doesn't exist"
                )));
            }
            let member_ids =
                crate::app::registry::party_members_of(party_id).await.unwrap_or_default();
            let shares =
                crate::app::registry::party_shares_of(party_id).await.unwrap_or_default();
            let mut members = Vec::with_capacity(member_ids.len());
            for (i, token_id) in member_ids.iter().enumerate() {
                let consented = crate::app::registry::party_consent_of(party_id, *token_id)
                    .await
                    .unwrap_or(false);
                members.push(serde_json::json!({
                    "token_id": token_id,
                    "bps": shares.get(i).copied().unwrap_or(0),
                    "consented": consented,
                }));
            }
            Ok(serde_json::json!({
                "party_id": party_id,
                "creator": p.creator,
                "status": p.status_label(),
                "pot_lh": format_lh(p.escrow_wei),
                "expiry": p.expiry,
                "members": members,
            }))
        },
    )
}
