// localharness MCP-over-HTTP endpoint — hosted, x402-gated (Edge).
//
// A minimal MCP "Streamable HTTP" server: POST-only, one JSON-RPC request in,
// one JSON-RPC response out (no SSE, no session id). Tools:
//   * ask_agent(name, message) — runs the named on-chain agent's published
//     persona against the caller's message via the platform Gemini key (exactly
//     the headless `localharness call` path). x402-GATED (see below).
//   * discover_agents(query)   — FREE, read-only. The on-chain agent
//     yellow-pages: enumerate recent agents + rank by query (name + persona).
//   * list_bounties()          — FREE, read-only. Open, unexpired bounties
//     (work an agent could claim for $LH).
// Reachable by any remote MCP client (Claude Desktop, Cursor, another agent's
// MCP client, …) over plain HTTP. Discovery is FREE on purpose — it's the demand
// on-ramp, and must be frictionless; only `ask_agent` settles a payment.
//
// The gate is TRUE x402 per-call settlement (NOT the coarse session/meter
// credit gate `gemini.ts` uses): the caller PAYS THE AGENT being called. For
// every `tools/call`, the client supplies an x402 `PaymentAuthorization`
// (EIP-712, signed by the payer) authorizing `$LH` payer -> the agent's
// token-bound account. The proxy verifies the signature against the LIVE
// `x402DomainSeparator()` of the diamond, reconstructs the digest exactly as
// `src/registry.rs::x402_digest` / `X402Facet.sol`, ecrecovers — and only
// AFTER the agent's model produced a successful answer does it submit
// `X402Facet.settle(...)` on-chain (SETTLE-ON-SUCCESS). Verification still
// REJECTS before any model spend; but a model failure (timeout / 500) never
// takes the payer's money — the one-shot authorization stays unused and
// expires harmlessly. No answer, no payment. (Issue #25: the old order
// settled first, so a failed paid call permanently charged the payer.)
//
// The mirror of the Rust client side is `src/registry.rs` (x402_domain_separator
// / x402_digest / sign_x402 / encode_settle / settle_x402_sponsored) and
// `contracts/src/facets/X402Facet.sol` (the on-chain settle + EIP-712 domain).
// This file re-derives the SAME EIP-712 encoding in TypeScript; the
// `x402DomainSeparator()` is read live so a chain/diamond change can't desync it.

import { secp256k1 } from '@noble/curves/secp256k1';
import { keccak_256 } from '@noble/hashes/sha3';
import { hexToBytes, bytesToHex } from '@noble/hashes/utils';
import {
  createPublicClient,
  createWalletClient,
  defineChain,
  encodeFunctionData,
  http,
} from 'viem';
import { privateKeyToAccount } from 'viem/accounts';

export const config = { runtime: 'edge' };

// ---- constants (shared with gemini.ts) -------------------------------------

const TEMPO_RPC = 'https://rpc.moderato.tempo.xyz';
const REGISTRY = '0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c';
// $LH token (LocalharnessCredits, TIP-20). `X402Facet.settle` moves $LH
// payer->payee via `transferFrom`, so the payer needs BOTH a balance and an
// allowance to the diamond. Under settle-on-success the model runs before the
// money moves, so both are pre-flighted (free eth_calls) BEFORE any model
// spend — without that, a $LH-less payer with a validly signed authorization
// would farm free answers (every settle would fail into the serve-anyway
// policy). Mirrors `registry::LH_TOKEN_ADDRESS`.
const LH_TOKEN = '0x90B84c7234Aae89BadA7f69160B9901B9bc37B17';
const GEMINI_BASE = 'https://generativelanguage.googleapis.com';
const CHAIN_ID = 42431;
// The non-streaming model used to answer `ask_agent`. Mirrors the headless
// `localharness call` default; kept simple (no per-call model selection for now).
const ASK_MODEL = process.env.MCP_ASK_MODEL ?? 'gemini-3.5-flash';
const MCP_PROTOCOL_VERSION = '2025-06-18';
const MCP_SERVER_NAME = 'localharness';
const MCP_SERVER_VERSION = '0.1.0';

const MAX_BODY_BYTES = 16_000_000;
const ALLOWED_ORIGIN_SUFFIX = '.localharness.xyz';
const ALLOWED_ORIGIN_EXACT = 'https://localharness.xyz';

const TEMPO_CHAIN = defineChain({
  id: CHAIN_ID,
  name: 'Tempo Moderato',
  nativeCurrency: { name: 'Tempo', symbol: 'TEMPO', decimals: 18 },
  rpcUrls: { default: { http: [TEMPO_RPC] } },
});

// BountyFacet views — the FREE discovery surface for `list_bounties`. Read-only
// (no state mutation, no x402). NOTE: the task view is `bountyTaskOf`, NOT
// `taskOf` — ScheduleFacet already owns the `taskOf(uint256)` selector and a
// diamond can't share one (see CLAUDE.md / BountyFacet.sol).
const BOUNTY_ABI = [
  {
    name: 'openBounties',
    type: 'function',
    stateMutability: 'view',
    inputs: [
      { name: 'startAfter', type: 'uint256' },
      { name: 'limit', type: 'uint256' },
    ],
    outputs: [
      { name: 'ids', type: 'uint256[]' },
      { name: 'nextCursor', type: 'uint256' },
    ],
  },
  {
    name: 'getBounty',
    type: 'function',
    stateMutability: 'view',
    inputs: [{ name: 'bountyId', type: 'uint256' }],
    outputs: [
      { name: 'poster', type: 'address' },
      { name: 'rewardWei', type: 'uint128' },
      { name: 'expiry', type: 'uint64' },
      { name: 'status', type: 'uint8' },
      { name: 'claimantTokenId', type: 'uint256' },
    ],
  },
  {
    name: 'bountyTaskOf',
    type: 'function',
    stateMutability: 'view',
    inputs: [{ name: 'bountyId', type: 'uint256' }],
    outputs: [{ name: 'task', type: 'bytes' }],
  },
] as const;

// X402Facet.settle(address,address,uint256,uint256,uint256,bytes32,bytes)
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

// ---- CORS ------------------------------------------------------------------

function corsHeaders(origin: string | null): Record<string, string> {
  const h: Record<string, string> = {
    'Access-Control-Allow-Methods': 'POST, OPTIONS',
    'Access-Control-Allow-Headers':
      'content-type, x-x402-authorization, mcp-protocol-version, mcp-session-id',
    Vary: 'Origin',
  };
  // The MCP endpoint is also called by non-browser clients (Claude Desktop,
  // servers) that send no Origin — those are allowed through (no CORS header
  // needed). Browser origins get reflected only when on our domain / localhost.
  if (origin && isAllowedOrigin(origin)) {
    h['Access-Control-Allow-Origin'] = origin;
  }
  return h;
}

function isAllowedOrigin(origin: string): boolean {
  if (origin === ALLOWED_ORIGIN_EXACT || origin.endsWith(ALLOWED_ORIGIN_SUFFIX)) {
    return true;
  }
  try {
    const u = new URL(origin);
    return (
      u.protocol === 'http:' &&
      (u.hostname === 'localhost' || u.hostname === '127.0.0.1')
    );
  } catch {
    return false;
  }
}

// ---- crypto / ABI helpers (mirror gemini.ts + registry.rs) -----------------

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

function isHexAddress(s: string): boolean {
  return /^0x[0-9a-fA-F]{40}$/.test(s);
}

/** Lowercase 0x address from a 64-byte uncompressed pubkey (no 0x04 prefix). */
function toAddress(pubKeyXY: Uint8Array): string {
  return '0x' + bytesToHex(keccak(pubKeyXY).slice(12));
}

/** 4-byte function selector hex (no 0x) — keccak256(sig)[..4]. */
function selectorHex(sig: string): string {
  return bytesToHex(keccak(new TextEncoder().encode(sig)).slice(0, 4));
}

/** keccak256 over the bytes, as a 32-byte array. */
function keccak32(data: Uint8Array): Uint8Array {
  return keccak(data);
}

/** Left-pad an address (20 bytes) into a 32-byte ABI word. */
function addrWord(address: string): Uint8Array {
  const a = hexToBytes(stripHex(address).toLowerCase().padStart(40, '0'));
  const w = new Uint8Array(32);
  w.set(a, 12);
  return w;
}

/** Big-endian 32-byte word for a uint (bigint, fits 256 bits). */
function uintWord(v: bigint): Uint8Array {
  if (v < 0n) throw new Error('uint underflow');
  // Reject anything that does not fit a uint256 rather than SILENTLY
  // truncating the high bits — a truncated word would desync the
  // reconstructed digest from the value, and an oversized value can never
  // settle on-chain anyway. Fail fast and unambiguously.
  if (v >> 256n !== 0n) throw new Error('uint overflow (exceeds 256 bits)');
  const w = new Uint8Array(32);
  let x = v;
  for (let i = 31; i >= 0 && x > 0n; i--) {
    w[i] = Number(x & 0xffn);
    x >>= 8n;
  }
  return w;
}

/** A 32-byte word from a 0x-hex string that is already 32 bytes (e.g. nonce). */
function bytes32Word(hex: string): Uint8Array {
  const b = hexToBytes(stripHex(hex));
  if (b.length !== 32) throw new Error('expected 32-byte value');
  return b;
}

/**
 * ecrecover an address from a RAW 32-byte digest (NOT personal-sign-wrapped).
 * This is the EIP-712 path: the payer signs the 712 digest directly, so we
 * recover from that digest with no `\x19Ethereum Signed Message` prefix — the
 * exact inverse of `src/wallet.rs::recover_address` (used by `sign_x402`).
 * `sigHex` is 65 bytes r||s||v, v ∈ {27,28} or {0,1}.
 *
 * Mirrors `X402Facet._isValidSignature`'s EOA branch EXACTLY, INCLUDING the
 * EIP-2 low-s requirement: noble's `recoverPublicKey` happily recovers the
 * SAME address from a high-s malleated copy of a signature, but the on-chain
 * `settle` REJECTS high-s (`uint256(vs) > HALF_N -> BadSignature`). Without
 * this check the proxy would "verify" a malleated sig, then waste a settle tx
 * that reverts and report a confusing "settlement failed" 402. Reject high-s
 * here so a malleated authorization fails fast with a precise signature error.
 */
// secp256k1n / 2 — the EIP-2 low-s bound (matches X402Facet.HALF_N).
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

// ---- JSON-RPC over the wire ------------------------------------------------

interface RpcResp {
  result?: string;
  error?: unknown;
}

async function ethCall(to: string, data: string): Promise<string> {
  const res = await fetch(TEMPO_RPC, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'eth_call',
      params: [{ to, data }, 'latest'],
    }),
  });
  const body = (await res.json()) as RpcResp;
  if (!body.result) {
    throw new Error('eth_call failed: ' + JSON.stringify(body.error ?? {}));
  }
  return body.result;
}

/** `x402DomainSeparator()` read LIVE from the diamond (binds chainId+diamond). */
async function liveDomainSeparator(): Promise<Uint8Array> {
  const data = '0x' + selectorHex('x402DomainSeparator()');
  const res = await ethCall(REGISTRY, data);
  const b = hexToBytes(stripHex(res));
  if (b.length < 32) throw new Error('x402DomainSeparator returned short word');
  return b.slice(0, 32);
}

/** `idOfName(string) -> uint256`. 0 = unregistered. (Mirrors registry::id_of_name.) */
async function idOfName(name: string): Promise<bigint> {
  const sel = selectorHex('idOfName(string)');
  const data = '0x' + sel + encodeStringArg(name);
  const res = await ethCall(REGISTRY, data);
  return BigInt(res);
}

/** `tokenBoundAccountByName(string) -> address`. (Mirrors registry::tba_of_name.) */
async function tbaOfName(name: string): Promise<string | null> {
  const sel = selectorHex('tokenBoundAccountByName(string)');
  const data = '0x' + sel + encodeStringArg(name);
  let res: string;
  try {
    res = await ethCall(REGISTRY, data);
  } catch (e) {
    const msg = (e as Error).message;
    if (msg.includes('name unregistered') || msg.includes('nonexistent token')) {
      return null;
    }
    throw e;
  }
  const t = stripHex(res);
  if (t.length < 64) return null;
  const addrHex = t.slice(t.length - 40);
  if (/^0+$/.test(addrHex)) return null;
  return '0x' + addrHex.toLowerCase();
}

/** `metadata(tokenId, keccak256("localharness.persona")) -> bytes` → UTF-8.
 * Mirrors `registry::persona_of` (ABI bytes: offset|length|payload). */
async function personaOf(tokenId: bigint): Promise<string | null> {
  const key = keccak32(new TextEncoder().encode('localharness.persona'));
  const sel = selectorHex('metadata(uint256,bytes32)');
  const data =
    '0x' + sel + bytesToHex(uintWord(tokenId)) + bytesToHex(key);
  const res = await ethCall(REGISTRY, data);
  const b = hexToBytes(stripHex(res));
  if (b.length < 64) return null;
  // length is the low 8 bytes of the second word.
  let len = 0;
  for (let i = 56; i < 64; i++) len = len * 256 + b[i];
  if (len === 0) return null;
  const payload = b.slice(64, 64 + len);
  if (payload.length < len) return null;
  const text = new TextDecoder().decode(payload).trim();
  return text.length ? text : null;
}

/** Floor applied when an agent has NOT advertised a price on-chain:
 * 0.01 $LH. Mirrors `registry::DEFAULT_ASK_PRICE_WEI`. */
const DEFAULT_ASK_PRICE_WEI = 10_000_000_000_000_000n;

/** Price-lock overpay tolerance, in basis points (1% = 100 bps). The x402
 * authorization signs an EXACT `value` and `settle` moves EXACTLY that — the
 * facilitator can never settle LESS than signed, so we can't "pay the cheaper
 * of signed/current". Instead we LOCK the signed value to the live price: an
 * underpay is rejected (the floor) and an overpay BEYOND this slack is rejected
 * too, so a caller whose quote went stale (price dropped after they signed)
 * re-quotes at the current price rather than SILENTLY overpaying. The band
 * absorbs benign drift / small deliberate tips. Mirrors
 * `registry::PRICE_LOCK_OVERPAY_TOLERANCE_BPS`. */
const PRICE_LOCK_OVERPAY_TOLERANCE_BPS = 1000n; // 10%

/** Max `value` the gate settles against a live `required` price:
 * `required + tolerance` (saturating-free; bigint can't overflow). A signed
 * authorization at or below this ceiling settles as signed; above it the gate
 * rejects and returns `required` so the caller re-quotes — no silent overpay
 * from a stale quote. Mirrors `registry::price_lock_ceiling`. */
function priceLockCeiling(required: bigint): bigint {
  return required + (required * PRICE_LOCK_OVERPAY_TOLERANCE_BPS) / 10_000n;
}

/** Render wei as a short decimal $LH string for prompt text (2dp, floor).
 * Mirrors scheduler.ts::weiToLhText. */
function weiToLhText(wei: bigint): string {
  const hundredths = wei / 10_000_000_000_000_000n; // 1e16 = 0.01 $LH
  return `${hundredths / 100n}.${(hundredths % 100n).toString().padStart(2, '0')}`;
}

/** `metadata(tokenId, keccak256("localharness.x402_price")) -> bytes`,
 * a decimal-wei UTF-8 string. null = never advertised (use the default).
 * Mirrors `registry::x402_price_of`. */
async function x402PriceOf(tokenId: bigint): Promise<bigint | null> {
  const key = keccak32(new TextEncoder().encode('localharness.x402_price'));
  const sel = selectorHex('metadata(uint256,bytes32)');
  const data = '0x' + sel + bytesToHex(uintWord(tokenId)) + bytesToHex(key);
  const res = await ethCall(REGISTRY, data);
  const b = hexToBytes(stripHex(res));
  if (b.length < 64) return null;
  let len = 0;
  for (let i = 56; i < 64; i++) len = len * 256 + b[i];
  if (len === 0) return null;
  const payload = b.slice(64, 64 + len);
  if (payload.length < len) return null;
  const text = new TextDecoder().decode(payload).trim();
  if (!/^[0-9]+$/.test(text)) return null;
  const wei = BigInt(text);
  return wei > 0n ? wei : null;
}

/** `nextId() -> uint256`. The next token id to mint; registered ids are
 * `1..nextId()-1` (ids start at 1 and are monotonic). 0/empty = nothing minted. */
async function nextId(): Promise<bigint> {
  const data = '0x' + selectorHex('nextId()');
  const res = await ethCall(REGISTRY, data);
  try {
    return BigInt(res);
  } catch {
    return 0n;
  }
}

/** `nameOfId(uint256) -> string`. Empty for an unregistered / burned id.
 * Decodes the ABI string return (offset|length|utf8). */
async function nameOfId(tokenId: bigint): Promise<string> {
  const sel = selectorHex('nameOfId(uint256)');
  const data = '0x' + sel + bytesToHex(uintWord(tokenId));
  const res = await ethCall(REGISTRY, data);
  const b = hexToBytes(stripHex(res));
  if (b.length < 64) return '';
  let len = 0;
  for (let i = 56; i < 64; i++) len = len * 256 + b[i];
  if (len === 0) return '';
  const payload = b.slice(64, 64 + len);
  if (payload.length < len) return '';
  return new TextDecoder().decode(payload).trim();
}

/** ABI-encode a single `string` arg (offset 0x20 | length | utf8 padded). */
function encodeStringArg(value: string): string {
  const bytes = new TextEncoder().encode(value);
  const len = bytes.length;
  const padded = Math.ceil(len / 32) * 32;
  const buf = new Uint8Array(32 + 32 + padded);
  buf[31] = 0x20; // offset
  // length in the low bytes of word 2
  let x = len;
  for (let i = 63; i >= 32 && x > 0; i--) {
    buf[i] = x & 0xff;
    x = Math.floor(x / 256);
  }
  buf.set(bytes, 64);
  return bytesToHex(buf);
}

// ---- bounty reads (FREE discovery — viem readContract over the same diamond) -
//
// The bounty views return a tuple (getBounty) + dynamic arrays (openBounties) +
// `bytes` (bountyTaskOf), so we decode them via viem's `readContract` (exactly
// the pattern scheduler.ts uses) rather than hand-rolling the ABI decode.

/** Bounty status enum (LibBountyStorage.Status). 0 = Open is the only one
 * `list_bounties` surfaces (openBounties already filters to Open + unexpired). */
const BOUNTY_STATUS_LABELS = [
  'open',
  'claimed',
  'submitted',
  'accepted',
  'cancelled',
  'reclaimed',
] as const;

function bountyStatusLabel(status: number): string {
  return BOUNTY_STATUS_LABELS[status] ?? `status-${status}`;
}

function bountyPublicClient() {
  return createPublicClient({ chain: TEMPO_CHAIN, transport: http(TEMPO_RPC) });
}

/** `openBounties(startAfter, limit)` — up to `limit` Open+unexpired bounty ids
 * in the index window after `startAfter`, plus the next cursor. */
async function openBounties(
  startAfter: bigint,
  limit: bigint,
): Promise<{ ids: bigint[]; nextCursor: bigint }> {
  const [ids, nextCursor] = (await bountyPublicClient().readContract({
    address: REGISTRY as `0x${string}`,
    abi: BOUNTY_ABI,
    functionName: 'openBounties',
    args: [startAfter, limit],
  })) as readonly [readonly bigint[], bigint];
  return { ids: [...ids], nextCursor };
}

interface BountyRecord {
  poster: string;
  rewardWei: bigint;
  expiry: bigint;
  status: number;
  claimantTokenId: bigint;
}

/** `getBounty(id)` — the full record (zeros / poster==0 for an unknown id). */
async function getBounty(bountyId: bigint): Promise<BountyRecord> {
  const r = (await bountyPublicClient().readContract({
    address: REGISTRY as `0x${string}`,
    abi: BOUNTY_ABI,
    functionName: 'getBounty',
    args: [bountyId],
  })) as readonly [string, bigint, bigint, number, bigint];
  return {
    poster: r[0],
    rewardWei: r[1],
    expiry: r[2],
    status: Number(r[3]),
    claimantTokenId: r[4],
  };
}

/** `bountyTaskOf(id)` — the task spec bytes, decoded as UTF-8 (best-effort). */
async function bountyTaskOf(bountyId: bigint): Promise<string> {
  const raw = (await bountyPublicClient().readContract({
    address: REGISTRY as `0x${string}`,
    abi: BOUNTY_ABI,
    functionName: 'bountyTaskOf',
    args: [bountyId],
  })) as `0x${string}`;
  const h = raw.startsWith('0x') ? raw.slice(2) : raw;
  if (h.length === 0) return '';
  const bytes = new Uint8Array(h.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(h.slice(i * 2, i * 2 + 2), 16);
  }
  return new TextDecoder().decode(bytes).trim();
}

// ---- x402 digest (mirror registry::x402_digest / X402Facet.sol) ------------

interface X402Auth {
  from: string; // payer (0x address)
  to: string; // payee (0x address) — the agent's TBA
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

/**
 * Reconstruct the EIP-712 digest a payer signs, EXACTLY as
 * `src/registry.rs::x402_digest` and `X402Facet.settle`:
 *   structHash = keccak256(PAYMENT_TYPEHASH, from, to, value, validAfter,
 *                          validBefore, nonce)
 *   digest     = keccak256(0x19 0x01 || domainSeparator || structHash)
 * `domainSeparator` is the LIVE value read from the diamond.
 */
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
  return keccak32(
    concatBytes(new Uint8Array([0x19, 0x01]), domainSeparator, structHash),
  );
}

/** `authorizationState(from, nonce) -> bool` — true if this nonce was settled. */
async function authorizationState(from: string, nonce: string): Promise<boolean> {
  const sel = selectorHex('authorizationState(address,bytes32)');
  const data = '0x' + sel + bytesToHex(addrWord(from)) + stripHex(nonce);
  const res = await ethCall(REGISTRY, data);
  return BigInt(res) !== 0n;
}

/** `balanceOf(addr)` on the $LH token (TIP-20 exposes the ERC-20 read surface). */
async function lhBalanceOf(addr: string): Promise<bigint> {
  const data =
    '0x' + selectorHex('balanceOf(address)') + bytesToHex(addrWord(addr));
  return BigInt(await ethCall(LH_TOKEN, data));
}

/** `allowance(payer, diamond)` on the $LH token — `settle` pulls the payer's
 * $LH via `transferFrom`, so the diamond must be approved for >= value. */
async function lhAllowanceToDiamond(payer: string): Promise<bigint> {
  const data =
    '0x' +
    selectorHex('allowance(address,address)') +
    bytesToHex(addrWord(payer)) +
    bytesToHex(addrWord(REGISTRY));
  return BigInt(await ethCall(LH_TOKEN, data));
}

/**
 * Submit `X402Facet.settle(...)` and AWAIT the receipt. Signed by the proxy's
 * settlement account (env `PROXY_METER_KEY`, the same write key gemini.ts uses
 * for `meter(...)`). A plain EIP-1559 tx via viem — Tempo accepts these.
 *
 * NOTE on gas payer: the Rust client path is `settle_x402_sponsored` (a Tempo
 * 0x76 sponsored tx where an embedded sponsor pays in AlphaUSD and the SENDER
 * holds zero). The proxy already runs a funded write account (`PROXY_METER_KEY`)
 * for `meter(...)`, so we settle from THAT account directly — it pays gas. This
 * is a legitimate "facilitator submits settle" per the x402 exact scheme:
 * `settle` moves $LH payer->payee purely from the signed authorization; the tx
 * submitter only pays gas and earns nothing. We do NOT need the Tempo-AA
 * sponsor envelope here because the proxy's account is itself gas-funded.
 *
 * Returns when the receipt is `success` OR when the receipt wait timed out
 * but `authorizationState` shows the nonce consumed (the payment landed; a
 * slow receipt must not fail a PAID call). Throws `SettlementError` on revert
 * (replayed nonce / expired / bad sig / insufficient $LH / missing allowance
 * all revert in `settle`), `SettlementUnconfirmedError` when the outcome is
 * genuinely unknown (tx submitted, no receipt, nonce not yet consumed — it
 * may STILL settle), or a plain error on a definitive submission failure.
 */
async function settleOnChain(a: X402Auth): Promise<`0x${string}`> {
  const pk = process.env.PROXY_METER_KEY;
  if (!pk) throw new Error('missing PROXY_METER_KEY (x402 settlement account)');
  const account = privateKeyToAccount(
    (pk.startsWith('0x') ? pk : `0x${pk}`) as `0x${string}`,
  );
  const wallet = createWalletClient({
    account,
    chain: TEMPO_CHAIN,
    transport: http(TEMPO_RPC),
  });
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
  const hash = await wallet.sendTransaction({
    to: REGISTRY as `0x${string}`,
    data,
    value: 0n,
  });
  const pub = createPublicClient({ chain: TEMPO_CHAIN, transport: http(TEMPO_RPC) });
  let status: 'success' | 'reverted';
  try {
    // 12s (matches gemini.ts::meterDebit / scheduler.ts::recordRun) — NOT 30s:
    // under settle-on-success the model call has ALREADY consumed part of
    // Edge's ~25s wall clock before we get here, so the receipt wait must
    // stay tightly bounded. Resolve any ambiguity against the chain below
    // instead of waiting longer.
    ({ status } = await pub.waitForTransactionReceipt({
      hash,
      timeout: 12_000,
      pollingInterval: 500,
    }));
  } catch {
    // Ambiguous: the settle was SUBMITTED but no receipt arrived in time (slow
    // RPC / Edge clock). The payer's $LH may ALREADY have moved and the
    // one-shot nonce burned — a generic error here would hide that, and a
    // fresh-nonce retry would DOUBLE-PAY if this tx lands. Disambiguate
    // against the chain itself: `authorizationState` flips true the moment
    // `settle` consumes the nonce, so one poll tells us whether it landed.
    try {
      if (await authorizationState(a.from, a.nonce)) {
        return hash; // payment landed — the receipt was just slow.
      }
    } catch {
      // the state read failed too — stay ambiguous and report it as such.
    }
    throw new SettlementUnconfirmedError(hash);
  }
  if (status === 'reverted') {
    throw new SettlementError('x402 settle reverted (replayed/expired nonce, bad sig, or insufficient $LH allowance)');
  }
  return hash;
}

/** Thrown when settlement definitively fails on-chain. Under settle-on-success
 * this fires AFTER the answer was produced, so it maps to the serve-anyway
 * policy (handleAskAgent step 10(b)), not a 402. */
class SettlementError extends Error {}

/**
 * Thrown when the settle tx was SUBMITTED but its fate is UNKNOWN: the receipt
 * wait timed out and `authorizationState` does not (yet) show the nonce as
 * consumed. The authorization MAY STILL SETTLE — the payer must not blindly
 * sign a fresh nonce (double-pay risk if this one lands) nor assume the $LH
 * never moved. Carries the tx hash so the caller can watch it, or retry the
 * SAME authorization (a later "replayed nonce" 402 is proof this one settled).
 */
class SettlementUnconfirmedError extends Error {
  readonly txHash: `0x${string}`;
  constructor(txHash: `0x${string}`) {
    super(
      `x402 settlement unconfirmed: settle tx ${txHash} was submitted but its receipt did not arrive and the nonce is not yet marked used — the authorization may still settle. Do NOT sign a new nonce; check the tx, or retry the SAME authorization (a "replayed nonce" rejection means it settled).`,
    );
    this.txHash = txHash;
  }
}

// ---- MCP JSON-RPC ----------------------------------------------------------

interface JsonRpcReq {
  jsonrpc?: string;
  id?: string | number | null;
  method?: string;
  params?: unknown;
}

function rpcResult(id: string | number | null, result: unknown): unknown {
  return { jsonrpc: '2.0', id: id ?? null, result };
}

/** A JSON-RPC error. `httpStatus` lets us carry 402 for payment-required. */
function rpcError(
  id: string | number | null,
  code: number,
  message: string,
  httpStatus = 200,
  extra?: Record<string, unknown>,
): { body: unknown; httpStatus: number } {
  return {
    body: {
      jsonrpc: '2.0',
      id: id ?? null,
      error: { code, message, ...(extra ? { data: extra } : {}) },
    },
    httpStatus,
  };
}

const ASK_AGENT_TOOL = {
  name: 'ask_agent',
  description:
    'Ask a localharness on-chain agent (by subdomain name) a question. The agent answers under its published on-chain persona. Each call requires an x402 payment in $LH to the agent (supply the authorization in the x-x402-authorization header or params._x402).',
  inputSchema: {
    type: 'object',
    properties: {
      name: {
        type: 'string',
        description: 'The agent subdomain name, e.g. "claude" for claude.localharness.xyz.',
      },
      message: {
        type: 'string',
        description: 'The message / question to send the agent.',
      },
    },
    required: ['name', 'message'],
  },
} as const;

// ---- FREE discovery tools (no x402) ----------------------------------------
//
// Discovery is the DEMAND on-ramp — it must be frictionless, so these two tools
// are FREE (no payment gate; only `ask_agent` settles). They are READ-ONLY:
// pure on-chain reads against the diamond, no writes, no spend. Each handler is
// fully try/caught so a bad RPC read degrades to a clean tool-error result and
// never 500s the endpoint.

// How many of the most-recent token ids `discover_agents` scans. Bounds RPC
// fan-out per call (one nameOfId + one persona read per id). Env-overridable.
const DISCOVER_SCAN_CAP = ((): number => {
  const n = Number(process.env.MCP_DISCOVER_SCAN_CAP ?? '100');
  return Number.isFinite(n) && n > 0 ? Math.min(Math.trunc(n), 250) : 100;
})();

// How many bounties `list_bounties` reads (Open + unexpired window). The
// `openBounties` view already filters; this just bounds the page size.
const BOUNTY_LIST_CAP = ((): number => {
  const n = Number(process.env.MCP_BOUNTY_LIST_CAP ?? '50');
  return Number.isFinite(n) && n > 0 ? Math.min(Math.trunc(n), 200) : 50;
})();

// Max chars of persona text returned per agent match (keeps the result compact).
const PERSONA_EXCERPT_LEN = 240;

const DISCOVER_AGENTS_TOOL = {
  name: 'discover_agents',
  description:
    'Find localharness on-chain agents by a free-text query (the agent yellow-pages). Scans the most recently registered agents and ranks them by how well the query matches their subdomain name and published persona — several keywords are ORed and ranked by overlap, so ONE call with "game tool puzzle" replaces a call per keyword. Returns the top matches as {name, tokenId, persona_excerpt}. FREE / read-only — no payment. Use this to locate an agent (e.g. "a solidity auditor"), then ask_agent it.',
  inputSchema: {
    type: 'object',
    properties: {
      query: {
        type: 'string',
        description:
          'What you are looking for, e.g. "solidity auditor" or "image generation". Empty returns the most recent agents unranked.',
      },
    },
    required: ['query'],
  },
} as const;

const LIST_BOUNTIES_TOOL = {
  name: 'list_bounties',
  description:
    'List OPEN, unexpired bounties on the localharness agent economy — work an agent could claim and get paid $LH for. Returns {id, reward_lh, task, status} for each open bounty. FREE / read-only — no payment. Surfaces the demand side of the economy.',
  inputSchema: {
    type: 'object',
    properties: {},
    required: [],
  },
} as const;

/**
 * Rank `(name, persona)` agents against a query — the EXACT mirror of
 * `src/registry/names.rs::rank_agent_matches` (keep the two in lockstep; the
 * Rust side has the unit tests). Multi-keyword: whitespace-split tokens, an
 * agent matches the whole phrase OR any token; name hits rank above
 * persona-only hits; within a tier a whole-phrase hit (+100) outranks token
 * hits, then more matched tokens rank higher, then original order (recency).
 * An empty query returns the input order unchanged (most-recent first).
 */
function rankAgentMatches(
  agents: { name: string; tokenId: bigint; persona: string }[],
  query: string,
): { name: string; tokenId: bigint; persona: string }[] {
  const q = query.trim().toLowerCase();
  if (q === '') return agents;
  const tokens = q.split(/\s+/).filter((t) => t.length > 0);

  const overlap = (text: string): number => {
    const lower = text.toLowerCase();
    let n = 0;
    for (const t of tokens) if (lower.includes(t)) n++;
    return n;
  };

  const nameHits: { agent: (typeof agents)[number]; score: number; order: number }[] = [];
  const personaHits: { agent: (typeof agents)[number]; score: number; order: number }[] = [];
  agents.forEach((agent, order) => {
    const nameLower = agent.name.toLowerCase();
    const personaLower = agent.persona.toLowerCase();
    if (nameLower.includes(q) || overlap(agent.name) > 0) {
      // a whole-phrase name hit (Rust's `name.contains(q)`) scores highest.
      const score = (nameLower.includes(q) ? 100 : 0) + overlap(agent.name);
      nameHits.push({ agent, score, order });
    } else if (personaLower.includes(q) || overlap(agent.persona) > 0) {
      const score = (personaLower.includes(q) ? 100 : 0) + overlap(agent.persona);
      personaHits.push({ agent, score, order });
    }
  });

  // Stable sort: higher score first, original order as the tiebreak (preserves
  // most-recent-first within equal scores, matching the Rust bucket order).
  const byScore = (
    a: { score: number; order: number },
    b: { score: number; order: number },
  ): number => (b.score !== a.score ? b.score - a.score : a.order - b.order);
  nameHits.sort(byScore);
  personaHits.sort(byScore);
  return [...nameHits, ...personaHits].map((h) => h.agent);
}

/**
 * `discover_agents(query)` — FREE on-chain agent yellow-pages. Enumerate the
 * most-recent ~DISCOVER_SCAN_CAP token ids (ids are 1..nextId()-1, monotonic),
 * read each name + persona, rank by the query, return the top matches.
 * Fully try/caught: a bad RPC read returns a clean tool-error result, never 500.
 */
async function handleDiscoverAgents(
  id: string | number | null,
  args: Record<string, unknown>,
): Promise<{ body: unknown; httpStatus: number }> {
  const query = typeof args.query === 'string' ? args.query : '';
  try {
    const next = await nextId();
    if (next <= 1n) {
      // Nothing minted yet — clean empty result, not an error.
      return {
        body: rpcResult(id, {
          content: [{ type: 'text', text: JSON.stringify({ query, matches: [] }, null, 2) }],
          _meta: { matchCount: 0, scanned: 0 },
        }),
        httpStatus: 200,
      };
    }
    // Most-recent first: scan ids [hi .. lo] descending, capped.
    const hi = next - 1n;
    const lo = hi - BigInt(DISCOVER_SCAN_CAP) + 1n;
    const start = lo > 1n ? lo : 1n;
    const idsDesc: bigint[] = [];
    for (let tid = hi; tid >= start; tid--) idsDesc.push(tid);

    const agents: { name: string; tokenId: bigint; persona: string }[] = [];
    for (const tid of idsDesc) {
      // Per-id reads are independently guarded so one burned/odd id can't abort
      // the whole scan. nameOfId is empty for a burned (released) id — skip it.
      let name = '';
      try {
        name = await nameOfId(tid);
      } catch {
        continue;
      }
      if (!name) continue;
      let persona = '';
      try {
        persona = (await personaOf(tid)) ?? '';
      } catch {
        persona = '';
      }
      agents.push({ name, tokenId: tid, persona });
    }

    const ranked = rankAgentMatches(agents, query);
    const matches = ranked.map((a) => ({
      name: a.name,
      tokenId: a.tokenId.toString(),
      persona_excerpt:
        a.persona.length > PERSONA_EXCERPT_LEN
          ? a.persona.slice(0, PERSONA_EXCERPT_LEN) + '…'
          : a.persona,
    }));
    return {
      body: rpcResult(id, {
        content: [
          { type: 'text', text: JSON.stringify({ query, matches }, null, 2) },
        ],
        _meta: { matchCount: matches.length, scanned: agents.length },
      }),
      httpStatus: 200,
    };
  } catch (e) {
    // A bad RPC read MUST NOT 500 the endpoint — degrade to a tool-error result.
    return {
      body: rpcResult(id, {
        content: [
          { type: 'text', text: `discover_agents failed: ${(e as Error).message}` },
        ],
        isError: true,
      }),
      httpStatus: 200,
    };
  }
}

/**
 * `list_bounties()` — FREE open-work board. Read `openBounties(0, N)` (already
 * filtered to Open + unexpired by the facet), then `getBounty` + `bountyTaskOf`
 * for each. Returns {id, reward_lh, task, status}. Fully try/caught; per-bounty
 * reads are independently guarded so one bad id can't drop the whole list.
 */
async function handleListBounties(
  id: string | number | null,
): Promise<{ body: unknown; httpStatus: number }> {
  try {
    const { ids } = await openBounties(0n, BigInt(BOUNTY_LIST_CAP));
    const bounties: {
      id: string;
      reward_lh: string;
      task: string;
      status: string;
    }[] = [];
    for (const bid of ids) {
      try {
        const rec = await getBounty(bid);
        // poster==0 => unknown/zeroed; openBounties shouldn't return these, but
        // guard anyway. Surface only Open ones (defensive; the view filters too).
        if (rec.poster === '0x0000000000000000000000000000000000000000') continue;
        let task = '';
        try {
          task = await bountyTaskOf(bid);
        } catch {
          task = '';
        }
        bounties.push({
          id: bid.toString(),
          // $LH is 18-decimal; render a human decimal string alongside the id.
          reward_lh: formatLh(rec.rewardWei),
          task,
          status: bountyStatusLabel(rec.status),
        });
      } catch {
        // One unreadable bounty must not drop the rest.
        continue;
      }
    }
    return {
      body: rpcResult(id, {
        content: [
          { type: 'text', text: JSON.stringify({ bounties }, null, 2) },
        ],
        _meta: { count: bounties.length },
      }),
      httpStatus: 200,
    };
  } catch (e) {
    return {
      body: rpcResult(id, {
        content: [
          { type: 'text', text: `list_bounties failed: ${(e as Error).message}` },
        ],
        isError: true,
      }),
      httpStatus: 200,
    };
  }
}

/** Render an 18-decimal $LH wei amount as a trimmed decimal string. */
function formatLh(wei: bigint): string {
  const base = 1_000_000_000_000_000_000n;
  const whole = wei / base;
  const frac = wei % base;
  if (frac === 0n) return whole.toString();
  // up to 18 fractional digits, trailing zeros trimmed.
  let fracStr = frac.toString().padStart(18, '0').replace(/0+$/, '');
  return `${whole.toString()}.${fracStr}`;
}

function defaultPersona(name: string): string {
  return (
    `You are ${name}, an autonomous agent on the localharness platform ` +
    `(a self-sovereign, browser-resident agent network on the Tempo testnet). ` +
    `You are reachable at ${name}.localharness.xyz. Answer the user's message ` +
    `helpfully and concisely, speaking as ${name}.`
  );
}

/** Non-streaming Gemini generateContent with the platform key. Returns text. */
async function runAgent(persona: string, message: string): Promise<string> {
  const apiKey = process.env.GEMINI_API_KEY;
  if (!apiKey) throw new Error('proxy misconfigured: missing GEMINI_API_KEY');
  const url = `${GEMINI_BASE}/v1beta/models/${ASK_MODEL}:generateContent`;
  const body = {
    systemInstruction: { parts: [{ text: persona }] },
    contents: [{ role: 'user', parts: [{ text: message }] }],
  };
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-goog-api-key': apiKey },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const t = await res.text();
    throw new Error(`gemini ${res.status}: ${t.slice(0, 500)}`);
  }
  const data = (await res.json()) as {
    candidates?: { content?: { parts?: { text?: string }[] } }[];
  };
  const parts = data.candidates?.[0]?.content?.parts ?? [];
  const text = parts
    .map((p) => p.text ?? '')
    .join('')
    .trim();
  // An empty model answer is NOT a billable success. THROW so ask_agent's
  // settle-on-success path (invariant a) leaves the authorization unsettled —
  // the caller is NOT charged for a zero-content reply. The old code returned
  // a "(the agent returned no text)" placeholder, which is a non-empty string,
  // so step 10 settled and the payer lost $LH for nothing (QA fleet repro:
  // "charged 0.05 $LH, got zero response"). The handler's catch surfaces
  // "payment NOT taken" and the one-shot nonce stays unused.
  if (!text.length) throw new Error('the agent returned no text');
  return text;
}

/**
 * Parse the x402 authorization a client supplies for a `tools/call`. It may
 * ride EITHER in the `x-x402-authorization` header (a JSON string) OR inside
 * `params._x402` (a JSON object). Header wins if both present.
 *
 * Shape (all addresses 0x-hex; integers as decimal strings or numbers):
 *   {
 *     "from": "0x<payer>",
 *     "to": "0x<agent TBA>",        // optional — proxy resolves+verifies it
 *     "value": "10000000000000000", // $LH wei
 *     "validAfter": 0,
 *     "validBefore": 1999999999,
 *     "nonce": "0x<32-byte hex>",
 *     "signature": "0x<65-byte hex>"
 *   }
 */
function parseAuth(headerVal: string | null, params: Record<string, unknown>): X402Auth | null {
  let raw: unknown = null;
  if (headerVal) {
    try {
      raw = JSON.parse(headerVal);
    } catch {
      throw new Error('x-x402-authorization is not valid JSON');
    }
  } else if (params && typeof params._x402 === 'object' && params._x402 !== null) {
    raw = params._x402;
  }
  if (!raw || typeof raw !== 'object') return null;
  const o = raw as Record<string, unknown>;

  const toBig = (v: unknown, field: string): bigint => {
    // Money/time fields are $LH wei (18 decimals): real values (>=1e16) sit far
    // above Number.MAX_SAFE_INTEGER, so a JS number has already lost precision
    // before we see it. Accept numbers ONLY when exactly representable; force
    // large values to arrive as decimal strings (BigInt parses those losslessly)
    // rather than silently corrupting a signed, money-moving field.
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

/**
 * Gate + run an `ask_agent` call. Resolves the agent, requires + verifies an
 * x402 authorization paying the agent's TBA, settles on-chain, then runs.
 */
async function handleAskAgent(
  id: string | number | null,
  args: Record<string, unknown>,
  headerAuth: string | null,
  params: Record<string, unknown>,
): Promise<{ body: unknown; httpStatus: number }> {
  const name = typeof args.name === 'string' ? args.name.trim() : '';
  const message = typeof args.message === 'string' ? args.message : '';
  if (!name) return rpcError(id, -32602, 'ask_agent: missing "name"');
  // Trim: a whitespace-only message is empty input — reject it BEFORE any
  // payment gate or model spend, not after charging for a blank prompt.
  if (!message.trim()) return rpcError(id, -32602, 'ask_agent: missing "message"');

  // 1. Resolve the agent on-chain (tokenId + payee TBA).
  const tokenId = await idOfName(name);
  if (tokenId === 0n) {
    return rpcError(id, -32602, `no such agent: "${name}" is not registered`);
  }
  const payee = await tbaOfName(name);
  if (!payee) {
    return rpcError(
      id,
      -32602,
      `agent "${name}" has no token-bound account to receive payment`,
    );
  }

  // 2. Resolve the agent's effective price: advertised on-chain, else the
  //    platform default. This is the live PRICE-LOCK reference the
  //    authorization is bound to — the floor it must meet AND (step 4b) the
  //    ceiling it must not exceed beyond tolerance, so a stale quote can
  //    neither underpay nor SILENTLY overpay. Re-read here at settle-time, so
  //    the price the caller actually pays is the price NOW, not whatever they
  //    quoted earlier. A read failure falls back to the default (never blocks
  //    on a flaky eth_call; settle stays authoritative).
  let required = DEFAULT_ASK_PRICE_WEI;
  try {
    required = (await x402PriceOf(tokenId)) ?? DEFAULT_ASK_PRICE_WEI;
  } catch {
    // default stands
  }

  // 3. Require an x402 authorization (this is the payment-gated path).
  let auth: X402Auth | null;
  try {
    auth = parseAuth(headerAuth, params);
  } catch (e) {
    return rpcError(id, -32602, (e as Error).message, 402);
  }
  if (!auth) {
    return rpcError(
      id,
      -32602,
      `payment required: supply an x402 authorization (x-x402-authorization header or params._x402) paying at least ${required} wei $LH to ${name}'s account ${payee}`,
      402,
      { payTo: payee, scheme: 'x402-exact', asset: '$LH', chainId: CHAIN_ID, minValue: required.toString() },
    );
  }

  // 4. The authorization MUST pay the resolved agent TBA. If the client
  //    supplied a `to`, it must match; otherwise we fill it in.
  if (auth.to && auth.to.toLowerCase() !== payee.toLowerCase()) {
    return rpcError(
      id,
      -32602,
      `x402 authorization "to" (${auth.to}) does not match agent "${name}" payee ${payee}`,
      402,
    );
  }
  auth.to = payee;
  if (auth.value <= 0n) {
    return rpcError(id, -32602, 'x402 authorization: value must be > 0', 402);
  }
  if (auth.value < required) {
    return rpcError(
      id,
      -32602,
      `payment below "${name}"'s price: authorized ${auth.value} wei, requires ${required} wei $LH`,
      402,
      { payTo: payee, scheme: 'x402-exact', asset: '$LH', chainId: CHAIN_ID, minValue: required.toString() },
    );
  }
  // 4b. Price-lock the OTHER direction. `settle` moves EXACTLY `auth.value`, so
  //     if the price DROPPED after the caller signed, settling the stale signed
  //     value would SILENTLY overpay the agent (issue #72). Reject anything
  //     above the live price + tolerance and return `required` so the caller
  //     re-quotes at the current price. The band (priceLockCeiling) absorbs
  //     benign drift / small deliberate tips; only a real overpay gap rejects.
  //     A read-failure `required` falls back to the default above, so this lock
  //     is never stricter than the floor the caller could have read.
  const ceiling = priceLockCeiling(required);
  if (auth.value > ceiling) {
    return rpcError(
      id,
      -32602,
      `payment above "${name}"'s current price: authorized ${auth.value} wei, current price is ${required} wei $LH (the price changed after you quoted) — re-sign for the current price`,
      402,
      {
        payTo: payee,
        scheme: 'x402-exact',
        asset: '$LH',
        chainId: CHAIN_ID,
        priceChanged: true,
        currentPriceWei: required.toString(),
        maxValue: ceiling.toString(),
      },
    );
  }

  // 5. Validity window + replay checks (the contract enforces these too, but we
  //    fail fast and with a clear 402 before spending gas on a doomed settle).
  const now = BigInt(Math.floor(Date.now() / 1000));
  if (now <= auth.validAfter) {
    return rpcError(id, -32602, 'x402 authorization not yet valid', 402);
  }
  if (now >= auth.validBefore) {
    return rpcError(id, -32602, 'x402 authorization expired', 402);
  }

  // 6. Verify the EIP-712 signature against the LIVE domain separator.
  let domain: Uint8Array;
  try {
    domain = await liveDomainSeparator();
  } catch (e) {
    return rpcError(id, -32603, 'failed to read x402 domain separator: ' + (e as Error).message);
  }
  let recovered: string;
  try {
    const digest = x402Digest(domain, auth);
    recovered = recoverFromDigest(digest, auth.signature);
  } catch (e) {
    return rpcError(id, -32602, 'x402 signature error: ' + (e as Error).message, 402);
  }
  if (recovered.toLowerCase() !== auth.from.toLowerCase()) {
    return rpcError(
      id,
      -32602,
      `x402 signature does not match "from" (recovered ${recovered})`,
      402,
    );
  }

  // 7. Replay guard (best-effort pre-check; settle is the authoritative one).
  try {
    if (await authorizationState(auth.from, auth.nonce)) {
      return rpcError(id, -32602, 'x402 authorization already used (replayed nonce)', 402);
    }
  } catch {
    // ignore — settle will revert AuthAlreadyUsed if so.
  }

  // 8. Funds pre-flight (read-only, still BEFORE any model spend). Under
  //    settle-on-success the model runs before money moves, so a payer with
  //    no $LH balance — or no `approve(diamond, …)` allowance, since `settle`
  //    pulls via `transferFrom` — must be rejected HERE, or every such call
  //    would land in the serve-anyway branch below (free answers forever).
  //    Best-effort: a flaky eth_call must not block a funded caller; settle
  //    stays authoritative, and a balance drained between this check and the
  //    settle is the step-10(b) race — bounded to one model call's cost.
  try {
    const [balance, allowance] = await Promise.all([
      lhBalanceOf(auth.from),
      lhAllowanceToDiamond(auth.from),
    ]);
    if (balance < auth.value) {
      return rpcError(
        id,
        -32002,
        `insufficient $LH: payer holds ${balance} wei, authorization is ${auth.value} wei — payment NOT taken`,
        402,
      );
    }
    if (allowance < auth.value) {
      return rpcError(
        id,
        -32002,
        `insufficient $LH allowance: payer approved ${allowance} wei to the diamond, authorization is ${auth.value} wei — approve(${REGISTRY}, value) first; payment NOT taken`,
        402,
      );
    }
  } catch {
    // reads failed — proceed; `settle` remains the authoritative gate.
  }

  // 9. Run the agent FIRST — SETTLE-ON-SUCCESS (issue #25; the old order
  //    settled before the model, so a timeout/500 permanently charged the
  //    payer for nothing).
  //
  //    Ordering invariants — the contract of this handler:
  //    * Everything ABOVE this line is reject-fast and irreversibly spends
  //      NOTHING: signature, payee binding, price floor, validity window,
  //      replay, funds — all checked before a single model token streams.
  //    * (a) A model failure NEVER settles. The authorization is single-use
  //      (its one-shot nonce only burns inside `settle`) and time-bounded
  //      (`validBefore`), so an unsettled authorization expires HARMLESSLY on
  //      its own — the payer may even retry the SAME authorization. The error
  //      body states "payment NOT taken" explicitly.
  //    * (b) If `settle` fails AFTER a successful model call, the answer was
  //      produced but unpaid. Policy (testnet): SERVE THE ANSWER ANYWAY and
  //      log loudly — the model cost becomes the platform's loss, never the
  //      user's. This also covers an authorization that expires DURING a slow
  //      model call, and the parallel-replay duplication (one auth POSTed N
  //      times concurrently: one settle wins, the rest fall here) — exposure
  //      is bounded to model-inference cost per replay, narrowed by the
  //      step-7 replay pre-check and the step-8 funds pre-flight; accepted
  //      on testnet.
  //    * (c) Meter debit ordering: this route NEVER touches CreditMeterFacet —
  //      the x402 settle IS the payment for ask_agent. The caller-meter debit
  //      lives on gemini.ts's session/meter route, whose ordering is
  //      unchanged by this fix.
  let persona: string;
  try {
    persona = (await personaOf(tokenId)) ?? defaultPersona(name);
  } catch {
    persona = defaultPersona(name);
  }
  // The advertised price is part of the agent's self-knowledge. Without this
  // line a PAID agent answered "my price is 0, fully sponsored" to the caller
  // who had just settled 0.05 $LH to its TBA (fleet repro). `required` is the
  // SAME effective price the payment gate above enforced.
  persona += `\n\nYour advertised price is ${weiToLhText(required)} $LH per paid call; callers may have paid it. Never claim to be free or sponsored.`;
  let answer: string;
  try {
    answer = await runAgent(persona, message);
  } catch (e) {
    // Invariant (a): no answer, no payment. The nonce is still unused.
    return {
      body: rpcResult(id, {
        content: [
          {
            type: 'text',
            text:
              `the agent failed to respond: ${(e as Error).message} — ` +
              `payment NOT taken: your x402 authorization was not settled ` +
              `(its one-shot nonce is unused and it expires on its own at ` +
              `validBefore). You may retry with the SAME authorization.`,
          },
        ],
        isError: true,
        _meta: { settlement: 'not-attempted' },
      }),
      httpStatus: 200,
    };
  }

  // 10. Answer in hand — settle NOW. Await the receipt.
  let txHash: string;
  try {
    txHash = await settleOnChain(auth);
  } catch (e) {
    if (e instanceof SettlementUnconfirmedError) {
      // The settle tx exists and MAY still land (receipt slow, nonce not yet
      // visibly consumed). Serve the answer; surface the hash + state so the
      // payer does NOT re-sign a fresh nonce (double-pay if this one settles).
      console.error(
        `x402 settle UNCONFIRMED after successful answer (tx ${e.txHash}) — payer ${auth.from} -> ${auth.to}, ${auth.value} wei`,
      );
      return {
        body: rpcResult(id, {
          content: [{ type: 'text', text: answer }],
          _meta: {
            settlement: 'unconfirmed',
            x402SettlementTx: e.txHash,
            paidTo: payee,
            value: auth.value.toString(),
          },
        }),
        httpStatus: 200,
      };
    }
    // Invariant (b): definitive settle failure (revert / submission error)
    // after a successful model call. Serve the answer anyway — platform eats
    // the model cost — and log LOUDLY so the operator sees revenue leakage.
    console.error(
      `x402 settle FAILED after successful answer — serving unpaid (platform loss). payer ${auth.from} -> ${auth.to}, ${auth.value} wei: ${(e as Error).message}`,
    );
    return {
      body: rpcResult(id, {
        content: [{ type: 'text', text: answer }],
        _meta: {
          settlement: 'failed',
          paidTo: payee,
          value: auth.value.toString(),
        },
      }),
      httpStatus: 200,
    };
  }

  return {
    body: rpcResult(id, {
      content: [{ type: 'text', text: answer }],
      _meta: {
        settlement: 'settled',
        x402SettlementTx: txHash,
        paidTo: payee,
        value: auth.value.toString(),
      },
    }),
    httpStatus: 200,
  };
}

// ---- body reading (size-capped; canonical sibling in gemini.ts) ------------

/** Thrown by `readTextCapped` when the streamed body exceeds `MAX_BODY_BYTES`. */
class BodyTooLargeError extends Error {}

/**
 * Read a request body to a string, ABORTING past `MAX_BODY_BYTES`. The
 * up-front `Content-Length` check is advisory only — a chunked request (no
 * declared length) bypasses it, so without this a caller could stream an
 * unbounded body into the Edge function's memory BEFORE we ever reach the
 * auth/x402 gate (the JSON-RPC envelope must be parsed first to know the
 * method). This streams the reader and throws the moment the running total
 * crosses the cap, so the buffer can never exceed it regardless of headers.
 * Copied from gemini.ts (the canonical sibling — the two files deliberately
 * share patterns this way); fix bugs in BOTH.
 */
async function readTextCapped(req: Request): Promise<string> {
  const body = req.body;
  if (!body) return '';
  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    if (value) {
      total += value.length;
      if (total > MAX_BODY_BYTES) {
        await reader.cancel().catch(() => {});
        throw new BodyTooLargeError('request body too large');
      }
      chunks.push(value);
    }
  }
  const merged = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) {
    merged.set(c, off);
    off += c.length;
  }
  return new TextDecoder().decode(merged);
}

// ---- handler ---------------------------------------------------------------

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');

  if (req.method === 'OPTIONS') {
    return new Response(null, { status: 204, headers: corsHeaders(origin) });
  }
  if (req.method !== 'POST') {
    return new Response(JSON.stringify({ error: 'method not allowed (MCP is POST-only)' }), {
      status: 405,
      headers: { 'content-type': 'application/json', ...corsHeaders(origin) },
    });
  }
  // Advisory only — a chunked body declares no Content-Length; the real
  // guard is `readTextCapped` below.
  const declaredLen = Number(req.headers.get('content-length') ?? '0');
  if (Number.isFinite(declaredLen) && declaredLen > MAX_BODY_BYTES) {
    return new Response(JSON.stringify({ error: 'request body too large' }), {
      status: 413,
      headers: { 'content-type': 'application/json', ...corsHeaders(origin) },
    });
  }

  const respond = (body: unknown, httpStatus: number): Response =>
    new Response(JSON.stringify(body), {
      status: httpStatus,
      headers: { 'content-type': 'application/json', ...corsHeaders(origin) },
    });

  let rpc: JsonRpcReq;
  try {
    rpc = JSON.parse(await readTextCapped(req)) as JsonRpcReq;
  } catch (e) {
    if (e instanceof BodyTooLargeError) {
      return respond({ error: 'request body too large' }, 413);
    }
    return respond(
      { jsonrpc: '2.0', id: null, error: { code: -32700, message: 'parse error: invalid JSON' } },
      200,
    );
  }

  const id = rpc.id ?? null;
  const method = rpc.method ?? '';
  const params = (rpc.params && typeof rpc.params === 'object' ? rpc.params : {}) as Record<
    string,
    unknown
  >;

  try {
    switch (method) {
      // --- lifecycle ------------------------------------------------------
      case 'initialize': {
        const clientProto =
          typeof params.protocolVersion === 'string'
            ? params.protocolVersion
            : MCP_PROTOCOL_VERSION;
        return respond(
          rpcResult(id, {
            protocolVersion: clientProto,
            capabilities: { tools: { listChanged: false } },
            serverInfo: { name: MCP_SERVER_NAME, version: MCP_SERVER_VERSION },
            instructions:
              'localharness MCP. Tools: discover_agents(query) + list_bounties() are FREE read-only discovery (find agents / open work). ask_agent(name, message) runs an agent under its on-chain persona and requires an x402 $LH payment to the agent (x-x402-authorization header or params._x402).',
          }),
          200,
        );
      }

      // Notifications: acknowledge with 202 and NO body.
      case 'notifications/initialized':
      case 'notifications/cancelled':
        return new Response(null, { status: 202, headers: corsHeaders(origin) });

      case 'ping':
        return respond(rpcResult(id, {}), 200);

      // --- tools ----------------------------------------------------------
      case 'tools/list':
        return respond(
          rpcResult(id, {
            tools: [DISCOVER_AGENTS_TOOL, LIST_BOUNTIES_TOOL, ASK_AGENT_TOOL],
          }),
          200,
        );

      case 'tools/call': {
        const toolName = typeof params.name === 'string' ? params.name : '';
        const args =
          params.arguments && typeof params.arguments === 'object'
            ? (params.arguments as Record<string, unknown>)
            : {};
        // FREE read-only discovery tools (no x402 gate). Self-contained +
        // try/caught inside each handler so a bad RPC can't 500 the endpoint.
        if (toolName === 'discover_agents') {
          const out = await handleDiscoverAgents(id, args);
          return respond(out.body, out.httpStatus);
        }
        if (toolName === 'list_bounties') {
          const out = await handleListBounties(id);
          return respond(out.body, out.httpStatus);
        }
        if (toolName !== 'ask_agent') {
          const err = rpcError(id, -32602, `unknown tool: "${toolName}"`);
          return respond(err.body, err.httpStatus);
        }
        const headerAuth = req.headers.get('x-x402-authorization');
        const out = await handleAskAgent(id, args, headerAuth, params);
        return respond(out.body, out.httpStatus);
      }

      // --- unknown --------------------------------------------------------
      default: {
        const err = rpcError(id, -32601, `method not found: "${method}"`);
        return respond(err.body, err.httpStatus);
      }
    }
  } catch (e) {
    return respond(
      { jsonrpc: '2.0', id, error: { code: -32603, message: (e as Error).message } },
      200,
    );
  }
}
