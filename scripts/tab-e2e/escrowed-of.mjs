// Read InviteFacet.escrowedOf(funder) via raw JSON-RPC — the CI-side
// on-chain truth for the iOS full-onboarding E2E: when the simulator's app
// accepts the bearer invite, the funder's escrowed total DROPS by the invite
// amount. No cast/foundry needed on the runner.
//
//   node scripts/tab-e2e/escrowed-of.mjs <funder-0xaddr>   → prints raw wei
//
// Selector 0xe1e6f37c = keccak("escrowedOf(address)")[0..4] (verified with
// `cast sig` 2026-07-07). Diamond + RPC = mainnet (src/registry/chain.rs).
const DIAMOND = '0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77';
const RPC = process.env.LH_RPC || 'https://rpc.tempo.xyz';

const addr = (process.argv[2] || '').toLowerCase().replace(/^0x/, '');
if (!/^[0-9a-f]{40}$/.test(addr)) {
  console.error('usage: node escrowed-of.mjs <funder-0xaddress>');
  process.exit(2);
}
const res = await fetch(RPC, {
  method: 'POST',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify({
    jsonrpc: '2.0',
    id: 1,
    method: 'eth_call',
    params: [{ to: DIAMOND, data: '0xe1e6f37c' + addr.padStart(64, '0') }, 'latest'],
  }),
});
const j = await res.json();
if (j.error) {
  console.error('eth_call error:', JSON.stringify(j.error));
  process.exit(1);
}
console.log(BigInt(j.result).toString());
