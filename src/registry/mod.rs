//! JSON-RPC client for `LocalharnessRegistry` — read AND write.
//!
//! Hand-rolled instead of pulling alloy: the apex chrome only needs a
//! handful of methods (`eth_call`, `eth_chainId`, `eth_gasPrice`,
//! `eth_getTransactionCount`, `eth_estimateGas`,
//! `eth_sendRawTransaction`, `eth_getTransactionReceipt`) and we
//! already have `reqwest` in the bundle. Avoiding alloy also sidesteps
//! the `serde::__private` compat snag we hit during the M6 spike.
//!
//! `REGISTRY_ADDRESS` is a baked-in non-zero constant; the historical
//! per-view "registry pending deploy" zero-address guards (which could
//! never fire) are gone — every read view goes straight to the chain
//! via the crate-internal `read_view` (`selector ++ words`) helper.

mod abi;
mod bounty;
pub mod chain;
mod credits;
mod diamond;
mod feedback;
mod guild;
mod invite;
mod names;
mod party;
mod push;
mod reputation;
mod rpc;
mod schedule;
mod sessionroom;
mod signaling;
mod subscribe;
mod tba;
mod tithe;
mod tx;
mod validation;
mod voting;
mod x402;

pub(crate) use abi::*;
pub use bounty::*;
pub use credits::*;
pub use diamond::*;
pub use feedback::*;
pub use guild::*;
pub use invite::*;
pub use names::*;
pub use party::*;
pub use push::*;
pub use reputation::*;
pub use rpc::*;
pub use schedule::*;
pub use sessionroom::*;
pub use signaling::*;
pub use subscribe::*;
pub use tba::*;
pub use tithe::*;
pub use tx::*;
pub use validation::*;
pub use voting::*;
pub use x402::*;

/// Active-chain RPC endpoint (default Moderato testnet; `mainnet` feature →
/// Tempo mainnet). Sourced from [`chain::ACTIVE`].
pub const RPC_URL: &str = chain::ACTIVE.rpc_url;

/// `LocalharnessRegistry` Diamond address on the active chain (default the
/// Moderato deployment; `mainnet` feature → the mainnet diamond). Sourced from
/// [`chain::ACTIVE`].
///
/// The diamond proxy holds the storage; `register / ownerOfName / idOfName / …`
/// selectors dispatch to per-facet addresses. Owner (deployer/admin):
/// `0x313b1659F5037080aA0C113D386C5954F348EF1e`.
pub const REGISTRY_ADDRESS: &str = chain::ACTIVE.diamond;

/// Active-chain id — used in EIP-155 v computation. Sourced from [`chain::ACTIVE`].
pub const CHAIN_ID: u64 = chain::ACTIVE.chain_id;

// `BOOTSTRAP_FAUCET_ADDRESS` (the dormant BootstrapFaucet.sol breadcrumb —
// unusable on Tempo Moderato, which refuses EOA↔contract native value
// transfers) was removed as dead code; all distribution flows through
// [`LOCALHARNESS_TOKEN_ADDRESS`].

/// `LocalharnessCredits` — TIP-20-shaped credit token (currency =
/// "credits", explicitly NOT USD so it's NOT fee-token-eligible).
/// Replaces the standalone `LocalharnessToken.sol` at
/// `0xcC8A300658…` (orphaned — old balances do not migrate; testnet
/// reset).
///
/// Deployed 2026-05-26 alongside `CreditsFacet` on the diamond. The
/// diamond holds ISSUER_ROLE on this token, so the only path to
/// fresh supply is through the facet's `claimDaily()`. Owner can
/// tune the per-day allowance via `setDailyAllowance` on the diamond.
///
/// name: "localharness credits", symbol: "LH", decimals: 18. Address sourced
/// from [`chain::ACTIVE`] (default Moderato; `mainnet` feature → mainnet $LH).
pub const LOCALHARNESS_TOKEN_ADDRESS: &str = chain::ACTIVE.lh_token;

// Shared test helpers re-exported for the facet submodules' own test mods. The
// `use` precedes the module so `test_support` stays the file's LAST item (Rust
// resolves the re-export regardless of order) — clippy's items-after-test-module
// lint fires on anything declared after a `#[cfg(test)] mod`.
#[cfg(test)]
pub(crate) use test_support::*;

#[cfg(test)]
mod test_support {
    // ─── ABI dynamic-decode edge cases (untrusted RPC hex must never panic) ──
    //
    // The decoders below read offsets/lengths out of attacker-controlled words
    // (the low 8 bytes → up to u64::MAX) and then slice the response. These tests
    // feed deliberately empty / truncated / malformed-offset / huge-length hex
    // and assert the decoder returns empty/None/Err WITHOUT panicking. The test
    // profile has overflow-checks ON, so an unchecked `64 + len` / `arr_off + 32`
    // would panic here — that's exactly the regression these pin down.

    // 64 hex chars per ABI word.
    pub(crate) const Z: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    pub(crate) fn word_usize(v: u64) -> String {
        format!("{:064x}", v)
    }
    /// A 32-byte word whose LOW 8 bytes are u64::MAX (the largest value the
    /// low-8-bytes offset/length readers will extract → forces overflow if any
    /// add is unchecked).
    pub(crate) fn word_u64_max() -> String {
        format!("{:048x}{:016x}", 0u64, u64::MAX)
    }
}
