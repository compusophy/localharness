// Shared Stripe + on-chain helpers for the fiat on-ramp (Stripe → Tempo $LH).
// Imported by stripe-checkout.ts (creates the Checkout Session, binds lh_address)
// and stripe-webhook.ts (NODE runtime: raw-body HMAC → mintFromFiat / clawback).
//
// Money-safety rules encoded here (design/custody-security.md + stripe-mainnet §6):
//   * PEG fixes $LH-wei per USD cent; mint against GROSS charged USD (fees
//     absorbed — $LH is decoupled from $, $1 = 100 $LH, no net-of-fees haircut).
//   * receiptId derives ONLY from the immutable Stripe PaymentIntent id, so a
//     replayed webhook hits the on-chain one-shot receipt (idempotent).
//   * FIAT_ISSUER_KEY only SIGNS the EIP-712 FiatMint; it must be DISTINCT from
//     PROXY_METER_KEY (asserted). Gas is paid by the (already-funded) meter key.

import Stripe from 'stripe';
import { keccak_256 } from '@noble/hashes/sha3';
import { bytesToHex } from '@noble/hashes/utils';
import {
  createPublicClient,
  createWalletClient,
  defineChain,
  encodeFunctionData,
  http,
} from 'viem';
import { privateKeyToAccount } from 'viem/accounts';
// The personal-sign auth token + hex helper now live in the shared, dep-light
// _authcore (audit L7/L10 dedup — ONE verifier behind every route). Re-export
// the THROWING variant under the name this module's importers
// (publish/telemetry/mpp/stripe-checkout/stripe-finalize) already use, plus
// isHexAddress (used locally in mintSettledPayment below + by _mpp).
import { verifyAuthTokenOrThrow, isHexAddress } from './_authcore';
export { verifyAuthTokenOrThrow as verifyAuthToken, isHexAddress };

// The on-ramp targets Tempo MAINNET, decoupled from `_chain.ts` (which the
// AI-metering path still points at testnet). Override via ONRAMP_* env. Defaults
// are the live mainnet diamond + $LH token (chain 4217).
const TEMPO_RPC = process.env.ONRAMP_RPC ?? 'https://rpc.tempo.xyz';
const REGISTRY = process.env.ONRAMP_REGISTRY ?? '0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77';
const CHAIN_ID = Number(process.env.ONRAMP_CHAIN_ID ?? '4217');

// --- peg ---------------------------------------------------------------

// $LH wei per USD cent. $LH is DECOUPLED from the dollar (a credit/points token,
// NOT a stablecoin), so this "peg" is just the issuance RATE we choose, not a
// backing ratio. Default: $1 = 100 $LH → 1e18 wei/cent (1e20/dollar). At
// 1 $LH = 1 message that's "$1 = 100 messages"; a $2 buy mints 200 $LH (the
// first-subdomain bundle amount). Env-overridable so the rate is config.
export const PEG_WEI_PER_USD_CENT = ((): bigint => {
  try {
    return BigInt(process.env.LH_PEG_WEI_PER_USD_CENT ?? '1000000000000000000');
  } catch {
    return 1_000_000_000_000_000_000n;
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

// --- mint orchestration (shared by the webhook + the client finalize path) ---

// NET settled amount in cents: expand the PaymentIntent → latest charge →
// balance transaction `net` (gross minus Stripe fees). FAIL-CLOSED: if net
// isn't available yet (async settlement / transient API error) we THROW so the
// caller can retry — minting GROSS would over-issue by the Stripe fee and
// permanently breach circulating ≤ usd_held/peg. The one-shot receiptId makes
// the eventual retry idempotent. Card + Link both settle synchronously, so net
// is normally available by webhook/finalize time.
export async function netSettledCents(piId: string): Promise<number> {
  const pi = await stripe().paymentIntents.retrieve(piId, {
    expand: ['latest_charge.balance_transaction'],
  });
  const charge = pi.latest_charge as Stripe.Charge | null;
  const bt = charge?.balance_transaction as Stripe.BalanceTransaction | null;
  // FAIL CLOSED on the peg: `bt.net` is in the ACCOUNT's SETTLEMENT currency's
  // minor unit, which `centsToWei` treats as USD cents. The charge is created in
  // USD, so a USD-settling account gives `bt.currency==='usd'` and net IS cents.
  // But if the account ever settled in another currency (esp. a zero-decimal one
  // like JPY, where `net` is whole units), `centsToWei(net)` would mis-mint. So
  // require USD settlement explicitly — never guess the peg.
  if (bt && bt.currency === 'usd' && typeof bt.net === 'number' && bt.net > 0) {
    return bt.net;
  }
  throw new Error(`net settled USD amount not yet available for ${piId}; retry`);
}

// GROSS amount the buyer was charged, in USD cents. $LH is DECOUPLED from the
// dollar (NOT a stablecoin), so we mint the full round amount at the rate and
// ABSORB Stripe fees — no net-of-fees haircut (the old "$1 → 0.67 $LH"). The PI
// `amount_received` is known immediately on a succeeded charge (no
// balance_transaction wait), so the prior "net not ready, retry" path is gone.
// Requires a succeeded USD charge; FAIL-CLOSED otherwise (never mint on a
// non-succeeded or non-USD PI).
export async function grossPaidCents(piId: string): Promise<number> {
  const pi = await stripe().paymentIntents.retrieve(piId);
  if (pi.status !== 'succeeded') {
    throw new Error(`PaymentIntent ${piId} not succeeded (${pi.status})`);
  }
  if (pi.currency !== 'usd') {
    throw new Error(`PaymentIntent ${piId} not charged in USD (${pi.currency})`);
  }
  const cents =
    typeof pi.amount_received === 'number' && pi.amount_received > 0
      ? pi.amount_received
      : pi.amount;
  if (!Number.isInteger(cents) || cents <= 0) {
    throw new Error(`PaymentIntent ${piId} has no positive charged amount`);
  }
  return cents;
}

// Idempotent GROSS mint for a SETTLED PaymentIntent. Mints `mintFromFiat` to the
// PI's bound `lh_address` for the GROSS charged USD at the issuance rate, guarded
// by the on-chain one-shot receipt — so the webhook AND the client
// `/stripe/finalize` call are both idempotent (whichever lands first wins; the
// other is a no-op). THROWS on not-succeeded / RPC / submit failure so the
// webhook 500s → Stripe retries; finalize catches the throw and reports `pending`.
export async function mintSettledPayment(
  piId: string,
  lhAddress: string,
): Promise<{ minted: boolean; idempotent?: boolean; tx?: string }> {
  if (!isHexAddress(lhAddress)) return { minted: false };
  const receiptId = receiptIdFor(piId);
  const r = await readReceipt(receiptId);
  if (r.used) return { minted: true, idempotent: true };
  const grossCents = await grossPaidCents(piId); // THROWS if not succeeded → retry
  const amountWei = centsToWei(grossCents);
  const validBefore = BigInt(Math.floor(Date.now() / 1000) + 3600);
  const signature = await signFiatMint(lhAddress, amountWei, receiptId, validBefore);
  const tx = await submitMintFromFiat(lhAddress, amountWei, receiptId, validBefore, signature);
  return { minted: true, tx };
}
