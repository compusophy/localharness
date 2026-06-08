// localharness MCP-over-HTTP endpoint — hosted, x402-gated (Edge).
//
// A minimal MCP "Streamable HTTP" server: POST-only, one JSON-RPC request in,
// one JSON-RPC response out (no SSE, no session id). It exposes ONE tool,
// `ask_agent(name, message)`, which runs the named on-chain agent's published
// persona against the caller's message via the platform Gemini key — exactly
// the headless `localharness call` path, but reachable by any remote MCP client
// (Claude Desktop, Cursor, another agent's MCP client, …) over plain HTTP.
//
// The gate is TRUE x402 per-call settlement (NOT the coarse session/meter
// credit gate `gemini.ts` uses): the caller PAYS THE AGENT being called. For
// every `tools/call`, the client supplies an x402 `PaymentAuthorization`
// (EIP-712, signed by the payer) authorizing `$LH` payer -> the agent's
// token-bound account. The proxy verifies the signature against the LIVE
// `x402DomainSeparator()` of the diamond, reconstructs the digest exactly as
// `src/registry.rs::x402_digest` / `X402Facet.sol`, ecrecovers, and submits
// `X402Facet.settle(...)` on-chain — awaiting the receipt — BEFORE running the
// agent. No payment, no answer.
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
 */
function recoverFromDigest(digest: Uint8Array, sigHex: string): string {
  const sig = hexToBytes(stripHex(sigHex));
  if (sig.length !== 65) throw new Error('signature must be 65 bytes');
  const r = sig.slice(0, 32);
  const s = sig.slice(32, 64);
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
 * Returns when the receipt is `success`; throws on revert (replayed nonce /
 * expired / bad sig / insufficient $LH / missing allowance all revert in
 * `settle`) or on a definitive submission failure.
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
  const { status } = await pub.waitForTransactionReceipt({
    hash,
    timeout: 30_000,
    pollingInterval: 500,
  });
  if (status === 'reverted') {
    throw new SettlementError('x402 settle reverted (replayed/expired nonce, bad sig, or insufficient $LH allowance)');
  }
  return hash;
}

/** Thrown when settlement definitively fails on-chain (maps to JSON-RPC 402). */
class SettlementError extends Error {}

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
  return text.length ? text : '(the agent returned no text)';
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
    if (typeof v === 'number') return BigInt(Math.trunc(v));
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
  if (!message) return rpcError(id, -32602, 'ask_agent: missing "message"');

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

  // 2. Require an x402 authorization (this is the payment-gated path).
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
      `payment required: supply an x402 authorization (x-x402-authorization header or params._x402) paying $LH to ${name}'s account ${payee}`,
      402,
      { payTo: payee, scheme: 'x402-exact', asset: '$LH', chainId: CHAIN_ID },
    );
  }

  // 3. The authorization MUST pay the resolved agent TBA. If the client
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

  // 4. Validity window + replay checks (the contract enforces these too, but we
  //    fail fast and with a clear 402 before spending gas on a doomed settle).
  const now = BigInt(Math.floor(Date.now() / 1000));
  if (now <= auth.validAfter) {
    return rpcError(id, -32602, 'x402 authorization not yet valid', 402);
  }
  if (now >= auth.validBefore) {
    return rpcError(id, -32602, 'x402 authorization expired', 402);
  }

  // 5. Verify the EIP-712 signature against the LIVE domain separator.
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

  // 6. Replay guard (best-effort pre-check; settle is the authoritative one).
  try {
    if (await authorizationState(auth.from, auth.nonce)) {
      return rpcError(id, -32602, 'x402 authorization already used (replayed nonce)', 402);
    }
  } catch {
    // ignore — settle will revert AuthAlreadyUsed if so.
  }

  // 7. SETTLE on-chain BEFORE running the agent. Await the receipt.
  let txHash: string;
  try {
    txHash = await settleOnChain(auth);
  } catch (e) {
    if (e instanceof SettlementError) {
      return rpcError(id, -32002, 'x402 settlement failed: ' + e.message, 402);
    }
    return rpcError(id, -32603, 'x402 settlement error: ' + (e as Error).message, 502);
  }

  // 8. Paid — now run the agent under its persona.
  let persona: string;
  try {
    persona = (await personaOf(tokenId)) ?? defaultPersona(name);
  } catch {
    persona = defaultPersona(name);
  }
  let answer: string;
  try {
    answer = await runAgent(persona, message);
  } catch (e) {
    // Payment already settled; surface the failure but the call is non-refundable
    // (the agent was paid for the attempt). Report as a tool error, not a 402.
    return {
      body: rpcResult(id, {
        content: [
          {
            type: 'text',
            text: `payment settled (tx ${txHash}) but the agent failed to respond: ${(e as Error).message}`,
          },
        ],
        isError: true,
      }),
      httpStatus: 200,
    };
  }

  return {
    body: rpcResult(id, {
      content: [{ type: 'text', text: answer }],
      _meta: { x402SettlementTx: txHash, paidTo: payee, value: auth.value.toString() },
    }),
    httpStatus: 200,
  };
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
    rpc = (await req.json()) as JsonRpcReq;
  } catch {
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
              'localharness MCP. One tool: ask_agent(name, message). Each call requires an x402 $LH payment to the agent (x-x402-authorization header or params._x402).',
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
        return respond(rpcResult(id, { tools: [ASK_AGENT_TOOL] }), 200);

      case 'tools/call': {
        const toolName = typeof params.name === 'string' ? params.name : '';
        const args =
          params.arguments && typeof params.arguments === 'object'
            ? (params.arguments as Record<string, unknown>)
            : {};
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
