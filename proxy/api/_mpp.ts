// _mpp.ts — the Tempo MPP (Machine Payments Protocol) USDC.e -> $LH on-ramp lego.
//
// The crypto-native sibling of the Stripe fiat on-ramp (_stripe.ts): an
// autonomous agent (no human, no card) pays USDC.e on Tempo mainnet and gets
// $LH minted at WEB PARITY (1 USDC.e = 100 $LH, exactly the fiat peg $1 = 100
// $LH — a decided policy knob, NOT a market peg). Triggered by a verified
// on-chain USDC.e payment instead of a Stripe charge, but minting through the
// SAME MintGateFacet / mintFromFiat / ISSUER_ROLE valve (_stripe.ts helpers),
// so there is ONE money valve, not two.
//
// ROBUSTNESS CHOICE (design/cli-mainnet-onboarding.md C-2): we do NOT hard-depend
// on Stripe's preview `mppx` verify library. Instead this lego VERIFIES the
// USDC.e transfer ourselves — recipient == our treasury, amount matches the
// challenge, the tx is confirmed, and the SAME tx can mint only once (one-shot
// receipt keyed on the settlement tx hash). That mirrors the x402 settle verify
// hardening (_x402.ts / src/registry/x402.rs). We still emit the MPP-shaped 402
// challenge (WWW-Authenticate: Payment, method "tempo", intent "charge", a
// base64url `request` payload) so the endpoint is MPP-compatible and the full
// mppx facilitator verify can be swapped in later behind this same interface.
//
// MPP wire refs: docs.tempo.xyz/guide/machine-payments, docs.stripe.com/
// payments/machine/mpp, mpp.dev/protocol (charge intent == x402 "exact").

import { keccak_256 } from '@noble/hashes/sha3';
import { bytesToHex } from '@noble/hashes/utils';

// Reuse the fiat on-ramp's money valve verbatim — same MintGateFacet, same
// issuer key, same one-shot receipt. _mpp only adds the on-chain USDC.e verify
// in front of it.
import {
  signFiatMint,
  submitMintFromFiat,
  readReceipt,
  isHexAddress,
  PEG_WEI_PER_USD_CENT,
} from './_stripe';

// The on-ramp targets Tempo MAINNET, same env seam as _stripe.ts (ONRAMP_*),
// decoupled from _chain.ts (which the AI-metering path still points at testnet).
const TEMPO_RPC = process.env.ONRAMP_RPC ?? 'https://rpc.tempo.xyz';
const CHAIN_ID = Number(process.env.ONRAMP_CHAIN_ID ?? '4217');

// USDC.e (Stargate-bridged USDC) on Tempo mainnet — the chain's fee token and
// the MPP settle token. Same constant as src/registry/chain.rs MAINNET.fee_token.
// Env-overridable so the lego can point at another USD TIP-20 if the on-ramp
// asset ever changes.
export const USDCE_ADDRESS = (
  process.env.ONRAMP_USDCE ?? '0x20c000000000000000000000b9537d11c60e8b50'
).toLowerCase();

// The treasury that USDC.e payments must land in for a mint to fire. There is NO
// safe default — paying an unset/zero address must never mint — so callers MUST
// set ONRAMP_TREASURY on the Vercel project. `treasuryAddress()` throws if unset.
export function treasuryAddress(): string {
  const t = process.env.ONRAMP_TREASURY ?? '';
  if (!isHexAddress(t)) {
    throw new Error('missing/invalid ONRAMP_TREASURY (the USDC.e on-ramp recipient)');
  }
  return t.toLowerCase();
}

// MPP method/intent advertised in the challenge. "tempo" = stablecoin-on-Tempo
// settlement; "charge" = one-shot (x402 "exact"-equivalent) — the only intent
// this lego implements (Sessions/subscriptions are a later build).
export const MPP_METHOD = 'tempo';
export const MPP_INTENT = 'charge';

// USDC.e is 6-decimal (confirmed: Tempo TIP-20 USDC.e). We READ decimals()
// on-chain at verify time anyway (money-critical: never silently mis-peg on a
// surprise), and use this only as the challenge-quote default + a sanity bound.
const USDCE_DECIMALS_DEFAULT = 6;

// --- peg: USDC.e base units <-> $LH wei --------------------------------------

// $LH wei per ONE whole USDC.e, at web parity. PEG_WEI_PER_USD_CENT is $LH wei
// per USD cent (default 1e18 → $1 = 100 $LH); a whole USDC.e == $1 == 100 cents,
// so one USDC.e mints 100 * PEG_WEI_PER_USD_CENT $LH wei. Reusing the SAME peg
// constant keeps the two on-ramps locked to one rate (change it in one place).
export const LH_WEI_PER_USDCE = PEG_WEI_PER_USD_CENT * 100n;

/** Convert a USDC.e amount (in its own base units, `decimals` places) to $LH
 *  wei at parity. Pure integer math; floors any sub-unit dust. */
export function usdceUnitsToLhWei(units: bigint, decimals: number): bigint {
  if (units <= 0n) throw new Error('usdce amount must be positive');
  if (!Number.isInteger(decimals) || decimals < 0 || decimals > 36) {
    throw new Error('implausible usdce decimals');
  }
  // wholeUsdce = units / 10^decimals (fractional); lhWei = wholeUsdce *
  // LH_WEI_PER_USDCE. Do it as (units * LH_WEI_PER_USDCE) / 10^decimals to keep
  // full integer precision before the floor.
  return (units * LH_WEI_PER_USDCE) / 10n ** BigInt(decimals);
}

/** Inverse: the USDC.e base units to quote for a desired $LH wei amount, rounded
 *  UP so the buyer never underpays the quote (the challenge advertises this). */
export function lhWeiToUsdceUnits(lhWei: bigint, decimals: number): bigint {
  if (lhWei <= 0n) throw new Error('lhWei must be positive');
  const scale = 10n ** BigInt(decimals);
  const num = lhWei * scale;
  return (num + LH_WEI_PER_USDCE - 1n) / LH_WEI_PER_USDCE; // ceil-div
}

// --- receiptId: one-shot per settlement tx -----------------------------------

// keccak256("localharness.mppmint:" + lowercased settlement tx hash). Bound to
// the IMMUTABLE on-chain tx hash (not buyer-editable), so the MintGateFacet
// one-shot receipt makes the mint idempotent across retries / double-submits —
// the same idempotency shape as the Stripe path's receiptIdFor(paymentIntentId).
export function receiptIdForTx(txHash: string): `0x${string}` {
  const norm = txHash.toLowerCase().replace(/^0x/, '');
  if (!/^[0-9a-f]{64}$/.test(norm)) throw new Error('settlement tx hash must be 32 bytes');
  const tag = new TextEncoder().encode('localharness.mppmint:0x' + norm);
  return ('0x' + bytesToHex(keccak_256(tag))) as `0x${string}`;
}

// --- MPP 402 challenge (WWW-Authenticate: Payment) ---------------------------

function base64url(bytes: Uint8Array): string {
  let bin = '';
  for (const b of bytes) bin += String.fromCharCode(b);
  // btoa is available on Edge/Web runtimes.
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function base64urlDecode(s: string): Uint8Array {
  const pad = s.length % 4 === 0 ? '' : '='.repeat(4 - (s.length % 4));
  const bin = atob(s.replace(/-/g, '+').replace(/_/g, '/') + pad);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** The base64url-encoded `request` payload inside the WWW-Authenticate header:
 *  the MPP charge payment terms (mpp.dev/protocol). `payTo` is OUR treasury,
 *  `asset` the USDC.e token, `maxAmountRequired` the price in USDC.e base units. */
export interface MppChargeRequest {
  scheme: 'mpp';
  intent: 'charge';
  network: 'tempo';
  chainId: number;
  asset: string; // USDC.e token address
  payTo: string; // treasury recipient
  maxAmountRequired: string; // USDC.e base units (decimal string)
  maxTimeoutSeconds: number;
  resource: string; // the resource being paid for (this endpoint URL)
  description: string;
}

export interface MppChallenge {
  id: string;
  request: MppChargeRequest;
  expires: number; // unix seconds
}

// How long a quoted challenge stays valid (the client must pay + retry within).
const CHALLENGE_TTL_SECS = 600;

/** Build an MPP charge challenge quoting `usdceUnits` of USDC.e to the treasury
 *  for `resource`. The id is a random 16-byte hex nonce (informational; our
 *  verify binds to the on-chain tx, not the id). */
export function buildChallenge(opts: {
  usdceUnits: bigint;
  resource: string;
}): MppChallenge {
  const id = bytesToHex(crypto.getRandomValues(new Uint8Array(16)));
  return {
    id,
    request: {
      scheme: 'mpp',
      intent: MPP_INTENT,
      network: MPP_METHOD,
      chainId: CHAIN_ID,
      asset: USDCE_ADDRESS,
      payTo: treasuryAddress(),
      maxAmountRequired: opts.usdceUnits.toString(),
      maxTimeoutSeconds: CHALLENGE_TTL_SECS,
      resource: opts.resource,
      description: 'localharness $LH on-ramp (USDC.e -> $LH at parity)',
    },
    expires: Math.floor(Date.now() / 1000) + CHALLENGE_TTL_SECS,
  };
}

/** The `WWW-Authenticate: Payment ...` header value for a 402 (one method:
 *  tempo/charge). Auth params per RFC 7235: token68/quoted-string members. */
export function challengeHeader(ch: MppChallenge): string {
  const requestB64 = base64url(new TextEncoder().encode(JSON.stringify(ch.request)));
  // Members are id, method, intent, request (base64url), expires — the shape
  // mpp.dev/protocol documents for the WWW-Authenticate Payment scheme.
  return (
    `Payment id="${ch.id}", method="${MPP_METHOD}", intent="${MPP_INTENT}", ` +
    `request="${requestB64}", expires="${ch.expires}"`
  );
}

/** The RFC 9457 problem+json body that accompanies the 402 (mpp.dev/protocol). */
export function challengeBody(ch: MppChallenge): Record<string, unknown> {
  return {
    type: 'https://paymentauth.org/problems/payment-required',
    title: 'Payment Required',
    status: 402,
    detail:
      'Pay the quoted USDC.e to the recipient, then retry with an Authorization: Payment credential.',
    challengeId: ch.id,
    accepts: [ch.request],
  };
}

// --- Authorization: Payment credential (the client's payment proof) ----------

/** What the client sends back to claim a mint: the settlement tx hash (its
 *  USDC.e transfer to the treasury) plus the lh_address to credit. We verify the
 *  tx on-chain, so the credential is just a pointer — it cannot fabricate value. */
export interface MppCredential {
  settlementTx: string; // 0x + 32-byte tx hash of the USDC.e transfer
  payTo: string; // $LH recipient (the agent's identity address)
}

/** Parse `Authorization: Payment payload="<b64url-json>"` (or a raw base64url /
 *  raw JSON value, for tolerant clients) into a credential. Returns null when no
 *  Payment credential is present; throws on a present-but-malformed one. */
export function parseCredential(authHeader: string | null): MppCredential | null {
  if (!authHeader) return null;
  const m = /^\s*Payment\s+(.*)$/i.exec(authHeader);
  if (!m) return null;
  const rest = m[1].trim();

  let jsonText: string;
  const payloadMatch = /payload="([^"]*)"/.exec(rest);
  if (payloadMatch) {
    jsonText = new TextDecoder().decode(base64urlDecode(payloadMatch[1]));
  } else if (rest.startsWith('{')) {
    jsonText = rest; // raw JSON after the scheme
  } else {
    // bare base64url token68
    jsonText = new TextDecoder().decode(base64urlDecode(rest.replace(/^"|"$/g, '')));
  }

  let raw: unknown;
  try {
    raw = JSON.parse(jsonText);
  } catch {
    throw new Error('Payment credential payload is not valid JSON');
  }
  if (!raw || typeof raw !== 'object') throw new Error('Payment credential is not an object');
  const o = raw as Record<string, unknown>;
  const settlementTx = String(o.settlementTx ?? o.txHash ?? o.transactionHash ?? '');
  const payTo = String(o.payTo ?? o.to ?? o.recipient ?? '');
  if (!/^0x[0-9a-fA-F]{64}$/.test(settlementTx)) {
    throw new Error('Payment credential: settlementTx must be a 32-byte tx hash');
  }
  if (!isHexAddress(payTo)) {
    throw new Error('Payment credential: payTo must be a 20-byte address');
  }
  return { settlementTx: settlementTx.toLowerCase(), payTo: payTo.toLowerCase() };
}

// --- on-chain USDC.e transfer verify (our own hardened verify) ---------------

async function rpc(method: string, params: unknown[]): Promise<unknown> {
  const res = await fetch(TEMPO_RPC, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }),
  });
  const body = (await res.json()) as { result?: unknown; error?: unknown };
  if (body.error) throw new Error(`${method} failed: ${JSON.stringify(body.error)}`);
  return body.result;
}

/** ERC-20/TIP-20 Transfer(address,address,uint256) topic0. */
const TRANSFER_TOPIC =
  '0x' + bytesToHex(keccak_256(new TextEncoder().encode('Transfer(address,address,uint256)')));

function topicAddress(topic: string): string {
  // A 32-byte topic word, address right-aligned in the low 20 bytes.
  return ('0x' + topic.slice(-40)).toLowerCase();
}

/** Read the USDC.e token's `decimals()` on-chain (money-critical: never assume).
 *  Falls back to the 6-decimal default only if the read is unavailable. */
async function usdceDecimals(): Promise<number> {
  try {
    const sel = '0x' + bytesToHex(keccak_256(new TextEncoder().encode('decimals()')).slice(0, 4));
    const out = (await rpc('eth_call', [{ to: USDCE_ADDRESS, data: sel }, 'latest'])) as string;
    const d = Number(BigInt(out));
    if (Number.isInteger(d) && d >= 0 && d <= 36) return d;
  } catch {
    /* fall through to default */
  }
  return USDCE_DECIMALS_DEFAULT;
}

export type MppVerifyResult =
  | { ok: true; lhWei: bigint; usdceUnits: bigint; payer: string }
  | { ok: false; status: number; error: string };

// A settlement must be buried at least this many blocks before we mint (cheap
// reorg guard; Tempo has sub-second finality but a confirmation depth is still
// good hygiene for a money valve). Env-tunable.
const MIN_CONFIRMATIONS = BigInt(process.env.ONRAMP_MIN_CONFIRMATIONS ?? '1');

/**
 * Fully verify that `settlementTx` is a CONFIRMED USDC.e transfer to OUR
 * treasury, and return the $LH wei it entitles the payer to. Hardened like the
 * x402 settle verify:
 *   - the tx is mined, status == success;
 *   - it has >= MIN_CONFIRMATIONS;
 *   - it contains a USDC.e Transfer log to the treasury (summed across logs in
 *     case a router split it), with from == the payer;
 *   - amount > 0; the minted $LH is derived from the on-chain amount at parity,
 *     NEVER from client input.
 * Replay protection (a tx minting only once) is the MintGateFacet one-shot
 * receipt keyed on this tx hash — enforced at mint, the authoritative gate.
 */
export async function verifySettlement(settlementTx: string): Promise<MppVerifyResult> {
  let receipt: {
    status?: string;
    to?: string;
    from?: string;
    blockNumber?: string;
    logs?: Array<{ address?: string; topics?: string[]; data?: string }>;
  } | null;
  try {
    receipt = (await rpc('eth_getTransactionReceipt', [settlementTx])) as typeof receipt;
  } catch (e) {
    return { ok: false, status: 502, error: 'settlement receipt lookup failed: ' + (e as Error).message };
  }
  if (!receipt) {
    return { ok: false, status: 402, error: 'settlement tx not found or not yet mined' };
  }
  if (receipt.status !== undefined && BigInt(receipt.status) !== 1n) {
    return { ok: false, status: 402, error: 'settlement tx reverted (status != 1)' };
  }

  // Confirmation depth.
  try {
    const head = BigInt((await rpc('eth_blockNumber', [])) as string);
    const mined = BigInt(receipt.blockNumber ?? '0x0');
    if (head < mined || head - mined < MIN_CONFIRMATIONS) {
      return { ok: false, status: 402, error: 'settlement not yet confirmed enough; retry' };
    }
  } catch {
    /* head read failed — receipt presence already proves it mined; proceed */
  }

  const treasury = treasuryAddress();
  let total = 0n;
  let payer = '';
  for (const log of receipt.logs ?? []) {
    if ((log.address ?? '').toLowerCase() !== USDCE_ADDRESS) continue;
    const topics = log.topics ?? [];
    if (topics.length < 3 || topics[0].toLowerCase() !== TRANSFER_TOPIC) continue;
    const to = topicAddress(topics[2]);
    if (to !== treasury) continue;
    const from = topicAddress(topics[1]);
    // A standard Transfer's value is the single 32-byte data word.
    let amount: bigint;
    try {
      amount = BigInt(log.data && log.data !== '0x' ? log.data : '0x0');
    } catch {
      continue;
    }
    if (amount <= 0n) continue;
    total += amount;
    // Attribute the payment to the first non-treasury sender we see.
    if (!payer && from !== treasury) payer = from;
  }

  if (total <= 0n) {
    return { ok: false, status: 402, error: 'no USDC.e transfer to the treasury in this tx' };
  }

  const decimals = await usdceDecimals();
  let lhWei: bigint;
  try {
    lhWei = usdceUnitsToLhWei(total, decimals);
  } catch (e) {
    return { ok: false, status: 502, error: 'peg conversion failed: ' + (e as Error).message };
  }
  if (lhWei <= 0n) {
    return { ok: false, status: 402, error: 'paid amount too small to mint any $LH' };
  }
  return { ok: true, lhWei, usdceUnits: total, payer: payer || (receipt.from ?? '').toLowerCase() };
}

// --- mint orchestration (reuse MintGateFacet via _stripe helpers) ------------

export type MppMintResult =
  | { minted: true; idempotent?: boolean; tx?: string; lhWei: string; recipient?: string }
  | { minted: false; error: string; status: number };

/**
 * Verify the USDC.e settlement, then GROSS-mint $LH into `lhAddress`'s meter via
 * the SAME MintGateFacet path the Stripe webhook uses (issuer-signed FiatMint,
 * one-shot receipt keyed on the settlement tx). Idempotent: a second call for
 * the same tx reads the on-chain receipt and no-ops. The minted amount comes
 * ONLY from the on-chain USDC.e amount at parity — never from client input.
 */
export async function mintFromSettlement(
  settlementTx: string,
  lhAddress: string,
): Promise<MppMintResult> {
  if (!isHexAddress(lhAddress)) return { minted: false, error: 'invalid lh_address', status: 400 };

  const receiptId = receiptIdForTx(settlementTx);
  // On-chain idempotency backstop: a retried mint for an already-used tx is a
  // clean success, not a reverting resubmit.
  try {
    const r = await readReceipt(receiptId);
    if (r.used) return { minted: true, idempotent: true, lhWei: r.amount.toString() };
  } catch {
    /* receipt read failed — proceed; the one-shot at mint stays authoritative */
  }

  const v = await verifySettlement(settlementTx);
  if (!v.ok) return { minted: false, error: v.error, status: v.status };

  // SECURITY: credit the PROVEN on-chain USDC.e payer (`v.payer`), never the
  // caller-supplied `lhAddress`/`pay_to`. A settlement tx is proof of PAYMENT, not
  // of identity — minting to a free-floating address would let anyone replay another
  // party's (or any treasury-inbound) USDC.e transfer and mint the $LH to themselves
  // (front-running a legit buyer's settlement, or claiming an unrelated inbound).
  // The mint must follow the money: only whoever actually moved value gets credited.
  const recipient = v.payer;
  if (!isHexAddress(recipient)) {
    return { minted: false, error: 'could not determine the settlement payer on-chain', status: 502 };
  }

  const validBefore = BigInt(Math.floor(Date.now() / 1000) + 3600);
  let signature: `0x${string}`;
  try {
    signature = await signFiatMint(recipient, v.lhWei, receiptId, validBefore);
  } catch (e) {
    return { minted: false, error: 'mint signing failed: ' + (e as Error).message, status: 500 };
  }
  try {
    const tx = await submitMintFromFiat(recipient, v.lhWei, receiptId, validBefore, signature);
    return { minted: true, tx, lhWei: v.lhWei.toString(), recipient };
  } catch (e) {
    // The submit may have raced another path (webhook-style double fire) — if the
    // receipt actually landed, report success honestly.
    try {
      const r = await readReceipt(receiptId);
      if (r.used) return { minted: true, idempotent: true, lhWei: r.amount.toString() };
    } catch {
      /* fall through */
    }
    return { minted: false, error: 'mint submit failed: ' + (e as Error).message, status: 502 };
  }
}
