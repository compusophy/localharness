// localharness scheduler worker — the tab-independent job firer (Edge).
//
// This is the engine that makes scheduled agent jobs run WITHOUT a browser
// tab. The durable job registry lives OFF-CHAIN in the GitHub-backed jobstore
// (`_jobstore.ts`; jobs are created/cancelled via /api/schedule): a Vercel Cron
// ticks this function on a crontab, it reads the due set from the store, and
// fires each due job:
//   * REMINDER — a pure web-push of the task text (zero chain, zero $LH).
//   * AGENT — a BOUNDED AGENT PING-PONG loop under the target's on-chain
//     persona (the agent can `call_agent` other localharness agents during the
//     run — multi-agent orchestration with no tab open), billed per
//     generateContent against the OWNER's meter (COST_WEI each, clamped to the
//     live balance — same as an interactive message). See `runPingPong`.
//
// PER-TICK SPEND CAPS (#1): on top of each owner's balance, the worker bounds
// the TOTAL real (Gemini) $LH it commits across one cron tick —
// GLOBAL_TICK_CAP_WEI globally and PER_OWNER_TICK_CAP_WEI per owner. A job that
// would breach either cap SPILLS to the next tick (its file is left unclaimed;
// logged, never dropped). So no matter how many jobs are due, the platform's
// upstream API spend per tick is hard-capped.
//
// PER-TICK WALL-CLOCK BUDGET (TICK_SOFT_BUDGET_MS): the tick also self-limits
// its OWN runtime so the Edge platform never kills it mid-batch (that kill
// SILENTLY skipped every job after a heavy one — no log, no summary). Each
// agent job gets a fair-share model deadline (a heavy run is truncated +
// billed, never starves the rest); a due job the tick cannot reach is left
// UNCLAIMED (logged) and re-fires next tick.
//
// CONCURRENCY: firing is CAS-guarded by the store's sha-conditional
// claim-delete (`claimJob`, _jobstore.ts) — of N overlapping ticks only the
// claim winner runs + bills. Lose-not-duplicate.
//
// SAFETY (this runs autonomously + spends $LH):
//   * CRON_SECRET gate — only Vercel's cron (or a manual dogfood POST carrying
//     the secret) may invoke it; the public cannot trigger a spend.
//   * Error -> still consumed — if the model call ERRORS, the run is still
//     consumed (next slot written, the real calls made are debited) so a broken
//     job re-fires at most once per interval — never a hot loop.
//   * Broke owner -> consumed without a model call — an owner who can't fund
//     even one call has the run consumed for free, so a broke job can't
//     hot-loop the due scan either.
//
// Reuses gemini.ts / mcp.ts setup verbatim: the diamond address, Tempo chain,
// RPC, the PROXY_METER_KEY wallet (also the scheduler-role signer for the
// permissionless `collectTithe` write), persona resolution
// (`metadata(tokenId, keccak256("localharness.persona"))`), and the
// non-streaming Gemini generateContent pattern. GEMINI_API_KEY is in env.

import { keccak_256 } from '@noble/hashes/sha3';
import { bytesToHex } from '@noble/hashes/utils';
import { deliverOwnerPush } from './_notifycore';
import {
  createPublicClient,
  createWalletClient,
  defineChain,
  http,
} from 'viem';
import { privateKeyToAccount } from 'viem/accounts';

// Edge runtime — matches gemini.ts / mcp.ts, which use the SAME Web
// `Request`->`Response` handler shape. That shape runs on Edge, NOT on Vercel's
// Node runtime (a Node function expects `(req, res)`, so a Web handler there
// 500s with FUNCTION_INVOCATION_FAILED). Edge's ~25s wall-clock caps the
// per-tick batch (see MAX_OFFCHAIN_JOBS_PER_TICK + TICK_SOFT_BUDGET_MS);
// leftover due jobs spill to the next cron tick.
export const config = { runtime: 'edge' };

// ---- constants (shared with gemini.ts / mcp.ts) ----------------------------

import { TEMPO_RPC, REGISTRY, CHAIN_ID, FEE_TOKEN } from './_chain';
const GEMINI_BASE = 'https://generativelanguage.googleapis.com';
// Mirrors mcp.ts ASK_MODEL / the headless `call` default. No per-job model
// selection in the MVP — every scheduled run uses the platform Gemini model.
const RUN_MODEL = process.env.MCP_ASK_MODEL ?? 'gemini-3.7-flash';

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

// The OFF-CHAIN job store (GitHub-backed) + the meter debit. The store holds
// the job records (no escrow, no gas) and an AGENT job bills the owner's
// existing meter per run (same as an interactive message — no schedule tax); a
// REMINDER job is a pure web-push (zero chain, zero $LH).
import {
  listDue as listDueOffchain,
  claimJob,
  writeNextSlot,
  jobStoreConfigured,
  MAX_OFFCHAIN_JOBS_PER_TICK,
  type OffchainJob,
} from './_jobstore';
import { meterDebit, creditOf } from './_auth';

// Env assertions + the hourly health self-check (road-to-v1 step 2: the proxy
// is the SPOF and had zero monitoring). The sponsor address / breaker floor are
// the live values sponsor.ts itself signs with — one source of truth.
import { missingEnv } from './_env';
import {
  envHealth,
  sponsorFloatHealth,
  githubHealth,
  alertHealth,
  type HealthCheck,
} from './_health';
import { SPONSOR_ADDRESS, MIN_FLOAT_WEI } from './sponsor';

// ---- per-TICK spend caps (#1 — the strongest bill-shock fix) ----------------
//
// The owner's meter balance bounds ONE job's run. These two caps bound the
// WHOLE TICK across ALL jobs/owners, so the worker's real upstream (Gemini) cost
// per cron invocation is HARD-bounded regardless of how many jobs are due or how
// funded individual owners are — the platform's API spend per tick cannot exceed
// GLOBAL_TICK_CAP_WEI, and no single owner's jobs can consume more than
// PER_OWNER_TICK_CAP_WEI of that in one tick.
//
// Enforcement (fireOffchainJob + runPingPong): we track a running tick total and
// a per-owner running total. BEFORE running a job — and BEFORE EACH metered call
// inside runPingPong — we check that the projected spend (running total + this
// call's COST_WEI) stays under both caps. If a call would breach either cap we
// STOP the job there; its file is left unclaimed, so it SPILLS to the next tick
// (logged, never silently dropped). These are an ADDITIONAL ceiling on top of
// every existing bound (owner balance, MAX_PINGPONG_ROUNDS,
// MAX_OFFCHAIN_JOBS_PER_TICK).

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
// skipped with NO log line and NO tick summary — the fleet's "goal job silently
// never fired" repro. Two fixes hang off this soft budget:
//   * FAIR-SHARE MODEL DEADLINES — batch job i may run its model loop until
//     the (i+1)/batchSize fraction of the budget (cumulative, so a quick early
//     job rolls unused time forward). A heavy early job is TRUNCATED — its
//     partial work is still billed + noted 'wall-clock capped' — instead of
//     starving every job behind it.
//   * OBSERVABLE DEFERRALS — any due job the tick cannot reach (batch cap or
//     budget already gone) is left UNCLAIMED with a log line instead of
//     vanishing with a killed function; it re-fires next tick.
const TICK_SOFT_BUDGET_MS = ((): number => {
  const n = Number(process.env.SCHEDULER_TICK_SOFT_BUDGET_MS ?? '20000');
  return Number.isFinite(n) && n >= 5000 ? Math.min(Math.trunc(n), 290_000) : 20_000;
})();

// How many DUE off-chain files the tick READS bodies for (the scan window),
// kept LARGER than the agent processing cap (MAX_OFFCHAIN_JOBS_PER_TICK). The
// store sorts due files most-overdue-first and slices to the limit BEFORE the
// reminder/agent split, so a burst of overdue AGENT jobs could slice
// time-sensitive REMINDERS out of the batch entirely (the "remind me in 15 min
// is never starved" guarantee broke — L48). Scanning a wider window then firing
// ALL due reminders (cheap pushes, no model / wall-clock) on top of up-to-cap
// agent runs reserves capacity for reminders. Env-overridable; floored at the
// processing cap and clamped under the store's 1000-entry listing cap.
const OFFCHAIN_DUE_SCAN = ((): number => {
  const n = Number(process.env.SCHEDULER_OFFCHAIN_DUE_SCAN ?? '64');
  const floor = MAX_OFFCHAIN_JOBS_PER_TICK;
  return Number.isFinite(n) && n > 0
    ? Math.min(Math.max(Math.trunc(n), floor), 1000)
    : Math.max(64, floor);
})();

const TEMPO_CHAIN = defineChain({
  id: CHAIN_ID,
  name: 'Tempo Moderato',
  nativeCurrency: { name: 'Tempo', symbol: 'TEMPO', decimals: 18 },
  rpcUrls: { default: { http: [TEMPO_RPC] } },
});

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

// Self-recorded lessons slot — written by the browser app's record_lesson
// tool (src/app/chat/tools/misc.rs; merge bounds in src/lessons.rs).
// keccak256("localharness.lessons"), precomputed + inlined; pinned by the
// Rust test `lessons_key_distinct_from_other_metadata_keys`.
const LESSONS_KEY =
  '0x08564cae936ec460c48a23578c7df5665bad18fe42f3c5dbde517ad67a9d9c89' as `0x${string}`;

// ---- on-chain reads (viem readContract; same RPC/diamond as gemini.ts) ------

function publicClient() {
  return createPublicClient({ chain: TEMPO_CHAIN, transport: http(TEMPO_RPC) });
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

/**
 * Send ONE Web Push {title, body} to every device subscription in `owner`'s
 * OFF-CHAIN push-store blob (`push-subs/<address>.json`, _pushstore.ts — the
 * ONLY enroll source). Returns true iff a push service accepted. NEVER
 * throws: missing VAPID env / no subscription / a send failure all resolve
 * to false. Bounded: one store read + 5s-capped POSTs. The shared plumbing
 * behind both [`notifyOwnerOfRun`] (the post-run summary) and the agent's
 * `notify_owner` tool (the in-run "buzz my owner" affordance, #69).
 */
async function sendOwnerPush(
  owner: string,
  title: string,
  body: string,
): Promise<boolean> {
  // Thin delegate — the delivery core moved to _notifycore.ts (shared with
  // any future server-side owner push; telemetry #78).
  return deliverOwnerPush(owner, title, body);
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
  jobId: string,
  targetName: string,
  output: string,
): Promise<void> {
  try {
    const body = output.length > 120 ? `${output.slice(0, 119)}…` : output;
    await sendOwnerPush(owner, `${targetName} job #${jobId}`, body);
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
// AGENT PING-PONG. A scheduled AGENT run is a BOUNDED tool loop: the agent gets
// `call_agent(name, message)` (consult/delegate to another agent IN-TICK),
// `notify_owner(title, body)` (Web-Push the JOB OWNER's registered device —
// feedback #69), `finish_goal(report)` (end a /goal job), and
// `collect_tithe(account)` (trigger the permissionless TitheFacet revenue→
// treasury pull for a consented account). Each loop round is one
// generateContent under the JOB's target persona; a `call_agent` functionCall
// resolves that target's on-chain persona and runs ONE generateContent for the
// sub-agent (sub-agents are SINGLE turns — no nested loops, so the call tree is
// bounded to depth 1), feeding the reply back as a functionResponse. Text with
// no call = the final answer; stop.
//
// The whole run is bounded by: (1) MAX_PINGPONG_ROUNDS caps the agent's own
// turns; (2) sub-agents never loop; (3) the per-run budget (the owner's live
// meter balance, capped per fire) — every generateContent costs COST_WEI and we
// STOP before any call the budget can't cover; (4) the per-TICK caps (#1) — the
// running global/per-owner tick spend can't exceed GLOBAL_TICK_CAP_WEI /
// PER_OWNER_TICK_CAP_WEI, else the run STOPS and spills.

// ---- /goal — the ralph goal loop ---------------------------------------------
//
// A job whose task begins with the exact marker `GOAL: ` is a GOAL LOOP (the
// Ralph technique): the SAME goal prompt is re-fed every iteration, durable
// progress lives in external (e.g. on-chain) state — not the model's memory,
// there is none across ticks — and the loop ends itself when the agent verifies
// the goal is met and calls `finish_goal`, which ENDS the job in the store (no
// next slot is written) and pushes the final report to the owner. Until then,
// every fire is one bounded iteration: inspect state, take the single most
// valuable next step, leave a progress note.

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
// additionalProperties (Gemini 400s on those — see CLAUDE.md gotcha). FOUR tools:
//   * call_agent      — consult/delegate to another agent THIS run (in-tick).
//   * notify_owner    — Web-Push a note to the JOB OWNER's registered device
//                       (feedback #69). Budget-counted like a model call so a
//                       loop can't spam the owner's phone.
//   * finish_goal     — declare the job's GOAL verifiably complete: ends the
//                       recurring job (no next slot is written) and pushes the
//                       final report to the owner. The /goal ralph-loop exit.
//   * collect_tithe   — trigger TitheFacet.collectTithe(account), the
//                       PERMISSIONLESS revenue→treasury pull. Zero new authority
//                       (the facet pulls only the account's OWN consented share
//                       into its OWN guild); budget-counted like a model call for
//                       anti-spam. The treasurer-without-a-tab affordance.
//
// NOTE: there is NO `post_bounty` tool. The existing permissionless
// `BountyFacet.postBounty` escrows from `msg.sender` (the scheduler key → the
// PLATFORM would fund the reward, not the owner) and gates accept/cancel on the
// poster (→ the bounty + its refund strand under the PLATFORM). See `findToolCall`.
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
        'Declare this scheduled job\'s GOAL complete. This permanently ENDS the recurring job — there are no more iterations after this. Call it ONLY when you have verified, against current state, that the goal is fully achieved. Pass a final report summarizing the outcome and the evidence.',
      parameters: {
        type: 'object',
        properties: {
          report: {
            type: 'string',
            description:
              'The final outcome summary: what was achieved, and the evidence that proves the goal is complete.',
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
 * notify_owner / finish_goal / collect_tithe), if any.
 *
 * NOTE: `post_bounty` is deliberately NOT here. It cannot reuse the existing
 * permissionless `BountyFacet.postBounty` from the scheduler key — that escrows
 * `transferFrom(msg.sender, …)` (so the PLATFORM funds the reward, not the
 * owner) and gates `acceptResult`/`cancelBounty` on `msg.sender == poster` (so
 * the bounty + its refund strand under the PLATFORM, never the owner). Wiring
 * it would silently spend platform funds and break the trust envelope. */
function findToolCall(parts: GeminiPart[]): GeminiFunctionCall | null {
  for (const p of parts) {
    if (
      p.functionCall &&
      (p.functionCall.name === 'call_agent' ||
        p.functionCall.name === 'notify_owner' ||
        p.functionCall.name === 'finish_goal' ||
        p.functionCall.name === 'collect_tithe')
    ) {
      return p.functionCall;
    }
  }
  return null;
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
   * ENDS the job (writes no next slot) and pushes the report to the owner. */
  goalReport?: string;
  /** Set when the agent's OWN model turn (generateContent) threw mid-loop: the
   * error message. `calls` still carries the TRUE count made so far, so the
   * caller bills the real spend (NOT a hard-coded 1) and the on-chain debit /
   * meter stays in lockstep with the per-tick ledger (L44). */
  error?: string;
}

/**
 * The bounded agent tool loop ("agent ping-pong").
 *
 * Every metered action passes TWO gates that are ANDed together:
 *   1. PER-RUN budget — `maxCalls` = how many COST_WEI units this run may spend
 *      (computed by the caller from the owner's live meter balance, capped per
 *      fire). We never make call N+1 unless `calls < maxCalls`, so `calls`
 *      returned here is always <= maxCalls and the caller can debit
 *      `calls * COST_WEI` clamped to the live balance.
 *   2. PER-TICK caps (#1) — `canSpend(tb, owner, COST_WEI)`: the running
 *      tick-global + per-owner totals + this call must stay under
 *      GLOBAL_TICK_CAP_WEI / PER_OWNER_TICK_CAP_WEI. If not, we STOP
 *      (`tickCapped`).
 * BEFORE every metered action (the agent's own turn and each sub-agent turn) we
 * check BOTH and `commitSpend` after counting it. A blocked action by EITHER
 * gate halts the run.
 *
 * Tools the agent may call: `call_agent` (consult/delegate in-tick, depth-1 sub-
 * agents that never recurse), `notify_owner` (Web-Push a note to the JOB owner's
 * registered device via `sendOwnerPush` — the owner is wired from the job
 * record, never from model args, so a run can only ever buzz its OWN owner),
 * `finish_goal` (end a /goal job with a final report), and `collect_tithe`
 * (trigger the permissionless `TitheFacet.collectTithe(account)` — the facet
 * pulls only the account's OWN consented share into its OWN guild, so the
 * scheduler signs it with zero new authority). A tool error (bad args,
 * unregistered target, facet revert) becomes an error functionResponse — the
 * loop continues, never hangs.
 *
 * Bounds: MAX_PINGPONG_ROUNDS on the agent's turns, single-turn sub-agents, the
 * per-run budget, the per-tick caps, and Edge's wall-clock. The caller
 * guarantees `maxCalls >= 1` (it doesn't enter the loop otherwise).
 */
async function runPingPong(
  persona: string,
  task: string,
  maxCalls: number,
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
    if (calls > 0 && Date.now() >= modelDeadlineMs) {
      // Wall-clock fair share spent: stop HERE so the jobs behind this one in
      // the batch still get processed (and so the platform never kills the
      // function mid-batch, which would skip them SILENTLY). The FIRST call
      // (`calls === 0`) is ALWAYS allowed past the CLOCK gate (it still honors
      // the per-job budget + per-tick caps above): a job the caller already
      // SELECTED/CLAIMED — especially an off-chain agent job, whose claim
      // already DELETED its file — must get at least one model call, else it is
      // consumed (a one-shot job lost) having never run the model (M10).
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
    let parts: GeminiPart[];
    try {
      parts = await generateContent(persona, contents, true);
    } catch (e) {
      // The agent's OWN model turn failed (Gemini non-2xx). `calls` already
      // counts this turn (incremented + committed to the shared ledger BEFORE
      // the await), as do any earlier rounds (e.g. a successful call_agent in
      // round 0). RETURN with the TRUE count instead of throwing: the old throw
      // lost the partial count, so the callers hard-coded 1 and UNDER-BILLED the
      // owner by (N-1) while the ledger held N (and the off-chain catch even
      // double-committed) — L44. Surfacing it as a result keeps the on-chain
      // debit / meter debit and the per-tick ledger in lockstep.
      return {
        output: (e as Error).message,
        calls,
        rounds: round + 1,
        budgetCapped,
        tickCapped,
        clockCapped,
        error: (e as Error).message,
      };
    }

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

    // GOAL COMPLETE (finish_goal). Not metered — it's not a model call; it only
    // ENDS the job (the caller writes no next slot). The report is the run's
    // final output; the loop stops HERE.
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

    if (call.name === 'notify_owner') {
      // OWNER PUSH (the goal-loop "notify my owner" affordance — #69).
      // Sends to the JOB OWNER's push-store subscriptions via the same
      // sendOwnerPush plumbing as the post-run summary; the owner comes
      // from the job record, NOT from model args (a run can only buzz its own
      // owner). Not a model call, but COUNTED through the same gate + ledger
      // as one: each push costs COST_WEI from the run's budget, so a runaway
      // loop can't spam the owner's phone for free.
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
        // sendOwnerPush never throws; false = unconfigured push, no enrolled
        // subscription, or the push service rejected — the agent can report
        // that in its final answer instead of retrying.
        const sent = await sendOwnerPush(owner, title, pushBody);
        responsePayload = sent
          ? { sent: true }
          : {
              sent: false,
              note: 'push not delivered (owner has no enrolled push subscription, or the push service refused)',
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
      // but COUNTED through the same gate + ledger as one (mirrors
      // notify_owner): it spends scheduler gas + is an agent-initiated action, so
      // metering it keeps a loop from spamming collectTithe for free AND keeps the
      // per-run-budget / per-tick-cap accounting in lockstep. No new authority —
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

// ---- scheduler-role writes (PROXY_METER_KEY signs; gas-only) -----------------

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
 * by the scheduler key. Simulate first so a facet revert (NotConfigured /
 * UnknownGuild / NothingToCollect) is decoded into a readable reason BEFORE
 * spending gas, then write + await the 12s receipt.
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

// ---- job firing (GitHub store) -----------------------------------------------
//
// Fires ONE due job, reusing the shared helpers (persona/lessons, runPingPong,
// sendOwnerPush):
//   * REMINDER — no model call, no charge: web-push `task` to the owner, consume
//     the run. Zero chain, zero $LH. This is the "notify me in 15 minutes" case.
//   * AGENT — run the target agent (bounded ping-pong), then debit the OWNER's
//     meter for the calls made (clamped to live balance, exactly like an
//     interactive message).
// Outcome is committed to the store: advance the file to nextRun+interval, or
// delete it when exhausted / on a finish_goal. Per-tick caps (#1) bound agent
// runs on top of the owner's balance via the shared tick ledger `tb`.

interface OffchainResult {
  id: string;
  kind: OffchainJob['kind'];
  outcome: 'pushed' | 'ran' | 'skipped' | 'spilled' | 'exhausted' | 'error';
  calls?: number;
  spentWei?: string;
  note?: string;
}

async function fireOffchainJob(
  entry: { job: OffchainJob; path: string; sha: string },
  tb: TickBudget,
  modelDeadlineMs: number,
): Promise<OffchainResult> {
  const { job, path, sha } = entry;
  const fallbackTokenId = (() => {
    try {
      return BigInt(job.targetId);
    } catch {
      return 0n;
    }
  })();

  // REMINDER — pure web-push, no model, no charge. CLAIM first (CAS via the
  // sha-conditional delete): only the delete-winner pushes + advances, so
  // overlapping ticks can't double-push; a lost claim (another tick won, or a
  // transient failure) skips and the file re-fires next tick if still present.
  if (job.kind === 'reminder') {
    if (!(await claimJob(path, sha))) {
      return { id: job.id, kind: 'reminder', outcome: 'skipped', note: 'lost the fire race (or transient) — not pushed' };
    }
    let pushed = false;
    try {
      pushed = await sendOwnerPush(job.owner, 'Reminder', job.task.slice(0, 200));
    } catch {
      /* a push failure never fails — or re-fires — the reminder */
    }
    const next = await writeNextSlot(job);
    console.log(`[scheduler] offchain reminder ${job.id} owner ${job.owner.slice(0, 10)} pushed=${pushed} ${next ? `(${next.runsLeft} left)` : '(done)'}`);
    return {
      id: job.id,
      kind: 'reminder',
      outcome: next ? 'pushed' : 'exhausted',
      note: pushed ? undefined : 'no enrolled push subscription (reminder consumed)',
    };
  }

  // AGENT — per-tick cap gate FIRST, BEFORE any claim: if the tick can't afford
  // one call, SPILL — do NOT claim (leave the file so it re-fires next tick).
  if (!canSpend(tb, job.owner, COST_WEI)) {
    return { id: job.id, kind: 'agent', outcome: 'spilled', note: 'per-tick spend cap — re-fires next tick' };
  }

  // CLAIM (CAS) — the serialization point. Only the delete-winner runs + bills;
  // a lost claim (overlapping tick won, or a transient delete failure) skips
  // WITHOUT billing.
  // After a win the old slot is GONE: we MUST writeNextSlot (or leave it
  // exhausted) so the job is represented again — lose-not-duplicate (a crash
  // before that drops ONE fire, never a double-charge).
  if (!(await claimJob(path, sha))) {
    return { id: job.id, kind: 'agent', outcome: 'skipped', note: 'lost the fire race (or transient) — not billed' };
  }

  // The owner's LIVE meter balance is the budget (no escrow). If it can't fund
  // even one call, skip the run but CONSUME it (write next slot) so a broke job
  // never hot-loops every tick.
  let credit: bigint;
  try {
    credit = await creditOf(job.owner);
  } catch {
    credit = 0n;
  }
  if (credit < COST_WEI) {
    await writeNextSlot(job);
    return { id: job.id, kind: 'agent', outcome: 'skipped', note: 'owner out of $LH (run skipped, consumed)' };
  }

  // Balance → max calls, additionally capped so one run can't drain a fat
  // balance in a single fire (bound the per-run blast radius). Past the
  // credit-floor check, so maxCalls >= 1.
  const maxCalls = Math.max(1, Math.min(Number(credit / COST_WEI), MAX_PINGPONG_ROUNDS * 2));
  const name = await nameOfId(fallbackTokenId);

  let calls = 0;
  let ran: 'ok' | 'error' = 'ok';
  let note = '';
  let goalReport: string | undefined;
  // `ranLoop` = the ping-pong loop actually started (and self-committed its calls
  // to the tick ledger). A PRE-loop read failure (persona/lessons) leaves it
  // false → the loop committed nothing, so the catch charges + commits ONE call.
  // Replaces the old `calls === 0` guard that double-committed when runPingPong
  // threw mid-loop after committing N (that throw is gone — L44).
  let ranLoop = false;
  try {
    const basePersona = withLessons(
      (await personaOf(fallbackTokenId)) ?? defaultPersona(name),
      await lessonsOf(fallbackTokenId),
    );
    const rawTask = job.task.trim();
    const isGoal = rawTask.startsWith(GOAL_PREFIX);
    const persona = isGoal ? goalSystemPrompt(basePersona, job.runsLeft, credit) : basePersona;
    const task = isGoal
      ? `THE GOAL:\n${rawTask.slice(GOAL_PREFIX.length).trim()}`
      : rawTask || 'Perform your scheduled task and report concisely.';
    ranLoop = true;
    const result = await runPingPong(
      persona,
      task,
      maxCalls,
      job.owner,
      tb,
      modelDeadlineMs,
    );
    calls = result.calls;
    goalReport = result.goalReport;
    note = result.output;
    // A model error inside the loop is RETURNED with the true call count (L44):
    // mark the run errored + bill the real `calls` (the meter debit below uses
    // `calls`); the catch now only fires for a PRE-loop read failure.
    if (result.error !== undefined) ran = 'error';
    console.log(
      `[scheduler] offchain agent ${job.id} target ${name} calls=${calls}/${maxCalls} reply: ${note.slice(0, 600)}`,
    );
  } catch (e) {
    ran = 'error';
    note = (e as Error).message;
    // Only a PRE-loop read failure reaches here now (runPingPong RETURNS its own
    // model errors with the true count). The loop never started, so it committed
    // nothing: charge ONE call + commit it so the ledger matches the meter debit
    // below. (Gating on `!ranLoop` instead of `calls === 0` removes the old
    // double-commit when runPingPong threw mid-loop after committing N — L44.)
    if (!ranLoop) {
      calls = 1;
      commitSpend(tb, job.owner, COST_WEI);
    }
    console.error(`[scheduler] offchain agent ${job.id} target ${name} ERROR: ${note}`);
  }

  // DEBIT the owner's meter for the calls made, CLAMPED to live balance (mirrors
  // gemini.ts: never debit more than the caller holds). A debit revert/timeout
  // is non-fatal — the run already happened; we still commit the store outcome.
  let spentWei = BigInt(calls) * COST_WEI;
  let liveCredit = credit;
  try {
    liveCredit = await creditOf(job.owner);
  } catch {
    /* keep the start-of-run snapshot */
  }
  if (spentWei > liveCredit) spentWei = liveCredit;
  if (spentWei > 0n) {
    try {
      await meterDebit(job.owner, spentWei, true);
    } catch (e) {
      console.warn(`[scheduler] offchain agent ${job.id} meter debit (${spentWei}) failed: ${(e as Error).message}`);
    }
  }

  // COMMIT the store outcome (the claim already deleted the old slot). A
  // finish_goal report ENDS the job — write NO next slot (it stays deleted) and
  // push the report. Otherwise write the next (drift-corrected) slot, or leave it
  // exhausted when runs are spent.
  let outcome: OffchainResult['outcome'];
  if (goalReport !== undefined) {
    outcome = 'exhausted';
    if (ran === 'ok') await notifyOwnerOfRun(job.owner, job.id, `GOAL COMPLETE: ${name}`, goalReport);
  } else {
    const next = await writeNextSlot(job);
    outcome = next ? 'ran' : 'exhausted';
    // Push the result only on a TERMINAL run (last fire) — a recurring job that
    // pushed every run would buzz the owner once an interval while it works.
    if (ran === 'ok' && next === null) {
      await notifyOwnerOfRun(job.owner, job.id, name, note);
    }
  }

  return {
    id: job.id,
    kind: 'agent',
    outcome: ran === 'error' ? 'error' : outcome,
    calls,
    spentWei: spentWei.toString(),
    note: ran === 'error' ? note.slice(0, 200) : undefined,
  };
}

/**
 * Fire the due set this tick. `tb` is the tick's shared spend ledger, so agent
 * runs count against the per-tick spend caps (#1). Bounded by
 * MAX_OFFCHAIN_JOBS_PER_TICK + the tick's remaining wall-clock.
 */
async function fireOffchainDue(
  tb: TickBudget,
  tickStart: number,
): Promise<{ scanned: number; results: OffchainResult[] }> {
  if (!jobStoreConfigured()) return { scanned: 0, results: [] };
  const now = Math.floor(Date.now() / 1000);
  let due: { job: OffchainJob; path: string; sha: string }[];
  try {
    // Scan a WIDER window than we process (OFFCHAIN_DUE_SCAN) so a burst of
    // overdue AGENT jobs can't slice time-sensitive REMINDERS out of the due set
    // before the kind split below (L48). Agents are still capped to
    // MAX_OFFCHAIN_JOBS_PER_TICK when we split.
    due = await listDueOffchain(now, OFFCHAIN_DUE_SCAN);
  } catch (e) {
    console.error(`[scheduler] offchain due scan failed: ${(e as Error).message}`);
    return { scanned: 0, results: [] };
  }
  const results: OffchainResult[] = [];

  // REMINDERS FIRST, exempt from the wall-clock gate: a reminder is ~one push
  // (no model, no receipt wait), and the advertised "remind me in 15 minutes"
  // case must NOT be starved by slow agent runs that already spent the tick's
  // soft budget. Fire them all (claim-gated, so still single-fire).
  const reminders = due.filter((d) => d.job.kind === 'reminder');
  // Cap AGENT runs at the processing budget (the wider scan was only to keep
  // reminders in the batch — L48); agents beyond the cap are left UNCLAIMED, so
  // they re-fire next tick.
  const agents = due.filter((d) => d.job.kind === 'agent').slice(0, MAX_OFFCHAIN_JOBS_PER_TICK);
  for (const entry of reminders) {
    try {
      results.push(await fireOffchainJob(entry, tb, Date.now()));
    } catch (e) {
      console.error(`[scheduler] offchain reminder ${entry.job.id} unexpected error: ${(e as Error).message}`);
      results.push({ id: entry.job.id, kind: 'reminder', outcome: 'error', note: (e as Error).message });
    }
  }

  // AGENT jobs share the tick's remaining wall-clock: stop STARTING new ones
  // once the soft budget is gone (they re-fire next tick — their file is
  // untouched because we never claimed it).
  for (let i = 0; i < agents.length; i++) {
    const nowMs = Date.now();
    // Per-agent fair share of the REMAINING wall-clock over the REMAINING agents,
    // anchored to NOW — not tickStart. The agent batch runs AFTER the reminders,
    // so a tickStart-anchored deadline (the old `tickStart + budget*(i+1)/N`)
    // could already be IN THE PAST: fireOffchainJob would CLAIM (delete) the
    // job, runPingPong would clock-cap at 0 calls, and writeNextSlot would
    // CONSUME the fire — a one-shot agent job lost having never run the model
    // (M10). SPILL the rest (leave the files untouched so they re-fire next
    // tick) once no usable wall-clock remains. (runPingPong additionally
    // GUARANTEES the first model call past its clock gate, so a claimed job
    // always runs at least once.)
    const slice = Math.floor((TICK_SOFT_BUDGET_MS - (nowMs - tickStart)) / (agents.length - i));
    if (slice <= 0) {
      console.warn(`[scheduler] offchain: ${agents.length - i} agent job(s) deferred — tick wall-clock budget exhausted`);
      break;
    }
    const modelDeadline = nowMs + slice;
    try {
      results.push(await fireOffchainJob(agents[i], tb, modelDeadline));
    } catch (e) {
      console.error(`[scheduler] offchain agent ${agents[i].job.id} unexpected error: ${(e as Error).message}`);
      results.push({ id: agents[i].job.id, kind: 'agent', outcome: 'error', note: (e as Error).message });
    }
  }
  return { scanned: due.length, results };
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

// ---- env assertions + hourly health self-check (road-to-v1 step 2) ----------
//
// The scheduler is the platform's heartbeat (it already fires every minute), so
// it carries (a) the fail-LOUD assertion for ITS critical env — a missing
// GEMINI_API_KEY bills owners for runs that can never succeed (error runs are
// still consumed + billed); a missing PROXY_METER_KEY runs the model then can't
// bill = free inference — and (b) the HOURLY health block (minute==0): sponsor-float
// headroom (warn at 10x the LH_RELAY_MIN_FLOAT_WEI breaker floor, not at
// death), GitHub store reachability (one cheap read), and an env dry-run across
// routes. A failing set files ONE deduped telemetry issue (_ghissue rails) +
// best-effort LH_ALERT_OWNER web-push (unset ⇒ the issue IS the alert).
const SCHEDULER_REQUIRED_ENV = ['GEMINI_API_KEY', 'PROXY_METER_KEY'];
// Cross-route dry-run set for the hourly check. LH_SPONSOR_KEY is only required
// on mainnet (testnet keeps the committed play-money fallback by design);
// optional-by-design toggles (VAPID_*, TURN_*, LH_METER_PAYEE) are NOT here.
const HEALTH_ENV = [
  ...SCHEDULER_REQUIRED_ENV,
  'CRON_SECRET',
  ...(CHAIN_ID === 4217 ? ['LH_SPONSOR_KEY'] : []),
];
// Every GitHub-store route (jobs/push/apps/chat/signal) falls back to the
// telemetry PAT — at least one of the pair must exist.
const HEALTH_ENV_ANYOF = [['GH_JOBS_TOKEN', 'GH_TELEMETRY_TOKEN']];

function alertDeps() {
  return {
    repo: process.env.LH_TELEMETRY_REPO ?? 'compusophy/localharness-telemetry',
    token: process.env.GH_TELEMETRY_TOKEN ?? '',
    pushOwner: process.env.LH_ALERT_OWNER,
    push: sendOwnerPush,
  };
}

/** The hourly checks (each I/O read is 5s-capped in _health.ts, so a health
 * pass can't eat the tick's wall-clock budget). */
async function runHealthChecks(): Promise<HealthCheck[]> {
  return [
    envHealth(HEALTH_ENV, HEALTH_ENV_ANYOF),
    await sponsorFloatHealth(TEMPO_RPC, FEE_TOKEN, SPONSOR_ADDRESS, MIN_FLOAT_WEI),
    await githubHealth(
      process.env.GH_JOBS_REPO ?? 'compusophy/localharness-jobs',
      process.env.GH_JOBS_TOKEN ?? process.env.GH_TELEMETRY_TOKEN ?? '',
    ),
  ];
}

function misconfigResponse(missing: string[]): Response {
  return new Response(
    JSON.stringify({
      error: `scheduler misconfigured: missing ${missing.join(', ')}`,
      code: 'LH_PROXY_MISCONFIG',
    }),
    { status: 503, headers: { 'content-type': 'application/json' } },
  );
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

  // FAIL-LOUD env assertion (see the SCHEDULER_REQUIRED_ENV block above).
  // Cron-authed callers get the deduped alert filed below.
  const misconfigured = missingEnv(SCHEDULER_REQUIRED_ENV);

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

  // Cron-authed misconfig: file the deduped alert (self-alerting within one
  // cron minute — one open issue per missing-set) then fail loud.
  if (misconfigured.length > 0) {
    await alertHealth(
      [{ name: 'env', ok: false, detail: `missing ${misconfigured.join(', ')}` }],
      alertDeps(),
    );
    return misconfigResponse(misconfigured);
  }

  // HOURLY health self-check (minute==0; `?health=1` forces it on an authed
  // manual POST). Runs BEFORE the batch so a low sponsor float / unreachable
  // GitHub store is alerted even on a tick with no due jobs; issue dedupe makes
  // an extra run (isolate churn, forced runs) a no-op.
  let health: HealthCheck[] | undefined;
  if (new URL(req.url).searchParams.has('health') || new Date().getUTCMinutes() === 0) {
    health = await runHealthChecks();
    const failing = health.filter((c) => !c.ok);
    if (failing.length > 0) {
      const alert = await alertHealth(failing, alertDeps());
      console.warn(
        `[scheduler] health: ${failing.length} failing — issue ${alert.url ?? 'not filed'}${alert.deduped ? ' (deduped)' : ''}`,
      );
    }
  }

  const tickStart = Date.now();
  // ONE per-tick spend ledger (#1), threaded through every job. Bounds the
  // worker's total real (Gemini) spend this tick globally + per owner.
  const tickBudget = newTickBudget();

  // The due set (GitHub store). Self-bounded (job cap + wall-clock); never
  // throws (its own try/catch).
  const offchain = await fireOffchainDue(tickBudget, tickStart);
  const offchainPushed = offchain.results.filter((r) => r.outcome === 'pushed').length;
  const offchainRan = offchain.results.filter((r) => r.outcome === 'ran' || r.outcome === 'exhausted').length;
  const offchainErrored = offchain.results.filter((r) => r.outcome === 'error').length;

  // Total generateContent calls across the tick (agent + sub-agent turns) — the
  // metered unit; lets a dogfood POST see the ping-pong fan-out at a glance.
  const totalCalls = offchain.results.reduce((acc, r) => acc + (r.calls ?? 0), 0);
  const summary = {
    ok: true,
    totalCalls,
    // Real $LH the worker committed to spend this tick (the per-tick cap unit).
    tickSpentWei: tickBudget.global.toString(),
    globalTickCapWei: GLOBAL_TICK_CAP_WEI.toString(),
    perOwnerTickCapWei: PER_OWNER_TICK_CAP_WEI.toString(),
    durationMs: Date.now() - tickStart,
    // Hourly self-check results (only on the minute==0 / ?health=1 ticks).
    health,
    // The jobstore firing this tick. (Spilled jobs are BY DESIGN, not failures:
    // their file is unclaimed and they re-fire next tick — per-tick caps.)
    offchainScanned: offchain.scanned,
    offchainPushed,
    offchainRan,
    offchainErrored,
    offchainJobs: offchain.results,
  };
  console.log(
    `[scheduler] tick: scanned=${offchain.scanned} pushed=${offchainPushed} ran=${offchainRan} errored=${offchainErrored} calls=${totalCalls} spentWei=${tickBudget.global} in ${summary.durationMs}ms`,
  );
  return new Response(JSON.stringify(summary), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}
