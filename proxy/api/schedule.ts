// /api/schedule — OFF-CHAIN scheduled-job WRITE path (create / cancel / list).
//
// The browser `schedule_task`/`cancel_task` tools and the CLI `schedule`/`goal`/
// `jobs`/`unschedule` POST here instead of submitting a sponsored Tempo tx to
// ScheduleFacet. Jobs are off-chain data (see `_jobstore.ts`): no gas, no
// escrow, no sponsor drain. A `reminder` job is just a future web-push (zero
// chain, zero $LH); an `agent` job runs the target agent each fire, billed per
// run from the OWNER's existing meter (same cost as an interactive message — no
// schedule tax). The cron worker (`scheduler.ts`) fires the due set.
//
// Auth = the SAME personal-sign token as gemini.ts/publish.ts/telemetry.ts
// (`address:timestamp:signature` in `x-goog-api-key`, 300s freshness). The
// authed address is the job OWNER — the billing + push identity. cancel/list
// only ever touch the caller's OWN jobs (owner match).

import { verifyAuthToken, ethCall, selector, isAllowedOrigin } from './_auth';
import { bytesToHex } from '@noble/hashes/utils';
import {
  createJob,
  findById,
  listByOwner,
  countByOwner,
  deleteJob,
  jobStoreConfigured,
  MAX_TASK_BYTES,
  MIN_INTERVAL_SECS,
  MAX_RUNS,
  MAX_JOBS_PER_OWNER,
  type JobKind,
  type OffchainJob,
} from './_jobstore';

export const config = { runtime: 'edge' };

// --- CORS (same policy as publish.ts / telemetry.ts) -------------------------
function corsHeaders(origin: string | null): Record<string, string> {
  const h: Record<string, string> = {
    'Access-Control-Allow-Methods': 'POST, OPTIONS',
    'Access-Control-Allow-Headers': 'content-type, x-goog-api-key, x-api-key',
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

/** ABI-encode a single `string` arg (offset 0x20 | length | utf8 padded) —
 * mirrors publish.ts (idOfName takes one string). */
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

/** `idOfName(string) -> uint256`. 0n = unregistered. */
async function idOfName(name: string): Promise<bigint> {
  const res = await ethCall('0x' + selector('idOfName(string)') + encodeStringArg(name));
  try {
    return BigInt(res);
  } catch {
    return 0n;
  }
}

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');
  if (req.method === 'OPTIONS') return new Response(null, { status: 204, headers: corsHeaders(origin) });
  if (req.method !== 'POST') return json({ error: 'POST only' }, 405, origin);
  if (!jobStoreConfigured()) return json({ error: 'scheduler store not configured (no GitHub token)' }, 503, origin);

  // Auth — personal-sign token (address:ts:sig), 300s freshness. The authed
  // address is the job owner (billing + push identity).
  const token = req.headers.get('x-goog-api-key') ?? req.headers.get('x-api-key') ?? '';
  const now = Math.floor(Date.now() / 1000);
  const auth = verifyAuthToken(token, now);
  if (!auth.ok) return json({ error: 'auth: ' + auth.error }, auth.status, origin);
  const owner = auth.address.toLowerCase();

  let payload: Record<string, unknown>;
  try {
    payload = await req.json();
  } catch {
    return json({ error: 'bad json' }, 400, origin);
  }

  const action = String(payload.action ?? '').trim();

  // ---- cancel: delete the caller's OWN job by id --------------------------
  if (action === 'cancel') {
    const id = String(payload.id ?? '').trim();
    if (!id) return json({ error: 'cancel requires "id"' }, 400, origin);
    let found: { job: OffchainJob; path: string } | null;
    try {
      found = await findById(id);
    } catch (e) {
      return json({ error: 'store: ' + (e as Error).message }, 502, origin);
    }
    if (!found) return json({ error: `no job ${id}` }, 404, origin);
    if (found.job.owner.toLowerCase() !== owner) {
      return json({ error: 'not your job' }, 403, origin);
    }
    try {
      await deleteJob(found.path, `cancel job ${id} (owner ${owner.slice(0, 10)})`);
    } catch (e) {
      return json({ error: 'store: ' + (e as Error).message }, 502, origin);
    }
    return json({ cancelled: true, id }, 200, origin);
  }

  // ---- list: the caller's OWN jobs ----------------------------------------
  if (action === 'list') {
    try {
      const jobs = await listByOwner(owner);
      return json({ jobs }, 200, origin);
    } catch (e) {
      return json({ error: 'store: ' + (e as Error).message }, 502, origin);
    }
  }

  // ---- create -------------------------------------------------------------
  if (action !== 'create') {
    return json({ error: 'action must be "create", "cancel", or "list"' }, 400, origin);
  }

  const kind: JobKind = payload.kind === 'agent' ? 'agent' : 'reminder';
  const task = String(payload.task ?? '').trim();
  if (!task) return json({ error: 'create requires a non-empty "task"' }, 400, origin);
  if (new TextEncoder().encode(task).length > MAX_TASK_BYTES) {
    return json({ error: `task too large (> ${MAX_TASK_BYTES} bytes)` }, 413, origin);
  }

  const intervalSecs = Math.trunc(Number(payload.intervalSecs ?? payload.interval_secs ?? 0));
  if (!Number.isFinite(intervalSecs) || intervalSecs < MIN_INTERVAL_SECS) {
    return json({ error: `intervalSecs must be >= ${MIN_INTERVAL_SECS}` }, 400, origin);
  }
  const runs = Math.trunc(Number(payload.runs ?? 1));
  if (!Number.isFinite(runs) || runs < 1 || runs > MAX_RUNS) {
    return json({ error: `runs must be 1..${MAX_RUNS}` }, 400, origin);
  }

  // For an AGENT job, resolve the target name → tokenId (persona + push slot).
  // For a REMINDER, no target/model — the push goes to the OWNER's subscription
  // (the cron resolves mainOf(owner)).
  let target = '';
  let targetId = '0';
  if (kind === 'agent') {
    target = String(payload.target ?? '').trim().toLowerCase();
    if (!/^[a-z0-9-]{1,63}$/.test(target)) {
      return json({ error: 'agent job requires a valid "target" subdomain' }, 400, origin);
    }
    let id: bigint;
    try {
      id = await idOfName(target);
    } catch (e) {
      return json({ error: 'rpc: ' + (e as Error).message }, 502, origin);
    }
    if (id === 0n) return json({ error: `"${target}" is not a registered agent` }, 404, origin);
    targetId = id.toString();
  }

  // Per-owner active-job cap — bounds the shared store so one keypair can't flood
  // it with free reminders and crowd the cron's due scan (finding: cross-tenant
  // DoS on a free creation primitive).
  let owned: number;
  try {
    owned = await countByOwner(owner);
  } catch (e) {
    return json({ error: 'store: ' + (e as Error).message }, 502, origin);
  }
  if (owned >= MAX_JOBS_PER_OWNER) {
    return json(
      { error: `you have ${owned} scheduled jobs (max ${MAX_JOBS_PER_OWNER}) — cancel some first` },
      429,
      origin,
    );
  }

  const job: OffchainJob = {
    id: crypto.randomUUID(),
    owner,
    kind,
    target,
    targetId,
    task,
    intervalSecs,
    runsLeft: runs,
    nextRun: now + intervalSecs,
    createdAt: now,
  };

  try {
    await createJob(job);
  } catch (e) {
    return json({ error: 'store: ' + (e as Error).message }, 502, origin);
  }

  return json(
    { scheduled: true, id: job.id, kind, target, intervalSecs, runs, nextRun: job.nextRun },
    200,
    origin,
  );
}
