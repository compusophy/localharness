//! Per-chain config seam: the registry handles read from ONE [`ChainConfig`]
//! so mainnet becomes a RUNTIME flip, not a fork. Default ([`MODERATO`]) holds
//! today's exact testnet values, so a normal build is byte-for-byte unchanged.
//!
//! Native: [`active`] resolves the preset ONCE from the `LH_CHAIN` env var
//! (`mainnet` → [`MAINNET`], anything else → [`MODERATO`]), so ONE published
//! binary targets either chain with no recompile and no money-key embedded
//! (the mainnet sponsor lives server-side — `design/cli-mainnet-relay.md`).
//! wasm: no env, so the preset stays compile-time (`#[cfg(feature="mainnet")]`,
//! fixed at build by `build-web.sh`).

#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;

/// Network constants the registry/tx layer depends on. Pure data; [`ACTIVE`]
/// picks the preset at compile time.
pub struct ChainConfig {
    /// Human-facing network name (e.g. for self-docs / UI), NOT a wire value.
    pub name: &'static str,
    /// JSON-RPC endpoint.
    pub rpc_url: &'static str,
    /// EIP-155 chain id (binds every signature + the x402 domain).
    pub chain_id: u64,
    /// `LocalharnessRegistry` diamond proxy address.
    pub diamond: &'static str,
    /// `LocalharnessCredits` ($LH) token address.
    pub lh_token: &'static str,
    /// Default USD-currency TIP-20 used as the sponsor `fee_token` (NOT $LH).
    pub fee_token: &'static str,
}

/// Tempo Moderato testnet — the live deployment today.
pub const MODERATO: ChainConfig = ChainConfig {
    name: "Tempo Moderato",
    rpc_url: "https://rpc.moderato.tempo.xyz",
    chain_id: 42431,
    diamond: "0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c",
    lh_token: "0x90B84c7234Aae89BadA7f69160B9901B9bc37B17",
    fee_token: "0x20c0000000000000000000000000000000000001", // AlphaUSD
};

/// Tempo mainnet (chain 4217, live since 2026-03-18). rpc/chain are confirmed;
/// `diamond`/`lh_token`/`fee_token` stay EMPTY until the mainnet deploy
/// (`design/stripe-mainnet.md` step 12) — an empty diamond fails loudly rather
/// than silently transacting against the testnet deployment, so the `mainnet`
/// feature cannot ship by accident before those addresses are filled in.
pub const MAINNET: ChainConfig = ChainConfig {
    name: "Tempo mainnet",
    rpc_url: "https://rpc.tempo.xyz",
    chain_id: 4217,
    // Deployed 2026-06-16 (on-ramp money core). The full economy ladder is not
    // yet cut on mainnet (testnet has it); this is the diamond + token + meter +
    // MintGate on-ramp slice.
    diamond: "0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77",
    lh_token: "0x7ba3c9a39596e438b05c56dfc779700b58aea814",
    // USDC.e (Stargate-bridged USDC) — a USD-currency TIP-20 the chain accepts as
    // a gas/fee token (Tempo has no native coin); confirmed on the mainnet token list.
    fee_token: "0x20c000000000000000000000b9537d11c60e8b50",
};

/// The active chain, resolved ONCE on first read. Native: from `LH_CHAIN`
/// (`"mainnet"` → [`MAINNET`]; unset/anything else → [`MODERATO`]). wasm:
/// compile-time (`mainnet` feature). Resolve-once means the chain can't flip
/// mid-process — a tx signed for one chain is never submitted to another.
///
/// Tests exercising both presets must use [`MODERATO`]/[`MAINNET`] directly,
/// never `active()` (the `OnceLock` caches the first read for the process).
pub fn active() -> &'static ChainConfig {
    #[cfg(target_arch = "wasm32")]
    {
        #[cfg(feature = "mainnet")]
        {
            &MAINNET
        }
        #[cfg(not(feature = "mainnet"))]
        {
            &MODERATO
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        static ACTIVE_CHAIN: OnceLock<&'static ChainConfig> = OnceLock::new();
        ACTIVE_CHAIN.get_or_init(|| resolve_chain(std::env::var("LH_CHAIN").ok().as_deref()))
    }
}

/// Pure preset resolver from an `LH_CHAIN` value: `Some("mainnet")` →
/// [`MAINNET`]; anything else (unset, empty, "moderato", "testnet", junk) →
/// [`MODERATO`]. Default-to-testnet so the money path is opt-in. Split out so
/// the selection is unit-tested without touching the process-wide `OnceLock`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn resolve_chain(lh_chain: Option<&str>) -> &'static ChainConfig {
    match lh_chain {
        Some("mainnet") => &MAINNET,
        _ => &MODERATO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default (env-unset) native build is testnet, byte-for-byte: the seam must
    /// not have moved any value off Moderato. Reads `active()` (the live path),
    /// which on an unset `LH_CHAIN` resolves to MODERATO. (CI must not set
    /// `LH_CHAIN`; the test harness doesn't.)
    #[test]
    #[cfg(all(not(feature = "mainnet"), not(target_arch = "wasm32")))]
    fn active_is_moderato_by_default() {
        let a = active();
        assert_eq!(a.chain_id, 42431);
        assert_eq!(a.rpc_url, "https://rpc.moderato.tempo.xyz");
        assert_eq!(a.diamond, "0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c");
        assert_eq!(a.lh_token, "0x90B84c7234Aae89BadA7f69160B9901B9bc37B17");
        assert_eq!(a.fee_token, "0x20c0000000000000000000000000000000000001");
    }

    /// Runtime chain selection (CLI #4): `LH_CHAIN=mainnet` picks chain 4217;
    /// everything else (unset, empty, "moderato", "testnet", junk) stays on
    /// testnet — the money path is opt-in, never the default. Tests the pure
    /// resolver so it doesn't touch the process-wide `active()` `OnceLock`.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn lh_chain_env_selects_chain() {
        assert_eq!(resolve_chain(Some("mainnet")).chain_id, 4217);
        assert_eq!(resolve_chain(None).chain_id, 42431);
        assert_eq!(resolve_chain(Some("")).chain_id, 42431);
        assert_eq!(resolve_chain(Some("moderato")).chain_id, 42431);
        assert_eq!(resolve_chain(Some("testnet")).chain_id, 42431);
        assert_eq!(resolve_chain(Some("MAINNET")).chain_id, 42431); // exact match only
    }

    /// Mainnet on-ramp deployed 2026-06-16: diamond + $LH token + fee token all
    /// pinned. (The full economy ladder is not yet cut on mainnet.)
    #[test]
    fn mainnet_addresses_pinned() {
        assert_eq!(MAINNET.chain_id, 4217);
        assert_eq!(MAINNET.rpc_url, "https://rpc.tempo.xyz");
        assert_eq!(MAINNET.diamond, "0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77");
        assert_eq!(MAINNET.lh_token, "0x7ba3c9a39596e438b05c56dfc779700b58aea814");
        assert_eq!(MAINNET.fee_token, "0x20c000000000000000000000b9537d11c60e8b50");
    }
}
