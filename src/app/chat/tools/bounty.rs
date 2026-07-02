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

/// Resolve the sender signer for a sponsored bounty write: the owner's local
/// credit key signs the sender_hash (the fee side — sponsor key or mainnet
/// relay — is resolved inside `registry::`).
pub(crate) async fn bounty_signer() -> Result<k256::ecdsa::SigningKey, crate::error::Error> {
    let (signer, _) = credit_signer()
        .await
        .ok_or_else(|| crate::error::Error::other("no identity — claim a subdomain first"))?;
    Ok(signer)
}

/// `post_bounty(task, reward_lh, ttl_hours?)` — escrow `$LH` behind an on-chain
/// task other agents can claim + fulfil. Reward is a decimal `$LH` figure;
/// `ttl_hours` defaults to 24h. Reuses `registry::post_bounty_sponsored`.
pub(crate) fn post_bounty_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    // Schema + lenient extraction from ONE hoisted table
    // (`crate::tool_params::PostBountyParams`), byte-identity-tested natively.
    let schema = crate::tool_params::PostBountyParams::schema();
    ClosureTool::new(
        "post_bounty",
        "Post a bounty to the on-chain bounty market: escrow `reward_lh` $LH behind a \
         `task` other agents can discover, claim, and fulfil. Use this to delegate a \
         task to the agent economy when you want it done by whoever can. Escrows from \
         your wallet (sponsored tx); pays out only when you accept a submitted result. \
         Returns { bounty_id, task, reward_lh, ttl_hours, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let params = crate::tool_params::PostBountyParams::lenient(&args);
            let task = params.task.trim();
            if task.is_empty() {
                return Err(crate::error::Error::other("task cannot be empty"));
            }
            let reward_arg = params.reward_lh.trim().to_string();
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
            let ttl_hours: f64 = match params.ttl_hours.as_deref() {
                Some(s) if !s.trim().is_empty() => s.trim().parse::<f64>().map_err(|_| {
                    crate::error::Error::other("ttl_hours must be a number")
                })?,
                _ => 24.0,
            };
            if ttl_hours <= 0.0 {
                return Err(crate::error::Error::other("ttl_hours must be greater than 0"));
            }
            let ttl_secs = (ttl_hours * 3600.0) as u64;
            let signer = bounty_signer().await?;
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
                task.as_bytes(),
                reward_wei,
                ttl_secs,
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
    // Schema + extraction from ONE hoisted table
    // (`crate::tool_params::ClaimBountyParams`), byte-identity-tested natively;
    // `bounty_id()` reproduces the old inline required-error exactly.
    let schema = crate::tool_params::ClaimBountyParams::schema();
    ClosureTool::new(
        "claim_bounty",
        "Claim an open bounty to work on it. THIS agent becomes the claimant (its \
         on-chain tokenId is resolved automatically). After claiming, do the work and \
         call submit_result with your deliverable. Returns { bounty_id, claimant_token_id, \
         tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let bounty_id = crate::tool_params::ClaimBountyParams::lenient(&args).bounty_id()?;
            // The claimant is THIS subdomain's own tokenId.
            let claimant_token_id = own_token_id().await?;
            // Surface the SPECIFIC cause (already claimed / doesn't exist / not
            // open) instead of a generic revert (#50) — shared with the CLI.
            if let Err(why) = crate::app::registry::bounty_preflight_check(bounty_id, "claim").await {
                return Err(crate::error::Error::other(why));
            }
            let signer = bounty_signer().await?;
            let tx_hash = crate::app::registry::claim_bounty_sponsored(&signer, bounty_id, claimant_token_id)
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
    // Hoisted table: `crate::tool_params::SubmitResultParams`.
    let schema = crate::tool_params::SubmitResultParams::schema();
    ClosureTool::new(
        "submit_result",
        "Submit your result for a bounty you have claimed. The poster reviews it and, if \
         satisfied, accepts it (which pays out the escrowed $LH to you). Returns \
         { bounty_id, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let params = crate::tool_params::SubmitResultParams::lenient(&args);
            let bounty_id = params.bounty_id()?;
            let result_text = params.result.trim();
            if result_text.is_empty() {
                return Err(crate::error::Error::other("result cannot be empty"));
            }
            // Specific cause if this bounty isn't in a submittable state (#50).
            if let Err(why) = crate::app::registry::bounty_preflight_check(bounty_id, "submit").await {
                return Err(crate::error::Error::other(why));
            }
            let signer = bounty_signer().await?;
            let tx_hash = crate::app::registry::submit_result_sponsored(&signer, bounty_id, result_text.as_bytes())
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
    // Hoisted table: `crate::tool_params::AcceptResultParams`.
    let schema = crate::tool_params::AcceptResultParams::schema();
    ClosureTool::new(
        "accept_result",
        "Accept the submitted result for a bounty you posted — this RELEASES the \
         escrowed $LH to the claimant. Call it only after reviewing the claimant's \
         submitted result (via discover_bounties / get_bounty). Moves value. Returns \
         { bounty_id, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let bounty_id = crate::tool_params::AcceptResultParams::lenient(&args).bounty_id()?;
            // Specific cause if there's no submitted result to accept (#50).
            if let Err(why) = crate::app::registry::bounty_preflight_check(bounty_id, "accept").await {
                return Err(crate::error::Error::other(why));
            }
            let signer = bounty_signer().await?;
            let tx_hash = crate::app::registry::accept_result_sponsored(&signer, bounty_id)
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

/// `attest(subject, rating, work_ref?, confirmation)` — write an on-chain
/// REPUTATION attestation: rate another agent's work 1..5 about an optional
/// `work_ref` (a bounty id). Reuses `registry::attest_sponsored`. Confirm-gated:
/// an attestation is a durable, per-`(subject, work_ref)`-one-shot signal that
/// drives hiring/promotion, so the owner confirms it like a value move.
pub(crate) fn attest_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "subject": {
                "type": "string",
                "description": "Who you are rating — a subdomain NAME (resolved to its \
                    on-chain tokenId) OR a raw numeric tokenId. Cannot be yourself."
            },
            "rating": {
                "type": "integer",
                "minimum": 1,
                "maximum": 5,
                "description": "Quality rating, an integer 1 (worst) to 5 (best)."
            },
            "work_ref": {
                "type": "string",
                "description": "OPTIONAL bounty id this attestation is about (a decimal \
                    integer), so the rating ties to specific work. Omit for a general \
                    attestation."
            },
            "confirmation": {
                "type": "string",
                "description": "Single-use confirmation code. OMIT (or pass \"\") on the \
                    first call — it returns a challenge code shown to the owner. Relay \
                    it, wait for the owner to TYPE the code in chat, then retry with it. \
                    Never invent it; only the platform issues it."
            }
        },
        "required": ["subject", "rating"]
    });
    ClosureTool::new(
        "attest",
        "Write an on-chain REPUTATION attestation: rate another agent's work 1..5, \
         optionally tied to a bounty id (`work_ref`). Reputation drives hiring + \
         promotion, and each (subject, work_ref) attestation is one-shot + durable, so \
         the first call does NOT execute: it returns a single-use confirmation code \
         (also shown to the owner). State the subject + rating, ask the owner to TYPE \
         the code, then retry with `confirmation` set to it. Reverts on a self-attest, \
         an unknown subject, or a duplicate. Returns { subject, subject_token_id, \
         rating, work_ref, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let subject_arg = args
                .get("subject")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if subject_arg.is_empty() {
                return Err(crate::error::Error::other("subject cannot be empty"));
            }
            // rating: accept an integer or a numeric string (1..=5).
            let rating = args
                .get("rating")
                .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
                .ok_or_else(|| crate::error::Error::other("rating is required"))?;
            if !(1..=5).contains(&rating) {
                return Err(crate::error::Error::other("rating must be an integer 1-5"));
            }
            // Belt-and-suspenders: confirm_guard denies any unconfirmed call before
            // this body runs; this guards a path that forgot the hook (attest writes a
            // durable one-shot reputation signal — same posture as the other gated tools).
            let confirmed = args
                .get("confirmation")
                .and_then(|v| v.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !confirmed {
                return Err(crate::error::Error::other(
                    "attest requires the platform-issued confirmation code",
                ));
            }
            // subject → tokenId: a bare integer is a direct tokenId; otherwise resolve
            // the subdomain name via id_of_name.
            let subject_token_id = match subject_arg.parse::<u64>() {
                Ok(id) if id != 0 => id,
                _ => match crate::app::registry::id_of_name(&subject_arg).await {
                    Ok(id) if id != 0 => id,
                    Ok(_) | Err(_) => {
                        return Err(crate::error::Error::other(format!(
                            "subject \"{subject_arg}\" is not a registered agent (check the name)"
                        )));
                    }
                },
            };
            // work_ref: an optional bounty id left-padded big-endian into the low 8
            // bytes of the 32-byte word (the colony attest convention); empty = zero.
            let work_ref_arg = args.get("work_ref").and_then(|v| v.as_str()).unwrap_or("").trim();
            let mut work_ref = [0u8; 32];
            if !work_ref_arg.is_empty() {
                let id = work_ref_arg.trim_start_matches('#').parse::<u64>().map_err(|_| {
                    crate::error::Error::other(format!(
                        "work_ref \"{work_ref_arg}\" must be a bounty id (integer) — omit it \
                         for a general attestation"
                    ))
                })?;
                work_ref[24..32].copy_from_slice(&id.to_be_bytes());
            }
            let signer = bounty_signer().await?;
            let tx_hash = crate::app::registry::attest_sponsored(&signer, subject_token_id, rating as u8, work_ref)
            .await
            .map_err(|e| crate::error::Error::other(format!("attest failed: {e}")))?;
            Ok(serde_json::json!({
                "subject": subject_arg,
                "subject_token_id": subject_token_id,
                "rating": rating,
                "work_ref": work_ref_arg,
                "tx_hash": tx_hash,
            }))
        },
    )
}
