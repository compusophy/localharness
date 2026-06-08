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
// job). See `runPingPong` for the loop + the budget gate. CROSS-TICK recursion
// (a scheduled agent scheduling CHILD jobs from its remaining budget via a
// scheduler-role `scheduleChildJob`) is the deliberate NEXT increment, out of
// this scope.
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
// per-tick batch (see MAX_JOBS_PER_TICK); leftover due jobs spill to the next
// cron tick. (For a future high-volume Node 300s budget, rewrite the handler to
// the `(req, res)` Node signature.)
export const config = { runtime: 'edge' };

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

/** `idOfName(name)` — the token id of a registered name; 0n if unregistered. */
async function idOfName(name: string): Promise<bigint> {
  return (await publicClient().readContract({
    address: REGISTRY as `0x${string}`,
    abi: ID_OF_NAME_ABI,
    functionName: 'idOfName',
    args: [name],
  })) as bigint;
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
// target persona) is replaced by a BOUNDED tool loop: the scheduled agent gets
// ONE tool, `call_agent(name, message)`, so a no-tab scheduled run can ORCHESTRATE
// other localharness agents. Each loop round is one generateContent under the
// JOB's target persona; if the model returns a `call_agent` functionCall we
// resolve that target's on-chain persona, run ONE generateContent for the
// sub-agent (sub-agents are SINGLE turns — no nested loops, so the call tree is
// bounded to depth 1 and can't fan out unbounded), feed its reply back as a
// functionResponse, and continue. Text with no call = the final answer; stop.
//
// The whole run is bounded THREE ways: (1) MAX_PINGPONG_ROUNDS caps the agent's
// own turns; (2) sub-agents never loop; (3) — the HARD ceiling — the per-job $LH
// budget: every generateContent costs COST_WEI and we STOP before any call the
// budget can't cover (see `runPingPong` + `processJob`). A runaway loop simply
// drains the escrow and the facet exhausts the job.
//
// CROSS-TICK recursion (a scheduled agent SCHEDULING child jobs funded from the
// parent's remaining budget, via a scheduler-role `scheduleChildJob` facet fn) is
// the deliberate NEXT increment and is intentionally OUT OF SCOPE here — this
// builds the in-tick, in-budget ping-pong core only.

function defaultPersona(name: string): string {
  return (
    `You are ${name}, an autonomous agent on the localharness platform ` +
    `(a self-sovereign, browser-resident agent network on the Tempo testnet). ` +
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

// The ONE tool the scheduled agent gets. A single-`type` schema with no union /
// additionalProperties (Gemini 400s on those — see CLAUDE.md gotcha).
const CALL_AGENT_TOOL = {
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
  if (withTool) body.tools = [CALL_AGENT_TOOL];
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

/** First functionCall part addressed to `call_agent`, if any. */
function findCallAgent(parts: GeminiPart[]): GeminiFunctionCall | null {
  for (const p of parts) {
    if (p.functionCall && p.functionCall.name === 'call_agent') {
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
  const parts = await generateContent(
    persona,
    [{ role: 'user', parts: [{ text: message }] }],
    false,
  );
  const text = partsText(parts);
  return text.length ? text : '(the agent returned no text)';
}

/** Outcome of a bounded ping-pong run. `calls` = generateContent calls made
 * (the agent's turns + each sub-agent turn) = the unit COST_WEI meters on. */
interface PingPongResult {
  output: string;
  calls: number;
  rounds: number;
  /** True if the loop stopped because the budget couldn't cover the next call. */
  budgetCapped: boolean;
}

/**
 * The bounded agent tool loop ("agent ping-pong").
 *
 * `maxCalls` is how many generateContent calls the JOB's remaining budget can pay
 * for (floor(remaining / COST_WEI), computed by the caller). It's the HARD
 * ceiling: BEFORE every generateContent (the agent's own turn AND each
 * sub-agent turn) we check `calls < maxCalls`; if not, we STOP rather than make a
 * call the budget can't cover. So `calls` returned here is always <= maxCalls,
 * and `processJob` can charge `calls * COST_WEI` knowing it never exceeds
 * budgetWei (the facet reverts SpendExceedsBudget otherwise).
 *
 * Bounds: MAX_PINGPONG_ROUNDS on the agent's turns, single-turn sub-agents (the
 * call tree is depth-1), the budget on total calls, and Edge's wall-clock
 * (rounds × ~3-5s each). A sub-agent error becomes an error functionResponse —
 * the loop continues, never hangs.
 *
 * The caller guarantees `maxCalls >= 1` (it doesn't enter the loop otherwise).
 */
async function runPingPong(
  persona: string,
  task: string,
  maxCalls: number,
): Promise<PingPongResult> {
  const contents: GeminiContent[] = [
    { role: 'user', parts: [{ text: task }] },
  ];
  let calls = 0;
  let lastText = '';
  let budgetCapped = false;

  for (let round = 0; round < MAX_PINGPONG_ROUNDS; round++) {
    // BUDGET GATE (the agent's own turn). Never make a call the budget can't pay.
    if (calls >= maxCalls) {
      budgetCapped = true;
      break;
    }
    calls++;
    const parts = await generateContent(persona, contents, true);

    const call = findCallAgent(parts);
    if (!call) {
      // Pure text → final answer. Stop.
      lastText = partsText(parts) || lastText;
      return { output: lastText || '(the agent returned no text)', calls, rounds: round + 1, budgetCapped };
    }

    // The model wants to ping another agent. Record the model's functionCall
    // turn in history so the subsequent functionResponse is well-formed.
    lastText = partsText(parts) || lastText;
    contents.push({ role: 'model', parts });

    const targetName =
      typeof call.args?.name === 'string' ? (call.args.name as string).trim() : '';
    const subMessage =
      typeof call.args?.message === 'string' ? (call.args.message as string) : '';

    // BUDGET GATE (the sub-agent turn). If we can't pay for the sub-agent call,
    // feed an error response so the agent can still wrap up on its NEXT turn
    // (which itself is budget-gated at the top of the loop) — don't half-run it.
    let responsePayload: Record<string, unknown>;
    if (!targetName || !subMessage) {
      responsePayload = { error: 'call_agent requires non-empty "name" and "message"' };
    } else if (calls >= maxCalls) {
      budgetCapped = true;
      responsePayload = {
        error: 'budget exhausted: not enough remaining $LH to call another agent',
      };
    } else {
      calls++;
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
  }

  // Fell out of the loop: either MAX_PINGPONG_ROUNDS hit or the budget capped us
  // mid-conversation. Return the best text we have (the last model text).
  return {
    output: lastText || '(the agent reached its round/budget limit without a final answer)',
    calls,
    rounds: MAX_PINGPONG_ROUNDS,
    budgetCapped,
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

// ---- one job ----------------------------------------------------------------

interface JobResult {
  id: string;
  outcome: 'recorded' | 'skipped' | 'stale';
  ran: 'ok' | 'error' | 'n/a';
  /** generateContent calls made this run (agent turns + sub-agent turns). */
  calls?: number;
  /** $LH wei actually debited (calls * COST_WEI, capped to budget). */
  spentWei?: string;
  note?: string;
}

/**
 * Process ONE due job: read its full record + task, (re)confirm it's Active and
 * due, run a BOUNDED agent ping-pong loop under its target's persona, then ALWAYS
 * recordRun (advancing nextRun + debiting `calls * COST_WEI`, capped to the
 * budget) — even if Gemini errored, so a broken job can never hot-loop and stays
 * bounded by its budget. The per-job budget bounds the WHOLE ping-pong run.
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
  // accounting-close, not a free model call.
  if (job.budgetWei < COST_WEI) {
    const outcome = await recordRun(id, expectedNextRun, job.budgetWei);
    return {
      id: idStr,
      outcome,
      ran: 'n/a',
      note: 'budget below per-run cost — closed (Exhausted) without a run',
    };
  }

  const name = await nameOfId(job.targetId);

  // BUDGET → MAX CALLS. The per-job budget is the HARD ceiling on the ENTIRE
  // ping-pong run: how many generateContent calls (agent turns + sub-agent turns)
  // can the remaining budget pay for? `floor(budgetWei / COST_WEI)`. We're past
  // the `budgetWei < COST_WEI` dust-close above, so maxCalls >= 1. The loop never
  // makes call N+1 unless `N < maxCalls`, so the total it returns is <= maxCalls,
  // and `min(calls * COST_WEI, budgetWei)` can never exceed budgetWei (the facet
  // reverts SpendExceedsBudget if it did). MAX_PINGPONG_ROUNDS additionally caps
  // the agent's OWN turns (so even a huge budget can't blow the Edge wall-clock).
  const maxCalls = Number(job.budgetWei / COST_WEI);

  // Build + run the bounded loop. Persona resolution + Gemini errors must NOT
  // abort the accounting: capture the outcome + the call count, then record once.
  let ran: 'ok' | 'error' = 'ok';
  let runNote = '';
  // Default to charging for ONE call: if even persona/task READS throw before the
  // loop runs, we still debit a cost so a perpetually-failing job drains its
  // budget and exhausts (never a hot loop). Overwritten by the loop's real count.
  let calls = 1;
  let budgetCapped = false;
  try {
    const persona = (await personaOf(job.targetId)) ?? defaultPersona(name);
    const rawTask = (await taskOf(id)).trim();
    // An empty task = "run under the persona's standing instruction" (sentinel).
    const task = rawTask || 'Perform your scheduled task and report concisely.';
    const result = await runPingPong(persona, task, maxCalls);
    calls = result.calls;
    budgetCapped = result.budgetCapped;
    runNote = result.output;
    // MVP output sink = the Vercel log. The reply is recorded here so a scheduled
    // run is observable in the function logs.
    // TODO: richer output routing — persist the transcript on-chain / in a store,
    // or push the final output to the owner, instead of only logging. (The
    // ping-pong replies ARE chained in-loop now; this TODO is the OUTPUT side.)
    console.log(
      `[scheduler] job ${idStr} target ${name} (#${job.targetId}) ` +
        `calls=${calls}/${maxCalls} rounds=${result.rounds}${budgetCapped ? ' (budget-capped)' : ''} ` +
        `reply: ${runNote.slice(0, 800)}`,
    );
  } catch (e) {
    ran = 'error';
    runNote = (e as Error).message;
    // CRITICAL: still record the run below (calls already defaults to 1). A
    // failing job advances its clock and debits a cost so it re-fires at most once
    // per interval and drains its budget — never a hot loop.
    console.error(`[scheduler] job ${idStr} target ${name} run ERROR (will still recordRun): ${runNote}`);
  }

  // METER: debit calls * COST_WEI, CAPPED to budgetWei (the facet reverts
  // SpendExceedsBudget if spentWei > remaining). `calls` is already bounded by
  // maxCalls in the success path; the `min` is belt-and-suspenders for the error
  // path's default and any rounding. When this debit consumes the budget the
  // facet marks the job Exhausted + refunds the remainder.
  let spentWei = BigInt(calls) * COST_WEI;
  if (spentWei > job.budgetWei) spentWei = job.budgetWei;

  const outcome = await recordRun(id, expectedNextRun, spentWei);
  return {
    id: idStr,
    outcome,
    ran,
    calls,
    spentWei: spentWei.toString(),
    note: ran === 'error' ? runNote.slice(0, 200) : budgetCapped ? 'budget-capped mid-run' : undefined,
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
  // Total generateContent calls across the tick (agent + sub-agent turns) — the
  // metered unit; lets a dogfood POST see the ping-pong fan-out at a glance.
  const totalCalls = results.reduce((acc, r) => acc + (r.calls ?? 0), 0);
  const summary = {
    ok: true,
    scanned,
    recorded,
    stale,
    skipped,
    errored,
    totalCalls,
    durationMs: Date.now() - tickStart,
    jobs: results,
  };
  console.log(
    `[scheduler] tick: scanned=${scanned} recorded=${recorded} stale=${stale} skipped=${skipped} errored=${errored} calls=${totalCalls} in ${summary.durationMs}ms`,
  );
  return new Response(JSON.stringify(summary), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}
