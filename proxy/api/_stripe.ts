// Shared Stripe + on-chain helpers for the fiat on-ramp (Stripe → Tempo $LH).
// Imported by stripe-checkout.ts (creates the Checkout Session, binds lh_address)
// and stripe-webhook.ts (NODE runtime: raw-body HMAC → mintFromFiat / clawback).
//
// Money-safety rules encoded here (design/custody-security.md + stripe-mainnet §6):
//   * PEG fixes $LH-wei per USD cent; mint against NET settled USD (fees out).
//   * receiptId derives ONLY from the immutable Stripe PaymentIntent id, so a
//     replayed webhook hits the on-chain one-shot receipt (idempotent).
//   * FIAT_ISSUER_KEY only SIGNS the EIP-712 FiatMint; it must be DISTINCT from
//     PROXY_METER_KEY (asserted). Gas is paid by the (already-funded) meter key.

import Stripe from 'stripe';
import { secp256k1 } from '@noble/curves/secp256k1';
import { keccak_256 } from '@noble/hashes/sha3';
import { bytesToHex, hexToBytes } from '@noble/hashes/utils';
import {
  createPublicClient,
  createWalletClient,
  defineChain,
  encodeFunctionData,
  http,
} from 'viem';
import { privateKeyToAccount } from 'viem/accounts';

// The on-ramp targets Tempo MAINNET, decoupled from `_chain.ts` (which the
// AI-metering path still points at testnet). Override via ONRAMP_* env. Defaults
// are the live mainnet diamond + $LH token (chain 4217).
const TEMPO_RPC = process.env.ONRAMP_RPC ?? 'https://rpc.tempo.xyz';
const REGISTRY = process.env.ONRAMP_REGISTRY ?? '0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77';
const CHAIN_ID = Number(process.env.ONRAMP_CHAIN_ID ?? '4217');

// --- peg ---------------------------------------------------------------

// $LH wei per USD cent. Default: 1 $LH = $1 → 1e16 wei/cent (1e18/dollar).
// Env-overridable so the peg is config, not a code constant.
export const PEG_WEI_PER_USD_CENT = ((): bigint => {
  try {
    return BigInt(process.env.LH_PEG_WEI_PER_USD_CENT ?? '10000000000000000');
  } catch {
    return 10_000_000_000_000_000n;
  }
})();

export function usdCentsToWei(cents: number): bigint {
  if (!Number.isInteger(cents) || cents <= 0) {
    throw new Error('cents must be a positive integer');
  }
  return BigInt(cents) * PEG_WEI_PER_USD_CENT;
}

// --- Stripe SDK --------------------------------------------------------

let _stripe: Stripe | null = null;
export function stripe(): Stripe {
  if (_stripe) return _stripe;
  const key = process.env.STRIPE_SECRET_KEY;
  if (!key) throw new Error('missing STRIPE_SECRET_KEY');
  // Edge runtime: Stripe must use fetch (no Node http module). apiVersion
  // omitted → the account's pinned default (test mode first).
  _stripe = new Stripe(key, { httpClient: Stripe.createFetchHttpClient() });
  return _stripe;
}

// WebCrypto-based HMAC verifier for the webhook (Edge has no Node crypto, so
// `constructEvent` (sync) is unavailable — use `constructEventAsync` with this).
export const stripeCryptoProvider = Stripe.createSubtleCryptoProvider();

// --- receiptId ---------------------------------------------------------

// keccak256("localharness.fiatmint:" + paymentIntentId). Bound to an IMMUTABLE,
// non-buyer-editable Stripe id, so the on-chain one-shot receipt makes the mint
// idempotent across Stripe's aggressive retries.
export function receiptIdFor(paymentIntentId: string): `0x${string}` {
  const tag = new TextEncoder().encode('localharness.fiatmint:' + paymentIntentId);
  return ('0x' + bytesToHex(keccak_256(tag))) as `0x${string}`;
}

// --- chain + contract --------------------------------------------------

export const TEMPO_CHAIN = defineChain({
  id: CHAIN_ID,
  name: 'Tempo',
  nativeCurrency: { name: 'Tempo', symbol: 'TEMPO', decimals: 18 },
  rpcUrls: { default: { http: [TEMPO_RPC] } },
});

const MINTGATE_ABI = [
  {
    name: 'mintFromFiat',
    type: 'function',
    stateMutability: 'nonpayable',
    inputs: [
      { name: 'to', type: 'address' },
      { name: 'amount', type: 'uint256' },
      { name: 'receiptId', type: 'bytes32' },
      { name: 'validBefore', type: 'uint256' },
      { name: 'signature', type: 'bytes' },
    ],
    outputs: [],
  },
  {
    name: 'clawbackFiatMint',
    type: 'function',
    stateMutability: 'nonpayable',
    inputs: [
      { name: 'receiptId', type: 'bytes32' },
      { name: 'maxWei', type: 'uint256' },
    ],
    outputs: [{ name: 'recovered', type: 'uint256' }],
  },
  {
    name: 'receiptInfo',
    type: 'function',
    stateMutability: 'view',
    inputs: [{ name: 'receiptId', type: 'bytes32' }],
    outputs: [
      { name: 'to', type: 'address' },
      { name: 'amount', type: 'uint256' },
      { name: 'used', type: 'bool' },
      { name: 'clawed', type: 'bool' },
      { name: 'clawedWei', type: 'uint256' },
    ],
  },
] as const;

function normKey(k: string): `0x${string}` {
  return (k.startsWith('0x') ? k : `0x${k}`) as `0x${string}`;
}

// FIAT_ISSUER_KEY only signs the EIP-712 FiatMint — never mints directly, and
// MUST be distinct from the on-ramp submitter key (red-team M: a proxy RCE then
// leaks a cap-bounded signing oracle, not the gas/submit key).
export function issuerAccount() {
  const k = process.env.FIAT_ISSUER_KEY;
  if (!k) throw new Error('missing FIAT_ISSUER_KEY');
  const submitter = process.env.ONRAMP_SUBMITTER_KEY ?? process.env.PROXY_METER_KEY;
  if (submitter && normKey(k).toLowerCase() === normKey(submitter).toLowerCase()) {
    throw new Error('FIAT_ISSUER_KEY must be distinct from the on-ramp submitter key');
  }
  return privateKeyToAccount(normKey(k));
}

const FIAT_MINT_TYPES = {
  FiatMint: [
    { name: 'to', type: 'address' },
    { name: 'amount', type: 'uint256' },
    { name: 'receiptId', type: 'bytes32' },
    { name: 'validBefore', type: 'uint256' },
  ],
} as const;

// EIP-712 sign — domain MUST match MintGateFacet.fiatMintDomainSeparator().
export async function signFiatMint(
  to: string,
  amountWei: bigint,
  receiptId: `0x${string}`,
  validBefore: bigint,
): Promise<`0x${string}`> {
  return issuerAccount().signTypedData({
    domain: {
      name: 'localharness-mintgate',
      version: '1',
      chainId: CHAIN_ID,
      verifyingContract: REGISTRY as `0x${string}`,
    },
    types: FIAT_MINT_TYPES,
    primaryType: 'FiatMint',
    message: { to: to as `0x${string}`, amount: amountWei, receiptId, validBefore },
  });
}

// The submitter pays gas only (its account fee token is set to USDC.e on Tempo).
// mintFromFiat's authorization is the signature, so msg.sender is irrelevant. A
// dedicated mainnet on-ramp key, separate from the testnet metering PROXY_METER_KEY
// (falls back to it only if unset, for the testnet-pipe path).
function submitterWallet() {
  const k = process.env.ONRAMP_SUBMITTER_KEY ?? process.env.PROXY_METER_KEY;
  if (!k) throw new Error('missing ONRAMP_SUBMITTER_KEY');
  return createWalletClient({
    account: privateKeyToAccount(normKey(k)),
    chain: TEMPO_CHAIN,
    transport: http(TEMPO_RPC),
  });
}

export async function submitMintFromFiat(
  to: string,
  amountWei: bigint,
  receiptId: `0x${string}`,
  validBefore: bigint,
  signature: `0x${string}`,
): Promise<string> {
  const data = encodeFunctionData({
    abi: MINTGATE_ABI,
    functionName: 'mintFromFiat',
    args: [to as `0x${string}`, amountWei, receiptId, validBefore, signature],
  });
  return submitterWallet().sendTransaction({ to: REGISTRY as `0x${string}`, data, value: 0n });
}

// Claw back a fiat mint. `maxWei` is the CUMULATIVE wei to have clawed by now
// (0 = the full receipt, for disputes / full refunds); a partial refund passes
// Stripe's cumulative amount_refunded in wei, so the contract claws the delta.
export async function submitClawback(
  receiptId: `0x${string}`,
  maxWei: bigint,
): Promise<string> {
  const data = encodeFunctionData({
    abi: MINTGATE_ABI,
    functionName: 'clawbackFiatMint',
    args: [receiptId, maxWei],
  });
  return submitterWallet().sendTransaction({ to: REGISTRY as `0x${string}`, data, value: 0n });
}

// On-chain idempotency backstop: read the receipt's state before acting so a
// Stripe retry is a clean 200 no-op instead of a reverting resubmit.
export async function readReceipt(
  receiptId: `0x${string}`,
): Promise<{ to: string; amount: bigint; used: boolean; clawed: boolean; clawedWei: bigint }> {
  const pub = createPublicClient({ chain: TEMPO_CHAIN, transport: http(TEMPO_RPC) });
  const [to, amount, used, clawed, clawedWei] = (await pub.readContract({
    address: REGISTRY as `0x${string}`,
    abi: MINTGATE_ABI,
    functionName: 'receiptInfo',
    args: [receiptId],
  })) as [string, bigint, boolean, boolean, bigint];
  return { to, amount, used, clawed, clawedWei };
}

// Shared peg conversion (also used by the webhook for refund-amount → wei).
export function centsToWei(cents: number): bigint {
  if (!Number.isInteger(cents) || cents <= 0) throw new Error('non-positive cents');
  return BigInt(cents) * PEG_WEI_PER_USD_CENT;
}

// --- off-session (Tier 2) customer mapping -----------------------------
//
// The lh_address ↔ Stripe Customer link is stored IN STRIPE
// (Customer.metadata.lh_address) — NOT an off-chain DB (respects the no-server-
// state rule) and NOT on-chain (no new gas/selector). The webhook tags the
// Customer on the first Checkout; the off-session top-up resolves it live. Cards
// are resolved live from Stripe at charge time (Stripe is the source of truth —
// never cache PaymentMethod ids).

// Tag a (Checkout-created) Customer with its lh_address so later off-session
// top-ups can find it. Idempotent (overwrites the same value).
export async function tagCustomerLhAddress(customerId: string, lhAddress: string): Promise<void> {
  await stripe().customers.update(customerId, {
    metadata: { lh_address: lhAddress.toLowerCase() },
  });
}

// Resolve the Stripe Customer for an lh_address (tagged on its first Checkout).
// null when the buyer has never completed a browser Checkout (→ no saved card).
export async function findCustomerIdByLhAddress(lhAddress: string): Promise<string | null> {
  const res = await stripe().customers.search({
    query: `metadata['lh_address']:'${lhAddress.toLowerCase()}'`,
    limit: 1,
  });
  return res.data[0]?.id ?? null;
}

// Sum (in cents) the SUCCEEDED off-session top-ups for a Customer since
// `sinceUnix` — the per-address rolling cap so a leaked auth token can't drain a
// saved card. Reads from Stripe (the source of truth); no local counter to drift.
// Bounded to one 100-item page (a sane day never has more off-session top-ups).
export async function rollingOffsessionCents(customerId: string, sinceUnix: number): Promise<number> {
  const page = await stripe().paymentIntents.list({
    customer: customerId,
    created: { gte: sinceUnix },
    limit: 100,
  });
  let total = 0;
  for (const pi of page.data) {
    if (pi.status === 'succeeded' && pi.metadata?.lh_flow === 'offsession_topup') total += pi.amount;
  }
  return total;
}

// --- caller auth (mirrors gemini.ts personal-sign auth token) ----------

function keccak(data: Uint8Array): Uint8Array {
  return keccak_256(data);
}

function concat(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

function stripHex(h: string): string {
  return h.startsWith('0x') ? h.slice(2) : h;
}

export function isHexAddress(s: string): boolean {
  return /^0x[0-9a-fA-F]{40}$/.test(s);
}

function recoverAddress(message: string, sigHex: string): string {
  const msgBytes = new TextEncoder().encode(message);
  const prefix = new TextEncoder().encode(`\x19Ethereum Signed Message:\n${msgBytes.length}`);
  const digest = keccak(concat(prefix, msgBytes));
  const sig = hexToBytes(stripHex(sigHex));
  if (sig.length !== 65) throw new Error('signature must be 65 bytes');
  const r = sig.slice(0, 32);
  const s = sig.slice(32, 64);
  let v = sig[64];
  if (v >= 27) v -= 27;
  const signature = secp256k1.Signature.fromCompact(bytesToHex(concat(r, s))).addRecoveryBit(v);
  const point = signature.recoverPublicKey(digest);
  return '0x' + bytesToHex(keccak(point.toRawBytes(false).slice(1)).slice(12));
}

const FRESHNESS_WINDOW_SECS = 300;

// Verify the `<address>:<timestamp>:<signature>` auth token (same scheme as the
// gemini proxy). Returns the authenticated lowercase address or throws.
export function verifyAuthToken(token: string): string {
  const parts = (token ?? '').split(':');
  if (parts.length !== 3) throw new Error('missing or malformed auth token');
  const [address, tsStr, signature] = parts;
  const timestamp = Number(tsStr);
  if (!address || !signature || !Number.isInteger(timestamp) || timestamp < 0) {
    throw new Error('malformed auth token');
  }
  if (!isHexAddress(address)) throw new Error('malformed auth token: address');
  const now = Math.floor(Date.now() / 1000);
  if (Math.abs(now - timestamp) > FRESHNESS_WINDOW_SECS) {
    throw new Error('stale or future timestamp');
  }
  const message = `localharness-proxy:${address.toLowerCase()}:${timestamp}`;
  const recovered = recoverAddress(message, signature);
  if (recovered.toLowerCase() !== address.toLowerCase()) {
    throw new Error('signature does not match address');
  }
  return address.toLowerCase();
}
