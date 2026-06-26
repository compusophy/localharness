// _auth.ts — the proxy's SHARED on-chain-read + meter primitives.
//
// The personal-sign auth token (recovery, freshness, route-binding) + CORS rules
// now live in _authcore.ts — dep-light, shared by EVERY route incl. sponsor /
// publish / telemetry (audit L7/L10). This module re-exports them unchanged and
// adds the viem-backed reads (`creditOf` / `sessionExpiryOf`) + the
// `CreditMeterFacet.meter` debit, which pull in viem and so stay OUT of the core.

import {
  createPublicClient,
  createWalletClient,
  defineChain,
  encodeFunctionData,
  http,
} from 'viem';
import { privateKeyToAccount } from 'viem/accounts';
import { keccak_256 } from '@noble/hashes/sha3';
import { bytesToHex } from '@noble/hashes/utils';

import { TEMPO_RPC, REGISTRY, CHAIN_ID } from './_chain';

// Re-export the shared auth/origin/hex primitives so existing importers
// (gemini / schedule / notify / broadcast / chat / signal / fetch / mcp) keep
// working unchanged — there is now ONE implementation behind them (_authcore).
import {
  isAllowedOrigin,
  ALLOWED_ORIGIN_SUFFIX,
  ALLOWED_ORIGIN_EXACT,
  stripHex,
  isHexAddress,
  recoverAddress,
  FRESHNESS_WINDOW_SECS,
  verifyAuthToken,
  verifyAuthTokenOrThrow,
} from './_authcore';
export {
  isAllowedOrigin,
  ALLOWED_ORIGIN_SUFFIX,
  ALLOWED_ORIGIN_EXACT,
  stripHex,
  isHexAddress,
  recoverAddress,
  FRESHNESS_WINDOW_SECS,
  verifyAuthToken,
  verifyAuthTokenOrThrow,
};

// ---- ABI helpers (keccak-based; used by the eth_call reads below) -----------

export function keccak(data: Uint8Array): Uint8Array {
  return keccak_256(data);
}

/** Lowercase 0x address from a 64-byte uncompressed pubkey (no 0x04 prefix). */
export function toAddress(pubKeyXY: Uint8Array): string {
  return '0x' + bytesToHex(keccak(pubKeyXY).slice(12));
}

export function encodeAddressWord(address: string): string {
  return stripHex(address).toLowerCase().padStart(64, '0');
}

/** 4-byte function selector hex (no 0x) — keccak256(sig)[..4]. */
export function selector(sig: string): string {
  return bytesToHex(keccak(new TextEncoder().encode(sig)).slice(0, 4));
}

// ---- on-chain reads + meter debit -------------------------------------------

export const TEMPO_CHAIN = defineChain({
  id: CHAIN_ID,
  name: 'Tempo Moderato',
  nativeCurrency: { name: 'Tempo', symbol: 'TEMPO', decimals: 18 },
  rpcUrls: { default: { http: [TEMPO_RPC] } },
});

const METER_ABI = [
  {
    name: 'meter',
    type: 'function',
    stateMutability: 'nonpayable',
    inputs: [
      { name: 'user', type: 'address' },
      { name: 'amount', type: 'uint256' },
    ],
    outputs: [],
  },
] as const;

/** One `eth_call` against the diamond; returns the raw result hex or throws. */
export async function ethCall(data: string): Promise<string> {
  const res = await fetch(TEMPO_RPC, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'eth_call',
      params: [{ to: REGISTRY, data }, 'latest'],
    }),
  });
  const body = (await res.json()) as { result?: string; error?: unknown };
  if (!body.result) {
    throw new Error('eth_call failed: ' + JSON.stringify(body.error ?? {}));
  }
  return body.result;
}

/** `sessionExpiryOf(address) -> uint256`, decoded as BigInt unix seconds.
 * Compare as BigInt — never lossily coerce a uint256 word to Number. */
export async function sessionExpiryOf(address: string): Promise<bigint> {
  return BigInt(
    await ethCall('0x' + selector('sessionExpiryOf(address)') + encodeAddressWord(address)),
  );
}

/** `creditOf(address) -> uint256` — the user's prepaid per-request balance. */
export async function creditOf(address: string): Promise<bigint> {
  return BigInt(
    await ethCall('0x' + selector('creditOf(address)') + encodeAddressWord(address)),
  );
}

/** Thrown when the on-chain debit REVERTED — the caller is actually out of
 * `$LH` for this request (`CreditMeterFacet.meter` reverts `InsufficientCredits`
 * rather than ever letting a balance go negative). The handler maps this to 402,
 * distinct from an ambiguous RPC failure (502). */
export class InsufficientCreditError extends Error {}

/**
 * Debit `amount` `$LH` from `user` via `CreditMeterFacet.meter`, signed by the
 * proxy's meter key (env `PROXY_METER_KEY`). The debit is AUTHORITATIVE: we
 * await the RECEIPT (when `confirm`), not just submission, and throw
 * `InsufficientCreditError` if it reverted. An ambiguous wait failure
 * (RPC/timeout) is deliberately NOT treated as a revert: we return normally so
 * the caller is still served, rather than risk a double-charge if they retry a
 * debit that actually landed.
 *
 * `confirm=false` (streaming callers) awaits only the broadcast — it must NOT
 * serialize first-byte latency behind the receipt. Burst safety then comes from
 * the broadcast assigning the address's account nonce serially + the caller's
 * own in-isolate reservation + an up-front floor debit.
 */
export async function meterDebit(
  user: string,
  amount: bigint,
  confirm = true,
): Promise<void> {
  const pk = process.env.PROXY_METER_KEY;
  if (!pk) throw new Error('missing PROXY_METER_KEY');
  const account = privateKeyToAccount(
    (pk.startsWith('0x') ? pk : `0x${pk}`) as `0x${string}`,
  );
  const wallet = createWalletClient({
    account,
    chain: TEMPO_CHAIN,
    transport: http(TEMPO_RPC),
  });
  const data = encodeFunctionData({
    abi: METER_ABI,
    functionName: 'meter',
    args: [user as `0x${string}`, amount],
  });
  const hash = await wallet.sendTransaction({
    to: REGISTRY as `0x${string}`,
    data,
    value: 0n,
  });

  if (!confirm) return;

  const pub = createPublicClient({ chain: TEMPO_CHAIN, transport: http(TEMPO_RPC) });
  let status: 'success' | 'reverted';
  try {
    ({ status } = await pub.waitForTransactionReceipt({
      hash,
      timeout: 12_000,
      pollingInterval: 500,
    }));
  } catch {
    return; // ambiguous (RPC/timeout) — serve; do NOT double-charge on retry
  }
  if (status === 'reverted') {
    throw new InsufficientCreditError('on-chain debit reverted (insufficient $LH)');
  }
}
