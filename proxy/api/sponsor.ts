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
import { TEMPO_RPC, REGISTRY, CHAIN_ID, LH_TOKEN, FEE_TOKEN } from './_chain';
// Personal-sign auth token + CORS origin policy are the SHARED _authcore impls
// (audit L7/L10 dedup): sponsor.ts no longer carries its own authenticate /
// personalSignRecover / isAllowedOrigin copies. The `'sponsor'` route argument
// (audit L9) binds a token to THIS endpoint so one minted for a cheap route
// (metering) can't be replayed to the fee-payer relay inside the 300s window.
import { verifyAuthToken, isAllowedOrigin } from './_authcore';
import { SlidingWindow } from './_ratelimit';
import { envGuard } from './_env';
import { welcomeNewAgent } from './_welcome';
import { waitUntil } from '@vercel/functions';

export const config = { runtime: 'edge' };

// --- constants -------------------------------------------------------------

// Absolute per-gas price ceiling, mirroring `src/registry/tx.rs::MAX_GAS_PRICE_WEI`
// (50 gwei — T7 hard-caps the base fee at 12 gwei and clients bid 2x spot, so
// legit prices stay <=24 gwei). A hostile/MITM'd CLI RPC could otherwise submit
// an inflated price to drain the sponsor's fee-token float — we refuse rather
// than clamp. Keep the two constants in lockstep.
const MAX_GAS_PRICE_WEI = 50_000_000_000n;
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
// Exported for the scheduler's hourly float health check (_health.ts).
export const SPONSOR_ADDRESS = addressFromPrivKey(SPONSOR_KEY); // lowercase 0x

const DIAMOND = REGISTRY.toLowerCase();
const TOKEN = LH_TOKEN.toLowerCase();
const SPONSOR_FEE_TOKEN = FEE_TOKEN.toLowerCase();

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
  // withdrawCredits moves the caller's OWN escrowed meter $LH back to the
  // caller's wallet (msg.sender both sides) — the exact settle/transfer shape
  // this set exists for. Without it, `credits --reclaim` hits LH_RELAY_FUNDED
  // for precisely its target audience (a funded-wallet user with meter dust).
  selector('withdrawCredits(uint256)'),
  // reclaimInvite refunds the invite's FUNDER no matter who calls (the
  // permissionless expiry poke) — it can only move escrow back where it came
  // from. Without this, a funded operator could createInvite but never take
  // it back (LH_RELAY_FUNDED on reclaim — found by the 2026-07-05 fleet).
  selector('reclaimInvite(bytes32)'),
  // depositCredits is withdrawCredits' EXACT inverse (wallet→meter, msg.sender
  // both sides) — a funded agent MUST be able to fund its own meter, yet
  // `topup` hit LH_RELAY_FUNDED for exactly that caller (2026-07-06 fleet).
  selector('depositCredits(uint256)'),
  // redeem mints against an owner-issued ONE-SHOT code (amount fixed at
  // addRedeemCodes time, claimed-flag dedup) — the relay only ever pays gas,
  // never the mint, and supply stays owner-controlled. Without it a funded
  // agent could never redeem a top-up code (LH_RELAY_FUNDED, same fleet).
  selector('redeem(string)'),
]);

// ALWAYS-FREE writes — sponsored regardless of the caller's $LH balance because
// the platform wants them UNCONDITIONALLY available, not just during onboarding.
// (Feedback + push enrollment left this set entirely: both moved OFF-CHAIN —
// proxy /api/telemetry and /api/push-sub — and their selectors are no longer
// relayed at all.)
// `register(string)`: claiming a name is the fundamental onboarding +
// actor-model action, and it is necessarily done by a "funded" caller — the name
// claim costs 1 $LH (pulled on-chain), and that caller can't self-pay gas on
// mainnet (no fee token), so the onboarding-only gate was a hard CATCH-22 (you
// need 1 $LH to register, but holding it triggers LH_RELAY_FUNDED). Abuse stays
// bounded by the per-name 1-$LH cost + the rate caps + the float breaker.
const ALWAYS_FREE_SELECTORS = new Set([
  selector('register(string)'),
  // releaseName is register's lifecycle INVERSE — a user managing their own names
  // (claim ↔ release). It's destructive to the caller's OWN asset, moves no value,
  // and can't touch the sponsor's fee-token float (gas-only), so a funded owner must
  // not be locked out by the onboarding-only gate (#62: bulk_release_subdomains).
  // Bounded by the rate caps + float breaker.
  selector('releaseName(uint256)'),
]);

// The BOUNTY / economy-lifecycle surface — the agent economy's core loop
// (post → claim → submit → accept → attest) plus its escrow-recovery inverses.
// Every one either moves the caller's OWN $LH — `postBounty` escrows it and
// `acceptResult` releases the already-escrowed funds to the worker, both
// supply-neutral exactly like `createInvite`/`settle`/`transfer` — or is gas-only
// with no value (`claimBounty`/`submitResult`/`attest`/`cancelBounty`/
// `reclaimExpired`, like `releaseName`). NONE can touch the sponsor's AlphaUSD
// fee-token float. A FUNDED agent is the ONLY kind that can run a colony cycle
// (escrowing a reward REQUIRES holding $LH), so the onboarding-only gate locked
// the whole economy out at `postBounty` (LH_RELAY_FUNDED — `colony run` couldn't
// even start). Abuse stays bounded by the rate caps + the sponsor-float breaker,
// the same bound the existing self-pay/always-free exemptions rely on.
const BOUNTY_LIFECYCLE_SELECTORS = new Set([
  selector('postBounty(bytes,uint128,uint64)'),
  selector('claimBounty(uint256,uint256)'),
  selector('submitResult(uint256,bytes)'),
  selector('acceptResult(uint256)'),
  selector('cancelBounty(uint256)'),
  selector('reclaimExpired(uint256)'),
  selector('attest(uint256,uint8,bytes32)'),
]);

// Everything exempt from the onboarding-only gate below: self-pay (move the
// caller's OWN $LH) + always-free (register/release) + the bounty/economy lifecycle
// (escrow the caller's own $LH or gas-only, never the sponsor float). A call
// batch made up entirely of these is sponsored even for a funded caller.
const GATE_EXEMPT_SELECTORS = new Set([
  ...SELF_PAY_SELECTORS,
  ...ALWAYS_FREE_SELECTORS,
  ...BOUNTY_LIFECYCLE_SELECTORS,
]);

// `setMetadata` is a SELF-EDIT of the caller's OWN identity (persona, x402 price,
// lessons, small html face) — owner-gated on-chain, moves no value, can't touch the
// sponsor's fee-token float. A funded agent (any agent that has earned/holds $LH)
// was locked out of ALL of these on mainnet by the onboarding-only gate — the core
// self-sovereign-agent surface. It's gas-only, so we exempt it, but ONLY for a
// SMALL payload: `setMetadata` costs ~7.6k gas/byte, so a size cap keeps a funded
// caller's per-call gas draw bounded (app CARTRIDGES are off-chain now, so on-chain
// metadata is small text — persona/price/lessons all fit). A large write stays
// gated. Bounded further by the rate caps + float breaker + MAX_GAS_LIMIT.
const SETMETADATA_SELECTOR = selector('setMetadata(uint256,bytes32,bytes)');
const SETMETA_EXEMPT_MAX_BYTES = 4096;

/** The byte-length of a `setMetadata(uint256,bytes32,bytes)` value, or null if the
 *  calldata isn't a standard-encoded setMetadata. Layout: selector(4) + tokenId(32)
 *  + key(32) + offset(32, must be 0x60) + len(32) + value. */
function setMetadataValueLen(input: Uint8Array): number | null {
  if (input.length < 132) return null;
  const off = BigInt('0x' + bytesToHex(input.slice(68, 100)));
  if (off !== 0x60n) return null; // only the standard single-dynamic-arg encoding
  return Number(BigInt('0x' + bytesToHex(input.slice(100, 132))));
}

/** True if a call is exempt from the onboarding-only funded gate: a gate-exempt
 *  selector, or a SMALL `setMetadata` self-edit on the diamond. */
function isGateExemptCall(c: TempoCallIntent): boolean {
  const sel = bytesToHex(c.input.slice(0, 4));
  if (GATE_EXEMPT_SELECTORS.has(sel)) return true;
  if (sel === SETMETADATA_SELECTOR && '0x' + bytesToHex(c.to) === DIAMOND) {
    const len = setMetadataValueLen(c.input);
    return len !== null && len <= SETMETA_EXEMPT_MAX_BYTES;
  }
  return false;
}

// --- CORS / json -----------------------------------------------------------
// isAllowedOrigin is the shared _authcore impl (imported above).

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
    // CONTRACT-CREATION intent (CLI `facet deploy` / `facet diamond`): the
    // sender hash encodes each call's `to` EMPTY (rlp_create_call) — without
    // this flag the recompute used the 20-byte `to` and every sponsored deploy
    // 403'd LH_RELAY_SIG (telemetry #45). Anything but literal true = false.
    create: body.create === true,
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

// CREATE intents have NO selector/`to` to allowlist — the init-code IS the
// payload. A deploy is gas-only (zero value; it can't move anyone's $LH or the
// sponsor's fee-token float), so like releaseName it is also gate-EXEMPT below:
// child-diamond genesis / `facet deploy` are normal-user ops and a funded agent
// still can't self-pay gas on mainnet. Bounds: exactly ONE call (the Rust
// `create_sponsored` shape), zero value, non-empty init-code capped at the
// EIP-3860 limit — plus the shared rate caps + float breaker + MAX_GAS_LIMIT.
const MAX_INITCODE_BYTES = 49152;

function checkCreate(calls: TempoCallIntent[]): string | null {
  if (calls.length !== 1) return 'create tx must contain exactly one call';
  const c = calls[0];
  if (c.value !== 0n) return 'create: raw value sends are not sponsorable';
  if (c.input.length === 0) return 'create: init-code must be non-empty';
  if (c.input.length > MAX_INITCODE_BYTES) {
    return `create: init-code exceeds ${MAX_INITCODE_BYTES} bytes (EIP-3860)`;
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
export const MIN_FLOAT_WEI = BigInt(process.env.LH_RELAY_MIN_FLOAT_WEI ?? '50000');
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

  // MAINNET fail-LOUD (road-to-v1 step 2): with LH_SPONSOR_KEY unset the relay
  // silently falls back to the COMMITTED testnet key — a public, unfunded
  // fee-payer on mainnet, so every sponsorship fails confusingly (or signs with
  // a key anyone holds). 503 with a named code instead. Testnet keeps the
  // committed play-money fallback by design.
  const misconfig = envGuard(
    'sponsor',
    CHAIN_ID === 4217 ? ['LH_SPONSOR_KEY'] : [],
    [],
    corsHeaders(origin),
  );
  if (misconfig) return misconfig;

  const token = req.headers.get('x-goog-api-key') ?? '';
  // Route-bind the token to THIS endpoint (audit L9): a token minted for a cheap
  // route can't be replayed to the fee-payer relay inside the 300s window. The
  // relay still lowercases the caller + compares it to senderHex below.
  const auth = verifyAuthToken(token, Math.floor(Date.now() / 1000), 'sponsor');
  if (!auth.ok) return json({ error: auth.error, code: 'LH_RELAY_AUTH' }, auth.status, origin);
  const caller = auth.address.toLowerCase();

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
  // 4. fee_token must be this chain's canonical sponsor fee token. fee_token is the
  //    ONE intent field the sender's signature does NOT commit to on a sponsored tx
  //    (sponsoredSenderHash blanks it to 0x80), so the relay's fee_payer signature
  //    is the SOLE authorization of which token pays gas — pin it to FEE_TOKEN or a
  //    hostile/MITM'd client could name a different token for the sponsor to pay in.
  if ('0x' + bytesToHex(intent.feeToken).toLowerCase() !== SPONSOR_FEE_TOKEN) {
    return json({ error: "fee_token is not this chain's sponsor fee token", code: 'LH_RELAY_FEETOKEN' }, 400, origin);
  }
  // 5. Gas re-clamp (mirror clamp_gas_price + a limit ceiling).
  if (intent.maxFeePerGas > MAX_GAS_PRICE_WEI || intent.maxPriorityFeePerGas > MAX_GAS_PRICE_WEI) {
    return json({ error: 'gas price exceeds the relay ceiling', code: 'LH_RELAY_GAS' }, 400, origin);
  }
  if (intent.gasLimit === 0n || intent.gasLimit > MAX_GAS_LIMIT) {
    return json({ error: 'gas limit out of range', code: 'LH_RELAY_GAS' }, 400, origin);
  }
  // 6. Selector allowlist (default-deny) — or the CREATE bounds when there is
  //    no selector/`to` to check (sponsored contract deployment).
  const allowErr = intent.create ? checkCreate(intent.calls) : checkAllowlist(intent.calls);
  if (allowErr) return json({ error: allowErr, code: 'LH_RELAY_SELECTOR' }, 403, origin);

  // 7. Onboarding-only spend gate: sponsor only zero/near-zero-$LH callers —
  //    UNLESS every call is on the gate-exempt surface: self-pay (approve +
  //    settle + $LH transfer — a funded agent can't hold the fee token to pay its
  //    own gas) OR always-free (register/releaseName — the name lifecycle must
  //    never be locked behind funding). A mix with any other (onboarding/economy)
  //    write still gates. CREATE intents are gas-only deploys (checkCreate) —
  //    exempt like releaseName; funded agents genesis diamonds / deploy facets.
  const gateExempt = intent.create || intent.calls.every(isGateExemptCall);
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
