// =============================================================================
// Bounty economy tools — the in-tab agent participates in the on-chain bounty
// market (BountyFacet) via the SAME sponsored path as send_lh / create_subdomain
// (owner authority: the owner's apex wallet signs, the bundle sponsor pays gas).
// The registry helpers (post_bounty_sponsored, claim_bounty_sponsored,
// submit_result_sponsored, accept_result_sponsored, open_bounties, get_bounty,
// task_of_bounty, discover_bounties) are reused — never re-encoded here.
// =============================================================================

use crate::app::chat::access::{credit_address_existing, credit_signer};
use crate::tools::ClosureTool;

use super::guild::{format_lh, own_token_id};

/// Resolve the (signer, fee_payer) pair for a sponsored bounty write: the
/// owner's local credit key signs the sender_hash, the embedded sponsor pays
/// the fee in AlphaUSD. Mirrors `events::schedule_job_pressed`'s acquisition.
pub(crate) async fn bounty_signers(
) -> Result<(k256::ecdsa::SigningKey, k256::ecdsa::SigningKey), crate::error::Error> {
    let (signer, _) = credit_signer()
        .await
        .ok_or_else(|| crate::error::Error::other("no identity — claim a subdomain first"))?;
    let fee_payer = crate::app::sponsor::signer().map_err(crate::error::Error::other)?;
    Ok((signer, fee_payer))
}

/// `post_bounty(task, reward_lh, ttl_hours?)` — escrow `$LH` behind an on-chain
/// task other agents can claim + fulfil. Reward is a decimal `$LH` figure;
/// `ttl_hours` defaults to 24h. Reuses `registry::post_bounty_sponsored`.
pub(crate) fn post_bounty_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "task": {
                "type": "string",
                "description": "The task to be done — a clear, self-contained \
                    description of what a claimant must deliver to earn the reward."
            },
            "reward_lh": {
                "type": "string",
                "description": "Reward in $LH, as a decimal string (\"5\", \"1.5\"). \
                    Escrowed from YOUR wallet when the bounty is posted; paid out to \
                    the claimant when you accept their result. Must be > 0."
            },
            "ttl_hours": {
                "type": "string",
                "description": "OPTIONAL lifetime in hours before the bounty expires \
                    (decimal). Omit for the 24h default."
            }
        },
        "required": ["task", "reward_lh"]
    });
    ClosureTool::new(
        "post_bounty",
        "Post a bounty to the on-chain bounty market: escrow `reward_lh` $LH behind a \
         `task` other agents can discover, claim, and fulfil. Use this to delegate a \
         task to the agent economy when you want it done by whoever can. Escrows from \
         your wallet (sponsored tx); pays out only when you accept a submitted result. \
         Returns { bounty_id, task, reward_lh, ttl_hours, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let task = args.get("task").and_then(|v| v.as_str()).unwrap_or("").trim();
            if task.is_empty() {
                return Err(crate::error::Error::other("task cannot be empty"));
            }
            let reward_arg = args
                .get("reward_lh")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let reward_wei = crate::encoding::parse_token_amount(&reward_arg).ok_or_else(|| {
                crate::error::Error::other(format!(
                    "could not parse reward_lh \"{reward_arg}\" — pass a decimal $LH \
                     figure like \"5\" or \"1.5\""
                ))
            })?;
            if reward_wei == 0 {
                return Err(crate::error::Error::other("reward_lh must be greater than 0"));
            }
            // TTL: hours → seconds. Default 24h.
            let ttl_hours: f64 = match args.get("ttl_hours").and_then(|v| v.as_str()) {
                Some(s) if !s.trim().is_empty() => s.trim().parse::<f64>().map_err(|_| {
                    crate::error::Error::other("ttl_hours must be a number")
                })?,
                _ => 24.0,
            };
            if ttl_hours <= 0.0 {
                return Err(crate::error::Error::other("ttl_hours must be greater than 0"));
            }
            let ttl_secs = (ttl_hours * 3600.0) as u64;
            let (signer, fee_payer) = bounty_signers().await?;
            // Escrow auto-bridge (feedback #63): a wallet shortfall covered by
            // unspent chat-meter credits rides as a withdrawCredits call in the
            // SAME atomic tx as approve+postBounty. Pot-aware error when both
            // pots together are short.
            let from_hex =
                crate::encoding::bytes_to_hex_str(&crate::wallet::address(&signer));
            let bridge_wei = crate::app::chat::escrow_bridge_wei(&from_hex, reward_wei)
                .await
                .map_err(crate::error::Error::other)?;
            let tx_hash = crate::app::registry::post_bounty_sponsored_bridged(
                &signer,
                &fee_payer,
                task.as_bytes(),
                reward_wei,
                ttl_secs,
                crate::app::registry::ALPHA_USD_ADDRESS,
                bridge_wei,
            )
            .await
            .map_err(|e| crate::error::Error::other(format!("post_bounty failed: {e}")))?;
            // Newest bounty id = caller's last entry in bounties_of (best-effort).
            let bounty_id = match credit_address_existing().await {
                Some(addr) => crate::app::registry::bounties_of(&addr)
                    .await
                    .ok()
                    .and_then(|ids| ids.last().copied()),
                None => None,
            };
            let mut result = serde_json::json!({
                "task": task,
                "reward_lh": reward_arg,
                "ttl_hours": ttl_hours,
                "tx_hash": tx_hash,
            });
            if let Some(id) = bounty_id {
                result["bounty_id"] = serde_json::json!(id);
            }
            Ok(result)
        },
    )
}

/// `claim_bounty(bounty_id)` — claim an open bounty as THIS agent (its on-chain
/// tokenId is resolved automatically as the claimant). Reuses
/// `registry::claim_bounty_sponsored`.
pub(crate) fn claim_bounty_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "bounty_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of the open bounty to claim (from \
                    discover_bounties / the bounty board)."
            }
        },
        "required": ["bounty_id"]
    });
    ClosureTool::new(
        "claim_bounty",
        "Claim an open bounty to work on it. THIS agent becomes the claimant (its \
         on-chain tokenId is resolved automatically). After claiming, do the work and \
         call submit_result with your deliverable. Returns { bounty_id, claimant_token_id, \
         tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let bounty_id = args
                .get("bounty_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("bounty_id is required"))?;
            // The claimant is THIS subdomain's own tokenId.
            let claimant_token_id = own_token_id().await?;
            let (signer, fee_payer) = bounty_signers().await?;
            let tx_hash = crate::app::registry::claim_bounty_sponsored(
                &signer,
                &fee_payer,
                bounty_id,
                claimant_token_id,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
            .map_err(|e| crate::error::Error::other(format!("claim_bounty failed: {e}")))?;
            Ok(serde_json::json!({
                "bounty_id": bounty_id,
                "claimant_token_id": claimant_token_id,
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `submit_result(bounty_id, result)` — submit a deliverable for a bounty this
/// agent has claimed. Reuses `registry::submit_result_sponsored`.
pub(crate) fn submit_result_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "bounty_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of the bounty you previously claimed."
            },
            "result": {
                "type": "string",
                "description": "Your deliverable / result for the bounty — the work \
                    product the poster will review before accepting + paying out."
            }
        },
        "required": ["bounty_id", "result"]
    });
    ClosureTool::new(
        "submit_result",
        "Submit your result for a bounty you have claimed. The poster reviews it and, if \
         satisfied, accepts it (which pays out the escrowed $LH to you). Returns \
         { bounty_id, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let bounty_id = args
                .get("bounty_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("bounty_id is required"))?;
            let result_text = args
                .get("result")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if result_text.is_empty() {
                return Err(crate::error::Error::other("result cannot be empty"));
            }
            let (signer, fee_payer) = bounty_signers().await?;
            let tx_hash = crate::app::registry::submit_result_sponsored(
                &signer,
                &fee_payer,
                bounty_id,
                result_text.as_bytes(),
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
            .map_err(|e| crate::error::Error::other(format!("submit_result failed: {e}")))?;
            Ok(serde_json::json!({
                "bounty_id": bounty_id,
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `accept_result(bounty_id)` — accept the submitted result for a bounty THIS
/// agent posted, paying out the escrowed `$LH` to the claimant. Reuses
/// `registry::accept_result_sponsored`.
pub(crate) fn accept_result_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "bounty_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of a bounty YOU posted whose submitted result \
                    you want to accept (releases the escrowed $LH to the claimant)."
            }
        },
        "required": ["bounty_id"]
    });
    ClosureTool::new(
        "accept_result",
        "Accept the submitted result for a bounty you posted — this RELEASES the \
         escrowed $LH to the claimant. Call it only after reviewing the claimant's \
         submitted result (via discover_bounties / get_bounty). Moves value. Returns \
         { bounty_id, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let bounty_id = args
                .get("bounty_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("bounty_id is required"))?;
            let (signer, fee_payer) = bounty_signers().await?;
            let tx_hash = crate::app::registry::accept_result_sponsored(
                &signer,
                &fee_payer,
                bounty_id,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
            .map_err(|e| crate::error::Error::other(format!("accept_result failed: {e}")))?;
            Ok(serde_json::json!({
                "bounty_id": bounty_id,
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `discover_bounties(query?)` — find open bounties to work on. Read-only:
/// reuses `registry::discover_bounties` (ranked id/task/reward matches), falling
/// back to a plain `open_bounties` scan (resolved via `get_bounty` /
/// `task_of_bounty`) when no query is given.
pub(crate) fn discover_bounties_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    ClosureTool::new(
        "discover_bounties",
        "Find open bounties in the on-chain bounty market. Read-only registry scan: \
         returns open bounties whose task matches `query` (or the most recent open \
         bounties when `query` is empty), each with its id, task, and reward. Use this \
         to find work you can claim (then claim_bounty + submit_result). Returns \
         { bounties: [ { bounty_id, task, reward_lh } ], count }.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What to look for — a capability/topic/keyword matched \
                        (case-insensitively) against open-bounty tasks. Empty returns \
                        recent open bounties."
                }
            },
            "required": []
        }),
        |args: serde_json::Value, _ctx| async move {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let matches = crate::app::registry::discover_bounties(&query, 64)
                .await
                .map_err(crate::error::Error::other)?;
            let bounties: Vec<_> = matches
                .iter()
                .map(|(id, task, reward_wei)| {
                    serde_json::json!({
                        "bounty_id": id,
                        "task": task,
                        "reward_lh": format_lh(*reward_wei),
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "count": bounties.len(),
                "bounties": bounties,
            }))
        },
    )
}
