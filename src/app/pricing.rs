//! Per-tenant payment-gate pricing — visitor pays the agent's
//! ERC-6551 TBA `per_turn_wei` test ETH before a turn runs.
//!
//! State lives at `.lh_pricing.json` in the tenant subdomain's OPFS,
//! so it's only the owner who can set it (and only while they're the
//! one browsing the subdomain — visitors can read the field but can't
//! write since OPFS is per-origin, and any other origin's bundle
//! can't reach it). Format is intentionally minimal:
//!
//! ```json
//! { "per_turn_wei": "1000000000000000" }
//! ```
//!
//! `per_turn_wei` is a **string** so JSON survives the round-trip
//! without overflowing `Number.MAX_SAFE_INTEGER` (2^53 — easy to
//! exceed at high wei prices). Empty/missing/zero means "free".

use serde::{Deserialize, Serialize};


const PRICING_FILE: &str = ".lh_pricing.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PricingFile {
    #[serde(default)]
    per_turn_wei: String,
}

/// Returns the configured per-turn price in wei. `None` (or `Some(0)`)
/// means the agent is free.
pub(crate) async fn load() -> Option<u128> {
    let fs = super::shared_opfs();
    let bytes = fs.read(PRICING_FILE).await.ok()?;
    if bytes.is_empty() {
        return None;
    }
    let parsed: PricingFile = serde_json::from_slice(&bytes).ok()?;
    if parsed.per_turn_wei.is_empty() {
        return Some(0);
    }
    parsed.per_turn_wei.parse::<u128>().ok()
}

/// Overwrite the pricing config with a new per-turn price. Pass `0`
/// to mark the agent free (still writes the file so the existence
/// signals "owner has thought about pricing").
pub(crate) async fn save(per_turn_wei: u128) -> Result<(), String> {
    let fs = super::shared_opfs();
    let body = PricingFile {
        per_turn_wei: per_turn_wei.to_string(),
    };
    let bytes = serde_json::to_vec(&body).map_err(|e| format!("encode pricing: {e}"))?;
    fs.write_atomic(PRICING_FILE, &bytes)
        .await
        .map_err(|e| format!("save pricing: {e}"))?;
    Ok(())
}
