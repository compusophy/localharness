//! Bundle-side sponsor key — the wallet that signs as `fee_payer`
//! on every user Tempo tx so users themselves never need to hold any
//! native gas OR any TIP-20 stablecoin.
//!
//! ## Trust model
//!
//! On **testnet** the bundle holds a committed **private key** (the const lives
//! in `registry::sponsor`, the one key home; this module is a thin app-side
//! alias) — anyone running localharness.xyz can extract it, which is accepted
//! because the funds are play-money and the sponsor is refillable via
//! `tempo_fundAddress`. On **mainnet** the bundle embeds NO key: [`signer`]
//! returns the committed testnet key as an unused PLACEHOLDER, and the actual
//! `fee_payer` half is signed SERVER-SIDE by the rate-capped relay
//! (`registry::sponsor_relay`, design/cli-mainnet-relay.md §2.2) — the submit
//! chokepoints (`registry::tx`) and the self-assembled `run_sponsored_tempo_call`
//! both route through it when `registry::is_mainnet()`. So a mainnet bundle
//! carries no money-moving key to extract. (The live mainnet sponsor —
//! `0x066E748367df1c2bfEdA9C445fBaAa093e10168f` — lives ONLY in the proxy env,
//! never here; it replaced `0xE70f4B…065E`, which was rotated out after being
//! exposed in earlier bundles.)
//!
//! ## Refilling
//!
//! The sponsor's `fee_token` (AlphaUSD) balance is what gets debited
//! for every sponsored tx. When it runs low:
//!
//! ```sh
//! cast call $ALPHA_USD "balanceOf(address)(uint256)" $SPONSOR \
//!     --rpc-url tempo_moderato
//!
//! # If low:
//! EVM_PRIVATE_KEY=<deployer> cast send $ALPHA_USD \
//!     "transfer(address,uint256)" $SPONSOR 1000000000000 \
//!     --rpc-url tempo_moderato
//! ```
//!
//! Or call `tempo_fundAddress` against the sponsor's address — that
//! drips all four TIP-20 stablecoins + native to it.

use k256::ecdsa::SigningKey;

/// Return the sponsor's `SigningKey` for `fee_payer` signing on Tempo txs —
/// a thin alias of the ONE key home, [`crate::registry::sponsor::fee_payer`]
/// (which carries the committed-testnet-key / mainnet-DUMMY cfg split). Kept
/// for the app's self-assembled sponsored paths (`run_sponsored_tempo_call`
/// and friends); the `registry::*_sponsored` wrappers resolve it themselves.
pub(crate) fn signer() -> Result<SigningKey, String> {
    crate::registry::sponsor::fee_payer()
}
