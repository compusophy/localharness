//! Per-chain config seam: the registry handles read from ONE [`ChainConfig`]
//! so mainnet vs testnet is a RUNTIME flip, not a fork.
//!
//! Native: [`active`] resolves the preset ONCE from the `LH_CHAIN` env var.
//! Default ([`MAINNET`]) is the LIVE chain — the CLI exists for agents using the
//! real platform; testnet is an explicit DEV opt-in (`LH_CHAIN=testnet`/`moderato`/
//! `dev`, or the `--dev` flag). An UNRECOGNIZED `LH_CHAIN` is a HARD ERROR, never a
//! silent fallback (a typo must not quietly sign on a chain you didn't mean). No
//! money key is embedded — the mainnet sponsor lives server-side
//! (`design/cli-mainnet-relay.md`). wasm: no env, so the preset stays compile-time
//! (`#[cfg(feature="mainnet")]`, fixed at build by `build-web.sh`).

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
    /// Block-explorer base URL (e.g. `https://explore.tempo.xyz`). The UI builds
    /// `{explorer_url}/address/{addr}` links from this so it never hardcodes a
    /// per-chain explorer host (which used to point mainnet links at the testnet).
    pub explorer_url: &'static str,
}

/// Tempo Moderato testnet — the native CLI/SDK default + the wasm-no-mainnet
/// preview build. cfg-gated OUT of the prod wasm+mainnet bundle so NO testnet
/// config (rpc / chain id / diamond / token / explorer) is even compiled into
/// the shipped browser binary; `active()` there is pinned to [`MAINNET`] and
/// never names this const, native (`resolve_chain`) + wasm-no-mainnet keep it.
#[cfg(any(not(target_arch = "wasm32"), not(feature = "mainnet")))]
pub const MODERATO: ChainConfig = ChainConfig {
    name: "Tempo Moderato",
    rpc_url: "https://rpc.moderato.tempo.xyz",
    chain_id: 42431,
    diamond: "0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c",
    lh_token: "0x90B84c7234Aae89BadA7f69160B9901B9bc37B17",
    fee_token: "0x20c0000000000000000000000000000000000001", // AlphaUSD
    explorer_url: "https://moderato.tempo.xyz",
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
    // Tempo mainnet block explorer (Blockscout-style /address/{0x…}), chain 4217.
    explorer_url: "https://explore.tempo.xyz",
};

/// The active chain, resolved ONCE on first read. Native: from `LH_CHAIN`
/// (unset/`"mainnet"` → [`MAINNET`]; `"testnet"`/`"moderato"`/`"dev"` →
/// [`MODERATO`]; anything else PANICS — call [`validate_chain_env`] first so the
/// CLI reports the typo cleanly). wasm: compile-time (`mainnet` feature).
/// Resolve-once means the chain can't flip mid-process — a tx signed for one
/// chain is never submitted to another.
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
        ACTIVE_CHAIN.get_or_init(|| {
            resolve_chain(std::env::var("LH_CHAIN").ok().as_deref()).unwrap_or_else(|e| panic!("{e}"))
        })
    }
}

/// Pure preset resolver from an `LH_CHAIN` value. Unset or `"mainnet"` → the LIVE
/// [`MAINNET`] (the DEFAULT — the CLI is for agents on the real platform);
/// `"testnet"`/`"moderato"`/`"dev"` → [`MODERATO`] (the explicit dev opt-in);
/// ANYTHING ELSE → `Err` (a typo must never silently sign on a chain you didn't
/// mean — especially not default-to-mainnet-money on a junk value). Split out so
/// the selection is unit-tested without touching the process-wide `OnceLock`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn resolve_chain(lh_chain: Option<&str>) -> Result<&'static ChainConfig, String> {
    match lh_chain {
        None | Some("mainnet") => Ok(&MAINNET),
        Some("testnet") | Some("moderato") | Some("dev") => Ok(&MODERATO),
        Some(other) => Err(format!(
            "unrecognized LH_CHAIN '{other}' — use 'mainnet' (default) or \
             'testnet'/'moderato'/'dev' for the dev chain"
        )),
    }
}

/// Validate the `LH_CHAIN` env var for the CLI: `Ok(())` if unset or a recognized
/// value, `Err(msg)` for a typo. Lets `main.rs` fail fast with a clean message
/// BEFORE any [`active`] read (which would otherwise panic on a junk value).
#[cfg(not(target_arch = "wasm32"))]
pub fn validate_chain_env() -> Result<(), String> {
    resolve_chain(std::env::var("LH_CHAIN").ok().as_deref()).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default (env-unset) native resolves to MAINNET — the CLI targets the live
    /// platform; testnet is an explicit opt-in. Reads the pure resolver (not
    /// `active()`, whose `OnceLock` caches and whose env read CI must not perturb).
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn resolve_chain_defaults_to_mainnet() {
        let a = resolve_chain(None).expect("unset LH_CHAIN is valid");
        assert_eq!(a.chain_id, 4217);
        assert_eq!(a.rpc_url, "https://rpc.tempo.xyz");
        assert_eq!(a.diamond, "0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77");
    }

    /// Runtime chain selection: unset/`mainnet` → 4217 (the default LIVE chain);
    /// `testnet`/`moderato`/`dev` → 42431 (the explicit dev opt-in); an
    /// UNRECOGNIZED value is a HARD ERROR, never a silent fallback (a typo must
    /// not quietly sign on the wrong chain). Exact-match only ("MAINNET" errors).
    /// Tests the pure resolver so it doesn't touch the process-wide `OnceLock`.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn lh_chain_env_selects_chain() {
        assert_eq!(resolve_chain(None).unwrap().chain_id, 4217);
        assert_eq!(resolve_chain(Some("mainnet")).unwrap().chain_id, 4217);
        assert_eq!(resolve_chain(Some("testnet")).unwrap().chain_id, 42431);
        assert_eq!(resolve_chain(Some("moderato")).unwrap().chain_id, 42431);
        assert_eq!(resolve_chain(Some("dev")).unwrap().chain_id, 42431);
        // Unrecognized values ERROR — no silent default-to-mainnet-money footgun.
        assert!(resolve_chain(Some("")).is_err());
        assert!(resolve_chain(Some("MAINNET")).is_err()); // exact match only
        assert!(resolve_chain(Some("prod")).is_err());
        assert!(resolve_chain(Some("xyz")).is_err());
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

    /// SSOT drift guard (tech-debt report §3): `proxy/api/_chain.ts` falls back to
    /// the testnet defaults with env unset, and those fallbacks MUST mirror this
    /// crate's [`MODERATO`] preset. The proxy and the SDK both talk to the same
    /// diamond; if the testnet addresses move here (e.g. a reset) and the proxy
    /// defaults don't, the off-chain proxy silently targets a stale deployment.
    /// Reads the TS at test time; skips if the proxy tree isn't present (a packaged
    /// crate), so it only enforces inside the repo where both live.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn proxy_chain_ts_defaults_match_moderato() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("proxy/api/_chain.ts");
        let Ok(src) = std::fs::read_to_string(&path) else {
            eprintln!("skip: {} not present (packaged crate?)", path.display());
            return;
        };
        let chain_id = MODERATO.chain_id.to_string();
        let want = [
            ("TEMPO_RPC", MODERATO.rpc_url),
            ("REGISTRY", MODERATO.diamond),
            ("CHAIN_ID", chain_id.as_str()),
            ("LH_TOKEN", MODERATO.lh_token),
        ];
        for (key, expect) in want {
            let line = src
                .lines()
                .find(|l| l.contains(&format!("export const {key} ")))
                .unwrap_or_else(|| panic!("_chain.ts missing `export const {key}`"));
            assert!(
                line.contains(expect),
                "_chain.ts default for {key} drifted from Rust MODERATO (expected `{expect}`).\n  \
                 line: {line}\n  Update proxy/api/_chain.ts (or MODERATO) so they match."
            );
        }
    }
}
