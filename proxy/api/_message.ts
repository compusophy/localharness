// localharness async agent INBOX writer (MessageFacet.sendMessage) — shared.
//
// The permissionless on-chain inbox (#35): any identity can `sendMessage(toId,
// body)`; the recipient POLLS it (src/app/notifications.rs::import_onchain_
// messages) so a note surfaces in their in-app bell next time the tab opens,
// with NO Web Push subscription required — durable, push-independent delivery.
//
// The proxy is the sender of record (its PROXY_METER_KEY signs the tx); the
// human-readable attribution (`@<from>: …`) lives in the BODY, exactly as
// api/notify.ts already does. This module is the ONE home for that write so both
// notify.ts (cross-agent no-push fallback) and sponsor.ts (welcome-on-creation)
// share it instead of forking the viem/encoding logic.
//
// The underscore prefix keeps Vercel from deploying this file as a route.

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
import { TEMPO_RPC, REGISTRY, CHAIN_ID } from './_chain';

// MessageFacet body cap (matches the on-chain `MessageTooLong` guard at 1024).
export const MAX_MESSAGE_BODY_BYTES = 1024;

const TEMPO_CHAIN = defineChain({
  id: CHAIN_ID,
  name: 'Tempo',
  nativeCurrency: { name: 'Tempo', symbol: 'TEMPO', decimals: 18 },
  rpcUrls: { default: { http: [TEMPO_RPC] } },
});

const MESSAGE_ABI = [
  {
    name: 'sendMessage',
    type: 'function',
    stateMutability: 'nonpayable',
    inputs: [
      { name: 'toId', type: 'uint256' },
      { name: 'body', type: 'string' },
    ],
    outputs: [],
  },
] as const;

function stripHex(h: string): string {
  return h.startsWith('0x') ? h.slice(2) : h;
}

function selector(sig: string): string {
  return bytesToHex(keccak_256(new TextEncoder().encode(sig)).slice(0, 4));
}

/** ABI-encode a single dynamic `string` argument (offset + len + padded). */
function encodeStringArg(s: string): string {
  const bytes = new TextEncoder().encode(s);
  const padded = Math.ceil(bytes.length / 32) * 32;
  let hex = '';
  for (const b of bytes) hex += b.toString(16).padStart(2, '0');
  return (
    (32).toString(16).padStart(64, '0') +
    bytes.length.toString(16).padStart(64, '0') +
    hex.padEnd(padded * 2, '0')
  );
}

/** One `eth_call` against the diamond; returns the raw result hex or throws. */
async function ethCall(data: string): Promise<string> {
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

/** `idOfName(name) -> uint256` — 0 when the name is not registered. */
export async function idOfName(name: string): Promise<bigint> {
  return BigInt(
    await ethCall('0x' + selector('idOfName(string)') + encodeStringArg(name)),
  );
}

/**
 * RECORD a note in the recipient tokenId's on-chain inbox
 * (`MessageFacet.sendMessage`), signed by the proxy meter key. `body` is the
 * already-attributed text; it is trimmed to the on-chain `MessageTooLong` cap
 * (utf-8). Awaits the receipt; throws on a definitive revert, returns on an
 * ambiguous wait failure (the tx likely landed — never risk a perceived
 * double-send on retry). Best-effort by design — callers treat any throw as
 * "could not record".
 */
export async function recordOnChainMessage(toId: bigint, body: string): Promise<void> {
  const pk = process.env.PROXY_METER_KEY;
  if (!pk) throw new Error('missing PROXY_METER_KEY');
  // Trim to the on-chain byte cap (utf-8); sendMessage reverts past 1024 bytes.
  let bytes = new TextEncoder().encode(body);
  if (bytes.length > MAX_MESSAGE_BODY_BYTES) {
    bytes = bytes.slice(0, MAX_MESSAGE_BODY_BYTES);
    body = new TextDecoder().decode(bytes); // may drop a trailing partial char
  }
  if (!body) throw new Error('empty message body');
  const account = privateKeyToAccount(
    (pk.startsWith('0x') ? pk : `0x${pk}`) as `0x${string}`,
  );
  const wallet = createWalletClient({
    account,
    chain: TEMPO_CHAIN,
    transport: http(TEMPO_RPC),
  });
  const data = encodeFunctionData({
    abi: MESSAGE_ABI,
    functionName: 'sendMessage',
    args: [toId, body],
  });
  const hash = await wallet.sendTransaction({
    to: REGISTRY as `0x${string}`,
    data,
    value: 0n,
  });
  const pub = createPublicClient({ chain: TEMPO_CHAIN, transport: http(TEMPO_RPC) });
  let status: 'success' | 'reverted';
  try {
    ({ status } = await pub.waitForTransactionReceipt({
      hash,
      timeout: 12_000,
      pollingInterval: 500,
    }));
  } catch {
    return; // ambiguous (RPC/timeout) — assume stored; don't fail the note
  }
  if (status === 'reverted') {
    throw new Error('on-chain message store reverted');
  }
}

export { stripHex, selector, encodeStringArg };
