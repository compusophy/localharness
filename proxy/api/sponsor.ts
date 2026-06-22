// localharness credit proxy — rate-capped sponsor RELAY (Edge).
//
// POST /api/sponsor signs the fee_payer half of a user's Tempo 0x76 tx so the
// published `localharness` CLI ships NO money-moving key (design/cli-mainnet-
// relay.md §2.2). The caller (CLI) signs the SENDER half locally with its
// identity key (needs no funds), submits the sender-signed INTENT here; this
// route re-derives the fee_payer hash, signs it with the server-held sponsor
// key, and returns the signature for the CLI to assemble + submit itself. The
// relay never touches the chain — a relay outage degrades to "no sponsorship",
// never a half-sent tx.
//
// THREE abuse caps, all enforced BEFORE signing:
//   1. Selector allowlist (default-deny): each call's `to` must be the diamond
//      or the $LH token, and its 4-byte selector must be a sponsorable
//      onboarding/participation write (mirrors the browser's
//      run_sponsored_tempo_call surface). No raw value sends. On the $LH token,
//      transfer (send_lh / meter→wallet bridge moves the caller's OWN $LH) and
//      approve-to-the-diamond only — never approve to an arbitrary spender.
//   2. Per-address rate window (post-auth — a fee signature is valuable).
//   3. Onboarding-only spend gate: sponsor only zero/near-zero-$LH callers; a
//      funded agent self-pays (it already earns $LH). This is the durable
//      ceiling given the per-isolate rate limit (the on-chain balance is the
//      shared, global signal).
//
// Auth reuses the existing personal-sign token verbatim (`x-goog-api-key:
// addr:ts:sig`, 300s freshness) — no new auth surface. The wire-format port
// lives in `_tempo.ts` and is pinned to the Rust golden vectors
// (`test/tempo-feepayer.mjs`).

import { keccak_256 } from '@noble/hashes/sha3';
import { secp256k1 } from '@noble/curves/secp256k1';
import { hexToBytes, bytesToHex } from '@noble/hashes/utils';
import {
  feePayerHash,
  sponsoredSenderHash,
  signHash65,
  recoverAddressFromDigest,
  addressFromPrivKey,
  stripHex,
  type TempoIntent,
  type TempoCallIntent,
} from './_tempo';
import { TEMPO_RPC, REGISTRY, CHAIN_ID, LH_TOKEN } from './_chain';
import { SlidingWindow } from './_ratelimit';
import { welcomeNewAgent } from './_welcome';
import { waitUntil } from '@vercel/functions';

export const config = { runtime: 'edge' };

// --- constants -------------------------------------------------------------

const FRESHNESS_WINDOW_SECS = 300; // mirror gemini.ts — tight replay window
// Absolute per-gas price ceiling, mirroring `src/registry/tx.rs::MAX_GAS_PRICE_WEI`
// (1000 gwei). A hostile/MITM'd CLI RPC could otherwise submit an inflated price
// to drain the sponsor's fee-token float — we refuse rather than clamp.
const MAX_GAS_PRICE_WEI = 1_000_000_000_000n;
// Bound the per-tx gas LIMIT. A 2KB on-chain setMetadata is ~18M gas
// (7.6k/byte); 50M leaves headroom for the largest legitimate onboarding write
// while refusing an absurd limit. (Real cost is gas USED, but a sane ceiling
// caps the worst case.)
const MAX_GAS_LIMIT = 50_000_000n;
// Onboarding-only gate: sponsor a caller only while its $LH wallet balance is at
// or below this. A brand-new identity holds 0; once it redeems/earns $LH it pays
// its own fees. Default 1 $LH (room for a few onboarding writes if dusted);
// env-tunable.
const BALANCE_CEILING_WEI = BigInt(
  process.env.LH_RELAY_BALANCE_CEILING_WEI ?? '1000000000000000000',
);
// Per-address sponsorships per window + a global per-isolate ceiling.
const PER_ADDR_LIMIT = Number(process.env.LH_RELAY_RATE_LIMIT ?? '30');
const RATE_WINDOW_MS = Number(process.env.LH_RELAY_RATE_WINDOW_MS ?? '3600000'); // 1h
const GLOBAL_LIMIT = Number(process.env.LH_RELAY_GLOBAL_LIMIT ?? '600');
const perAddr = new SlidingWindow(PER_ADDR_LIMIT, RATE_WINDOW_MS);
const globalWindow = new SlidingWindow(GLOBAL_LIMIT, RATE_WINDOW_MS);

// Sponsor (fee_payer) key. Testnet: the committed low-budget Moderato sponsor
// (also in src/app/sponsor.rs) — public, play-money. Mainnet (the gated money
// path, design §5/§6): set LH_SPONSOR_KEY on the Vercel env; NEVER committed.
const TESTNET_SPONSOR_KEY =
  '0x046a830b5203d1d2c0a205a1432746e4381d0874711b2de7f575a973644b9d43';
const SPONSOR_KEY = process.env.LH_SPONSOR_KEY ?? TESTNET_SPONSOR_KEY;
const SPONSOR_ADDRESS = addressFromPrivKey(SPONSOR_KEY); // lowercase 0x

const DIAMOND = REGISTRY.toLowerCase();
const TOKEN = LH_TOKEN.toLowerCase();

// --- selector allowlist (default-deny) -------------------------------------
// Sponsorable onboarding + economy WRITES on the diamond. Signatures are the
// EXACT ABI strings used by `src/registry/*` (so the 4-byte selectors match the
// Rust `selector()`). Admin/owner-gated calls (diamondCut, adminReset*,
// mintFromFiat, meter, recordRun) are deliberately ABSENT — they revert without
// the role anyway and must never be relay-sponsored.
const DIAMOND_WRITE_SIGS = [
  'register(string)',
  'registerMain(uint256)',
  // releaseName burns a name the CALLER owns (on-chain holder-gated, refuses MAIN);
  // bulk_release_subdomains fires one per name. Was missing → LH_RELAY_SELECTOR
  // 0x48e69e68 (on-chain feedback #62). Gas-only, no value/float touch.
  'releaseName(uint256)',
  'createTokenBoundAccount(uint256)',
  'setMetadata(uint256,bytes32,bytes)',
  'withdrawCredits(uint256)',
  'depositCredits(uint256)',
  'redeem(string)',
  'openSession()',
  'submitFeedback(string)',
  'scheduleJob(uint256,bytes,uint64,uint128,uint32)',
  'cancelJob(uint256)',
  'createInvite(bytes32,uint256,uint64)',
  'acceptInvite(string)',
  'reclaimInvite(bytes32)',
  'postBounty(bytes,uint128,uint64)',
  'claimBounty(uint256,uint256)',
  'submitResult(uint256,bytes)',
  'acceptResult(uint256)',
  // x402 self-pay: a payee/facilitator settles a payer-signed $LH payment.
  // EXEMPT from the onboarding-only gate below (see GATE_EXEMPT_SELECTORS) —
  // on mainnet no agent holds the AlphaUSD fee token, so even a funded agent
  // can't self-pay gas; it can only move its own $LH.
  'settle(address,address,uint256,uint256,uint256,bytes32,bytes)',
  'cancelBounty(uint256)',
  'reclaimExpired(uint256)',
  'attest(uint256,uint8,bytes32)',
  'announce(bytes32,address,address,bytes,bytes)',
  'leave(bytes32,address,address,bytes)',
  'postSignal(address,bytes)',
  'setPushSub(bytes)',
  'createGuild(string)',
  'inviteToGuild(uint256,address)',
  'setRole(uint256,address,uint8)',
  'fundGuild(uint256,uint256)',
  'spendTreasury(uint256,address,uint256,bytes)',
  'acceptGuildInvite(uint256)',
  'leaveGuild(uint256)',
  'formParty(uint256[],uint16[],uint64)',
  'fundParty(uint256,uint128)',
  'propose(uint256,address,uint256,bytes,uint64)',
  'vote(uint256,bool)',
  'proposeWeighted(uint256,address,uint256,uint256,string)',
  'voteWeighted(uint256,bool)',
  'executeWeighted(uint256)',
  'setShares(uint256,address,uint256)',
  'createRoom()',
  'roomAddMember(uint256,address)',
  'appendOp(uint256,bytes)',
  'clearRoom(uint256)',
  'stakeValidation(bytes32,uint256,bool,uint256)',
  'resolveValidation(uint256,bool)',
  'setTithe(uint256,uint256)',
  'revokeTithe()',
  'collectTithe(address)',
];

/** 4-byte selector hex (no 0x) — keccak256(sig)[..4]. */
function selector(sig: string): string {
  return bytesToHex(keccak_256(new TextEncoder().encode(sig)).slice(0, 4));
}

const APPROVE_SELECTOR = selector('approve(address,uint256)');
const TRANSFER_SELECTOR = selector('transfer(address,uint256)');
const REGISTER_SELECTOR = selector('register(string)');
const DIAMOND_SELECTORS = new Set(DIAMOND_WRITE_SIGS.map(selector));

/**
 * The name in a sponsored `register(string)` call's calldata, or null if this
 * call isn't a diamond register. The arg is a single dynamic string: word0 =
 * offset (0x20), word1 = byte length, then the utf-8 bytes. Defensive — any
 * malformed encoding returns null (the welcome is best-effort either way).
 */
function registeredName(c: TempoCallIntent): string | null {
  if ('0x' + bytesToHex(c.to) !== DIAMOND) return null;
  if (c.input.length < 4 || bytesToHex(c.input.slice(0, 4)) !== REGISTER_SELECTOR) return null;
  const args = c.input.slice(4);
  if (args.length < 64) return null;
  const len = Number(BigInt('0x' + bytesToHex(args.slice(32, 64))));
  if (len === 0 || len > 63 || args.length < 64 + len) return null;
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(args.slice(64, 64 + len));
  } catch {
    return null;
  }
}

// The SELF-PAY surface — selectors a FUNDED caller may still relay. On mainnet
// agents only ever hold $LH (never the AlphaUSD fee token), so an agent that has
// graduated past onboarding STILL cannot self-pay gas; it can only spend its own
// $LH. These two move/authorize the caller's OWN $LH (x402: approve the diamond
// once, then settle), so we sponsor their gas regardless of $LH balance — the
// rate caps + sponsor-float breaker remain the abuse bound. (Deliberate policy:
// no chargebacks, no 90-day lock — meter/wallet $LH is the agent's to spend.)
const SELF_PAY_SELECTORS = new Set([
  selector('settle(address,address,uint256,uint256,uint256,bytes32,bytes)'),
  APPROVE_SELECTOR,
  // send_lh's direct $LH transfer is the same shape as settle: it moves the
  // caller's OWN $LH and can't touch the sponsor's fee-token float, so a
  // GRADUATED (wallet-funded) agent — which still can't self-pay gas on mainnet
  // — may relay it too. (A meter-funded sender with a 0 wallet passes the
  // onboarding gate regardless; this line is what lets a funded agent send.)
  TRANSFER_SELECTOR,
  // createInvite escrows the funder's OWN $LH behind a code (supply-neutral,
  // refundable) — same shape as settle/transfer: moves the caller's own $LH, can't
  // touch the sponsor's fee-token float. An operator funding a new agent via an
  // invite IS funded by definition, so the onboarding-only gate must not block it
  // (it did — LH_RELAY_FUNDED on `invite create`, breaking the documented flow).
  selector('createInvite(bytes32,uint256,uint64)'),
]);

// ALWAYS-FREE writes — sponsored regardless of the caller's $LH balance because
// the platform wants them UNCONDITIONALLY available, not just during onboarding.
// `submitFeedback` is the canonical case: feedback must always be free and
// encouraged, so a graduated/funded agent (which on mainnet holds $LH, not the
// AlphaUSD fee token, and so STILL can't self-pay gas) must not be locked out of
// it with LH_RELAY_FUNDED. Abuse stays bounded by the rate caps + float breaker.
// `register(string)` joins it: claiming a name is the fundamental onboarding +
// actor-model action, and it is necessarily done by a "funded" caller — the name
// claim costs 1 $LH (pulled on-chain), and that caller can't self-pay gas on
// mainnet (no fee token), so the onboarding-only gate was a hard CATCH-22 (you
// need 1 $LH to register, but holding it triggers LH_RELAY_FUNDED). Abuse stays
// bounded by the per-name 1-$LH cost + the rate caps + the float breaker.
const ALWAYS_FREE_SELECTORS = new Set([
  selector('submitFeedback(string)'),
  selector('register(string)'),
  // releaseName is register's lifecycle INVERSE — a user managing their own names
  // (claim ↔ release). It's destructive to the caller's OWN asset, moves no value,
  // and can't touch the sponsor's fee-token float (gas-only), so a funded owner must
  // not be locked out by the onboarding-only gate (#62: bulk_release_subdomains).
  // Bounded by the rate caps + float breaker.
  selector('releaseName(uint256)'),
]);

// Everything exempt from the onboarding-only gate below: self-pay (move the
// caller's OWN $LH) + always-free (feedback). A call batch made up entirely of
// these is sponsored even for a funded caller.
const GATE_EXEMPT_SELECTORS = new Set([...SELF_PAY_SELECTORS, ...ALWAYS_FREE_SELECTORS]);

// --- CORS / json -----------------------------------------------------------

const ALLOWED_ORIGIN_SUFFIX = '.localharness.xyz';
const ALLOWED_ORIGIN_EXACT = 'https://localharness.xyz';

function isAllowedOrigin(origin: string): boolean {
  if (origin === ALLOWED_ORIGIN_EXACT || origin.endsWith(ALLOWED_ORIGIN_SUFFIX)) return true;
  try {
    const u = new URL(origin);
    return u.protocol === 'http:' && (u.hostname === 'localhost' || u.hostname === '127.0.0.1');
  } catch {
    return false;
  }
}

function corsHeaders(origin: string | null): Record<string, string> {
  const h: Record<string, string> = {
    'Access-Control-Allow-Methods': 'POST, OPTIONS',
    'Access-Control-Allow-Headers': 'content-type, x-goog-api-key',
    Vary: 'Origin',
  };
  if (origin && isAllowedOrigin(origin)) h['Access-Control-Allow-Origin'] = origin;
  return h;
}

function json(body: unknown, status: number, origin: string | null): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json', ...corsHeaders(origin) },
  });
}

// --- auth (personal-sign token, mirror of gemini.ts) -----------------------

function isHexAddress(s: string): boolean {
  return /^0x[0-9a-fA-F]{40}$/.test(s);
}

/** Recover the signer of an Ethereum personal_sign over `message`. */
function personalSignRecover(message: string, sigHex: string): string {
  const msgBytes = new TextEncoder().encode(message);
  const prefix = new TextEncoder().encode(`\x19Ethereum Signed Message:\n${msgBytes.length}`);
  const digest = keccak_256(concat(prefix, msgBytes));
  const sig = hexToBytes(stripHex(sigHex));
  if (sig.length !== 65) throw new Error('signature must be 65 bytes');
  let v = sig[64];
  if (v >= 27) v -= 27;
  const signature = secp256k1.Signature.fromCompact(bytesToHex(sig.slice(0, 64))).addRecoveryBit(v);
  const point = signature.recoverPublicKey(digest);
  return '0x' + bytesToHex(keccak_256(point.toRawBytes(false).slice(1)).slice(12));
}

function concat(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

/** Recovered lowercase address, or an error string mapped to a 401. */
function authenticate(token: string): { address: string } | { error: string } {
  const parts = token.split(':');
  if (parts.length !== 3) return { error: 'malformed auth token' };
  const [address, tsStr, signature] = parts;
  if (!isHexAddress(address)) return { error: 'malformed auth token: address' };
  const timestamp = Number(tsStr);
  if (!Number.isInteger(timestamp) || timestamp < 0) return { error: 'malformed auth token: timestamp' };
  const now = Math.floor(Date.now() / 1000);
  if (Math.abs(now - timestamp) > FRESHNESS_WINDOW_SECS) return { error: 'stale or future timestamp' };
  const message = `localharness-proxy:${address.toLowerCase()}:${timestamp}`;
  let recovered: string;
  try {
    recovered = personalSignRecover(message, signature);
  } catch (e) {
    return { error: 'bad signature: ' + (e as Error).message };
  }
  if (recovered.toLowerCase() !== address.toLowerCase()) return { error: 'signature does not match address' };
  return { address: address.toLowerCase() };
}

// --- request parsing -------------------------------------------------------

const HEX = /^0x[0-9a-fA-F]*$/;

function bytesFromHex(h: unknown, name: string): Uint8Array {
  if (typeof h !== 'string' || !HEX.test(h) || h.length % 2 !== 0) {
    throw new Error(`${name} must be 0x-prefixed even-length hex`);
  }
  return hexToBytes(stripHex(h));
}

function bigFrom(v: unknown, name: string): bigint {
  // Accept a decimal string (preferred — avoids JS number precision) or a
  // non-negative integer number.
  if (typeof v === 'string' && /^[0-9]+$/.test(v)) return BigInt(v);
  if (typeof v === 'number' && Number.isInteger(v) && v >= 0) return BigInt(v);
  throw new Error(`${name} must be a non-negative integer (decimal string preferred)`);
}

interface SponsorRequest {
  intent: TempoIntent;
  senderAddress: Uint8Array; // 20 bytes
  senderSignature: Uint8Array; // 65 bytes
}

function parseRequest(body: any): SponsorRequest {
  if (typeof body !== 'object' || body === null) throw new Error('body must be a JSON object');
  const chainId = bigFrom(body.chainId, 'chainId');

  if (!Array.isArray(body.calls) || body.calls.length === 0) throw new Error('calls must be a non-empty array');
  if (body.calls.length > 8) throw new Error('too many calls (max 8)');
  const calls: TempoCallIntent[] = body.calls.map((c: any, i: number) => {
    const to = bytesFromHex(c?.to, `calls[${i}].to`);
    if (to.length !== 20) throw new Error(`calls[${i}].to must be 20 bytes`);
    const value = bigFrom(c?.value ?? '0', `calls[${i}].value`);
    const input = bytesFromHex(c?.input ?? '0x', `calls[${i}].input`);
    return { to, value, input };
  });

  const senderAddress = bytesFromHex(body.senderAddress, 'senderAddress');
  if (senderAddress.length !== 20) throw new Error('senderAddress must be 20 bytes');
  const senderSignature = bytesFromHex(body.senderSignature, 'senderSignature');
  if (senderSignature.length !== 65) throw new Error('senderSignature must be 65 bytes');
  const feeToken = bytesFromHex(body.feeToken, 'feeToken');
  if (feeToken.length !== 20) throw new Error('feeToken must be 20 bytes');

  const optBig = (v: unknown, name: string): bigint | null =>
    v === null || v === undefined || v === 0 || v === '0' ? null : bigFrom(v, name);

  const intent: TempoIntent = {
    chainId,
    maxPriorityFeePerGas: bigFrom(body.maxPriorityFeePerGas, 'maxPriorityFeePerGas'),
    maxFeePerGas: bigFrom(body.maxFeePerGas, 'maxFeePerGas'),
    gasLimit: bigFrom(body.gasLimit, 'gasLimit'),
    calls,
    nonceKey: bigFrom(body.nonceKey ?? '0', 'nonceKey'),
    nonce: bigFrom(body.nonce, 'nonce'),
    validBefore: optBig(body.validBefore, 'validBefore'),
    validAfter: optBig(body.validAfter, 'validAfter'),
    feeToken,
  };
  return { intent, senderAddress, senderSignature };
}

// --- caps ------------------------------------------------------------------

/** Refuse anything outside the sponsorable onboarding/participation surface. */
function checkAllowlist(calls: TempoCallIntent[]): string | null {
  for (let i = 0; i < calls.length; i++) {
    const c = calls[i];
    if (c.value !== 0n) return `calls[${i}]: raw value sends are not sponsorable`;
    if (c.input.length < 4) return `calls[${i}]: calldata too short for a selector`;
    const sel = bytesToHex(c.input.slice(0, 4));
    const to = '0x' + bytesToHex(c.to);
    if (to === DIAMOND) {
      if (!DIAMOND_SELECTORS.has(sel)) return `calls[${i}]: selector 0x${sel} not in the diamond sponsor allowlist`;
    } else if (to === TOKEN) {
      // $LH token: transfer(to, amount) — send_lh / the meter→wallet bridge
      // moving the caller's OWN $LH (any recipient; the whole point is paying
      // another address) — and approve(diamond, amount) for meter top-ups. A
      // transfer can't touch the sponsor's fee-token float, so the only cost is
      // gas, bounded by the rate caps + float breaker (same profile as the
      // settle self-pay path and the diamond economy writes). An approve to an
      // ARBITRARY spender stays refused — that WOULD underwrite a drain.
      if (sel === TRANSFER_SELECTOR) {
        // recipient unrestricted by design — send_lh pays an external address
      } else if (sel === APPROVE_SELECTOR) {
        const spender = '0x' + bytesToHex(c.input.slice(16, 36)); // first arg, right-aligned
        if (spender !== DIAMOND) return `calls[${i}]: approve spender must be the diamond`;
      } else {
        return `calls[${i}]: only transfer() or approve(diamond) is sponsorable on the $LH token`;
      }
    } else {
      return `calls[${i}]: to ${to} is not the diamond or $LH token`;
    }
  }
  return null;
}

/** balanceOf(addr) on an arbitrary TIP-20/ERC-20 token via raw JSON-RPC eth_call. */
async function erc20BalanceOf(token: string, addr: string): Promise<bigint> {
  const data = '0x' + selector('balanceOf(address)') + stripHex(addr).toLowerCase().padStart(64, '0');
  const res = await fetch(TEMPO_RPC, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'eth_call',
      params: [{ to: token, data }, 'latest'],
    }),
  });
  if (!res.ok) throw new Error(`RPC ${res.status}`);
  const j = await res.json();
  if (j.error) throw new Error(`eth_call: ${j.error?.message ?? 'error'}`);
  const hex = stripHex(j.result ?? '0x');
  return hex ? BigInt('0x' + hex) : 0n;
}

/** balanceOf(addr) on the $LH token (the onboarding-gate read). */
function lhBalanceOf(addr: string): Promise<bigint> {
  return erc20BalanceOf(LH_TOKEN, addr);
}

// --- sponsor float circuit-breaker -----------------------------------------
// Refuse to sign once the sponsor's fee_token float drops below MIN_FLOAT, so a
// near-empty sponsor returns a clean LH_RELAY_FLOAT_LOW instead of letting the
// CLI assemble a tx that reverts on-chain (the sponsor can't pay). The float is
// the hard spend ceiling; this is the clean-error + alarm at the edge of it.
// Floor in fee_token base units (USDC.e is 6-dec) — default 0.05 USDC.e; env
// LH_RELAY_MIN_FLOAT_WEI tunes it (0 disables). The balance is cached per-isolate
// for a short TTL so we don't eth_call on every request.
const MIN_FLOAT_WEI = BigInt(process.env.LH_RELAY_MIN_FLOAT_WEI ?? '50000');
const FLOAT_CACHE_MS = 30_000;
let floatCache: { token: string; wei: bigint; at: number } | null = null;

/** The sponsor's fee_token float (cached). `nowMs` is injectable for tests. */
async function sponsorFloat(feeToken: string, nowMs: number): Promise<bigint> {
  if (floatCache && floatCache.token === feeToken && nowMs - floatCache.at < FLOAT_CACHE_MS) {
    return floatCache.wei;
  }
  const wei = await erc20BalanceOf(feeToken, SPONSOR_ADDRESS);
  floatCache = { token: feeToken, wei, at: nowMs };
  return wei;
}

/** Test-only: clear the in-isolate float cache between cases. */
export function __resetFloatCache(): void {
  floatCache = null;
}

// --- handler ---------------------------------------------------------------

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');
  if (req.method === 'OPTIONS') return new Response(null, { status: 204, headers: corsHeaders(origin) });
  if (req.method !== 'POST') return json({ error: 'method not allowed' }, 405, origin);

  const token = req.headers.get('x-goog-api-key') ?? '';
  const auth = authenticate(token);
  if ('error' in auth) return json({ error: auth.error, code: 'LH_RELAY_AUTH' }, 401, origin);
  const caller = auth.address;

  // Rate limit AFTER auth (a fee signature is valuable — verify first).
  const globalWait = globalWindow.hit('*');
  if (globalWait > 0) return json({ error: 'relay busy, retry later', code: 'LH_RELAY_GLOBAL_RATE', retryAfter: globalWait }, 429, origin);
  const wait = perAddr.hit(caller);
  if (wait > 0) return json({ error: 'per-address sponsorship rate exceeded', code: 'LH_RELAY_RATE', retryAfter: wait }, 429, origin);

  let parsed: SponsorRequest;
  try {
    const body = await req.json();
    parsed = parseRequest(body);
  } catch (e) {
    return json({ error: (e as Error).message, code: 'LH_RELAY_BADREQ' }, 400, origin);
  }
  const { intent, senderAddress, senderSignature } = parsed;
  const senderHex = '0x' + bytesToHex(senderAddress);

  // 1. Chain coherence — refuse a chainId that isn't this relay's chain.
  if (intent.chainId !== BigInt(CHAIN_ID)) {
    return json({ error: `chainId ${intent.chainId} != relay chain ${CHAIN_ID}`, code: 'LH_RELAY_CHAIN' }, 400, origin);
  }
  // 2. The caller must be the sender — you can't sponsor a tx you didn't sign.
  if (senderHex !== caller) {
    return json({ error: 'senderAddress does not match the authenticated caller', code: 'LH_RELAY_SENDER' }, 403, origin);
  }
  // 3. No blind signing: the senderSignature must recover the senderAddress over
  //    the sponsored sender hash we recompute from the submitted intent.
  let recoveredSender: string;
  try {
    recoveredSender = recoverAddressFromDigest(senderSignature, sponsoredSenderHash(intent));
  } catch (e) {
    return json({ error: 'sender signature invalid: ' + (e as Error).message, code: 'LH_RELAY_SIG' }, 400, origin);
  }
  if (recoveredSender.toLowerCase() !== senderHex) {
    return json({ error: 'sender signature does not authorize this intent', code: 'LH_RELAY_SIG' }, 403, origin);
  }
  // 4. fee_token must be this chain's fee token (no sponsoring a foreign token).
  //    (The chain only accepts a USD-currency TIP-20 as fee_token anyway.)
  // 5. Gas re-clamp (mirror clamp_gas_price + a limit ceiling).
  if (intent.maxFeePerGas > MAX_GAS_PRICE_WEI || intent.maxPriorityFeePerGas > MAX_GAS_PRICE_WEI) {
    return json({ error: 'gas price exceeds the relay ceiling', code: 'LH_RELAY_GAS' }, 400, origin);
  }
  if (intent.gasLimit === 0n || intent.gasLimit > MAX_GAS_LIMIT) {
    return json({ error: 'gas limit out of range', code: 'LH_RELAY_GAS' }, 400, origin);
  }
  // 6. Selector allowlist (default-deny).
  const allowErr = checkAllowlist(intent.calls);
  if (allowErr) return json({ error: allowErr, code: 'LH_RELAY_SELECTOR' }, 403, origin);

  // 7. Onboarding-only spend gate: sponsor only zero/near-zero-$LH callers —
  //    UNLESS every call is on the gate-exempt surface: self-pay (approve +
  //    settle + $LH transfer — a funded agent can't hold the fee token to pay its
  //    own gas) OR always-free (submitFeedback — feedback must never be locked
  //    behind funding). A mix with any other (onboarding/economy) write still gates.
  const gateExempt = intent.calls.every((c) => GATE_EXEMPT_SELECTORS.has(bytesToHex(c.input.slice(0, 4))));
  if (!gateExempt) {
    try {
      const bal = await lhBalanceOf(caller);
      if (bal > BALANCE_CEILING_WEI) {
        return json(
          {
            error: 'caller is funded — sponsorship is onboarding-only; self-pay your fees',
            code: 'LH_RELAY_FUNDED',
          },
          403,
          origin,
        );
      }
    } catch (e) {
      // Fail CLOSED — if we can't prove the caller is unfunded, don't sponsor.
      return json({ error: 'balance check failed: ' + (e as Error).message, code: 'LH_RELAY_BALANCE' }, 502, origin);
    }
  }

  // 8. Float circuit-breaker: refuse cleanly when the sponsor can't cover fees
  //    (fail-CLOSED — never sign a tx the sponsor will revert on-chain).
  if (MIN_FLOAT_WEI > 0n) {
    try {
      const float = await sponsorFloat('0x' + bytesToHex(intent.feeToken), Date.now());
      if (float < MIN_FLOAT_WEI) {
        return json(
          { error: 'sponsor float exhausted — top up the fee_token; sponsorship paused', code: 'LH_RELAY_FLOAT_LOW' },
          503,
          origin,
        );
      }
    } catch (e) {
      return json({ error: 'sponsor float check failed: ' + (e as Error).message, code: 'LH_RELAY_FLOAT' }, 502, origin);
    }
  }

  // All caps passed — sign the fee_payer half.
  const fpHash = feePayerHash(intent, senderAddress);
  const fpSig = signHash65(fpHash, SPONSOR_KEY);

  // WELCOME-ON-CREATION: a sponsored `register(string)` is a brand-new agent
  // being minted. Send a warm welcome from the platform's `localharness` agent
  // into the new name's on-chain inbox (it lands in the agent's bell on first
  // open — push-free, durable). `waitUntil` keeps the Edge function alive to
  // finish this AFTER the response returns — a plain fire-and-forget is KILLED
  // on response, before the helper's name-resolve poll completes (the caller
  // assembles + submits the register tx right AFTER this returns, so the name
  // doesn't exist yet). Never throws; never delays the relay response.
  const newName = intent.calls.map(registeredName).find((n) => n !== null);
  if (newName) {
    waitUntil(welcomeNewAgent(newName));
  }

  return json(
    {
      feePayer: SPONSOR_ADDRESS,
      feeToken: '0x' + bytesToHex(intent.feeToken),
      feePayerSignature: '0x' + bytesToHex(fpSig), // 65 bytes r||s||v (v∈{27,28})
      feePayerHash: '0x' + bytesToHex(fpHash),
    },
    200,
    origin,
  );
}
