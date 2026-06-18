// lessons.ts — public read-only GET /lessons: HARVEST on-chain agent "lessons".
//
// Every localharness agent accumulates short self-recorded lessons (one per real
// error/correction; see src/lessons.rs + the record_lesson tool) stored on-chain
// as registry metadata under keccak256("localharness.lessons") for its tokenId,
// and folded into the agent's system prompt on every surface. This endpoint lets
// an EXTERNAL developer harvest that silo knowledge so it can be incorporated
// recursively — fetch one agent's lessons (by name or tokenId) or enumerate the
// most-recently-registered agents' lessons in one call.
//
// No auth, no chain WRITE — a read of public metadata via the same RPC/diamond as
// scheduler.ts. Short cache; lessons change slowly. Empty/unset is a 200 with an
// empty string, NOT an error (an agent with no lessons is normal).

export const config = { runtime: 'edge' };

import { createPublicClient, defineChain, http } from 'viem';
import { TEMPO_RPC, REGISTRY, CHAIN_ID } from './_chain';

const TEMPO_CHAIN = defineChain({
  id: CHAIN_ID,
  name: 'Tempo',
  nativeCurrency: { name: 'Tempo', symbol: 'TEMPO', decimals: 18 },
  rpcUrls: { default: { http: [TEMPO_RPC] } },
});

// metadata(uint256,bytes32) -> bytes — the same lessons-slot read scheduler.ts
// (lessonsOf) and the browser app do.
const METADATA_ABI = [
  {
    name: 'metadata',
    type: 'function',
    stateMutability: 'view',
    inputs: [
      { name: 'tokenId', type: 'uint256' },
      { name: 'key', type: 'bytes32' },
    ],
    outputs: [{ name: '', type: 'bytes' }],
  },
] as const;

const NAME_ABI = [
  {
    name: 'nameOfId',
    type: 'function',
    stateMutability: 'view',
    inputs: [{ name: 'id', type: 'uint256' }],
    outputs: [{ name: '', type: 'string' }],
  },
] as const;

const ID_OF_NAME_ABI = [
  {
    name: 'idOfName',
    type: 'function',
    stateMutability: 'view',
    inputs: [{ name: 'name', type: 'string' }],
    outputs: [{ name: '', type: 'uint256' }],
  },
] as const;

const NEXT_ID_ABI = [
  {
    name: 'nextId',
    type: 'function',
    stateMutability: 'view',
    inputs: [],
    outputs: [{ name: '', type: 'uint256' }],
  },
] as const;

// Self-recorded lessons slot — keccak256("localharness.lessons"), precomputed +
// inlined, IDENTICAL to scheduler.ts (LESSONS_KEY); pinned by the Rust test
// `lessons_key_distinct_from_other_metadata_keys` in src/registry/names.rs.
const LESSONS_KEY =
  '0x08564cae936ec460c48a23578c7df5665bad18fe42f3c5dbde517ad67a9d9c89' as `0x${string}`;

// Cap on `?recent=N` enumeration so one request can't fan out into hundreds of
// per-id RPC reads. Mirrors mcp.ts's DISCOVER_SCAN_CAP intent.
const RECENT_CAP = 50;

function publicClient() {
  return createPublicClient({ chain: TEMPO_CHAIN, transport: http(TEMPO_RPC) });
}

/** Self-recorded lessons blob for a tokenId (trimmed UTF-8; '' when unset).
 * viem unwraps the ABI `bytes` return to a raw 0x payload, then we decode it. */
async function lessonsOf(tokenId: bigint): Promise<string> {
  const raw = (await publicClient().readContract({
    address: REGISTRY as `0x${string}`,
    abi: METADATA_ABI,
    functionName: 'metadata',
    args: [tokenId, LESSONS_KEY],
  })) as `0x${string}`;
  return decodeUtf8Bytes(raw).trim();
}

/** `idOfName(name)` — the token id of a registered name; 0n if unregistered. */
async function idOfName(name: string): Promise<bigint> {
  return (await publicClient().readContract({
    address: REGISTRY as `0x${string}`,
    abi: ID_OF_NAME_ABI,
    functionName: 'idOfName',
    args: [name],
  })) as bigint;
}

/** `nameOfId(tokenId)` — empty for an unregistered / burned id. */
async function nameOfId(tokenId: bigint): Promise<string> {
  try {
    const name = (await publicClient().readContract({
      address: REGISTRY as `0x${string}`,
      abi: NAME_ABI,
      functionName: 'nameOfId',
      args: [tokenId],
    })) as string;
    return (name || '').trim();
  } catch {
    return '';
  }
}

/** `nextId()` — next id to mint; registered ids are 1..nextId()-1 (monotonic). */
async function nextId(): Promise<bigint> {
  try {
    return (await publicClient().readContract({
      address: REGISTRY as `0x${string}`,
      abi: NEXT_ID_ABI,
      functionName: 'nextId',
      args: [],
    })) as bigint;
  } catch {
    return 0n;
  }
}

/** Decode an ABI-`bytes` 0x word (viem already unwraps to the raw 0x payload).
 * Mirrors scheduler.ts::decodeUtf8Bytes exactly. */
function decodeUtf8Bytes(hex: `0x${string}`): string {
  const h = hex.startsWith('0x') ? hex.slice(2) : hex;
  if (h.length === 0) return '';
  const bytes = new Uint8Array(h.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(h.slice(i * 2, i * 2 + 2), 16);
  }
  return new TextDecoder().decode(bytes);
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      'content-type': 'application/json',
      'access-control-allow-origin': '*',
      'cache-control': 'public, max-age=60',
    },
  });
}

export default async function handler(req: Request): Promise<Response> {
  if (req.method === 'OPTIONS') {
    return new Response(null, {
      status: 204,
      headers: { 'Access-Control-Allow-Methods': 'GET, OPTIONS', 'Access-Control-Allow-Origin': '*' },
    });
  }
  if (req.method !== 'GET') {
    return json({ error: 'method not allowed' }, 405);
  }

  const url = new URL(req.url);
  const nameParam = (url.searchParams.get('name') ?? '').trim().toLowerCase();
  const idParam = (url.searchParams.get('id') ?? '').trim();
  const recentParam = (url.searchParams.get('recent') ?? '').trim();

  try {
    // --- Enumerate recent agents' lessons: ?recent=N ------------------------
    // Walks nextId down, reads name + lessons for the most-recently-registered
    // ids, and returns only those that actually have lessons recorded.
    if (recentParam) {
      const n = Number(recentParam);
      if (!Number.isInteger(n) || n <= 0) {
        return json({ error: 'recent must be a positive integer' }, 400);
      }
      const limit = Math.min(n, RECENT_CAP);
      const next = await nextId();
      if (next <= 1n) return json({ count: 0, lessons: [] });
      const hi = next - 1n;
      const lo = hi - BigInt(limit) + 1n;
      const start = lo > 1n ? lo : 1n;

      const out: { id: number; name: string; lessons: string }[] = [];
      // Most-recent first. Per-id reads are independently guarded so one
      // burned/odd id can't abort the whole scan.
      for (let tid = hi; tid >= start; tid--) {
        try {
          const name = await nameOfId(tid);
          if (!name) continue; // burned / released id
          const lessons = await lessonsOf(tid);
          if (!lessons) continue; // only return agents that have lessons
          out.push({ id: Number(tid), name, lessons });
        } catch {
          // skip a single bad id; keep harvesting the rest
        }
      }
      return json({ count: out.length, scanned: limit, lessons: out });
    }

    // --- Single agent by tokenId: ?id=<n> -----------------------------------
    if (idParam) {
      if (!/^[0-9]+$/.test(idParam)) {
        return json({ error: 'id must be a non-negative integer' }, 400);
      }
      const tokenId = BigInt(idParam);
      if (tokenId === 0n) return json({ error: 'id must be > 0' }, 400);
      const name = await nameOfId(tokenId);
      if (!name) return json({ error: 'no agent for that id' }, 404);
      const lessons = await lessonsOf(tokenId);
      return json({ id: Number(tokenId), name, lessons });
    }

    // --- Single agent by name: ?name=<subdomain> ----------------------------
    if (nameParam) {
      const tokenId = await idOfName(nameParam);
      if (tokenId === 0n) return json({ error: 'name not registered' }, 404);
      const lessons = await lessonsOf(tokenId);
      return json({ id: Number(tokenId), name: nameParam, lessons });
    }

    return json({ error: 'provide ?name=<subdomain>, ?id=<tokenId>, or ?recent=<N>' }, 400);
  } catch (e) {
    return json({ error: `lessons lookup failed: ${(e as Error).message}` }, 502);
  }
}
