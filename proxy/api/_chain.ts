// Per-chain config seam for the proxy — the TS mirror of src/registry/chain.rs.
// Each value reads from process.env (set on the Vercel project to go mainnet)
// and falls back to today's Moderato testnet values, so with env UNSET the
// proxy is byte-for-byte unchanged. To point the whole proxy at Tempo mainnet
// (chain 4217, https://rpc.tempo.xyz), set TEMPO_RPC / REGISTRY / CHAIN_ID /
// LH_TOKEN on the Vercel project — no code change. (process.env.X is statically
// inlined by the Edge build, so these resolve at deploy time.)

export const TEMPO_RPC = process.env.TEMPO_RPC ?? 'https://rpc.moderato.tempo.xyz';
export const REGISTRY = process.env.REGISTRY ?? '0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c';
export const CHAIN_ID = Number(process.env.CHAIN_ID ?? '42431');
export const LH_TOKEN = process.env.LH_TOKEN ?? '0x90B84c7234Aae89BadA7f69160B9901B9bc37B17';
// The chain's canonical sponsor fee token (a USD-currency TIP-20, NOT $LH). Default
// is Moderato AlphaUSD; prod sets FEE_TOKEN to mainnet USDC.e
// (0x20c000000000000000000000b9537d11c60e8b50) alongside the other env values. Used
// by the sponsor relay to pin which token pays gas (see sponsor.ts step 4).
export const FEE_TOKEN = process.env.FEE_TOKEN ?? '0x20c0000000000000000000000000000000000001';
