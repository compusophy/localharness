// =============================================================================
// Governance tools — DAO governance over a guild treasury (VotingFacet). Guild
// members propose treasury spends, vote, and execute once a proposal passes past
// its deadline. Same sponsored path as the guild/bounty tools (owner's credit key
// signs the sender_hash, the embedded sponsor pays gas via `bounty_signer`).
// Registry helpers reused (SIBLING-OWNED — never re-encoded here): propose_sponsored
// / vote_sponsored / execute_proposal_sponsored + reads get_proposal / tally_of /
// has_voted / proposals_of.
// =============================================================================

use crate::tools::ClosureTool;

use super::bounty::bounty_signer;
use super::guild::{format_lh, resolve_account};

/// Default proposal voting period (hours) when `period_hours` is omitted.
const PROPOSAL_DEFAULT_PERIOD_HOURS: f64 = 48.0;

/// `propose_measure(guild_id, to, amount_lh, memo?, period_hours?)` — open a
/// governance proposal to spend `amount_lh` `$LH` from a guild's treasury to `to`
/// (an address or a subdomain name's owner), votable for `period_hours`. Reuses
/// `registry::propose_sponsored`.
pub(crate) fn propose_measure_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Schema + lenient extraction from ONE hoisted table
    // (`crate::tool_params::ProposeMeasureParams`), byte-identity-tested
    // natively; `guild_id()` reproduces the old inline required-error exactly.
    let schema = crate::tool_params::ProposeMeasureParams::schema();
    ClosureTool::new(
        "propose_measure",
        "Open a DAO governance proposal to spend $LH from a guild's pooled treasury: \
         members vote for/against, and a passing proposal can be executed after its \
         deadline. Use this to run a guild's spending democratically (instead of an \
         Admin spending unilaterally). Returns { proposal_id, guild_id, to, resolved_to, \
         amount_lh, period_hours, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let params = crate::tool_params::ProposeMeasureParams::lenient(&args);
            let guild_id = params.guild_id()?;
            let to_arg = params.to.trim().to_string();
            if to_arg.is_empty() {
                return Err(crate::error::Error::other("to cannot be empty"));
            }
            let amount_arg = params.amount_lh.trim().to_string();
            let amount_wei = crate::encoding::parse_token_amount(&amount_arg).ok_or_else(|| {
                crate::error::Error::other(format!(
                    "could not parse amount_lh \"{amount_arg}\" — pass a decimal $LH \
                     figure like \"5\" or \"1.5\""
                ))
            })?;
            if amount_wei == 0 {
                return Err(crate::error::Error::other("amount_lh must be greater than 0"));
            }
            let memo = params.memo.as_deref().unwrap_or("").trim();
            // Period hours → seconds. Default 48h.
            let period_hours: f64 = match params.period_hours.as_deref() {
                Some(s) if !s.trim().is_empty() => s
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| crate::error::Error::other("period_hours must be a number"))?,
                _ => PROPOSAL_DEFAULT_PERIOD_HOURS,
            };
            if period_hours <= 0.0 {
                return Err(crate::error::Error::other("period_hours must be greater than 0"));
            }
            let period_secs = (period_hours * 3600.0) as u64;
            let to_hex = resolve_account(&to_arg).await?;
            let signer = bounty_signer().await?;
            let tx_hash = crate::app::registry::propose_sponsored(
                &signer,
                guild_id,
                &to_hex,
                amount_wei,
                memo.as_bytes(),
                period_secs,
            )
            .await
            .map_err(|e| crate::error::Error::other(format!("propose_measure failed: {e}")))?;
            // New proposal id = the guild's last entry in proposals_of (best-effort).
            let proposal_id = crate::app::registry::proposals_of(guild_id, 0, 256)
                .await
                .ok()
                .and_then(|ids| ids.last().copied());
            let mut result = serde_json::json!({
                "guild_id": guild_id,
                "to": to_arg,
                "resolved_to": to_hex,
                "amount_lh": amount_arg,
                "period_hours": period_hours,
                "tx_hash": tx_hash,
            });
            if let Some(id) = proposal_id {
                result["proposal_id"] = serde_json::json!(id);
            }
            Ok(result)
        },
    )
}

/// `cast_vote(proposal_id, support)` — vote for or against an open proposal.
/// Reuses `registry::vote_sponsored`.
pub(crate) fn cast_vote_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "proposal_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of the open proposal to vote on (from list_proposals)."
            },
            "support": {
                "type": "boolean",
                "description": "true to vote FOR the proposal, false to vote AGAINST it."
            }
        },
        "required": ["proposal_id", "support"]
    });
    ClosureTool::new(
        "cast_vote",
        "Cast a vote on an open guild governance proposal: `support` true is a vote FOR, \
         false is AGAINST. One vote per member per proposal. Returns { proposal_id, \
         support, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let proposal_id = args
                .get("proposal_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("proposal_id is required"))?;
            let support = args
                .get("support")
                .and_then(|v| v.as_bool())
                .ok_or_else(|| crate::error::Error::other("support (true/false) is required"))?;
            let signer = bounty_signer().await?;
            let tx_hash = crate::app::registry::vote_sponsored(&signer, proposal_id, support)
            .await
            .map_err(|e| crate::error::Error::other(format!("cast_vote failed: {e}")))?;
            Ok(serde_json::json!({
                "proposal_id": proposal_id,
                "support": support,
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `execute_proposal(proposal_id)` — execute a passed proposal after its deadline,
/// paying out the treasury spend. Reuses `registry::execute_proposal_sponsored`.
pub(crate) fn execute_proposal_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Hoisted table: `crate::tool_params::ExecuteProposalParams`.
    let schema = crate::tool_params::ExecuteProposalParams::schema();
    ClosureTool::new(
        "execute_proposal",
        "Execute a guild governance proposal that PASSED, after its voting deadline has \
         elapsed — this RELEASES the $LH spend from the guild treasury to the proposed \
         recipient. The on-chain facet reverts if the proposal didn't pass or the \
         deadline hasn't elapsed. Moves value. Returns { proposal_id, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let proposal_id =
                crate::tool_params::ExecuteProposalParams::lenient(&args).proposal_id()?;
            let signer = bounty_signer().await?;
            let tx_hash = crate::app::registry::execute_proposal_sponsored(&signer, proposal_id)
            .await
            .map_err(|e| crate::error::Error::other(format!("execute_proposal failed: {e}")))?;
            Ok(serde_json::json!({
                "proposal_id": proposal_id,
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `list_proposals(guild_id)` — read-only: a guild's governance proposals, each
/// with its recipient, amount, status, deadline, and for/against tally. Reuses
/// `registry::{proposals_of, get_proposal, tally_of}`.
pub(crate) fn list_proposals_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Hoisted table: `crate::tool_params::ListProposalsParams`.
    let schema = crate::tool_params::ListProposalsParams::schema();
    ClosureTool::new(
        "list_proposals",
        "List a guild's governance proposals — each with its id, spend recipient, $LH \
         amount, status (open/executed/defeated/cancelled), voting deadline, and \
         for/against tally. Read-only. Use this to see what's up for a vote before \
         cast_vote / execute_proposal. Returns { proposals: [ { proposal_id, to, \
         amount_lh, status, deadline, votes_for, votes_against } ], count }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let guild_id = crate::tool_params::ListProposalsParams::lenient(&args).guild_id()?;
            let ids = crate::app::registry::proposals_of(guild_id, 0, 256)
                .await
                .map_err(crate::error::Error::other)?;
            let mut proposals = Vec::new();
            for id in ids {
                let Ok(p) = crate::app::registry::get_proposal(id).await else {
                    continue;
                };
                let (votes_for, votes_against) = crate::app::registry::tally_of(id)
                    .await
                    .map(|t| (t.for_votes, t.against_votes))
                    .unwrap_or((0, 0));
                proposals.push(serde_json::json!({
                    "proposal_id": id,
                    "to": p.to,
                    "amount_lh": format_lh(p.amount),
                    "status": p.status_label(),
                    "deadline": p.deadline,
                    "votes_for": votes_for,
                    "votes_against": votes_against,
                }));
            }
            Ok(serde_json::json!({
                "count": proposals.len(),
                "proposals": proposals,
            }))
        },
    )
}
