// =============================================================================
// Validation-staking tools — the in-tab agent participates in the on-chain
// validation market (ValidationFacet, the money-backed half of reputation) via
// the SAME sponsored path as the bounty tools (owner authority: the owner's
// apex wallet signs, the bundle sponsor pays gas). A VALIDATOR escrows `$LH`
// behind a verdict about a subject's `workRef` (the platform convention is
// `workRef = bytes32(bountyId)`); a CHALLENGER counter-stakes the opposite
// verdict; the work's bounty poster (or the diamond owner) resolves, and the
// winner takes both stakes. Unchallenged stakes reclaim after the window; an
// unresolved challenge draws. The registry helpers
// (stake_validation_sponsored, challenge_validation_sponsored,
// resolve_validation_sponsored, reclaim_stake_sponsored,
// reclaim_unresolved_sponsored, get_validation) are reused — never re-encoded
// here. Mirrors `bounty.rs` + the CLI `validation` command's arg shapes.
// =============================================================================

use crate::tools::ClosureTool;

use super::bounty::bounty_signers;
use super::guild::format_lh;

/// workRef = `bytes32(bountyId)` — the same coupling the facet's resolver uses
/// (the poster of `uint256(workRef)` is the on-chain resolver). Mirrors the
/// CLI's `work_ref_of_bounty`.
fn work_ref_of_bounty(bounty_id: u64) -> [u8; 32] {
    let mut wr = [0u8; 32];
    wr[24..].copy_from_slice(&bounty_id.to_be_bytes());
    wr
}

/// Human label for the ABI status enum (0 Open … 5 Drawn). Mirrors the CLI's
/// `validation_status_label`.
fn validation_status_label(status: u8) -> &'static str {
    match status {
        0 => "open",
        1 => "challenged",
        2 => "reclaimed",
        3 => "validator won",
        4 => "challenger won",
        5 => "drawn",
        _ => "unknown",
    }
}

/// `stake_validation(subject, bounty_id, valid, amount_lh)` — escrow `$LH`
/// behind a verdict about a subject identity's work for a bounty, via ONE
/// sponsored Tempo tx (approve + stakeValidation). Reuses
/// `registry::stake_validation_sponsored`.
pub(crate) fn stake_validation_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "subject": {
                "type": "integer",
                "minimum": 0,
                "description": "The on-chain tokenId of the identity whose work you \
                    are validating (the subject). NOT yourself — self-validation is \
                    rejected on chain."
            },
            "bounty_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of the bounty whose result you are judging. \
                    The workRef is bytes32(bounty_id); the bounty's POSTER becomes \
                    the on-chain resolver."
            },
            "valid": {
                "type": "boolean",
                "description": "Your verdict: true = \"this work is VALID\", false = \
                    \"this work is INVALID\"."
            },
            "amount_lh": {
                "type": "string",
                "description": "Stake in $LH, as a decimal string (\"5\", \"1.5\"). \
                    Escrowed from YOUR wallet; it reclaims after the challenge window \
                    (unchallenged) or doubles/forfeits on resolution. Must be > 0."
            }
        },
        "required": ["subject", "bounty_id", "valid", "amount_lh"]
    });
    ClosureTool::new(
        "stake_validation",
        "Stake $LH behind a verdict about another identity's bounty work (ERC-8004 \
         validation). You escrow `amount_lh` claiming the work is `valid` (or not); a \
         challenger can counter-stake the opposite, and the bounty poster resolves — \
         the winner takes both stakes. Use this to put money behind a quality judgement. \
         Reverts on a zero stake, an unknown subject, self-validation, or a duplicate. \
         Returns { validation_id, subject, bounty_id, valid, amount_lh, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let subject = args
                .get("subject")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("subject (tokenId) is required"))?;
            let bounty_id = args
                .get("bounty_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("bounty_id is required"))?;
            let valid = args
                .get("valid")
                .and_then(|v| v.as_bool())
                .ok_or_else(|| crate::error::Error::other("valid (true/false verdict) is required"))?;
            let amount_arg = args
                .get("amount_lh")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let stake_wei = crate::encoding::parse_token_amount(&amount_arg).ok_or_else(|| {
                crate::error::Error::other(format!(
                    "could not parse amount_lh \"{amount_arg}\" — pass a decimal $LH \
                     figure like \"5\" or \"1.5\""
                ))
            })?;
            if stake_wei == 0 {
                return Err(crate::error::Error::other("amount_lh must be greater than 0"));
            }
            let (signer, fee_payer) = bounty_signers().await?;
            let tx_hash = crate::app::registry::stake_validation_sponsored(
                &signer,
                &fee_payer,
                work_ref_of_bounty(bounty_id),
                subject,
                valid,
                stake_wei,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
            .map_err(|e| crate::error::Error::other(format!("stake_validation failed: {e}")))?;
            // The new id = validation_count() - 1 after mining (ids monotonic).
            let validation_id = crate::app::registry::validation_count()
                .await
                .ok()
                .and_then(|n| n.checked_sub(1));
            let mut result = serde_json::json!({
                "subject": subject,
                "bounty_id": bounty_id,
                "valid": valid,
                "amount_lh": amount_arg,
                "tx_hash": tx_hash,
            });
            if let Some(id) = validation_id {
                result["validation_id"] = serde_json::json!(id);
            }
            Ok(result)
        },
    )
}

/// `challenge_validation(validation_id)` — counter-stake the OPPOSITE verdict
/// on an Open validation (the counter-stake equals its own stake, read first).
/// Reuses `registry::challenge_validation_sponsored`.
pub(crate) fn challenge_validation_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "validation_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of the OPEN validation to challenge (from \
                    get_validation)."
            }
        },
        "required": ["validation_id"]
    });
    ClosureTool::new(
        "challenge_validation",
        "Challenge an open validation by counter-staking the OPPOSITE verdict. You \
         escrow the SAME amount the validator staked (read from the record); the bounty \
         poster then resolves and the winner takes both stakes. Only works while the \
         validation is Open and you are not the validator. Returns { validation_id, \
         counter_stake_lh, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let validation_id = args
                .get("validation_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("validation_id is required"))?;
            // The counter-stake MUST equal the validation's own stake — read it
            // first (and surface a specific cause if it isn't challengeable).
            let v = crate::app::registry::get_validation(validation_id)
                .await
                .map_err(crate::error::Error::other)?
                .ok_or_else(|| {
                    crate::error::Error::other(format!(
                        "validation #{validation_id} doesn't exist"
                    ))
                })?;
            if v.status != 0 {
                return Err(crate::error::Error::other(format!(
                    "validation #{validation_id} is {} — only an OPEN validation can be challenged",
                    validation_status_label(v.status)
                )));
            }
            let (signer, fee_payer) = bounty_signers().await?;
            let tx_hash = crate::app::registry::challenge_validation_sponsored(
                &signer,
                &fee_payer,
                validation_id,
                v.stake_wei,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
            .map_err(|e| crate::error::Error::other(format!("challenge_validation failed: {e}")))?;
            Ok(serde_json::json!({
                "validation_id": validation_id,
                "counter_stake_lh": format_lh(v.stake_wei),
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `resolve_validation(validation_id, winner)` — rule a Challenged validation
/// (resolver-only on chain: the bounty poster or the diamond owner). The named
/// side is paid BOTH stakes. Reuses `registry::resolve_validation_sponsored`.
pub(crate) fn resolve_validation_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "validation_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of the CHALLENGED validation to resolve."
            },
            "winner": {
                "type": "string",
                "description": "Who wins, paid BOTH stakes: \"validator\" (the original \
                    verdict stands) or \"challenger\" (the counter-verdict stands)."
            }
        },
        "required": ["validation_id", "winner"]
    });
    ClosureTool::new(
        "resolve_validation",
        "Resolve a challenged validation, paying both stakes to the winner. RESOLVER-ONLY \
         on chain: only the poster of the referenced bounty, or the diamond owner, can \
         rule. `winner` is \"validator\" or \"challenger\". Moves value. Returns \
         { validation_id, winner, tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let validation_id = args
                .get("validation_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("validation_id is required"))?;
            let winner_raw = args
                .get("winner")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            let validator_wins = match winner_raw.as_str() {
                "validator" | "valid" => true,
                "challenger" | "invalid" => false,
                other => {
                    return Err(crate::error::Error::other(format!(
                        "winner must be 'validator' or 'challenger', got '{other}'"
                    )));
                }
            };
            let (signer, fee_payer) = bounty_signers().await?;
            let tx_hash = crate::app::registry::resolve_validation_sponsored(
                &signer,
                &fee_payer,
                validation_id,
                validator_wins,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
            .map_err(|e| {
                crate::error::Error::other(format!(
                    "resolve_validation failed (resolver-only: the bounty poster or diamond owner): {e}"
                ))
            })?;
            Ok(serde_json::json!({
                "validation_id": validation_id,
                "winner": if validator_wins { "validator" } else { "challenger" },
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `reclaim_validation(validation_id)` — refund a validation's stake(s): an
/// UNCHALLENGED stake reclaims to the validator after the challenge window; a
/// CHALLENGED-but-unresolved validation draws (both refunded) after the resolve
/// window. Picks the right path from the record. Reuses
/// `registry::{reclaim_stake_sponsored, reclaim_unresolved_sponsored}`.
pub(crate) fn reclaim_validation_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "validation_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of the validation to refund (its window must \
                    have passed)."
            }
        },
        "required": ["validation_id"]
    });
    ClosureTool::new(
        "reclaim_validation",
        "Refund a validation whose window has passed. An UNCHALLENGED stake reclaims to \
         the validator after the challenge window; a CHALLENGED-but-unresolved validation \
         draws (both sides refunded) after the resolve window. Permissionless poke — the \
         refund always goes to the rightful side(s). Returns { validation_id, mode, \
         tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let validation_id = args
                .get("validation_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("validation_id is required"))?;
            // Pick the path from the record: Open → reclaim stake; Challenged →
            // draw. Surface a specific cause for the already-settled states.
            let v = crate::app::registry::get_validation(validation_id)
                .await
                .map_err(crate::error::Error::other)?
                .ok_or_else(|| {
                    crate::error::Error::other(format!(
                        "validation #{validation_id} doesn't exist"
                    ))
                })?;
            let unresolved = match v.status {
                0 => false, // Open → reclaim the unchallenged stake
                1 => true,  // Challenged → draw (refund both, if unresolved)
                other => {
                    return Err(crate::error::Error::other(format!(
                        "validation #{validation_id} is already {} — nothing to refund",
                        validation_status_label(other)
                    )));
                }
            };
            let (signer, fee_payer) = bounty_signers().await?;
            let res = if unresolved {
                crate::app::registry::reclaim_unresolved_sponsored(
                    &signer,
                    &fee_payer,
                    validation_id,
                    crate::app::registry::ALPHA_USD_ADDRESS,
                )
                .await
            } else {
                crate::app::registry::reclaim_stake_sponsored(
                    &signer,
                    &fee_payer,
                    validation_id,
                    crate::app::registry::ALPHA_USD_ADDRESS,
                )
                .await
            };
            let tx_hash = res.map_err(|e| {
                crate::error::Error::other(format!(
                    "reclaim_validation failed (is the window over?): {e}"
                ))
            })?;
            Ok(serde_json::json!({
                "validation_id": validation_id,
                "mode": if unresolved { "draw" } else { "reclaim" },
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `get_validation(validation_id)` — read the decoded validation record.
/// Read-only: reuses `registry::get_validation`.
pub(crate) fn get_validation_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "validation_id": {
                "type": "integer",
                "minimum": 0,
                "description": "The id of the validation to read."
            }
        },
        "required": ["validation_id"]
    });
    ClosureTool::new(
        "get_validation",
        "Read an on-chain validation record: who staked, who challenged, the verdict, the \
         stake per side, and the lifecycle status. Read-only. Use it before challenging \
         (to see the stake you must match) or resolving. Returns the record fields, or \
         { found: false } for an unknown id.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let validation_id = args
                .get("validation_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| crate::error::Error::other("validation_id is required"))?;
            match crate::app::registry::get_validation(validation_id)
                .await
                .map_err(crate::error::Error::other)?
            {
                Some(v) => {
                    let challenger_zero =
                        v.challenger.trim_start_matches("0x").chars().all(|c| c == '0');
                    Ok(serde_json::json!({
                        "found": true,
                        "validation_id": validation_id,
                        "status": validation_status_label(v.status),
                        "subject_token_id": v.subject_token_id,
                        "verdict_valid": v.verdict_valid,
                        "validator": v.validator,
                        "challenger": if challenger_zero {
                            serde_json::Value::Null
                        } else {
                            serde_json::json!(v.challenger)
                        },
                        "stake_lh": format_lh(v.stake_wei),
                        "work_ref": v.work_ref_hex,
                    }))
                }
                None => Ok(serde_json::json!({
                    "found": false,
                    "validation_id": validation_id,
                })),
            }
        },
    )
}
