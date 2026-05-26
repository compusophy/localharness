//! Bundle-side sponsor key — the wallet that signs as `fee_payer`
//! on every user Tempo tx so users themselves never need to hold any
//! native gas OR any TIP-20 stablecoin.
//!
//! ## Trust model
//!
//! This module holds a **private key in the wasm bundle**. Anyone
//! running localharness.xyz can extract it. That's accepted on
//! testnet (Tempo Moderato — funds are play-money and the sponsor
//! is refillable via `tempo_fundAddress`), and **must change before
//! mainnet**.
//!
//! Replacement paths once we go mainnet:
//! - Tempo access keys with scoped fee_payer permission (if Tempo
//!   supports access keys for fee_payer signing — TBD by live test).
//! - WebAuthn passkeys per user (each user is their own sponsor).
//! - A 4337 paymaster with policy enforcement at the EntryPoint.
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
/// The key lives here so the build is self-contained — no env-var or
/// runtime fetch needed. **Do not commit a mainnet key here.** Use
/// a build-time env mechanism for that.
const SPONSOR_PRIVATE_KEY_HEX: &str =
    "0x046a830b5203d1d2c0a205a1432746e4381d0874711b2de7f575a973644b9d43";

/// Return the sponsor's `SigningKey` for `fee_payer` signing on
/// Tempo txs. Cheap to call repeatedly — k256 keys clone cheaply.
pub(crate) fn signer() -> Result<SigningKey, String> {
    let trimmed = SPONSOR_PRIVATE_KEY_HEX
        .trim_start_matches("0x")
        .trim_start_matches("0X");
    let bytes = decode_hex(trimmed)?;
    if bytes.len() != 32 {
        return Err(format!(
            "sponsor private key must be 32 bytes, got {}",
            bytes.len()
        ));
    }
    SigningKey::from_slice(&bytes).map_err(|e| format!("sponsor key invalid: {e}"))
}

fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("sponsor key hex odd length".into());
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = nibble(bytes[i])?;
        let lo = nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn nibble(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(format!("non-hex byte {b}")),
    }
}
