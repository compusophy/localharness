//! Bundle-side sponsor key — the wallet that signs as `fee_payer`
//! on every user Tempo tx so users themselves never need to hold any
//! native gas OR any TIP-20 stablecoin.
//!
//! ## Trust model
//!
//! On **testnet** this module holds a committed **private key in the wasm
//! bundle** — anyone running localharness.xyz can extract it, which is accepted
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

/// Testnet sponsor key (Tempo Moderato). Dedicated low-budget sponsor —
/// `0x0AFf88Ad13eF24caC5BeFD0F9Dc3A05DF79a922C`. Holds only the AlphaUSD
/// needed to pay user fees + a small native buffer. If the bundle is
/// extracted (XSS, mass scrape, etc.) the loss is capped at this
/// wallet's balance — neither the deployer nor any other admin key is
/// reachable from here. Top up via `tempo_fundAddress` (drips all four
/// TIP-20 stablecoins + native) when the balance drops.
///
/// Previous sponsor (rotated 2026-05-25): same address as the deployer,
/// `0x313b1659F5037080aA0C113D386C5954F348EF1e`. Funds remain there
/// untouched; they can be reclaimed by the deployer key.
///
/// Committed TESTNET sponsor key. On testnet it pays AlphaUSD fees directly. On
/// mainnet it is UNUSED: every sponsored path checks `registry::is_mainnet()` and
/// routes the `fee_payer` half to the server relay instead, so this key never signs
/// a mainnet tx (the mainnet sponsor `0x066E…0168f` is server-side only, never
/// embedded). `env!("LH_MAINNET_SPONSOR_KEY")` is GONE — no build embeds a mainnet
/// money key. The real key is embedded on native (CLI testnet) + wasm-no-mainnet
/// (testnet preview); the PROD wasm+mainnet bundle ships a DUMMY instead so no real
/// key is extractable from it (`signer()` still returns Ok — the value is unused there).
#[cfg(not(all(target_arch = "wasm32", feature = "mainnet")))]
const SPONSOR_PRIVATE_KEY_HEX: &str =
    "0x046a830b5203d1d2c0a205a1432746e4381d0874711b2de7f575a973644b9d43";
#[cfg(all(target_arch = "wasm32", feature = "mainnet"))]
const SPONSOR_PRIVATE_KEY_HEX: &str =
    "0x0000000000000000000000000000000000000000000000000000000000000001";

/// Return the sponsor's `SigningKey` for `fee_payer` signing on Tempo txs.
/// Cheap to call repeatedly — k256 keys clone cheaply. On mainnet the returned
/// key is an UNUSED placeholder (the relay signs `fee_payer`); callers keep
/// calling this so they flow into the submit chokepoints, where the mainnet
/// branch ignores it.
pub(crate) fn signer() -> Result<SigningKey, String> {
    crate::wallet::from_private_key_hex(SPONSOR_PRIVATE_KEY_HEX)
        .map_err(|e| format!("sponsor key invalid: {e}"))
}
