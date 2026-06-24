// localharness scheduler worker — the tab-independent job firer (Node).
//
// This is the engine that makes scheduled agent jobs run WITHOUT a browser
// tab. The durable job registry lives ON-CHAIN in `ScheduleFacet` on the
// diamond (design/agent-scheduling.md). This function is the ONE worker (the
// `scheduler` role = the proxy's PROXY_METER_KEY): a Vercel Cron ticks it on a
// crontab, it reads the due set off-chain, runs each due job as a BOUNDED AGENT
// PING-PONG loop under its target's on-chain persona (the agent can `call_agent`
// other localharness agents during the run — multi-agent orchestration with no
// tab open), and commits each run with `recordRun(jobId, expectedNextRun,
// spentWei)` — which atomically debits the job's escrowed budget and advances
// `nextRun`. AGENT PING-PONG (replaces the old single Gemini turn): every
// generateContent (the agent's turns + each sub-agent turn) costs COST_WEI and
// is metered against the job's per-run budget; the budget is the HARD ceiling on
// the whole run (a runaway loop just drains the escrow → the facet exhausts the
// job). See `runPingPong` for the loop + the budget gate. CROSS-TICK recursion is
// now SHIPPED: a scheduled agent can call `schedule_task`, which the worker relays
// to the scheduler-role `scheduleChildJob(parentJobId, …)` facet fn — the child's
// budget is DRAWN FROM the running job's remaining escrow (facet-enforced draw +
// MAX_DEPTH + root cap), so an agent can spawn bounded follow-up work autonomously.
//
// PER-TICK SPEND CAPS (#1): on top of every job's own budget, the worker bounds
// the TOTAL real (Gemini) $LH it commits across one cron tick — GLOBAL_TICK_CAP_WEI
// globally and PER_OWNER_TICK_CAP_WEI per owner. A job that would breach either cap
// SPILLS to the next tick (its nextRun is left unadvanced; logged, never dropped).
// So even with free/generous per-job budgets and many due jobs, the platform's
// upstream API spend per tick is hard-capped.
//
// PER-TICK WALL-CLOCK BUDGET (TICK_SOFT_BUDGET_MS): the tick also self-limits
// its OWN runtime so the Edge platform never kills it mid-batch (that kill
// SILENTLY skipped every job after a heavy one — no recordRun, no log, no
// summary). Each batch job gets a fair-share model deadline (a heavy run is
// truncated + recorded, never starves the rest), and any due job the tick
// cannot reach is reported as `deferred` (per-job result + log line; nextRun
// unchanged, re-fires next tick). A due job either RUNS or its skip is VISIBLE.
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
//   * Budget = hard stop — every generateContent in the loop debits COST_WEI; we
//     STOP before any call the remaining budget can't cover, then pass
//     `calls * COST_WEI` (capped to budget) to recordRun ONCE. The FACET decides
//     when the budget/runs are spent and exhausts+refunds. The per-job budget
//     thus bounds the ENTIRE ping-pong run, not one turn.
//   * Bounded per tick — at most MAX_JOBS_PER_TICK jobs are processed; the rest
//     spill to the next tick (fair by scan order). A ping-pong job is HEAVIER
//     than the old single-turn run (a bounded loop, not one call), so the
//     per-tick default is LOWER (2) — one heavy job must not starve the others'
//     wall-clock. recordRun receipts are AWAITED (accounting never fire-and-forget).
//
// Reuses gemini.ts / mcp.ts setup verbatim: the diamond address, Tempo chain,
// RPC, the PROXY_METER_KEY wallet (now ALSO the scheduler role), persona
// resolution (`metadata(tokenId, keccak256("localharness.persona"))`), and the
// non-streaming Gemini generateContent pattern. GEMINI_API_KEY is in env.

import { keccak_256 } from '@noble/hashes/sha3';
import { bytesToHex } from '@noble/hashes/utils';
import {
  parsePushSubs,
  dedupeSubs,
  sendWebPushAll,
  type PushSubscriptionJson,
} from './_webpush';
import {
  createPublicClient,
  createWalletClient,
  defineChain,
  http,
} from 'viem';
import { privateKeyToAccount } from 'viem/accounts';
import { SlidingWindow } from './_ratelimit';

// Per-IP cap for the public `?poke` heartbeat (best-effort, per-isolate); the
// only abuse is read-spam — a due poke is already CAS-bounded to one run/slot.
const pokeWindow = new SlidingWindow(60, 60_000);

// Edge runtime — matches gemini.ts / mcp.ts, which use the SAME Web
// `Request`->`Response` handler shape. That shape runs on Edge, NOT on Vercel's
// Node runtime (a Node function expects `(req, res)`, so a Web handler there
// 500s with FUNCTION_INVOCATION_FAILED). Edge's ~25s wall-clock caps the
// per-tick batch (see MAX_JOBS_PER_TICK); leftover due jobs spill to the next
// cron tick. (For a future high-volume Node 300s budget, rewrite the handler to
// the `(req, res)` Node signature.)
export const config = { runtime: 'edge' };

// ---- constants (shared with gemini.ts / mcp.ts) ----------------------------

import { TEMPO_RPC, REGISTRY, CHAIN_ID } from './_chain';
const GEMINI_BASE = 'https://generativelanguage.googleapis.com';
// Mirrors mcp.ts ASK_MODEL / the headless `call` default. No per-job model
// selection in the MVP — every scheduled run uses the platform Gemini model.
const RUN_MODEL = process.env.MCP_ASK_MODEL ?? 'gemini-3.5-flash';

// $LH (18-decimal wei) debited per scheduled run, matching the proxy's
// COST_PER_REQUEST_WEI (gemini.ts / _prices.ts) — the platform FLOOR price,
// 1 $LH default, env-overridable via COST_PER_REQUEST_WEI. Single source of
// truth: `_prices.ts`.
//
// ⚠️ MAINNET REQUIREMENT — PRICING. `COST_WEI` is the $LH the worker debits PER
// generateContent (one agent turn OR one sub-agent turn). It is what the owner
// actually PAYS for the platform's real upstream API spend. On mainnet it MUST be
// set (via COST_PER_REQUEST_WEI) to AT LEAST the real per-model-call cost — i.e.
// `$LH-priced(model API call) >= platform's USD cost for that call` — or the
// platform subsidizes every scheduled run out of pocket (bill-shock on the
// PLATFORM side; the per-tick caps below bound it but a too-low COST_WEI still
// means each call is sold below cost).
//
// ⚠️ FOLLOW-ON — PER-PROVIDER PRICING. COST_WEI is currently UNIFORM per call:
// every generateContent (the agent's own turns AND every sub-agent turn) costs
// the same. That is correct ONLY while all calls hit the SAME model — which they
// do today: sub-agents (`call_agent`) route to the platform Gemini model
// (RUN_MODEL), same as the parent. If sub-agents ever route to a DIFFERENT model
// (a Claude sub-call costs materially more than a Gemini one), a flat per-call
// COST_WEI under-charges the expensive calls and over-charges the cheap ones —
// switch to PER-PROVIDER / per-model pricing (charge each generateContent by the
// model it actually used) before enabling cross-model sub-agents. Tracked as a
// follow-on; not needed while sub-agents stay on the Gemini model.
import { COST_PER_REQUEST_WEI } from './_prices';
const COST_WEI = COST_PER_REQUEST_WEI;

// ---- per-TICK spend caps (#1 — the strongest bill-shock fix) ----------------
//
// The per-JOB budget (budgetWei) bounds ONE job's run. These two caps bound the
// WHOLE TICK across ALL jobs/owners, so the worker's real upstream (Gemini) cost
// per cron invocation is HARD-bounded regardless of how many jobs are due or how
// generous individual budgets are — even if $LH itself is free to the owner, the
// platform's API spend per tick cannot exceed GLOBAL_TICK_CAP_WEI, and no single
// owner's jobs can consume more than PER_OWNER_TICK_CAP_WEI of that in one tick.
//
// Enforcement (processJob + runPingPong): we track a running tick total and a
// per-owner running total. BEFORE running a job — and BEFORE EACH metered call
// inside runPingPong — we check that the projected spend (running total + this
// call's COST_WEI) stays under both caps. If a call would breach either cap we
// STOP the job there; its `nextRun` is NOT advanced, so it SPILLS to the next
// tick (logged, never silently dropped). These are an ADDITIONAL ceiling on top
// of every existing bound (per-job budget, MAX_PINGPONG_ROUNDS, MAX_JOBS_PER_TICK).

// Total $LH the worker may spend across ALL jobs in a SINGLE tick (default 2 $LH).
const GLOBAL_TICK_CAP_WEI = ((): bigint => {
  try {
    return BigInt(process.env.SCHEDULER_GLOBAL_TICK_CAP_WEI ?? '2000000000000000000');
  } catch {
    return 2_000_000_000_000_000_000n;
  }
})();

// Total $LH the worker may spend on ONE OWNER's jobs in a SINGLE tick (default
// 0.5 $LH). Stops one owner with many/large jobs from monopolizing the global cap
// in a tick (fairness) AND bounds a single owner's per-tick bill.
const PER_OWNER_TICK_CAP_WEI = ((): bigint => {
  try {
    return BigInt(process.env.SCHEDULER_PER_OWNER_TICK_CAP_WEI ?? '500000000000000000');
  } catch {
    return 500_000_000_000_000_000n;
  }
})();

// How many due jobs we read + process per cron tick. The chain may have more
// due than this; the rest fire on the next tick. Bounds sponsor gas + Gemini
// fan-out per invocation (design §4.3 "global worker budget").
const MAX_JOBS_PER_TICK = ((): number => {
  // Edge ~25s budget. A ping-pong job is now HEAVIER than the old single-turn
  // run: it's a BOUNDED tool loop (up to MAX_PINGPONG_ROUNDS generateContent
  // calls + one sub-agent generateContent per call_agent) followed by an awaited
  // recordRun receipt. A single heavy job can approach the whole wall-clock, so
  // the per-tick batch defaults LOWER than before (2, was 4) — leftover due jobs
  // spill to the next tick. (Raise via env on Pro/Node with a bigger budget.)
  const n = Number(process.env.SCHEDULER_MAX_JOBS_PER_TICK ?? '2');
  return Number.isFinite(n) && n > 0 ? Math.min(Math.trunc(n), 100) : 2;
})();

// Max rounds of the agent's OWN tool loop per scheduled run (the agent's turns,
// not counting sub-agent turns). Kept small so the whole ping-pong run fits
// inside Edge's ~25s wall-clock: each round is one generateContent (~3-5s), and
// a call_agent within a round adds one more sub-agent generateContent. 4 rounds
// ⇒ at most ~8 generateContent calls worst-case ⇒ comfortably under the budget.
// The PER-JOB $LH budget is the other (and harder) ceiling — see the loop.
const MAX_PINGPONG_ROUNDS = ((): number => {
  const n = Number(process.env.SCHEDULER_MAX_PINGPONG_ROUNDS ?? '4');
  return Number.isFinite(n) && n > 0 ? Math.min(Math.trunc(n), 16) : 4;
})();

// ---- per-tick WALL-CLOCK budget (the silent-fire-skip fix) -------------------
//
// Edge kills the function at ~25-30s. Before this guard, ONE heavy ping-pong
// job (8 metered calls ≈ 24-40s of model time) could eat the entire tick: the
// platform killed the worker MID-BATCH, so every job after it in the batch was
// skipped with NO recordRun, NO log line, and NO tick summary — the fleet's
// "goal job silently never fired" repro (job #31: due, never run, runsLeft
// intact, while job #28 fired 8-call runs every minute). Two fixes hang off
// this soft budget:
//   * FAIR-SHARE MODEL DEADLINES — batch job i may run its model loop until
//     the (i+1)/batchSize fraction of the budget (cumulative, so a quick early
//     job rolls unused time forward). A heavy early job is TRUNCATED — its
//     partial work is still recorded + noted 'wall-clock capped' — instead of
//     starving every job behind it.
//   * OBSERVABLE DEFERRALS — any due job the tick cannot reach (batch cap or
//     budget already gone) gets a `deferred` result row + a log line instead
//     of vanishing with a killed function. Its nextRun is untouched; it
//     re-fires next tick.
const TICK_SOFT_BUDGET_MS = ((): number => {
  const n = Number(process.env.SCHEDULER_TICK_SOFT_BUDGET_MS ?? '20000');
  return Number.isFinite(n) && n >= 5000 ? Math.min(Math.trunc(n), 290_000) : 20_000;
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
  // scheduleChildJob — SCHEDULER-ROLE-ONLY cross-tick recursion. A scheduled
  // agent (in runPingPong) spawns a FOLLOW-UP job whose budget is DRAWN FROM the
  // currently-running PARENT job's remaining escrow. The facet enforces the draw
  // (reverts InsufficientParentBudget), MAX_DEPTH (reverts MaxDepthExceeded), and
  // the root spend cap. Returns the new child job id. Signature must match the
  // sibling facet addition EXACTLY.
  {
    name: 'scheduleChildJob',
    type: 'function',
    stateMutability: 'nonpayable',
    inputs: [
      { name: 'parentJobId', type: 'uint256' },
      { name: 'targetId', type: 'uint256' },
      { name: 'task', type: 'bytes' },
      { name: 'interval', type: 'uint64' },
      { name: 'budgetWei', type: 'uint128' },
      { name: 'maxRuns', type: 'uint32' },
    ],
    outputs: [{ name: 'childJobId', type: 'uint256' }],
  },
  // completeJob — SCHEDULER-ROLE-ONLY goal completion (the /goal ralph loop).
  // When a run's agent calls its `finish_goal` tool, the worker relays it here:
  // the job goes terminal and the FULL remaining escrow refunds to the owner.
  // Called AFTER recordRun (so this run's calls are debited first; the refund
  // is the post-debit remainder). Signature must match the facet EXACTLY.
  {
    name: 'completeJob',
    type: 'function',
    stateMutability: 'nonpayable',
    inputs: [{ name: 'id', type: 'uint256' }],
    outputs: [],
  },
] as const;

// TitheFacet ABI — only `collectTithe(account)`, the PERMISSIONLESS revenue→
// treasury pull the scheduler may trigger (TitheFacet.sol). It reads ONLY
// `account`'s OWN stored `(guildId, bps)` and pulls
// `min(bps·balanceOf(account)/10000, allowance(account, diamond))` into the
// account's own pre-consented guild — the caller can neither redirect (guild/bps
// come from the account's config, never the caller) nor over-pull (capped by the
// account's own `approve` ceiling). So the scheduler key signs it with ZERO new
// authority: it is exactly the "anyone may trigger" path the facet was built for.
// Returns the amount pulled (0 reverts NothingToCollect on-chain).
const TITHE_ABI = [
  {
    name: 'collectTithe',
    type: 'function',
    stateMutability: 'nonpayable',
    inputs: [{ name: 'account', type: 'address' }],
    outputs: [{ name: 'amount', type: 'uint256' }],
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

// mainOf(address) -> uint256 — the owner's MAIN identity tokenId (0 = none).
// The browser app publishes its Web Push subscription under the MAIN slot
// (fallback: the name's own id), mirroring the Gemini-key-sync slot rule.
const MAIN_OF_ABI = [
  {
    name: 'mainOf',
    type: 'function',
    stateMutability: 'view',
    inputs: [{ name: 'owner', type: 'address' }],
    outputs: [{ name: '', type: 'uint256' }],
  },
] as const;

// idOfName(string) -> uint256 — resolves a `call_agent` target name to its token
// id (0 = unregistered). Mirrors mcp.ts::idOfName / registry::id_of_name.
const ID_OF_NAME_ABI = [
  {
    name: 'idOfName',
    type: 'function',
    stateMutability: 'view',
    inputs: [{ name: 'name', type: 'string' }],
    outputs: [{ name: '', type: 'uint256' }],
  },
] as const;

const PERSONA_KEY = ('0x' +
  bytesToHex(keccak_256(new TextEncoder().encode('localharness.persona')))) as `0x${string}`;

// Web Push subscription slot — written by the browser app's admin
// "enable notifications" flow (src/app/notifications.rs), v1 plaintext.
const PUSH_SUB_KEY = ('0x' +
  bytesToHex(keccak_256(new TextEncoder().encode('localharness.push_sub')))) as `0x${string}`;

// Self-recorded lessons slot — written by the browser app's record_lesson
// tool (src/app/chat/tools/misc.rs; merge bounds in src/lessons.rs).
// keccak256("localharness.lessons"), precomputed + inlined; pinned by the
// Rust test `lessons_key_distinct_from_other_metadata_keys`.
const LESSONS_KEY =
  '0x08564cae936ec460c48a23578c7df5665bad18fe42f3c5dbde517ad67a9d9c89' as `0x${string}`;

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

/** Self-recorded lessons blob for a tokenId. BEST-EFFORT: a read failure
 * degrades to no lessons rather than failing the run. */
async function lessonsOf(tokenId: bigint): Promise<string | null> {
  try {
    const raw = (await publicClient().readContract({
      address: REGISTRY as `0x${string}`,
      abi: METADATA_ABI,
      functionName: 'metadata',
      args: [tokenId, LESSONS_KEY],
    })) as `0x${string}`;
    const text = decodeUtf8Bytes(raw).trim();
    return text.length ? text : null;
  } catch {
    return null;
  }
}

/** Fold a target's self-recorded lessons into its persona — the SAME
 * "=== Lessons (self-recorded) ===" section every other surface appends
 * (browser session.rs, CLI call.rs), so a scheduled run embodies the same
 * learned behavior. No-op when there are no lessons. */
function withLessons(persona: string, lessons: string | null): string {
  if (!lessons) return persona;
  return persona + '\n\n=== Lessons (self-recorded) ===\n' + lessons;
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

/** `idOfName(name)` — the token id of a registered name; 0n if unregistered. */
async function idOfName(name: string): Promise<bigint> {
  return (await publicClient().readContract({
    address: REGISTRY as `0x${string}`,
    abi: ID_OF_NAME_ABI,
    functionName: 'idOfName',
    args: [name],
  })) as bigint;
}

// pushSubOf(address) -> bytes — the address-keyed PushFacet slot (device
// self-registration via the header bell).
const PUSH_SUB_OF_ABI = [
  {
    name: 'pushSubOf',
    type: 'function',
    stateMutability: 'view',
    inputs: [{ name: 'who', type: 'address' }],
    outputs: [{ name: '', type: 'bytes' }],
  },
] as const;

/**
 * ALL the job owner's published Web Push subscriptions ([] when none) — the
 * UNION of the MAIN-tokenId metadata slot (falling back to the job's own
 * targetId) and the address-keyed PushFacet slot; each slot holds a JSON
 * array of per-device entries (src/registry/push.rs::merge_push_sub), so a
 * phone AND a desktop both get buzzed. Best-effort: any failure contributes
 * nothing rather than failing the run.
 */
async function pushSubsOf(
  owner: string,
  fallbackTokenId: bigint,
): Promise<PushSubscriptionJson[]> {
  const out: PushSubscriptionJson[] = [];
  try {
    let tokenId = (await publicClient().readContract({
      address: REGISTRY as `0x${string}`,
      abi: MAIN_OF_ABI,
      functionName: 'mainOf',
      args: [owner as `0x${string}`],
    })) as bigint;
    if (tokenId === 0n) tokenId = fallbackTokenId;
    const raw = (await publicClient().readContract({
      address: REGISTRY as `0x${string}`,
      abi: METADATA_ABI,
      functionName: 'metadata',
      args: [tokenId, PUSH_SUB_KEY],
    })) as `0x${string}`;
    out.push(...parsePushSubs(decodeUtf8Bytes(raw).trim()));
  } catch {
    /* best-effort */
  }
  try {
    const raw = (await publicClient().readContract({
      address: REGISTRY as `0x${string}`,
      abi: PUSH_SUB_OF_ABI,
      functionName: 'pushSubOf',
      args: [owner as `0x${string}`],
    })) as `0x${string}`;
    out.push(...parsePushSubs(decodeUtf8Bytes(raw).trim()));
  } catch {
    /* best-effort */
  }
  return dedupeSubs(out);
}

/**
 * Send ONE Web Push {title, body} to `owner`'s on-chain subscription (slot
 * rule in [`pushSubOf`]). Returns true iff the push service accepted. NEVER
 * throws: missing VAPID env / no subscription / a send failure all resolve
 * to false. Bounded: one read + one 5s-capped POST. The shared plumbing
 * behind both [`notifyOwnerOfRun`] (the post-run summary) and the agent's
 * `notify_owner` tool (the in-run "buzz my owner" affordance, feedback #69).
 */
async function sendOwnerPush(
  owner: string,
  fallbackTokenId: bigint,
  title: string,
  body: string,
): Promise<boolean> {
  const publicKey = process.env.VAPID_PUBLIC_KEY;
  const privateKey = process.env.VAPID_PRIVATE_KEY;
  const subject = process.env.VAPID_SUBJECT;
  if (!publicKey || !privateKey || !subject) return false; // push not configured
  const subs = await pushSubsOf(owner, fallbackTokenId);
  if (subs.length === 0) return false; // owner never enabled notifications
  const sent = await sendWebPushAll(subs, JSON.stringify({ title, body }), {
    publicKey,
    privateKey,
    subject,
  });
  return sent > 0;
}

/**
 * Best-effort owner notification after a recorded run: Web-Push a {title,
 * body} JSON the service worker (web/sw.js) renders. Silently skips when push
 * is unconfigured or no subscription is published; NEVER throws (a push
 * failure must not fail — or re-fire — the run, whose accounting already
 * committed).
 */
async function notifyOwnerOfRun(
  owner: string,
  targetId: bigint,
  jobId: string,
  targetName: string,
  output: string,
): Promise<void> {
  try {
    const body = output.length > 120 ? `${output.slice(0, 119)}…` : output;
    await sendOwnerPush(owner, targetId, `${targetName} job #${jobId}`, body);
  } catch (e) {
    console.warn(`[scheduler] notify owner of job ${jobId} failed: ${(e as Error).message}`);
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

// ---- the agent run: a BOUNDED tool loop with agent ping-pong ----------------
//
// AGENT PING-PONG. The old single-turn `runAgent` (one generateContent under the
// target persona) is a BOUNDED tool loop: the scheduled agent gets
// `call_agent(name, message)` (consult/delegate to another agent IN-TICK),
// `schedule_task(target, task, interval_seconds, budget_lh, runs)` (spawn a
// CROSS-TICK follow-up job), `notify_owner(title, body)` (Web-Push the JOB
// OWNER's registered device — feedback #69), and `collect_tithe(account)` (trigger
// the permissionless TitheFacet revenue→treasury pull for a consented account).
// Each loop round is one generateContent under the
// JOB's target persona; a `call_agent` functionCall resolves that target's on-chain
// persona and runs ONE generateContent for the sub-agent (sub-agents are SINGLE
// turns — no nested loops, so the call tree is bounded to depth 1), feeding the
// reply back as a functionResponse. Text with no call = the final answer; stop.
//
// The whole run is bounded by: (1) MAX_PINGPONG_ROUNDS caps the agent's own turns;
// (2) sub-agents never loop; (3) the per-job $LH budget — every generateContent (+
// every schedule_task, which spends gas + future budget) costs COST_WEI and we STOP
// before any call the budget can't cover; (4) the per-TICK caps (#1) — the running
// global/per-owner tick spend can't exceed GLOBAL_TICK_CAP_WEI /
// PER_OWNER_TICK_CAP_WEI, else the run STOPS and spills. A runaway loop drains its
// escrow and the facet exhausts the job.
//
// CROSS-TICK recursion is SHIPPED via `schedule_task` → `scheduleChildJob(parentJobId
// = the running job, …)`: the child's budget is drawn from the parent's remaining
// escrow (facet enforces InsufficientParentBudget / MAX_DEPTH / root cap). A facet
// revert is fed back as a functionResponse error — never hangs. A schedule_task call
// is budget/cap-gated exactly like a model call so an agent can't drain via spawning.

// ---- /goal — the ralph-on-chain loop ----------------------------------------
//
// A job whose task begins with the exact marker `GOAL: ` is a GOAL LOOP (the
// Ralph technique): the SAME goal prompt is re-fed every iteration, durable
// progress lives in on-chain state (not the model's memory — there is none
// across ticks), and the loop ends itself when the agent verifies the goal is
// met and calls `finish_goal` — which the worker relays to the facet's
// scheduler-only `completeJob(jobId)`, refunding the unspent escrow to the
// owner. Until then, every fire is one bounded iteration: inspect state, take
// the single most valuable next step, leave a progress note.

const GOAL_PREFIX = 'GOAL: ';

/** Render wei as a short decimal $LH string for prompt text (2dp, floor). */
function weiToLhText(wei: bigint): string {
  const hundredths = wei / 10_000_000_000_000_000n; // 1e16 = 0.01 $LH
  return `${hundredths / 100n}.${(hundredths % 100n).toString().padStart(2, '0')}`;
}

/**
 * Wrap a persona with the ralph-style goal-loop frame. `runsLeft` includes the
 * current run; `budgetWei` is the job's remaining escrow (both straight off the
 * just-read Job record — the iteration COUNT isn't stored on-chain, so the
 * frame speaks in remaining-runs/budget terms rather than "iteration N of M").
 */
function goalSystemPrompt(persona: string, runsLeft: number, budgetWei: bigint): string {
  return (
    persona +
    '\n\n--- RECURRING GOAL LOOP ---\n' +
    'You are one iteration of a recurring goal loop: the SAME goal below is re-fed ' +
    'to you every run, and you remember NOTHING between runs — all durable progress ' +
    'lives in on-chain state. Runs remaining (including this one): ' +
    `${runsLeft}. Budget remaining: ~${weiToLhText(budgetWei)} $LH; when either runs out the loop ends unfinished.\n` +
    'This iteration: (1) INSPECT the current on-chain state relevant to the goal ' +
    'using your tools; (2) take the SINGLE most valuable next step toward the goal; ' +
    '(3) if and ONLY if you can verify against that state that the goal is fully ' +
    'complete, call finish_goal with a final report — that permanently ends the loop ' +
    'and refunds the remaining budget to your owner. Otherwise end your turn with a ' +
    'brief progress note (what you did, what is left); the loop will fire again on ' +
    'the next interval.'
  );
}

function defaultPersona(name: string): string {
  return (
    `You are ${name}, an autonomous agent on the localharness platform ` +
    `(a self-sovereign, browser-resident agent network on Tempo mainnet). ` +
    `You are reachable at ${name}.localharness.xyz. This is a SCHEDULED run — ` +
    `carry out the task below and report concisely, speaking as ${name}. ` +
    `You may use the call_agent tool to delegate to or consult other ` +
    `localharness agents when that helps you complete the task.`
  );
}

// ---- Gemini wire shapes (just the parts of generateContent we touch) --------

interface GeminiFunctionCall {
  name: string;
  args?: Record<string, unknown>;
}
interface GeminiPart {
  text?: string;
  functionCall?: GeminiFunctionCall;
  functionResponse?: { name: string; response: Record<string, unknown> };
}
interface GeminiContent {
  role: 'user' | 'model' | 'function';
  parts: GeminiPart[];
}

// The tools the scheduled agent gets. Single-`type` schemas with no union /
// additionalProperties (Gemini 400s on those — see CLAUDE.md gotcha). FIVE tools:
//   * call_agent      — consult/delegate to another agent THIS run (in-tick).
//   * schedule_task   — spawn a FOLLOW-UP scheduled job funded from THIS job's
//                       remaining budget (cross-tick recursion via
//                       scheduleChildJob). The facet enforces budget draw + depth.
//   * notify_owner    — Web-Push a note to the JOB OWNER's registered device
//                       (feedback #69). Budget-counted like a model call so a
//                       loop can't spam the owner's phone.
//   * finish_goal     — declare the job's GOAL verifiably complete: ends the
//                       recurring job on-chain (completeJob) and refunds the
//                       remaining escrow to the owner. The /goal ralph-loop exit.
//   * collect_tithe   — trigger TitheFacet.collectTithe(account), the
//                       PERMISSIONLESS revenue→treasury pull. Zero new authority
//                       (the facet pulls only the account's OWN consented share
//                       into its OWN guild); budget-counted like a model call for
//                       anti-spam. The treasurer-without-a-tab affordance.
//
// NOTE: there is NO `post_bounty` tool. The existing permissionless
// `BountyFacet.postBounty` escrows from `msg.sender` (the scheduler key → the
// PLATFORM funds the reward, not the job's escrow) and gates accept/cancel on the
// poster (→ the bounty + its refund strand under the PLATFORM). It needs a
// net-new scheduler-role `postBountyFromJob(jobId, …)` that draws from the job's
// ScheduleFacet escrow + sets the OWNER as poster (the maintainer cuts it); only
// then is `post_bounty` wired. See `findToolCall`.
const AGENT_TOOLS = {
  functionDeclarations: [
    {
      name: 'call_agent',
      description:
        'Send a message to another localharness agent (by its subdomain name) and get its reply. Use this to delegate work to, or consult, another agent during this scheduled run.',
      parameters: {
        type: 'object',
        properties: {
          name: {
            type: 'string',
            description:
              'The target agent subdomain name, e.g. "claude" for claude.localharness.xyz.',
          },
          message: {
            type: 'string',
            description: 'The message / question to send the target agent.',
          },
        },
        required: ['name', 'message'],
      },
    },
    {
      name: 'schedule_task',
      description:
        'Schedule a FOLLOW-UP recurring job that runs a target localharness agent on a fixed interval, WITHOUT a browser tab. Its budget is drawn from THIS scheduled job\'s remaining $LH budget (so you can only spawn work you can afford). Use this to set up autonomous follow-on work — e.g. "check this again every hour for the next 3 runs". The job persists on-chain across ticks.',
      parameters: {
        type: 'object',
        properties: {
          target: {
            type: 'string',
            description:
              'The agent subdomain name to run on the schedule, e.g. "claude" for claude.localharness.xyz.',
          },
          task: {
            type: 'string',
            description:
              'The instruction the target agent runs each time the job fires.',
          },
          interval_seconds: {
            type: 'integer',
            description:
              'Seconds between runs (minimum 60). The job fires no more often than this.',
            minimum: 60,
          },
          budget_lh: {
            type: 'number',
            description:
              'Total $LH to escrow for this child job, drawn from THIS job\'s remaining budget. Must be > 0 and within what remains.',
          },
          runs: {
            type: 'integer',
            description:
              'How many times the job may fire before it stops (must be >= 1).',
            minimum: 1,
          },
        },
        required: ['target', 'task', 'interval_seconds', 'budget_lh', 'runs'],
      },
    },
    {
      name: 'notify_owner',
      description:
        'Send a push notification to YOUR OWNER\'s phone/device (their registered Web Push subscription). Use it to flag something that deserves the owner\'s attention NOW — a milestone reached, a blocking problem, a result they asked to be told about. It costs budget like a model call, so notify sparingly: at most one per run, only when genuinely useful.',
      parameters: {
        type: 'object',
        properties: {
          title: {
            type: 'string',
            description: 'Short notification headline (max 80 chars).',
          },
          body: {
            type: 'string',
            description: 'One-or-two-sentence detail line (max 200 chars).',
          },
        },
        required: ['title'],
      },
    },
    {
      name: 'finish_goal',
      description:
        'Declare this scheduled job\'s GOAL complete. This permanently ENDS the recurring job on-chain and refunds its remaining $LH budget to the owner — there are no more iterations after this. Call it ONLY when you have verified, against current on-chain state, that the goal is fully achieved. Pass a final report summarizing the outcome and the evidence.',
      parameters: {
        type: 'object',
        properties: {
          report: {
            type: 'string',
            description:
              'The final outcome summary: what was achieved, and the evidence (on-chain state) that proves the goal is complete.',
          },
        },
        required: ['report'],
      },
    },
    {
      name: 'collect_tithe',
      description:
        'Trigger the on-chain auto-tithe for an account that has opted in (via setTithe): pull its consented share of $LH from its own balance into the guild treasury it chose. The destination guild and percentage come from THAT account\'s own prior consent, never from you — you can only TRIGGER a collection the account already configured, never redirect or inflate it. Use it as a guild treasurer to sweep a member\'s pledged revenue into the treasury without their tab open. The account address is a 0x… address with a live tithe consent.',
      parameters: {
        type: 'object',
        properties: {
          account: {
            type: 'string',
            description:
              'The 0x… account address whose consented tithe to collect. It must have an active setTithe consent and a standing $LH allowance to the diamond, or the collection reverts.',
          },
        },
        required: ['account'],
      },
    },
  ],
} as const;

/**
 * One non-streaming generateContent. `tools` is optional (the sub-agent path
 * passes none so a sub-agent can never itself call_agent — single turn, no
 * nesting). Returns the candidate's parts verbatim so the caller can inspect
 * functionCall vs text. Throws on a non-2xx (the caller decides whether to halt).
 */
async function generateContent(
  systemInstruction: string,
  contents: GeminiContent[],
  withTool: boolean,
): Promise<GeminiPart[]> {
  const apiKey = process.env.GEMINI_API_KEY;
  if (!apiKey) throw new Error('proxy misconfigured: missing GEMINI_API_KEY');
  const url = `${GEMINI_BASE}/v1beta/models/${RUN_MODEL}:generateContent`;
  const body: Record<string, unknown> = {
    systemInstruction: { parts: [{ text: systemInstruction }] },
    contents,
  };
  if (withTool) body.tools = [AGENT_TOOLS];
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
    candidates?: { content?: { parts?: GeminiPart[] } }[];
  };
  return data.candidates?.[0]?.content?.parts ?? [];
}

/** Join the text parts of a candidate (ignoring functionCall parts). */
function partsText(parts: GeminiPart[]): string {
  return parts
    .map((p) => p.text ?? '')
    .join('')
    .trim();
}

/** First functionCall part addressed to a known tool (call_agent /
 * schedule_task / notify_owner / finish_goal / collect_tithe), if any.
 *
 * NOTE: `post_bounty` is deliberately NOT here. It cannot reuse the existing
 * permissionless `BountyFacet.postBounty` from the scheduler key — that escrows
 * `transferFrom(msg.sender, …)` (so the PLATFORM funds the reward, not the job's
 * own escrow) and gates `acceptResult`/`cancelBounty` on `msg.sender == poster`
 * (so the bounty + its refund strand under the PLATFORM, never the owner). Wiring
 * it would silently spend platform funds and break the trust envelope. It needs a
 * net-new scheduler-role `postBountyFromJob(jobId, …)` facet fn that DRAWS the
 * reward from the job's ScheduleFacet escrow and sets the JOB OWNER as poster
 * (mirroring `scheduleChildJob`'s escrow-draw + pinned-parent pattern); the
 * maintainer cuts that, then `post_bounty` joins this allowlist + the tool list. */
function findToolCall(parts: GeminiPart[]): GeminiFunctionCall | null {
  for (const p of parts) {
    if (
      p.functionCall &&
      (p.functionCall.name === 'call_agent' ||
        p.functionCall.name === 'schedule_task' ||
        p.functionCall.name === 'notify_owner' ||
        p.functionCall.name === 'finish_goal' ||
        p.functionCall.name === 'collect_tithe')
    ) {
      return p.functionCall;
    }
  }
  return null;
}

/** Coerce a Gemini arg (number | numeric-string) to a positive bigint wei value,
 * or throw. `budget_lh` arrives as a decimal $LH amount; convert to 18-dec wei. */
function lhToWei(v: unknown): bigint {
  const n = typeof v === 'number' ? v : typeof v === 'string' ? Number(v) : NaN;
  if (!Number.isFinite(n) || n <= 0) {
    throw new Error('budget_lh must be a positive number');
  }
  // 18-decimal fixed-point. Round to avoid float dust; reject sub-wei.
  const wei = BigInt(Math.round(n * 1e6)) * 1_000_000_000_000n; // 1e6 * 1e12 = 1e18
  if (wei <= 0n) throw new Error('budget_lh too small (rounds to 0 wei)');
  return wei;
}

/** Coerce a Gemini integer arg to a bounded positive int, or throw. */
function toPositiveInt(v: unknown, field: string, max: number): number {
  const n = typeof v === 'number' ? v : typeof v === 'string' ? Number(v) : NaN;
  if (!Number.isFinite(n) || n <= 0) {
    throw new Error(`${field} must be a positive integer`);
  }
  const t = Math.trunc(n);
  return t > max ? max : t;
}

/**
 * Run ONE sub-agent turn: resolve `name`'s on-chain persona, run a SINGLE
 * generateContent (NO tools — sub-agents don't recurse) under it, return its
 * text. The caller has already CONFIRMED budget for this call. An unregistered
 * target / Gemini error is surfaced as a thrown Error so the loop can feed an
 * error functionResponse back (never hang). This is itself ONE generateContent —
 * the caller counts it toward the budget.
 */
async function runSubAgent(name: string, message: string): Promise<string> {
  const targetId = await idOfName(name);
  if (targetId === 0n) {
    throw new Error(`no such agent: "${name}" is not registered`);
  }
  let persona: string;
  try {
    persona = (await personaOf(targetId)) ?? defaultPersona(name);
  } catch {
    persona = defaultPersona(name);
  }
  persona = withLessons(persona, await lessonsOf(targetId));
  const parts = await generateContent(
    persona,
    [{ role: 'user', parts: [{ text: message }] }],
    false,
  );
  const text = partsText(parts);
  return text.length ? text : '(the agent returned no text)';
}

/**
 * Per-TICK spend ledger (#1). ONE instance is created per cron tick and threaded
 * through every job; it accrues the $LH the worker has COMMITTED to spend this
 * tick, globally and per-owner. `canSpend` is the gate every metered action calls
 * BEFORE incurring its COST_WEI: if adding it would breach GLOBAL_TICK_CAP_WEI or
 * the current owner's PER_OWNER_TICK_CAP_WEI, it returns false and the caller
 * STOPS (the job spills to the next tick). `commit` records an actually-incurred
 * spend. The per-job budget gate (maxCalls) is independent and ANDed with this —
 * a call proceeds only if BOTH allow it.
 */
interface TickBudget {
  global: bigint;
  perOwner: Map<string, bigint>;
}

function newTickBudget(): TickBudget {
  return { global: 0n, perOwner: new Map() };
}

/** Would charging `cost` to `owner` keep BOTH the global and the owner caps? */
function canSpend(tb: TickBudget, owner: string, cost: bigint): boolean {
  const ownerKey = owner.toLowerCase();
  const ownerSpent = tb.perOwner.get(ownerKey) ?? 0n;
  if (tb.global + cost > GLOBAL_TICK_CAP_WEI) return false;
  if (ownerSpent + cost > PER_OWNER_TICK_CAP_WEI) return false;
  return true;
}

/** Record an incurred spend of `cost` for `owner` against this tick's ledger. */
function commitSpend(tb: TickBudget, owner: string, cost: bigint): void {
  const ownerKey = owner.toLowerCase();
  tb.global += cost;
  tb.perOwner.set(ownerKey, (tb.perOwner.get(ownerKey) ?? 0n) + cost);
}

/** Outcome of a bounded ping-pong run. `calls` = generateContent calls made
 * (the agent's turns + each sub-agent turn) = the unit COST_WEI meters on. */
interface PingPongResult {
  output: string;
  calls: number;
  rounds: number;
  /** True if the loop stopped because the budget couldn't cover the next call. */
  budgetCapped: boolean;
  /** True if the loop stopped because a per-TICK cap (global/per-owner) blocked
   * the next call (the job spills to the next tick). */
  tickCapped: boolean;
  /** True if the loop stopped because the job's fair share of the tick's
   * wall-clock budget ran out (partial work is still recorded; the job
   * re-fires on its own interval — never a silent skip). */
  clockCapped: boolean;
  /** Set when the agent called `finish_goal`: its final report. The caller
   * relays this to the facet's `completeJob` (after recordRun debits this
   * run's calls) — the job ends + the remainder refunds to the owner. */
  goalReport?: string;
}

/**
 * The bounded agent tool loop ("agent ping-pong").
 *
 * Every metered action passes TWO gates that are ANDed together:
 *   1. PER-JOB budget — `maxCalls` = how many COST_WEI units the JOB's remaining
 *      budget can pay for (floor(remaining / COST_WEI), computed by the caller).
 *      We never make call N+1 unless `calls < maxCalls`, so `calls` returned here
 *      is always <= maxCalls and `processJob` can charge `calls * COST_WEI`
 *      without exceeding budgetWei (the facet reverts SpendExceedsBudget else).
 *   2. PER-TICK caps (#1) — `canSpend(tb, owner, COST_WEI)`: the running
 *      tick-global + per-owner totals + this call must stay under
 *      GLOBAL_TICK_CAP_WEI / PER_OWNER_TICK_CAP_WEI. If not, we STOP — the job's
 *      nextRun is not advanced, so it spills to the next tick (`tickCapped`).
 * BEFORE every metered action (the agent's own turn, each sub-agent turn, and
 * each schedule_task — which spends gas + sets up future spend) we check BOTH and
 * `commitSpend` after counting it. A blocked action by EITHER gate halts the run.
 *
 * Tools the agent may call: `call_agent` (consult/delegate in-tick, depth-1 sub-
 * agents that never recurse), `schedule_task` (spawn a child job funded from
 * THIS job's remaining escrow via `scheduleChildJob`, with `parentJobId` pinned
 * to the running job — the facet enforces budget draw + MAX_DEPTH),
 * `notify_owner` (Web-Push a note to the JOB owner's registered device via
 * `sendOwnerPush` — the owner is wired from the job record, never from model
 * args, so a run can only ever buzz its OWN owner), and `collect_tithe` (trigger
 * the permissionless `TitheFacet.collectTithe(account)` — the facet pulls only
 * the account's OWN consented share into its OWN guild, so the scheduler signs it
 * with zero new authority). A tool error (bad args,
 * unregistered target, facet revert) becomes an error functionResponse — the
 * loop continues, never hangs.
 *
 * Bounds: MAX_PINGPONG_ROUNDS on the agent's turns, single-turn sub-agents, the
 * per-job budget, the per-tick caps, and Edge's wall-clock. The caller guarantees
 * `maxCalls >= 1` (it doesn't enter the loop otherwise).
 */
async function runPingPong(
  persona: string,
  task: string,
  maxCalls: number,
  parentJobId: bigint,
  targetId: bigint,
  owner: string,
  tb: TickBudget,
  modelDeadlineMs: number,
): Promise<PingPongResult> {
  const contents: GeminiContent[] = [
    { role: 'user', parts: [{ text: task }] },
  ];
  let calls = 0;
  let lastText = '';
  let budgetCapped = false;
  let tickCapped = false;
  let clockCapped = false;

  // ALL gates (per-job budget AND per-tick caps AND the job's fair share of
  // the tick wall-clock) for the NEXT metered call. Returns true if we may
  // proceed; sets the matching *Capped flag + returns false if not. Caller
  // commits the spend after a true.
  const mayMeterCall = (): boolean => {
    if (calls >= maxCalls) {
      budgetCapped = true;
      return false;
    }
    if (!canSpend(tb, owner, COST_WEI)) {
      tickCapped = true;
      return false;
    }
    if (Date.now() >= modelDeadlineMs) {
      // Wall-clock fair share spent: stop HERE so the jobs behind this one in
      // the batch still get processed (and so the platform never kills the
      // function mid-batch, which would skip them SILENTLY).
      clockCapped = true;
      return false;
    }
    return true;
  };

  for (let round = 0; round < MAX_PINGPONG_ROUNDS; round++) {
    // GATE (the agent's own turn). Never make a call a gate can't allow.
    if (!mayMeterCall()) break;
    calls++;
    commitSpend(tb, owner, COST_WEI);
    const parts = await generateContent(persona, contents, true);

    const call = findToolCall(parts);
    if (!call) {
      // Pure text → final answer. Stop.
      lastText = partsText(parts) || lastText;
      return {
        output: lastText || '(the agent returned no text)',
        calls,
        rounds: round + 1,
        budgetCapped,
        tickCapped,
        clockCapped,
      };
    }

    // GOAL COMPLETE (finish_goal). Not metered — it's not a model call, and the
    // on-chain completeJob (relayed by the caller AFTER recordRun settles this
    // run's debit) only REFUNDS escrow, never spends it. The report is the
    // run's final output; the loop stops HERE.
    if (call.name === 'finish_goal') {
      const report =
        typeof call.args?.report === 'string' ? (call.args.report as string).trim() : '';
      const output =
        report || partsText(parts) || lastText || '(goal declared complete with no report)';
      return {
        output,
        calls,
        rounds: round + 1,
        budgetCapped,
        tickCapped,
        clockCapped,
        goalReport: output,
      };
    }

    // The model wants a tool. Record the model's functionCall turn in history so
    // the subsequent functionResponse is well-formed.
    lastText = partsText(parts) || lastText;
    contents.push({ role: 'model', parts });

    let responsePayload: Record<string, unknown>;

    if (call.name === 'schedule_task') {
      // CHILD-JOB SPAWN (cross-tick recursion). Counts toward the budget/cap like
      // a model call — it spends gas now AND sets up future spend (drawn from this
      // job's escrow). Gate it the same way.
      if (!mayMeterCall()) {
        responsePayload = {
          error: budgetCapped
            ? 'budget exhausted: not enough remaining $LH to schedule a child job'
            : clockCapped
              ? 'tick wall-clock budget reached: cannot schedule a child job this run'
              : 'per-tick spend cap reached: cannot schedule a child job this tick',
        };
      } else {
        calls++;
        commitSpend(tb, owner, COST_WEI);
        try {
          const target =
            typeof call.args?.target === 'string'
              ? (call.args.target as string).trim()
              : '';
          if (!target) throw new Error('schedule_task requires a non-empty "target"');
          const childTask =
            typeof call.args?.task === 'string' ? (call.args.task as string) : '';
          if (!childTask.trim()) {
            throw new Error('schedule_task requires a non-empty "task"');
          }
          const interval = BigInt(
            toPositiveInt(call.args?.interval_seconds, 'interval_seconds', 31_536_000),
          );
          if (interval < 60n) {
            throw new Error('interval_seconds must be at least 60');
          }
          const childBudget = lhToWei(call.args?.budget_lh);
          const childRuns = toPositiveInt(call.args?.runs, 'runs', 4_294_967_295);

          const targetId = await idOfName(target);
          if (targetId === 0n) {
            throw new Error(`no such agent: "${target}" is not registered`);
          }
          // parentJobId is PINNED to the running job — the agent can't redirect
          // the budget draw to another job. The facet enforces the escrow draw +
          // depth + root cap; a revert is surfaced as an error below.
          const childJobId = await scheduleChildJob(
            parentJobId,
            targetId,
            childTask,
            interval,
            childBudget,
            childRuns,
          );
          responsePayload = {
            scheduled: true,
            childJobId: childJobId.toString(),
            target,
            interval_seconds: Number(interval),
            runs: childRuns,
          };
        } catch (e) {
          // A facet revert (InsufficientParentBudget / MaxDepthExceeded), an
          // unregistered target, or a bad arg — feed it back; never hang.
          responsePayload = { error: (e as Error).message };
        }
      }
      contents.push({
        role: 'function',
        parts: [
          { functionResponse: { name: 'schedule_task', response: responsePayload } },
        ],
      });
      if (tickCapped || clockCapped) break; // a tick cap / spent wall-clock share halts the run
      continue;
    }

    if (call.name === 'notify_owner') {
      // OWNER PUSH (the goal-loop "notify my owner" affordance — feedback
      // #69). Sends to the JOB OWNER's on-chain subscription via the same
      // sendOwnerPush plumbing as the post-run summary; owner + targetId come
      // from the job record, NOT from model args (a run can only buzz its own
      // owner). Not a model call, but COUNTED through the same gate + ledger
      // as one (mirrors schedule_task): each push costs COST_WEI from the
      // job's budget, so a runaway loop can't spam the owner's phone for free.
      const title =
        typeof call.args?.title === 'string'
          ? (call.args.title as string).trim().slice(0, 80)
          : '';
      const pushBody =
        typeof call.args?.body === 'string'
          ? (call.args.body as string).trim().slice(0, 200)
          : '';
      if (!title) {
        responsePayload = { error: 'notify_owner requires a non-empty "title"' };
      } else if (!mayMeterCall()) {
        responsePayload = {
          error: budgetCapped
            ? 'budget exhausted: not enough remaining $LH to notify the owner'
            : clockCapped
              ? 'tick wall-clock budget reached: cannot notify the owner this run'
              : 'per-tick spend cap reached: cannot notify the owner this tick',
        };
      } else {
        calls++;
        commitSpend(tb, owner, COST_WEI);
        // sendOwnerPush never throws; false = unconfigured push, no on-chain
        // subscription, or the push service rejected — the agent can report
        // that in its final answer instead of retrying.
        const sent = await sendOwnerPush(owner, targetId, title, pushBody);
        responsePayload = sent
          ? { sent: true }
          : {
              sent: false,
              note: 'push not delivered (owner has no on-chain push subscription, or the push service refused)',
            };
      }
      contents.push({
        role: 'function',
        parts: [
          { functionResponse: { name: 'notify_owner', response: responsePayload } },
        ],
      });
      if (tickCapped || clockCapped) break; // a tick cap / spent wall-clock share halts the run
      continue;
    }

    if (call.name === 'collect_tithe') {
      // PERMISSIONLESS TITHE PULL (TitheFacet.collectTithe). Not a model call,
      // but COUNTED through the same gate + ledger as one (mirrors schedule_task /
      // notify_owner): it spends scheduler gas + is an agent-initiated action, so
      // metering it keeps a loop from spamming collectTithe for free AND keeps the
      // per-job-budget / per-tick-cap accounting in lockstep. No new authority —
      // the facet pulls only `account`'s own consented share into the account's own
      // guild; a revert (NotConfigured / UnknownGuild / NothingToCollect) is fed
      // back so the agent reacts or finishes.
      const account = asAddress(call.args?.account); // null on a malformed arg
      if (account === null) {
        responsePayload = { error: 'account must be a 0x… 20-byte address' };
      } else if (!mayMeterCall()) {
        responsePayload = {
          error: budgetCapped
            ? 'budget exhausted: not enough remaining $LH to collect a tithe'
            : clockCapped
              ? 'tick wall-clock budget reached: cannot collect a tithe this run'
              : 'per-tick spend cap reached: cannot collect a tithe this tick',
        };
      } else {
        calls++;
        commitSpend(tb, owner, COST_WEI);
        try {
          const amount = await collectTithe(account);
          responsePayload = { collected: true, amountWei: amount.toString() };
        } catch (e) {
          // A facet revert (NotConfigured / UnknownGuild / NothingToCollect) or
          // an unconfirmed receipt — surface it; never hang.
          responsePayload = { error: (e as Error).message };
        }
      }
      contents.push({
        role: 'function',
        parts: [
          { functionResponse: { name: 'collect_tithe', response: responsePayload } },
        ],
      });
      if (tickCapped || clockCapped) break; // a tick cap / spent wall-clock share halts the run
      continue;
    }

    // call.name === 'call_agent'
    const targetName =
      typeof call.args?.name === 'string' ? (call.args.name as string).trim() : '';
    const subMessage =
      typeof call.args?.message === 'string' ? (call.args.message as string) : '';

    // GATE (the sub-agent turn). If a gate blocks the sub-agent call, feed an
    // error response so the agent can still wrap up on its NEXT turn (itself gated
    // at the top of the loop) — don't half-run it.
    if (!targetName || !subMessage) {
      responsePayload = { error: 'call_agent requires non-empty "name" and "message"' };
    } else if (!mayMeterCall()) {
      responsePayload = {
        error: budgetCapped
          ? 'budget exhausted: not enough remaining $LH to call another agent'
          : clockCapped
            ? 'tick wall-clock budget reached: cannot call another agent this run'
            : 'per-tick spend cap reached: cannot call another agent this tick',
      };
    } else {
      calls++;
      commitSpend(tb, owner, COST_WEI);
      try {
        const reply = await runSubAgent(targetName, subMessage);
        responsePayload = { reply };
      } catch (e) {
        // A sub-agent error (unregistered target / Gemini failure) MUST NOT hang
        // or abort — feed it back so the agent can react or finish.
        responsePayload = { error: (e as Error).message };
      }
    }

    contents.push({
      role: 'function',
      parts: [
        { functionResponse: { name: 'call_agent', response: responsePayload } },
      ],
    });
    if (tickCapped || clockCapped) break; // a tick cap / spent wall-clock share halts the run
  }

  // Fell out of the loop: MAX_PINGPONG_ROUNDS hit, OR the per-job budget capped
  // us, OR a per-tick cap halted us mid-conversation, OR the job's wall-clock
  // fair share ran out. Return the best text we have.
  return {
    output: lastText || '(the agent reached its round/budget/tick/wall-clock limit without a final answer)',
    calls,
    rounds: MAX_PINGPONG_ROUNDS,
    budgetCapped,
    tickCapped,
    clockCapped,
  };
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
  let status: 'success' | 'reverted';
  try {
    // 12s (matches gemini.ts::meterDebit) — NOT 30s. The cron ticks every
    // minute and processes up to MAX_JOBS_PER_TICK jobs SEQUENTIALLY, each a
    // bounded ping-pong loop + this receipt wait; a 30s-per-job wait could let a
    // single tick blow past Edge's ~25s wall-clock and die mid-batch. Keep each
    // wait bounded so the whole tick fits.
    ({ status } = await pub.waitForTransactionReceipt({
      hash,
      timeout: 12_000,
      pollingInterval: 500,
    }));
  } catch {
    // Ambiguous: the tx was SUBMITTED but the receipt didn't arrive in time
    // (RPC slow / Edge clock). The recordRun is CAS-guarded on `nextRun`, so if
    // it lands it advances the job exactly once (no double-bill possible); if it
    // doesn't, next tick re-fires under the same CAS. Treat as a non-recorded
    // skip — do NOT throw (a throw would just be logged as an error upstream and
    // tells us nothing more). The chain is the source of truth.
    console.warn(`[scheduler] recordRun(${id}) submitted (tx ${hash}) but receipt wait timed out — chain CAS will reconcile`);
    return 'stale';
  }
  if (status === 'reverted') {
    console.warn(`[scheduler] recordRun(${id}) reverted on-chain (tx ${hash}) — treated as a benign skip`);
    return 'stale';
  }
  return 'recorded';
}

/**
 * scheduleChildJob — the cross-tick recursion write (scheduler-role). Spawns a
 * child job whose `budgetWei` is DRAWN FROM `parentJobId`'s remaining escrow. The
 * FACET enforces everything that matters (InsufficientParentBudget / depth / root
 * cap); the worker just relays the agent's request signed by the scheduler key,
 * with `parentJobId` pinned to the CURRENTLY-running job (the agent can't choose
 * a different parent — it's wired here, not from model args).
 *
 * Returns the new child job id on success, or throws on a facet revert (decoded
 * message — e.g. InsufficientParentBudget, MaxDepthExceeded) so the caller feeds
 * it back as a functionResponse error rather than hanging. Shares the scheduler
 * EOA + sequential nonce with recordRun (the tick processes jobs sequentially, so
 * these don't race the nonce).
 */
async function scheduleChildJob(
  parentJobId: bigint,
  targetId: bigint,
  task: string,
  interval: bigint,
  budgetWei: bigint,
  maxRuns: number,
): Promise<bigint> {
  const wallet = schedulerWallet();
  const taskBytes = ('0x' +
    bytesToHex(new TextEncoder().encode(task))) as `0x${string}`;
  // simulate first so a facet revert is decoded into a readable reason BEFORE we
  // spend gas (and surfaces the same error the on-chain write would).
  const pub = publicClient();
  const { request, result } = await pub.simulateContract({
    address: REGISTRY as `0x${string}`,
    abi: SCHEDULE_ABI,
    functionName: 'scheduleChildJob',
    args: [parentJobId, targetId, taskBytes, interval, budgetWei, maxRuns],
    account: wallet.account!,
  });
  const hash = await wallet.writeContract(request);
  try {
    const { status } = await pub.waitForTransactionReceipt({
      hash,
      timeout: 12_000,
      pollingInterval: 500,
    });
    if (status === 'reverted') {
      throw new Error(`scheduleChildJob reverted on-chain (tx ${hash})`);
    }
  } catch (e) {
    // Receipt timed out OR reverted. The simulate above already passed, so a
    // timeout most likely means it WILL land — but we can't confirm the child id,
    // so surface as an error the agent can react to (don't claim a child id we
    // didn't observe). A hard revert is rethrown verbatim.
    throw new Error(`scheduleChildJob unconfirmed: ${(e as Error).message}`);
  }
  // simulateContract returned the would-be childJobId; the write matched it.
  return result as bigint;
}

/** Normalize a Gemini-supplied `account` arg to a checksummed-lowercase 0x EVM
 * address, or null if it isn't a 20-byte hex address — so a malformed arg becomes
 * a functionResponse error and never reaches the chain. */
function asAddress(v: unknown): `0x${string}` | null {
  const s = (typeof v === 'string' ? v : '').trim();
  if (!/^0x[0-9a-fA-F]{40}$/.test(s)) return null;
  return s.toLowerCase() as `0x${string}`;
}

/**
 * collectTithe — the PERMISSIONLESS revenue→treasury pull (TitheFacet), signed
 * by the scheduler key. Same plumbing as `scheduleChildJob`: simulate first so a
 * facet revert (NotConfigured / UnknownGuild / NothingToCollect) is decoded into
 * a readable reason BEFORE spending gas, then write + await the 12s receipt.
 *
 * ZERO new authority is granted by the signer: the facet reads ONLY `account`'s
 * own stored `(guildId, bps)` and clamps the pull to the account's own
 * balance·bps AND its own `approve` ceiling, into the account's own consented
 * guild — the scheduler can neither redirect nor over-pull. The scheduler funds
 * only the tx gas; the $LH moved comes from `account`. Returns the amount pulled
 * (simulate's return value); throws a readable reason on a revert / unconfirmed
 * receipt so the caller feeds it back as a functionResponse error (never hangs).
 */
async function collectTithe(account: `0x${string}`): Promise<bigint> {
  const wallet = schedulerWallet();
  const pub = publicClient();
  const { request, result } = await pub.simulateContract({
    address: REGISTRY as `0x${string}`,
    abi: TITHE_ABI,
    functionName: 'collectTithe',
    args: [account],
    account: wallet.account!,
  });
  const hash = await wallet.writeContract(request);
  try {
    const { status } = await pub.waitForTransactionReceipt({
      hash,
      timeout: 12_000,
      pollingInterval: 500,
    });
    if (status === 'reverted') {
      throw new Error(`collectTithe reverted on-chain (tx ${hash})`);
    }
  } catch (e) {
    // simulate already passed, so a timeout most likely means it WILL land — but
    // we can't confirm the amount, so surface it rather than claim a pull we
    // didn't observe. A hard revert is rethrown verbatim.
    throw new Error(`collectTithe unconfirmed: ${(e as Error).message}`);
  }
  // simulateContract returned the would-be `amount`; the write matched it.
  return result as bigint;
}

/**
 * completeJob — the /goal exit write (scheduler-role, same plumbing as
 * recordRun). Marks the job terminal + refunds the FULL remaining escrow to
 * the owner. Called AFTER recordRun has settled this run's debit, so the
 * refund is the post-debit remainder and the run's model calls are paid for.
 *
 * A revert is BENIGN-adjacent: if recordRun's debit just exhausted the job
 * (budget drained on this very run), the facet already refunded via the
 * exhaust path and completeJob reverts JobNotActive — the goal outcome is the
 * same (job over, owner refunded), so we log + report 'failed' without
 * throwing. Returns 'completed' on a confirmed success.
 */
async function completeJob(id: bigint): Promise<'completed' | 'failed'> {
  const wallet = schedulerWallet();
  let hash: `0x${string}`;
  try {
    hash = await wallet.writeContract({
      address: REGISTRY as `0x${string}`,
      abi: SCHEDULE_ABI,
      functionName: 'completeJob',
      args: [id],
      account: wallet.account!,
      chain: TEMPO_CHAIN,
    });
  } catch (e) {
    console.warn(`[scheduler] completeJob(${id}) reverted on submit: ${(e as Error).message}`);
    return 'failed';
  }
  try {
    const { status } = await publicClient().waitForTransactionReceipt({
      hash,
      timeout: 12_000,
      pollingInterval: 500,
    });
    if (status === 'reverted') {
      console.warn(`[scheduler] completeJob(${id}) reverted on-chain (tx ${hash})`);
      return 'failed';
    }
  } catch {
    // Submitted but unconfirmed. completeJob is idempotent-safe (a replay on a
    // terminal job reverts, never double-refunds); if it lands the job is done,
    // if not the goal loop fires once more and the agent re-finishes. Report
    // failed so the log reflects the unconfirmed state.
    console.warn(`[scheduler] completeJob(${id}) submitted (tx ${hash}) but receipt wait timed out`);
    return 'failed';
  }
  return 'completed';
}

// ---- one job ----------------------------------------------------------------

interface JobResult {
  id: string;
  /** `deferred` = due this tick but never started (batch cap, or the tick's
   * wall-clock budget ran out first). nextRun is UNCHANGED — it re-fires next
   * tick. Always logged + reported, never silent. */
  outcome: 'recorded' | 'skipped' | 'stale' | 'spilled' | 'deferred';
  ran: 'ok' | 'error' | 'n/a';
  /** generateContent calls made this run (agent turns + sub-agent turns). */
  calls?: number;
  /** $LH wei actually debited (calls * COST_WEI, capped to budget). */
  spentWei?: string;
  /** /goal: the agent called finish_goal. 'completed' = completeJob confirmed
   * on-chain (job over, remainder refunded); 'complete-failed' = the relay
   * didn't confirm (the loop fires again and the agent can re-finish). */
  goal?: 'completed' | 'complete-failed';
  note?: string;
}

/**
 * Process ONE due job: read its full record + task, (re)confirm it's Active and
 * due, CHECK the per-tick caps (#1) can afford at least one call, run a BOUNDED
 * agent ping-pong loop under its target's persona, then ALWAYS recordRun
 * (advancing nextRun + debiting `calls * COST_WEI`, capped to the budget) — even
 * if Gemini errored, so a broken job can never hot-loop and stays bounded by its
 * budget. The per-job budget bounds the WHOLE ping-pong run; `tb` (the shared
 * tick ledger) caps the WHOLE tick across all jobs/owners on top;
 * `modelDeadlineMs` (this job's fair share of the tick wall-clock, computed by
 * the handler) truncates a heavy run so the jobs behind it still fire.
 */
async function processJob(
  id: bigint,
  tb: TickBudget,
  modelDeadlineMs: number,
): Promise<JobResult> {
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

  const expectedNextRun = job.nextRun;

  // The facet rejects spentWei > budgetWei. If the remaining budget can't cover
  // a FULL run (`budgetWei < COST_WEI`), we must NOT skip-and-leave-it: a skipped
  // job's `nextRun` is never advanced, so it stays Active+due and `jobsDue`
  // returns it EVERY tick forever (a permanent hot-loop that also starves the
  // per-tick batch, since a covered run can never land — the budget can only
  // shrink). Instead, record a FINAL run debiting exactly the dust remainder
  // (`spentWei = budgetWei`): the facet sees `remaining(0) < spentWei`, marks the
  // job Exhausted, and refunds 0 — clearing it from the due set for good. We do
  // NOT run Gemini for this (no budget to pay for it) — it's a pure
  // accounting-close, not a free model call. This does NOT touch the tick ledger
  // (no real model spend).
  if (job.budgetWei < COST_WEI) {
    const outcome = await recordRun(id, expectedNextRun, job.budgetWei);
    return {
      id: idStr,
      outcome,
      ran: 'n/a',
      note: 'budget below per-run cost — closed (Exhausted) without a run',
    };
  }

  // PER-TICK CAP GATE (#1) — BEFORE running the job at all. If the tick's global
  // total OR this owner's running total can't even cover ONE model call this tick,
  // SPILL the job to the next tick: do NOT recordRun (so nextRun is unchanged and
  // jobsDue returns it next tick), do NOT spend. This is the additional ceiling on
  // top of the per-job budget — it bounds the worker's REAL upstream cost per tick
  // regardless of how many jobs/owners are due. Logged, never silently dropped.
  if (!canSpend(tb, job.owner, COST_WEI)) {
    console.warn(
      `[scheduler] job ${idStr} owner ${job.owner} SPILLED — per-tick cap reached ` +
        `(global=${tb.global} cap=${GLOBAL_TICK_CAP_WEI}, ownerTickCap=${PER_OWNER_TICK_CAP_WEI}); re-fires next tick`,
    );
    return {
      id: idStr,
      outcome: 'spilled',
      ran: 'n/a',
      note: 'per-tick spend cap reached — spilled to next tick (nextRun unchanged)',
    };
  }

  const name = await nameOfId(job.targetId);

  // BUDGET → MAX CALLS. The per-job budget is the HARD ceiling on the ENTIRE
  // ping-pong run: how many generateContent calls (agent turns + sub-agent turns)
  // can the remaining budget pay for? `floor(budgetWei / COST_WEI)`. We're past
  // the `budgetWei < COST_WEI` dust-close above, so maxCalls >= 1. The loop never
  // makes call N+1 unless `N < maxCalls` AND the tick caps allow it, so the total
  // it returns is <= maxCalls, and `min(calls * COST_WEI, budgetWei)` can never
  // exceed budgetWei (the facet reverts SpendExceedsBudget if it did).
  // MAX_PINGPONG_ROUNDS additionally caps the agent's OWN turns.
  const maxCalls = Number(job.budgetWei / COST_WEI);

  // Build + run the bounded loop. Persona resolution + Gemini errors must NOT
  // abort the accounting: capture the outcome + the call count, then record once.
  let ran: 'ok' | 'error' = 'ok';
  let runNote = '';
  let calls = 0;
  let budgetCapped = false;
  let tickCapped = false;
  let clockCapped = false;
  // `ranLoop` = the ping-pong loop actually executed (and self-committed its calls
  // to the tick ledger). If a pre-loop READ (persona/task) throws, the loop never
  // ran and committed nothing — we charge ONE call below and must ALSO commit that
  // one call to the ledger ourselves so on-chain + ledger stay in lockstep.
  let ranLoop = false;
  // /goal: set when the agent called finish_goal — its final report. Relayed
  // to the facet's completeJob AFTER recordRun settles this run's debit.
  let goalReport: string | undefined;
  try {
    const basePersona = withLessons(
      (await personaOf(job.targetId)) ?? defaultPersona(name),
      await lessonsOf(job.targetId),
    );
    const rawTask = (await taskOf(id)).trim();
    // The exact `GOAL: ` marker flags a ralph goal loop: wrap the persona with
    // the goal-loop frame (inspect state → one step → finish_goal only when
    // verifiably done) and feed the goal text as the task each iteration.
    const isGoal = rawTask.startsWith(GOAL_PREFIX);
    const persona = isGoal
      ? goalSystemPrompt(basePersona, job.runsLeft, job.budgetWei)
      : basePersona;
    // An empty task = "run under the persona's standing instruction" (sentinel).
    const task = isGoal
      ? `THE GOAL:\n${rawTask.slice(GOAL_PREFIX.length).trim()}`
      : rawTask || 'Perform your scheduled task and report concisely.';
    ranLoop = true;
    const result = await runPingPong(
      persona,
      task,
      maxCalls,
      id,
      job.targetId,
      job.owner,
      tb,
      modelDeadlineMs,
    );
    calls = result.calls;
    budgetCapped = result.budgetCapped;
    tickCapped = result.tickCapped;
    clockCapped = result.clockCapped;
    goalReport = result.goalReport;
    runNote = result.output;
    // MVP output sink = the Vercel log. The reply is recorded here so a scheduled
    // run is observable in the function logs.
    // TODO: richer output routing — persist the transcript on-chain / in a store,
    // or push the final output to the owner, instead of only logging. (The
    // ping-pong replies ARE chained in-loop now; this TODO is the OUTPUT side.)
    console.log(
      `[scheduler] job ${idStr} target ${name} (#${job.targetId}) ` +
        `calls=${calls}/${maxCalls} rounds=${result.rounds}` +
        `${budgetCapped ? ' (budget-capped)' : ''}${tickCapped ? ' (tick-capped)' : ''}` +
        `${clockCapped ? ' (wall-clock-capped)' : ''} ` +
        `reply: ${runNote.slice(0, 800)}`,
    );
  } catch (e) {
    ran = 'error';
    runNote = (e as Error).message;
    // CRITICAL: still record the run below. A failing job advances its clock and
    // debits a cost so it re-fires at most once per interval and drains its budget
    // — never a hot loop.
    if (!ranLoop) {
      // The loop never started (a pre-loop read threw). Charge ONE call. The tick
      // gate above already confirmed one call fits; commit it to the ledger now so
      // the ledger matches the on-chain debit.
      calls = 1;
      commitSpend(tb, job.owner, COST_WEI);
    } else if (calls === 0) {
      // The loop entered but threw on its very first call (which it commits BEFORE
      // awaiting generateContent), so the ledger already has 1 call. Bill for it.
      calls = 1;
    }
    console.error(`[scheduler] job ${idStr} target ${name} run ERROR (will still recordRun): ${runNote}`);
  }

  // METER: debit calls * COST_WEI, CAPPED to the budget (the facet reverts
  // SpendExceedsBudget if spentWei > remaining). When this debit consumes the
  // budget the facet marks the job Exhausted + refunds the remainder.
  //
  // CRITICAL: cap to the LIVE budget, not the start-of-run `job` snapshot. The
  // agent can call schedule_task (scheduleChildJob) MID-RUN, which draws down the
  // parent's on-chain budget; capping to the stale snapshot would let spentWei
  // exceed the live budget → recordRun reverts SpendExceedsBudget → the worker
  // treats it as 'stale' → nextRun never advances → the job HOT-LOOPS every tick,
  // burning real upstream spend without ever debiting (the budget leash defeated).
  // Re-read here so the debit always lands (exhausting the job if depleted). On a
  // read failure, fall back to the snapshot (no worse than before on that path).
  let spentWei = BigInt(calls) * COST_WEI;
  const liveBudgetWei = await getJob(id)
    .then((j) => j.budgetWei)
    .catch(() => job.budgetWei);
  if (spentWei > liveBudgetWei) spentWei = liveBudgetWei;

  const outcome = await recordRun(id, expectedNextRun, spentWei);

  // /goal: the agent declared the goal complete — relay finish_goal to the
  // facet's completeJob. Ordering matters: recordRun FIRST (debit this run's
  // calls), completeJob SECOND (refund the post-debit remainder). If that very
  // debit exhausted the job, the facet already refunded via the exhaust path
  // and completeJob reports 'failed' on the JobNotActive revert — same outcome
  // for the owner (job over, remainder refunded), logged either way.
  // Gate on `outcome === 'recorded'` (matching the push block below): only relay
  // completeJob when THIS run's recordRun actually landed. On a benign
  // race/timeout (outcome 'stale' — another firer committed first, or the
  // receipt didn't arrive) the job was NOT advanced by us; calling completeJob
  // anyway would end an Active job early + refund its escrow on a run we never
  // recorded. The goal-loop simply re-fires next tick and the agent re-finishes.
  let goal: 'completed' | 'complete-failed' | undefined;
  if (outcome === 'recorded' && goalReport !== undefined) {
    goal = (await completeJob(id)) === 'completed' ? 'completed' : 'complete-failed';
    console.log(
      `[scheduler] job ${idStr} GOAL ${goal === 'completed' ? 'COMPLETE (job ended, remainder refunded)' : 'finish relay unconfirmed'} — report: ${goalReport.slice(0, 800)}`,
    );
  }

  // OWNER PUSH — TERMINAL OUTCOMES ONLY (a real user got buzzed once a
  // minute by a multi-run job that pushed on EVERY run while it flailed):
  // a goal completing, or this run exhausting the job (last run / budget
  // drained). A single-run job still pushes on its only run; a recurring
  // job buzzes when it finishes, not while it works. Still only for runs
  // WE recorded with output; missing VAPID env / subscription silently
  // skips; a push failure can never fail the run.
  // Mirror ScheduleFacet.recordRun's exhaust test EXACTLY (recordRun marks
  // Exhausted when `runsLeft == 0 || remaining < spentWei`, where
  // `remaining = budgetWei - spentWei`): the partial-remainder hard stop fires
  // when what's left can't fund another run of THIS size, not only when the
  // budget hit exactly zero. `spentWei` is already capped to `job.budgetWei`,
  // so `job.budgetWei - spentWei` never underflows.
  const exhaustedNow =
    outcome === 'recorded' &&
    (job.runsLeft <= 1 || job.budgetWei - spentWei < spentWei);
  if (outcome === 'recorded' && ran === 'ok' && (goal === 'completed' || exhaustedNow)) {
    const pushBody = goalReport !== undefined ? goalReport : runNote;
    const pushName = goal === 'completed' ? `GOAL COMPLETE: ${name}` : name;
    await notifyOwnerOfRun(job.owner, job.targetId, idStr, pushName, pushBody);
  }

  return {
    id: idStr,
    outcome,
    ran,
    calls,
    spentWei: spentWei.toString(),
    goal,
    note:
      ran === 'error'
        ? runNote.slice(0, 200)
        : goal
          ? runNote.slice(0, 200)
          : tickCapped
            ? 'tick-capped mid-run (remaining work spills to next tick)'
            : clockCapped
              ? 'wall-clock capped mid-run (partial work recorded; re-fires on its interval)'
              : budgetCapped
                ? 'budget-capped mid-run'
                : undefined,
  };
}

// ---- handler ----------------------------------------------------------------

function unauthorized(): Response {
  return new Response(JSON.stringify({ error: 'unauthorized' }), {
    status: 401,
    headers: { 'content-type': 'application/json' },
  });
}

/**
 * Constant-time string compare for the CRON_SECRET bearer check. A plain `!==`
 * short-circuits on the first differing byte, leaking the secret's length +
 * matched-prefix length through response timing — and this secret is a STATIC
 * shared bearer (unlike the per-request ECDSA tokens in gemini.ts/mcp.ts, which
 * are non-forgeable regardless of compare timing). Compare every byte; the
 * length check is folded into the accumulator so a length mismatch can't
 * short-circuit either. (Edge network jitter dwarfs the signal in practice —
 * this is defense-in-depth, cheap to do right.)
 */
function timingSafeEqual(a: string, b: string): boolean {
  const ab = new TextEncoder().encode(a);
  const bb = new TextEncoder().encode(b);
  let diff = ab.length ^ bb.length;
  const n = Math.max(ab.length, bb.length);
  for (let i = 0; i < n; i++) {
    diff |= (ab[i] ?? 0) ^ (bb[i] ?? 0);
  }
  return diff === 0;
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

  // PUBLIC keeper poke (decentralized heartbeat, krafto #1.5): `?poke=<jobId>`
  // runs ONE job. No cron secret needed — processJob re-validates (known + Active
  // + due) and recordRun is CAS-guarded, so a poke can only run a genuinely-due
  // job once; not-due → safe no-write skip. Lets any keeper be the heartbeat when
  // the Vercel cron stalls. Rate-limited per IP (read-spam is the only abuse).
  {
    const poke = new URL(req.url).searchParams.get('poke');
    if (poke !== null) {
      const ip = (req.headers.get('x-forwarded-for') ?? '').split(',')[0].trim() || 'unknown';
      const retry = pokeWindow.hit(ip);
      if (retry > 0) {
        return new Response(JSON.stringify({ error: 'rate limited', retryAfterSeconds: retry }), {
          status: 429,
          headers: { 'content-type': 'application/json', 'retry-after': String(retry) },
        });
      }
      let id: bigint;
      try {
        id = BigInt(poke);
      } catch {
        return new Response(JSON.stringify({ error: 'poke must be a numeric jobId' }), {
          status: 400,
          headers: { 'content-type': 'application/json' },
        });
      }
      const result = await processJob(id, newTickBudget(), Date.now() + TICK_SOFT_BUDGET_MS);
      return new Response(JSON.stringify({ poked: poke, result }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      });
    }
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
  if (!timingSafeEqual(auth, `Bearer ${secret}`)) {
    return unauthorized();
  }

  const tickStart = Date.now();
  const results: JobResult[] = [];
  // ONE per-tick spend ledger (#1), threaded through every job. Bounds the
  // worker's total real (Gemini) spend this tick globally + per owner.
  const tickBudget = newTickBudget();
  let scanned = 0;
  try {
    // Collect the FULL due set by FOLLOWING the cursor across pages.
    // jobsDue(startAfter, limit) scans the INDEX WINDOW
    // [startAfter, startAfter+limit) of the enumerable jobIds and returns the
    // due ones in it + nextCursor (the index after the window). A single page
    // from 0 STARVES newer due jobs once terminal (Exhausted/Cancelled) jobs
    // pile up at low indices — so page forward until the index is fully
    // scanned. Bounded (max 64 pages of view calls) so a huge index can't
    // spin. We scan PAST the batch cap on purpose: every due job we will NOT
    // process this tick must still be VISIBLE (a `deferred` result row), so a
    // skip is never silent.
    const dueAll: bigint[] = [];
    const SCAN_PAGE = 64n;
    let cursor = 0n;
    for (let page = 0; page < 64; page++) {
      const { ids: pageIds, nextCursor } = await jobsDue(cursor, SCAN_PAGE);
      for (const id of pageIds) dueAll.push(id);
      if (nextCursor <= cursor) break; // cursor didn't advance => fully scanned
      cursor = nextCursor;
    }
    const due = dueAll.slice(0, MAX_JOBS_PER_TICK);
    scanned = dueAll.length;

    // A due job this tick cannot reach is NEVER silent: log + report it.
    // Its nextRun is untouched, so jobsDue returns it again next tick.
    const defer = (id: bigint, why: string) => {
      console.warn(`[scheduler] job ${id} DEFERRED — ${why}; re-fires next tick (nextRun unchanged)`);
      results.push({
        id: id.toString(),
        outcome: 'deferred',
        ran: 'n/a',
        note: `${why} — re-fires next tick (nextRun unchanged)`,
      });
    };

    // Process SEQUENTIALLY so recordRun txs from the single scheduler account
    // don't collide on the nonce (they share one EOA). Each run's accounting is
    // awaited before the next starts — bounded + ordered.
    for (let i = 0; i < due.length; i++) {
      const id = due[i];
      // MID-BATCH WALL-CLOCK GUARD: if earlier jobs (model loops + receipt
      // waits) already consumed the tick's soft budget, defer the remainder
      // OBSERVABLY rather than letting the platform kill the function
      // mid-batch — which would skip them with no recordRun, no log, and no
      // tick summary (the fleet's "silent fire-skip").
      if (i > 0 && Date.now() - tickStart >= TICK_SOFT_BUDGET_MS) {
        for (const rest of due.slice(i)) defer(rest, 'tick wall-clock budget exhausted');
        break;
      }
      // FAIR-SHARE MODEL DEADLINE: job i may run its model loop until the
      // (i+1)/batchSize fraction of the tick budget. Cumulative, so a quick
      // early job rolls its unused time forward; a heavy early job is
      // truncated (its partial work recorded + noted) instead of eating the
      // whole tick and starving every job behind it.
      const modelDeadline = tickStart + Math.floor((TICK_SOFT_BUDGET_MS * (i + 1)) / due.length);
      try {
        results.push(await processJob(id, tickBudget, modelDeadline));
      } catch (e) {
        // A read failure for one job must not abort the whole tick.
        console.error(`[scheduler] job ${id} unexpected error: ${(e as Error).message}`);
        results.push({ id: id.toString(), outcome: 'skipped', ran: 'n/a', note: (e as Error).message });
      }
    }

    // Due jobs beyond the per-tick batch cap: visible, never silent.
    for (const id of dueAll.slice(MAX_JOBS_PER_TICK)) {
      defer(id, `due but beyond the per-tick job cap (${MAX_JOBS_PER_TICK})`);
    }
  } catch (e) {
    // Loud in the function logs too — a pre-loop failure (RPC scan, etc.)
    // means NOTHING ran this tick, which must be diagnosable after the fact.
    console.error(`[scheduler] tick FAILED before/while processing: ${(e as Error).message}`);
    return new Response(
      JSON.stringify({ error: 'scheduler tick failed: ' + (e as Error).message }),
      { status: 502, headers: { 'content-type': 'application/json' } },
    );
  }

  const recorded = results.filter((r) => r.outcome === 'recorded').length;
  const stale = results.filter((r) => r.outcome === 'stale').length;
  const skipped = results.filter((r) => r.outcome === 'skipped').length;
  // SPILLED = blocked by a per-tick cap (#1); re-fires next tick (NOT dropped).
  const spilled = results.filter((r) => r.outcome === 'spilled').length;
  // DEFERRED = due but never started this tick (batch cap / wall-clock budget);
  // re-fires next tick (NOT dropped). Reported per job so a skip is never silent.
  const deferred = results.filter((r) => r.outcome === 'deferred').length;
  const errored = results.filter((r) => r.ran === 'error').length;
  // /goal jobs that ended themselves this tick (finish_goal → completeJob).
  const goalsCompleted = results.filter((r) => r.goal === 'completed').length;
  // Total generateContent calls across the tick (agent + sub-agent turns) — the
  // metered unit; lets a dogfood POST see the ping-pong fan-out at a glance.
  const totalCalls = results.reduce((acc, r) => acc + (r.calls ?? 0), 0);
  const summary = {
    ok: true,
    scanned,
    recorded,
    stale,
    skipped,
    spilled,
    deferred,
    errored,
    goalsCompleted,
    totalCalls,
    // Spilled/deferred jobs are BY DESIGN, not failures: their nextRun is
    // unchanged and they re-fire next tick (per-tick spend/job/wall-clock caps).
    note:
      spilled + deferred > 0
        ? 'spilled/deferred jobs hit a per-tick cap (spend, job count, or wall-clock); nextRun unchanged — they re-fire next tick'
        : undefined,
    // Real $LH the worker committed to spend this tick (the per-tick cap unit).
    tickSpentWei: tickBudget.global.toString(),
    globalTickCapWei: GLOBAL_TICK_CAP_WEI.toString(),
    perOwnerTickCapWei: PER_OWNER_TICK_CAP_WEI.toString(),
    durationMs: Date.now() - tickStart,
    jobs: results,
  };
  console.log(
    `[scheduler] tick: scanned=${scanned} recorded=${recorded} stale=${stale} skipped=${skipped} spilled=${spilled} deferred=${deferred} errored=${errored} goalsCompleted=${goalsCompleted} calls=${totalCalls} spentWei=${tickBudget.global} in ${summary.durationMs}ms`,
  );
  return new Response(JSON.stringify(summary), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}
