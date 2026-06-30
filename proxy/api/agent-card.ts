// /api/agent-card — DYNAMIC ERC-8004 / A2A agent card for ANY localharness agent.
//
// Serves a `.well-known/agent.json`-shaped document assembled LIVE from on-chain
// state, so every colony agent (not just the flagship) advertises a verifiable
// identity + trust card without anyone hand-maintaining a file per agent. The
// static `web/.well-known/agent.json` covers the apex/flagship agent on a web
// deploy; THIS endpoint covers per-subdomain agents on a proxy deploy.
//
// Read-only, public, no auth, no $LH — it only READS the diamond (idOfName,
// ownerOf, tokenBoundAccount, metadata) + DiamondLoupe facetAddress to report
// which trust facets are actually cut. Cached at the CDN per name.
//
// Activation (see design/autonomous-business/colony/ERC-8004.md):
//   1. cd proxy && vercel --prod   (deploys this endpoint)
//   2. add a host-keyed rewrite in the WEB project's vercel.json so
//      `<name>.localharness.xyz/.well-known/agent.json` routes here:
//        { "source": "/.well-known/agent.json",
//          "has": [{ "type": "host", "value": "(?<name>.*)\\.localharness\\.xyz" }],
//          "destination": "https://proxy-tau-ten-15.vercel.app/api/agent-card?name=:name" }
//   Until then it is reachable directly at /api/agent-card?name=<name>.

import { ethCall, selector, keccak, isAllowedOrigin } from './_auth';
import { REGISTRY, CHAIN_ID, LH_TOKEN, FEE_TOKEN } from './_chain';
import { hexToBytes, bytesToHex } from '@noble/hashes/utils';

export const config = { runtime: 'edge' };

const EXPLORERS: Record<number, string> = {
  4217: 'https://explore.tempo.xyz',
  42431: 'https://moderato.tempo.xyz',
};

function cors(origin: string | null): Record<string, string> {
  // Agent cards are PUBLIC discovery data — allow any origin to fetch them.
  return {
    'Access-Control-Allow-Origin': origin && isAllowedOrigin(origin) ? origin : '*',
    'Access-Control-Allow-Methods': 'GET, OPTIONS',
    'Access-Control-Allow-Headers': 'content-type',
    Vary: 'Origin',
  };
}
function json(body: unknown, status: number, origin: string | null): Response {
  return new Response(JSON.stringify(body, null, 2), {
    status,
    headers: {
      'content-type': 'application/json; charset=utf-8',
      // Per-name CDN cache (public data); short so persona/price edits propagate.
      'cache-control': status === 200 ? 'public, max-age=60, s-maxage=300' : 'no-store',
      ...cors(origin),
    },
  });
}

// --- ABI helpers (mirror publish.ts / _auth) --------------------------------

/** ABI-encode a single `string` arg (offset 0x20 | length | utf8 padded). */
function encodeStringArg(value: string): string {
  const bytes = new TextEncoder().encode(value);
  const len = bytes.length;
  const padded = Math.ceil(len / 32) * 32;
  const buf = new Uint8Array(32 + 32 + padded);
  buf[31] = 0x20;
  let x = len;
  for (let i = 63; i >= 32 && x > 0; i--) {
    buf[i] = x & 0xff;
    x = Math.floor(x / 256);
  }
  buf.set(bytes, 64);
  return bytesToHex(buf);
}

const word = (n: bigint): string => n.toString(16).padStart(64, '0');
const keyOf = (label: string): string => bytesToHex(keccak(new TextEncoder().encode(label)));

async function idOfName(name: string): Promise<bigint> {
  try {
    return BigInt(await ethCall('0x' + selector('idOfName(string)') + encodeStringArg(name)));
  } catch {
    return 0n;
  }
}

/** Decode an `address` return word → lowercase 0x address, or null for zero. */
function addrFromWord(res: string): string | null {
  const h = res.replace(/^0x/, '');
  if (h.length < 64) return null;
  const addr = '0x' + h.slice(-40);
  return /^0x0+$/.test(addr) ? null : addr.toLowerCase();
}

async function addrCall(sig: string, arg: bigint): Promise<string | null> {
  try {
    return addrFromWord(await ethCall('0x' + selector(sig) + word(arg)));
  } catch {
    return null;
  }
}

/** `metadata(uint256,bytes32) -> bytes` decoded to raw bytes (empty if unset). */
async function metadataBytes(tokenId: bigint, label: string): Promise<Uint8Array> {
  try {
    const res = await ethCall(
      '0x' + selector('metadata(uint256,bytes32)') + word(tokenId) + keyOf(label),
    );
    const h = res.replace(/^0x/, '');
    if (h.length < 128) return new Uint8Array(0); // need at least offset + length words
    const len = Number(BigInt('0x' + h.slice(64, 128)));
    if (len <= 0) return new Uint8Array(0);
    const dataHex = h.slice(128, 128 + len * 2);
    if (dataHex.length < len * 2) return new Uint8Array(0);
    return hexToBytes(dataHex);
  } catch {
    return new Uint8Array(0);
  }
}

/** Is the facet exposing `sig` actually cut into the diamond? (DiamondLoupe.) */
async function facetCut(sig: string): Promise<{ live: boolean; facet: string | null }> {
  try {
    const sel = selector(sig); // 8 hex chars; bytes4 occupies the HIGH 4 bytes
    const res = await ethCall('0x' + selector('facetAddress(bytes4)') + sel.padEnd(64, '0'));
    const facet = addrFromWord(res);
    return { live: facet !== null, facet };
  } catch {
    return { live: false, facet: null };
  }
}

/** Format an 18-decimal wei BigInt as a trimmed decimal string. */
function weiToLh(wei: bigint): string {
  const D = 1_000_000_000_000_000_000n;
  const whole = wei / D;
  const frac = (wei % D).toString().padStart(18, '0').replace(/0+$/, '');
  return frac ? `${whole}.${frac}` : whole.toString();
}

const DEFAULT_ASK_PRICE_WEI = 10_000_000_000_000_000n; // 0.01 $LH (mirror x402.rs)

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');
  if (req.method === 'OPTIONS') return new Response(null, { status: 204, headers: cors(origin) });
  if (req.method !== 'GET') return json({ error: 'GET only' }, 405, origin);

  // Resolve the agent NAME: explicit ?name= wins, else the Host subdomain.
  const url = new URL(req.url);
  let name = (url.searchParams.get('name') ?? '').trim().toLowerCase();
  if (!name) {
    const host = (req.headers.get('host') ?? '').toLowerCase();
    const m = host.match(/^([a-z0-9-]+)\.localharness\.xyz$/);
    if (m && m[1] !== 'www') name = m[1];
  }
  if (!/^[a-z0-9-]{1,63}$/.test(name)) {
    return json({ error: 'supply ?name=<agent> (or call from <name>.localharness.xyz)' }, 400, origin);
  }

  const tokenId = await idOfName(name);
  if (tokenId === 0n) return json({ error: `"${name}" is not registered on-chain` }, 404, origin);

  const [controller, wallet, personaB, priceB, rep, val, x402] = await Promise.all([
    addrCall('ownerOf(uint256)', tokenId),
    addrCall('tokenBoundAccount(uint256)', tokenId),
    metadataBytes(tokenId, 'localharness.persona'),
    metadataBytes(tokenId, 'localharness.x402_price'),
    facetCut('attest(uint256,uint8,bytes32)'),
    facetCut('stakeValidation(bytes32,uint256,bool,uint256)'),
    facetCut('x402DomainSeparator()'),
  ]);

  const persona = new TextDecoder().decode(personaB).trim();
  const priceStr = new TextDecoder().decode(priceB).trim();
  let priceWei = DEFAULT_ASK_PRICE_WEI;
  try {
    const p = BigInt(priceStr || '0');
    if (p > 0n) priceWei = p;
  } catch {
    /* keep default */
  }

  const caip2 = `eip155:${CHAIN_ID}`;
  const registry = REGISTRY.toLowerCase();
  const agentRegistry = `${caip2}:${registry}`;
  const home = `https://${name}.localharness.xyz/`;
  const explorer = EXPLORERS[CHAIN_ID] ?? '';

  const supportedTrust: string[] = [];
  if (rep.live) supportedTrust.push('reputation');
  if (val.live) supportedTrust.push('crypto-economic');

  const trustModels: unknown[] = [];
  if (rep.live)
    trustModels.push({
      id: 'reputation',
      standard: 'ERC-8004 Reputation (localharness ReputationFacet)',
      status: 'live',
      registry: agentRegistry,
      facetImpl: rep.facet,
      methods: [
        'attest(uint256 subjectTokenId, uint8 rating, bytes32 workRef)',
        'reputationOf(uint256 tokenId) -> (uint256 count, uint256 ratingSum)',
        'attestationsOf(uint256 tokenId, uint256 start, uint256 limit)',
        'hasAttested(address attester, uint256 subjectTokenId, bytes32 workRef) -> bool',
      ],
    });
  if (val.live)
    trustModels.push({
      id: 'crypto-economic',
      standard: 'ERC-8004 Validation (localharness ValidationFacet)',
      status: 'live',
      registry: agentRegistry,
      facetImpl: val.facet,
      methods: [
        'stakeValidation(bytes32 workRef, uint256 subjectTokenId, bool valid, uint256 stakeWei) -> uint256 id',
        'challengeValidation(uint256 id)',
        'resolveValidation(uint256 id, bool validatorWins)',
        'getValidation(uint256 id)',
        'validationCount() -> uint256',
      ],
    });
  if (x402.live && wallet)
    trustModels.push({
      id: 'payment',
      standard: 'x402 exact ($LH, localharness X402Facet)',
      status: 'live',
      scheme: 'x402-exact',
      asset: { token: LH_TOKEN.toLowerCase(), symbol: 'LH', standard: 'TIP-20', chain: caip2 },
      payTo: wallet,
      price: weiToLh(priceWei),
      priceUnit: 'LH-per-call',
      priceNote: priceStr
        ? 'advertised on-chain'
        : 'x402_price unset; platform default (0.01 $LH) applies',
      description:
        "Pay this agent in $LH per call via x402 EIP-712 'exact' settlement; payment lands in the agent's ERC-6551 token-bound account.",
    });

  const card = {
    protocolVersion: '0.3.0',
    type: 'https://eips.ethereum.org/EIPS/eip-8004#registration-v1',
    name,
    description:
      persona ||
      `A localharness agent: an ERC-721 identity at ${name}.localharness.xyz with its own ERC-6551 wallet on Tempo.`,
    url: home,
    provider: { organization: 'localharness', url: 'https://localharness.xyz' },
    documentationUrl: 'https://localharness.xyz/llms.txt',
    capabilities: {
      streaming: true,
      pushNotifications: true,
      stateTransitionHistory: false,
      extendedAgentCard: false,
    },
    defaultInputModes: ['text/plain'],
    defaultOutputModes: ['text/plain'],
    services: [
      { type: 'web', url: home, description: 'Browser-resident agent UI / public face.' },
      {
        type: 'mcp',
        url: 'https://proxy-tau-ten-15.vercel.app/mcp',
        description: 'x402-gated MCP-over-HTTP for paid tool/agent invocation.',
      },
    ],
    registrations: [
      {
        agentRegistry,
        agentId: tokenId.toString(),
        ...(wallet ? { agentAddress: `${caip2}:${wallet}` } : {}),
      },
    ],
    supportedTrust,
    trustModels,
    'x-localharness': {
      platform: 'localharness',
      chain: { caip2, chainId: CHAIN_ID, registry, explorer },
      identity: {
        agentId: Number(tokenId),
        name,
        tokenStandard: 'ERC-721',
        registry,
        registryStandard: 'EIP-2535 Diamond',
        controller,
        wallet,
        walletStandard: 'ERC-6551',
        tokenUri: home,
      },
      tokens: {
        credit: { address: LH_TOKEN.toLowerCase(), symbol: 'LH', standard: 'TIP-20', currency: 'credits' },
        gasFeeToken: { address: FEE_TOKEN.toLowerCase() },
      },
      verification: {
        note: 'Every facet address is read live via the diamond DiamondLoupe (facetAddress); the diamond is the only durable handle.',
        docs: 'https://localharness.xyz/llms.txt',
      },
    },
    lastUpdated: new Date().toISOString().slice(0, 10),
  };

  return json(card, 200, origin);
}
