//! Per-chain config seam: the registry consts read from ONE [`ChainConfig`]
//! so mainnet becomes a feature flip, not a fork. Default ([`MODERATO`]) holds
//! today's exact testnet values, so a normal build is byte-for-byte unchanged;
//! the `mainnet` cargo feature swaps in [`MAINNET`] (Tempo mainnet, chain 4217).

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

/// The compile-time-selected active chain. `mainnet` feature off (default) =
/// [`MODERATO`]; on = [`MAINNET`].
#[cfg(not(feature = "mainnet"))]
pub const ACTIVE: ChainConfig = MODERATO;
#[cfg(feature = "mainnet")]
pub const ACTIVE: ChainConfig = MAINNET;

#[cfg(test)]
mod tests {
    use super::*;

    /// Default build is testnet, byte-for-byte: the seam must not have moved
    /// any value off Moderato when `mainnet` is unset.
    #[test]
    #[cfg(not(feature = "mainnet"))]
    fn active_is_moderato_by_default() {
        assert_eq!(ACTIVE.chain_id, 42431);
        assert_eq!(ACTIVE.rpc_url, "https://rpc.moderato.tempo.xyz");
        assert_eq!(ACTIVE.diamond, "0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c");
        assert_eq!(ACTIVE.lh_token, "0x90B84c7234Aae89BadA7f69160B9901B9bc37B17");
        assert_eq!(ACTIVE.fee_token, "0x20c0000000000000000000000000000000000001");
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
