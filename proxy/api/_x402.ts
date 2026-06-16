// _x402.ts — shared x402 ("exact" $LH payment) verify + settle for the proxy.
//
// The proxy-side half of the x402 scheme: a CALLER signs an EIP-712
// PaymentAuthorization (registry::sign_x402 / X402Facet) for `value` $LH to a
// payee; the proxy verifies it LOCALLY (signer, payee, price floor+ceiling,
// window, replay, funds+allowance) and submits X402Facet.settle. The contract
// moves payer->payee purely from the signature, so the submitter (the proxy's
// gas-funded key) is irrelevant to authorization — it only fronts gas.
//
// The verify + encoding here mirror the PROVEN ask_agent path in mcp.ts (and
// src/registry/x402.rs + X402Facet.sol) EXACTLY — same digest, same EIP-2 low-s
// reject, same price-lock band — so the per-call LLM meter (gemini.ts) reuses
// identical, audited plumbing. The domain separator is read LIVE from the
// diamond so a chain/diamond change can't desync the digest.
//
// (mcp.ts keeps its own private copies for now; consolidating it onto this
// module is a safe follow-up — it's the live ask_agent valve, untouched here.)

import { secp256k1 } from '@noble/curves/secp256k1';
import { keccak_256 } from '@noble/hashes/sha3';
import { hexToBytes, bytesToHex } from '@noble/hashes/utils';
import { createWalletClient, defineChain, encodeFunctionData, http } from 'viem';
import { privateKeyToAccount } from 'viem/accounts';
import { TEMPO_RPC, REGISTRY, LH_TOKEN, CHAIN_ID } from './_chain';

const TEMPO_CHAIN = defineChain({
  id: CHAIN_ID,
  name: 'Tempo',
  nativeCurrency: { name: 'Tempo', symbol: 'TEMPO', decimals: 18 },
  rpcUrls: { default: { http: [TEMPO_RPC] } },
});

const X402_ABI = [
  {
    name: 'settle',
    type: 'function',
    stateMutability: 'nonpayable',
    inputs: [
      { name: 'from', type: 'address' },
      { name: 'to', type: 'address' },
      { name: 'value', type: 'uint256' },
      { name: 'validAfter', type: 'uint256' },
      { name: 'validBefore', type: 'uint256' },
      { name: 'nonce', type: 'bytes32' },
      { name: 'signature', type: 'bytes' },
    ],
    outputs: [],
  },
] as const;

// ---- crypto / ABI helpers (verbatim from mcp.ts → digest parity) -----------

function keccak(data: Uint8Array): Uint8Array {
  return keccak_256(data);
}
function concatBytes(...parts: Uint8Array[]): Uint8Array {
  let len = 0;
  for (const p of parts) len += p.length;
  const out = new Uint8Array(len);
  let off = 0;
  for (const p of parts) {
    out.set(p, off);
    off += p.length;
  }
  return out;
}
function stripHex(h: string): string {
  return h.startsWith('0x') || h.startsWith('0X') ? h.slice(2) : h;
}
export function isHexAddress(s: string): boolean {
  return /^0x[0-9a-fA-F]{40}$/.test(s);
}
function toAddress(pubKeyXY: Uint8Array): string {
  return '0x' + bytesToHex(keccak(pubKeyXY).slice(12));
}
function selectorHex(sig: string): string {
  return bytesToHex(keccak(new TextEncoder().encode(sig)).slice(0, 4));
}
function keccak32(data: Uint8Array): Uint8Array {
  return keccak(data);
}
function addrWord(address: string): Uint8Array {
  const a = hexToBytes(stripHex(address).toLowerCase().padStart(40, '0'));
  const w = new Uint8Array(32);
  w.set(a, 12);
  return w;
}
function uintWord(v: bigint): Uint8Array {
  if (v < 0n) throw new Error('uint underflow');
  if (v >> 256n !== 0n) throw new Error('uint overflow (exceeds 256 bits)');
  const w = new Uint8Array(32);
  let x = v;
  for (let i = 31; i >= 0 && x > 0n; i--) {
    w[i] = Number(x & 0xffn);
    x >>= 8n;
  }
  return w;
}
function bytes32Word(hex: string): Uint8Array {
  const b = hexToBytes(stripHex(hex));
  if (b.length !== 32) throw new Error('expected 32-byte value');
  return b;
}

// secp256k1n / 2 — the EIP-2 low-s bound (matches X402Facet.HALF_N). The on-chain
// settle REJECTS high-s; noble would happily recover the same address from a
// malleated copy, so reject high-s here too or we'd "verify" then waste a
// reverting settle.
const SECP256K1_HALF_N =
  0x7fffffffffffffffffffffffffffffff5d576e7357a4501ddfe92f46681b20a0n;

function recoverFromDigest(digest: Uint8Array, sigHex: string): string {
  const sig = hexToBytes(stripHex(sigHex));
  if (sig.length !== 65) throw new Error('signature must be 65 bytes');
  const r = sig.slice(0, 32);
  const s = sig.slice(32, 64);
  const sVal = BigInt('0x' + bytesToHex(s));
  if (sVal > SECP256K1_HALF_N) {
    throw new Error('signature has high-s (EIP-2 malleable) — not accepted');
  }
  let v = sig[64];
  if (v >= 27) v -= 27;
  if (v !== 0 && v !== 1) throw new Error('invalid recovery id');
  const signature = secp256k1.Signature.fromCompact(
    bytesToHex(concatBytes(r, s)),
  ).addRecoveryBit(v);
  const point = signature.recoverPublicKey(digest);
  return toAddress(point.toRawBytes(false).slice(1));
}

// ---- on-chain reads --------------------------------------------------------

async function ethCall(to: string, data: string): Promise<string> {
  const res = await fetch(TEMPO_RPC, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'eth_call', params: [{ to, data }, 'latest'] }),
  });
  const body = (await res.json()) as { result?: string; error?: unknown };
  if (!body.result) throw new Error('eth_call failed: ' + JSON.stringify(body.error ?? {}));
  return body.result;
}

/** `x402DomainSeparator()` read LIVE from the diamond (binds chainId+diamond). */
async function liveDomainSeparator(): Promise<Uint8Array> {
  const res = await ethCall(REGISTRY, '0x' + selectorHex('x402DomainSeparator()'));
  const b = hexToBytes(stripHex(res));
  if (b.length < 32) throw new Error('x402DomainSeparator returned short word');
  return b.slice(0, 32);
}

/** `authorizationState(from, nonce) -> bool` — true if this nonce was settled. */
async function authorizationState(from: string, nonce: string): Promise<boolean> {
  const data = '0x' + selectorHex('authorizationState(address,bytes32)') + bytesToHex(addrWord(from)) + stripHex(nonce);
  return BigInt(await ethCall(REGISTRY, data)) !== 0n;
}

/** `balanceOf(addr)` on the $LH token. */
async function lhBalanceOf(addr: string): Promise<bigint> {
  return BigInt(await ethCall(LH_TOKEN, '0x' + selectorHex('balanceOf(address)') + bytesToHex(addrWord(addr))));
}

/** `allowance(payer, diamond)` — settle pulls via transferFrom, so the diamond
 * must be approved for >= value. */
async function lhAllowanceToDiamond(payer: string): Promise<bigint> {
  const data =
    '0x' + selectorHex('allowance(address,address)') + bytesToHex(addrWord(payer)) + bytesToHex(addrWord(REGISTRY));
  return BigInt(await ethCall(LH_TOKEN, data));
}

// ---- x402 digest + price lock (mirror registry::x402.rs / X402Facet) -------

export interface X402Auth {
  from: string; // payer (0x address)
  to: string; // payee (0x address)
  value: bigint; // $LH wei
  validAfter: bigint;
  validBefore: bigint;
  nonce: string; // 0x + 32-byte hex
  signature: string; // 0x + 65-byte hex
}

const PAYMENT_TYPEHASH = keccak32(
  new TextEncoder().encode(
    'PaymentAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)',
  ),
);

function x402Digest(domainSeparator: Uint8Array, a: X402Auth): Uint8Array {
  const structHash = keccak32(
    concatBytes(
      PAYMENT_TYPEHASH,
      addrWord(a.from),
      addrWord(a.to),
      uintWord(a.value),
      uintWord(a.validAfter),
      uintWord(a.validBefore),
      bytes32Word(a.nonce),
    ),
  );
  return keccak32(concatBytes(new Uint8Array([0x19, 0x01]), domainSeparator, structHash));
}

// Price-lock overpay tolerance: 10% (1000 bps), mirroring
// registry::PRICE_LOCK_OVERPAY_TOLERANCE_BPS. settle moves EXACTLY the signed
// value, so an underpay is rejected (floor) and an overpay beyond this band is
// rejected too (re-quote) — no silent overpay from a stale quote.
const PRICE_LOCK_OVERPAY_TOLERANCE_BPS = 1000n;
export function priceLockCeiling(required: bigint): bigint {
  return required + (required * PRICE_LOCK_OVERPAY_TOLERANCE_BPS) / 10_000n;
}

/** Parse the X-PAYMENT / x-x402-authorization header JSON into an X402Auth.
 * Returns null when no header is present (no x402 attempt). Throws on a present
 * but malformed payload. Mirrors mcp.ts::parseAuth + registry::x402_authorization_json. */
export function parseX402Header(headerVal: string | null): X402Auth | null {
  if (!headerVal) return null;
  let raw: unknown;
  try {
    raw = JSON.parse(headerVal);
  } catch {
    throw new Error('x402 authorization is not valid JSON');
  }
  if (!raw || typeof raw !== 'object') throw new Error('x402 authorization is not an object');
  const o = raw as Record<string, unknown>;
  const toBig = (v: unknown, field: string): bigint => {
    if (typeof v === 'number') {
      if (!Number.isSafeInteger(v)) {
        throw new Error(`x402 authorization: ${field} exceeds safe-integer range — pass it as a decimal string`);
      }
      return BigInt(v);
    }
    if (typeof v === 'string' && v.trim() !== '') return BigInt(v);
    throw new Error(`x402 authorization: missing/invalid ${field}`);
  };
  const str = (v: unknown, field: string): string => {
    if (typeof v !== 'string' || v === '') throw new Error(`x402 authorization: missing ${field}`);
    return v;
  };
  const from = str(o.from, 'from');
  if (!isHexAddress(from)) throw new Error('x402 authorization: from is not an address');
  const to = typeof o.to === 'string' ? o.to : '';
  const nonce = str(o.nonce, 'nonce');
  if (!/^0x[0-9a-fA-F]{64}$/.test(nonce)) throw new Error('x402 authorization: nonce must be 32 bytes');
  const signature = str(o.signature, 'signature');
  if (!/^0x[0-9a-fA-F]{130}$/.test(signature)) throw new Error('x402 authorization: signature must be 65 bytes');
  return {
    from,
    to,
    value: toBig(o.value, 'value'),
    validAfter: toBig(o.validAfter ?? 0, 'validAfter'),
    validBefore: toBig(o.validBefore, 'validBefore'),
    nonce,
    signature,
  };
}

export type X402Verdict =
  | { ok: true; auth: X402Auth }
  | { ok: false; status: number; error: string; quote?: Record<string, unknown> };

/**
 * Fully verify an x402 authorization for a metered call BEFORE serving. Returns
 * null when no header is present (caller falls back to credit/session). On a
 * present authorization, runs the SAME gauntlet as ask_agent: bind payer to the
 * authenticated identity, bind payee, price floor + ceiling, validity window,
 * EIP-712 signature (live domain, low-s), replay (authorizationState), and a
 * funds + allowance pre-flight — so an invalid authorization fails fast with a
 * precise 402 instead of a reverting settle. `settle` stays the authoritative
 * gate; these reads just avoid spending gas on a doomed one.
 */
export async function verifyX402Payment(
  header: string | null,
  opts: { expectedFrom: string; payee: string; requiredWei: bigint },
): Promise<X402Verdict | null> {
  let auth: X402Auth | null;
  try {
    auth = parseX402Header(header);
  } catch (e) {
    return { ok: false, status: 402, error: (e as Error).message };
  }
  if (!auth) return null;

  const { expectedFrom, payee, requiredWei } = opts;
  // Bind the payer to the authenticated identity, and the payee to the platform
  // sink — an authorization can only pay THIS proxy, FROM this caller.
  if (auth.from.toLowerCase() !== expectedFrom.toLowerCase()) {
    return { ok: false, status: 402, error: 'x402 "from" must be the authenticated identity' };
  }
  if (!auth.to || auth.to.toLowerCase() !== payee.toLowerCase()) {
    return { ok: false, status: 402, error: 'x402 "to" must be the platform meter payee' };
  }
  // Price floor + ceiling (no underpay, no silent overpay on a stale quote).
  if (auth.value < requiredWei) {
    return {
      ok: false,
      status: 402,
      error: `x402 underpay: authorized ${auth.value} wei, requires ${requiredWei} wei $LH`,
      quote: { payTo: payee, scheme: 'x402-exact', asset: '$LH', chainId: CHAIN_ID, minValue: requiredWei.toString() },
    };
  }
  const ceiling = priceLockCeiling(requiredWei);
  if (auth.value > ceiling) {
    return {
      ok: false,
      status: 402,
      error: `x402 overpay: authorized ${auth.value} wei, current price ${requiredWei} wei (re-sign for the current price)`,
      quote: { payTo: payee, scheme: 'x402-exact', asset: '$LH', chainId: CHAIN_ID, priceChanged: true, currentPriceWei: requiredWei.toString(), maxValue: ceiling.toString() },
    };
  }
  // Validity window (the contract enforces this too; fail fast before gas).
  const now = BigInt(Math.floor(Date.now() / 1000));
  if (now <= auth.validAfter) return { ok: false, status: 402, error: 'x402 authorization not yet valid' };
  if (now >= auth.validBefore) return { ok: false, status: 402, error: 'x402 authorization expired' };

  // EIP-712 signature against the LIVE domain separator.
  try {
    const domain = await liveDomainSeparator();
    const recovered = recoverFromDigest(x402Digest(domain, auth), auth.signature);
    if (recovered.toLowerCase() !== auth.from.toLowerCase()) {
      return { ok: false, status: 402, error: `x402 signature does not match "from" (recovered ${recovered})` };
    }
  } catch (e) {
    return { ok: false, status: 402, error: 'x402 signature error: ' + (e as Error).message };
  }
  // Replay guard (best-effort pre-check; settle is authoritative).
  try {
    if (await authorizationState(auth.from, auth.nonce)) {
      return { ok: false, status: 402, error: 'x402 authorization already used (replayed nonce)' };
    }
  } catch {
    /* settle reverts AuthAlreadyUsed if so */
  }
  // Funds + allowance pre-flight (settle pulls via transferFrom).
  try {
    const [balance, allowance] = await Promise.all([lhBalanceOf(auth.from), lhAllowanceToDiamond(auth.from)]);
    if (balance < auth.value) {
      return { ok: false, status: 402, error: `insufficient $LH: payer holds ${balance} wei, needs ${auth.value} wei` };
    }
    if (allowance < auth.value) {
      return {
        ok: false,
        status: 402,
        error: `insufficient $LH allowance: approve(${REGISTRY}, value) first — approved ${allowance} wei, needs ${auth.value} wei`,
      };
    }
  } catch {
    /* reads failed — proceed; settle stays authoritative */
  }
  return { ok: true, auth };
}

/**
 * Submit `X402Facet.settle(...)`, signed by the proxy's gas-funded write key
 * (env `PROXY_METER_KEY`). BROADCAST ONLY — does NOT await the receipt, so a
 * streaming metered response is not delayed behind on-chain confirmation
 * (mirrors gemini.ts::meterDebit `confirm=false`). The pre-flight in
 * `verifyX402Payment` makes the settle reliable; the one-shot nonce makes a
 * stray double-submit a no-op revert. Returns the tx hash; throws only if the
 * BROADCAST itself fails. msg.sender is irrelevant — the contract moves
 * payer->payee purely from the signed authorization; this key only pays gas.
 */
export async function settleX402NoWait(a: X402Auth): Promise<`0x${string}`> {
  const pk = process.env.PROXY_METER_KEY;
  if (!pk) throw new Error('missing PROXY_METER_KEY (x402 settlement account)');
  const account = privateKeyToAccount((pk.startsWith('0x') ? pk : `0x${pk}`) as `0x${string}`);
  const wallet = createWalletClient({ account, chain: TEMPO_CHAIN, transport: http(TEMPO_RPC) });
  const data = encodeFunctionData({
    abi: X402_ABI,
    functionName: 'settle',
    args: [
      a.from as `0x${string}`,
      a.to as `0x${string}`,
      a.value,
      a.validAfter,
      a.validBefore,
      a.nonce as `0x${string}`,
      a.signature as `0x${string}`,
    ],
  });
  return wallet.sendTransaction({ to: REGISTRY as `0x${string}`, data, value: 0n });
}
