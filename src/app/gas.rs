//! Gas budgets for sponsored on-chain writes.
//!
//! One canonical formula instead of the three variants that used to be
//! copy-pasted across the publish/persona/key-sync sites — two of which
//! still carried the OLD `~1.3M + words*40k` shape (~1.25k gas/byte),
//! about 6x below the measured cost, silently OOG-reverting large writes
//! (the feedback/redeem bug class: chain reverts, UI reports success).

/// Gas limit for a sponsored Tempo tx whose dominant cost is ONE
/// `setMetadata(uint256,bytes32,bytes)` write of `byte_len` payload bytes
/// (app.wasm / public.html / persona / sealed Gemini key).
///
/// `1.2M base + 8_500/byte`: storing bytes on-chain costs ~7.6k gas/BYTE
/// (measured via `debug_traceTransaction`, 2026-06-03 — same byte-storage
/// cost as the FeedbackFacet), plus the ~275k Tempo sponsorship overhead
/// and base call, with margin. Over-budget is FREE — the sponsor is billed
/// on gas USED, not the limit — so headroom is correct (see CLAUDE.md
/// "On-chain writes that store data are gas-HUNGRY").
///
/// Batches that add a second tiny `setMetadata` (the `public_face` choice
/// string) fit inside the base headroom; genuinely different writes
/// (mints, burns, TBA executes) budget separately at their call sites.
pub(crate) fn set_metadata_gas(byte_len: usize) -> u128 {
    1_200_000 + byte_len as u128 * 8_500
}
