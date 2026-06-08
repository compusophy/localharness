// localharness scheduler worker — the tab-independent job firer (Node).
//
// This is the engine that makes scheduled agent jobs run WITHOUT a browser
// tab. The durable job registry lives ON-CHAIN in `ScheduleFacet` on the
// diamond (design/agent-scheduling.md). This function is the ONE worker (the
// `scheduler` role = the proxy's PROXY_METER_KEY): a Vercel Cron ticks it on a
// crontab, it reads the due set off-chain, runs each due job through the
// existing headless model-bridge (target persona + the job's task prompt as the
// user message, the SAME Gemini generateContent path `mcp.ts::runAgent` uses),
// and commits each run with `recordRun(jobId, expectedNextRun, spentWei)` —
// which atomically debits the job's escrowed budget and advances `nextRun`.
//
// TRUST ENVELOPE (design §2.6): the worker gains NO new authority. It can only
// fire owner-defined jobs and spend their PRE-COMMITTED budgets. `recordRun` is
// the worker's ONLY on-chain write; the facet itself enforces the budget hard
// stop (marks a job Exhausted when its budget/runs run out and refunds the
// remainder), the CAS double-fire guard, and skip-don't-pile-up scheduling. The
// worker just (a) finds due jobs, (b) runs the turn, (c) records the run.
//
// SAFETY (this runs autonomously + spends $LH — design §2.5 / §4.1):
//   * CRON_SECRET gate — only Vercel's cron (or a manual dogfood POST carrying
//     the secret) may invoke it; the public cannot trigger a spend.
//   * Idempotent / no-hot-loop — `expectedNextRun = job.nextRun` (CAS). A racing
//     firer (another overlapping tick, or an open tab) that committed first
//     advances `nextRun`, so our `recordRun` reverts `StaleNextRun` and we do
//     NOT double-bill. We treat that revert as a benign skip.
//   * Error -> STILL recordRun — if the Gemini call ERRORS, we still record the
//     run (advance `nextRun` + debit COST_WEI) so a broken job re-fires at most
//     once per interval and stays bounded by its budget. A perpetually-failing
//     job drains its budget and the facet marks it Exhausted — it can never get
//     stuck in a hot loop.
//   * Budget = hard stop — every run debits COST_WEI; the FACET decides when the
//     budget/runs are spent and exhausts+refunds. We just pass the cost.
//   * Bounded per tick — at most MAX_JOBS_PER_TICK jobs are processed; the rest
//     spill to the next tick (fair by scan order). recordRun receipts are
//     AWAITED (the accounting is never fire-and-forget).
//
// Reuses gemini.ts / mcp.ts setup verbatim: the diamond address, Tempo chain,
// RPC, the PROXY_METER_KEY wallet (now ALSO the scheduler role), persona
// resolution (`metadata(tokenId, keccak256("localharness.persona"))`), and the
// non-streaming Gemini generateContent pattern. GEMINI_API_KEY is in env.

import { keccak_256 } from '@noble/hashes/sha3';
import { bytesToHex } from '@noble/hashes/utils';
import {
  createPublicClient,
  createWalletClient,
  defineChain,
  http,
} from 'viem';
import { privateKeyToAccount } from 'viem/accounts';

// Node serverless (NOT edge): Vercel Cron invokes a serverless function, and we
// want a generous wall-clock budget for the per-job Gemini calls + awaited
// recordRun receipts. The default Node runtime is correct here.
export const config = { maxDuration: 300 };

// ---- constants (shared with gemini.ts / mcp.ts) ----------------------------

const TEMPO_RPC = 'https://rpc.moderato.tempo.xyz';
const REGISTRY = '0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c';
const GEMINI_BASE = 'https://generativelanguage.googleapis.com';
const CHAIN_ID = 42431;
// Mirrors mcp.ts ASK_MODEL / the headless `call` default. No per-job model
// selection in the MVP — every scheduled run uses the platform Gemini model.
const RUN_MODEL = process.env.MCP_ASK_MODEL ?? 'gemini-3.5-flash';

// $LH (18-decimal wei) debited per scheduled run, matching the proxy's
// COST_PER_REQUEST_WEI (gemini.ts) — 0.01 $LH default. Env-overridable.
const COST_WEI = ((): bigint => {
  try {
    return BigInt(process.env.COST_PER_REQUEST_WEI ?? '10000000000000000');
  } catch {
    return 10_000_000_000_000_000n;
  }
})();

// How many due jobs we read + process per cron tick. The chain may have more
// due than this; the rest fire on the next tick. Bounds sponsor gas + Gemini
// fan-out per invocation (design §4.3 "global worker budget").
const MAX_JOBS_PER_TICK = ((): number => {
  const n = Number(process.env.SCHEDULER_MAX_JOBS_PER_TICK ?? '20');
  return Number.isFinite(n) && n > 0 ? Math.min(Math.trunc(n), 100) : 20;
})();

// Status enum (LibScheduleStorage.Status). Only Active (0) jobs are fired.
const STATUS_ACTIVE = 0;

const TEMPO_CHAIN = defineChain({
  id: CHAIN_ID,
  name: 'Tempo Moderato',
  nativeCurrency: { name: 'Tempo', symbol: 'TEMPO', decimals: 18 },
  rpcUrls: { default: { http: [TEMPO_RPC] } },
});

// ScheduleFacet ABI (only the selectors the worker touches).
const SCHEDULE_ABI = [
  {
    name: 'jobsDue',
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
    name: 'getJob',
    type: 'function',
    stateMutability: 'view',
    inputs: [{ name: 'id', type: 'uint256' }],
    // LibScheduleStorage.Job — field ORDER must match the struct exactly.
    outputs: [
      {
        name: 'job',
        type: 'tuple',
        components: [
          { name: 'owner', type: 'address' },
          { name: 'interval', type: 'uint64' },
          { name: 'status', type: 'uint8' },
          { name: 'nextRun', type: 'uint64' },
          { name: 'budgetWei', type: 'uint128' },
          { name: 'runsLeft', type: 'uint32' },
          { name: 'targetId', type: 'uint256' },
        ],
      },
    ],
  },
  {
    name: 'taskOf',
    type: 'function',
    stateMutability: 'view',
    inputs: [{ name: 'id', type: 'uint256' }],
    outputs: [{ name: 'task', type: 'bytes' }],
  },
  {
    name: 'recordRun',
    type: 'function',
    stateMutability: 'nonpayable',
    inputs: [
      { name: 'id', type: 'uint256' },
      { name: 'expectedNextRun', type: 'uint64' },
      { name: 'spentWei', type: 'uint128' },
    ],
    outputs: [{ name: 'newNextRun', type: 'uint64' }],
  },
] as const;

// metadata(uint256,bytes32) -> bytes — persona lookup (shared with mcp.ts).
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

// nameOfId(uint256) -> string — for the default persona text + logging only.
const NAME_ABI = [
  {
    name: 'nameOfId',
    type: 'function',
    stateMutability: 'view',
    inputs: [{ name: 'id', type: 'uint256' }],
    outputs: [{ name: '', type: 'string' }],
  },
] as const;

const PERSONA_KEY = ('0x' +
  bytesToHex(keccak_256(new TextEncoder().encode('localharness.persona')))) as `0x${string}`;

interface Job {
  owner: string;
  interval: bigint;
  status: number;
  nextRun: bigint;
  budgetWei: bigint;
  runsLeft: number;
  targetId: bigint;
}

// ---- on-chain reads (viem readContract; same RPC/diamond as gemini.ts) ------

function publicClient() {
  return createPublicClient({ chain: TEMPO_CHAIN, transport: http(TEMPO_RPC) });
}

/** `jobsDue(startAfter, limit)` — up to `limit` Active+due job ids. */
async function jobsDue(
  startAfter: bigint,
  limit: bigint,
): Promise<{ ids: bigint[]; nextCursor: bigint }> {
  const [ids, nextCursor] = (await publicClient().readContract({
    address: REGISTRY as `0x${string}`,
    abi: SCHEDULE_ABI,
    functionName: 'jobsDue',
    args: [startAfter, limit],
  })) as readonly [readonly bigint[], bigint];
  return { ids: [...ids], nextCursor };
}

async function getJob(id: bigint): Promise<Job> {
  const j = (await publicClient().readContract({
    address: REGISTRY as `0x${string}`,
    abi: SCHEDULE_ABI,
    functionName: 'getJob',
    args: [id],
  })) as {
    owner: string;
    interval: bigint;
    status: number;
    nextRun: bigint;
    budgetWei: bigint;
    runsLeft: number;
    targetId: bigint;
  };
  return {
    owner: j.owner,
    interval: j.interval,
    status: Number(j.status),
    nextRun: j.nextRun,
    budgetWei: j.budgetWei,
    runsLeft: Number(j.runsLeft),
    targetId: j.targetId,
  };
}

async function taskOf(id: bigint): Promise<string> {
  const raw = (await publicClient().readContract({
    address: REGISTRY as `0x${string}`,
    abi: SCHEDULE_ABI,
    functionName: 'taskOf',
    args: [id],
  })) as `0x${string}`;
  return decodeUtf8Bytes(raw);
}

/** persona text for a tokenId (the job's targetId IS the token id). */
async function personaOf(tokenId: bigint): Promise<string | null> {
  const raw = (await publicClient().readContract({
    address: REGISTRY as `0x${string}`,
    abi: METADATA_ABI,
    functionName: 'metadata',
    args: [tokenId, PERSONA_KEY],
  })) as `0x${string}`;
  const text = decodeUtf8Bytes(raw).trim();
  return text.length ? text : null;
}

async function nameOfId(tokenId: bigint): Promise<string> {
  try {
    const name = (await publicClient().readContract({
      address: REGISTRY as `0x${string}`,
      abi: NAME_ABI,
      functionName: 'nameOfId',
      args: [tokenId],
    })) as string;
    return name || `#${tokenId}`;
  } catch {
    return `#${tokenId}`;
  }
}

/** Decode an ABI-`bytes` 0x word (viem already unwraps to the raw 0x payload). */
function decodeUtf8Bytes(hex: `0x${string}`): string {
  const h = hex.startsWith('0x') ? hex.slice(2) : hex;
  if (h.length === 0) return '';
  const bytes = new Uint8Array(h.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(h.slice(i * 2, i * 2 + 2), 16);
  }
  return new TextDecoder().decode(bytes);
}

// ---- the agent turn (same path as mcp.ts::runAgent) -------------------------

function defaultPersona(name: string): string {
  return (
    `You are ${name}, an autonomous agent on the localharness platform ` +
    `(a self-sovereign, browser-resident agent network on the Tempo testnet). ` +
    `You are reachable at ${name}.localharness.xyz. This is a SCHEDULED run — ` +
    `carry out the task below and report concisely, speaking as ${name}.`
  );
}

/** Non-streaming Gemini generateContent with the platform key → reply text. */
async function runAgent(persona: string, task: string): Promise<string> {
  const apiKey = process.env.GEMINI_API_KEY;
  if (!apiKey) throw new Error('proxy misconfigured: missing GEMINI_API_KEY');
  const url = `${GEMINI_BASE}/v1beta/models/${RUN_MODEL}:generateContent`;
  const body = {
    systemInstruction: { parts: [{ text: persona }] },
    contents: [{ role: 'user', parts: [{ text: task }] }],
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

// ---- recordRun (the worker's ONLY on-chain write; scheduler role) -----------

let walletSingleton: ReturnType<typeof createWalletClient> | null = null;
function schedulerWallet() {
  if (walletSingleton) return walletSingleton;
  const pk = process.env.PROXY_METER_KEY;
  if (!pk) throw new Error('missing PROXY_METER_KEY (scheduler role account)');
  const account = privateKeyToAccount(
    (pk.startsWith('0x') ? pk : `0x${pk}`) as `0x${string}`,
  );
  walletSingleton = createWalletClient({
    account,
    chain: TEMPO_CHAIN,
    transport: http(TEMPO_RPC),
  });
  return walletSingleton;
}

/**
 * Commit one run: `recordRun(id, expectedNextRun, spentWei)`, signed by the
 * scheduler-role key (PROXY_METER_KEY). The facet atomically debits the job's
 * escrowed budget by `spentWei`, advances `nextRun` (skip-don't-pile-up), and —
 * when the budget/runs are spent — marks the job Exhausted + refunds the owner.
 *
 * AWAITS the receipt (the accounting is never fire-and-forget). `expectedNextRun`
 * is the job's CURRENT `nextRun` (the CAS key): a racing firer that already
 * advanced it makes this revert `StaleNextRun` — which is BENIGN (the run was
 * already recorded by the winner; we must NOT double-bill), so the caller treats
 * a revert as a skip rather than an error.
 *
 * Returns 'recorded' on success, 'stale' on a CAS/contract revert (benign skip).
 */
async function recordRun(
  id: bigint,
  expectedNextRun: bigint,
  spentWei: bigint,
): Promise<'recorded' | 'stale'> {
  const wallet = schedulerWallet();
  let hash: `0x${string}`;
  try {
    hash = await wallet.writeContract({
      address: REGISTRY as `0x${string}`,
      abi: SCHEDULE_ABI,
      functionName: 'recordRun',
      args: [id, expectedNextRun, spentWei],
      account: wallet.account!,
      chain: TEMPO_CHAIN,
    });
  } catch (e) {
    // A revert at submission time (CAS StaleNextRun / NotDue / JobNotActive —
    // another firer already committed this run, or the job changed state). This
    // is the idempotency guard doing its job: skip, never double-bill.
    console.warn(`[scheduler] recordRun(${id}) reverted on submit: ${(e as Error).message}`);
    return 'stale';
  }
  const pub = publicClient();
  const { status } = await pub.waitForTransactionReceipt({
    hash,
    timeout: 30_000,
    pollingInterval: 500,
  });
  if (status === 'reverted') {
    console.warn(`[scheduler] recordRun(${id}) reverted on-chain (tx ${hash}) — treated as a benign skip`);
    return 'stale';
  }
  return 'recorded';
}

// ---- one job ----------------------------------------------------------------

interface JobResult {
  id: string;
  outcome: 'recorded' | 'skipped' | 'stale';
  ran: 'ok' | 'error' | 'n/a';
  note?: string;
}

/**
 * Process ONE due job: read its full record + task, (re)confirm it's Active and
 * due, run the agent turn under its target's persona, then ALWAYS recordRun
 * (advancing nextRun + debiting COST_WEI) — even if the Gemini call errored, so
 * a broken job can never hot-loop and stays bounded by its budget.
 */
async function processJob(id: bigint): Promise<JobResult> {
  const idStr = id.toString();
  const job = await getJob(id);

  // Re-validate against the LATEST state (jobsDue read may be a tick stale, or a
  // racing firer/owner action changed it). Skip without writing if not eligible.
  if (job.owner === '0x0000000000000000000000000000000000000000') {
    return { id: idStr, outcome: 'skipped', ran: 'n/a', note: 'unknown job' };
  }
  if (job.status !== STATUS_ACTIVE) {
    return { id: idStr, outcome: 'skipped', ran: 'n/a', note: `status=${job.status} (not Active)` };
  }
  const nowTs = BigInt(Math.floor(Date.now() / 1000));
  if (job.nextRun > nowTs) {
    return { id: idStr, outcome: 'skipped', ran: 'n/a', note: 'not due yet' };
  }
  // The facet rejects spentWei > budgetWei. If the remaining budget can't cover
  // a run, the job will exhaust on the next eligible record anyway — but a
  // budget below COST_WEI means we can't even record this run. Skip; the facet
  // will exhaust it when a covered run lands (or the owner cancels for a refund).
  if (job.budgetWei < COST_WEI) {
    return { id: idStr, outcome: 'skipped', ran: 'n/a', note: 'budget below per-run cost' };
  }

  const expectedNextRun = job.nextRun;
  const name = await nameOfId(job.targetId);

  // Build + run the turn. Persona resolution + Gemini errors must NOT abort the
  // accounting: capture the outcome, then record the run regardless.
  let ran: 'ok' | 'error' = 'ok';
  let runNote = '';
  try {
    const persona = (await personaOf(job.targetId)) ?? defaultPersona(name);
    const task = (await taskOf(id)).trim();
    if (!task) {
      // An empty task = "run under the persona's standing instruction"
      // (design §3.4 sentinel). Use a minimal default tick prompt.
      const out = await runAgent(persona, 'Perform your scheduled task and report concisely.');
      runNote = out;
    } else {
      const out = await runAgent(persona, task);
      runNote = out;
    }
    // MVP output sink = the Vercel log. The reply is recorded here so a scheduled
    // run is observable in the function logs.
    // TODO: richer output routing — persist the transcript on-chain / in a
    // store, or chain the reply to another agent (recursion / "ping-pong",
    // design §4–5.2), instead of only logging.
    console.log(
      `[scheduler] job ${idStr} target ${name} (#${job.targetId}) reply: ${runNote.slice(0, 800)}`,
    );
  } catch (e) {
    ran = 'error';
    runNote = (e as Error).message;
    // CRITICAL: still record the run below. A failing job advances its clock and
    // debits a cost so it re-fires at most once per interval and drains its
    // budget — never a hot loop.
    console.error(`[scheduler] job ${idStr} target ${name} run ERROR (will still recordRun): ${runNote}`);
  }

  const outcome = await recordRun(id, expectedNextRun, COST_WEI);
  return { id: idStr, outcome, ran, note: ran === 'error' ? runNote.slice(0, 200) : undefined };
}

// ---- handler ----------------------------------------------------------------

function unauthorized(): Response {
  return new Response(JSON.stringify({ error: 'unauthorized' }), {
    status: 401,
    headers: { 'content-type': 'application/json' },
  });
}

export default async function handler(req: Request): Promise<Response> {
  // CRON_SECRET gate — Vercel's cron sends `Authorization: Bearer ${CRON_SECRET}`.
  // The same header gates a manual dogfood POST. The public can NEVER trigger a
  // spend. Vercel Cron uses GET; allow GET (cron) + POST (manual dogfood).
  if (req.method !== 'GET' && req.method !== 'POST') {
    return new Response(JSON.stringify({ error: 'method not allowed' }), {
      status: 405,
      headers: { 'content-type': 'application/json' },
    });
  }
  const secret = process.env.CRON_SECRET;
  if (!secret) {
    // Fail closed: with no secret configured, refuse to run rather than expose
    // an open, money-spending endpoint.
    return new Response(
      JSON.stringify({ error: 'scheduler misconfigured: missing CRON_SECRET' }),
      { status: 500, headers: { 'content-type': 'application/json' } },
    );
  }
  const auth = req.headers.get('authorization') ?? '';
  if (auth !== `Bearer ${secret}`) {
    return unauthorized();
  }

  const tickStart = Date.now();
  const results: JobResult[] = [];
  let scanned = 0;
  try {
    // Read up to MAX_JOBS_PER_TICK due jobs (one page is enough for the MVP —
    // overflow spills to the next tick). startAfter=0 scans from the start of
    // the enumerable index.
    const { ids } = await jobsDue(0n, BigInt(MAX_JOBS_PER_TICK));
    scanned = ids.length;

    // Process SEQUENTIALLY so recordRun txs from the single scheduler account
    // don't collide on the nonce (they share one EOA). Each run's accounting is
    // awaited before the next starts — bounded + ordered.
    for (const id of ids) {
      try {
        results.push(await processJob(id));
      } catch (e) {
        // A read failure for one job must not abort the whole tick.
        console.error(`[scheduler] job ${id} unexpected error: ${(e as Error).message}`);
        results.push({ id: id.toString(), outcome: 'skipped', ran: 'n/a', note: (e as Error).message });
      }
    }
  } catch (e) {
    return new Response(
      JSON.stringify({ error: 'scheduler tick failed: ' + (e as Error).message }),
      { status: 502, headers: { 'content-type': 'application/json' } },
    );
  }

  const recorded = results.filter((r) => r.outcome === 'recorded').length;
  const stale = results.filter((r) => r.outcome === 'stale').length;
  const skipped = results.filter((r) => r.outcome === 'skipped').length;
  const errored = results.filter((r) => r.ran === 'error').length;
  const summary = {
    ok: true,
    scanned,
    recorded,
    stale,
    skipped,
    errored,
    durationMs: Date.now() - tickStart,
    jobs: results,
  };
  console.log(
    `[scheduler] tick: scanned=${scanned} recorded=${recorded} stale=${stale} skipped=${skipped} errored=${errored} in ${summary.durationMs}ms`,
  );
  return new Response(JSON.stringify(summary), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}
