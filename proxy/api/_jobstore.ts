// _jobstore.ts — the OFF-CHAIN scheduled-job store (GitHub-backed).
//
// Scheduled jobs used to live ON-CHAIN in ScheduleFacet: `scheduleJob` cost
// ~2.88M gas (cold SSTOREs + enumerable pushes + the task bytes @7.6k/byte) +
// an escrow lock, and `recordRun` per fire — all sponsored. For the dominant
// case ("notify me in 15 minutes") that was absurd: a one-shot push cost two
// sponsored mainnet writes and locked $LH it could never even spend (a 0.1 $LH
// budget can't fund a 1 $LH/call run). So job STATE moves off-chain — the SAME
// model apps/feedback/telemetry already use (GitHub via the bot token, free, no
// gas, no sponsor drain). The chain keeps only ownership of the NAME; jobs are
// just data.
//
// A job is one small JSON file. The DUE TIME is encoded in the FILENAME
// (`<nextRun_pad>__<id>.json`) so the once-a-minute cron can find due jobs from
// a single directory LISTING (no per-job content read) — it only fetches the
// bodies it will actually fire.
//
// CONCURRENCY (the on-chain CAS, re-derived off-chain): firing a job is gated by
// `claimJob(path, sha)` — a GitHub Contents-API DELETE conditioned on the file's
// read-time blob sha. That is an optimistic compare-and-swap: of N overlapping
// cron ticks that read the same due file, only ONE delete matches the sha and
// wins; the rest skip WITHOUT running/billing. The winner runs+bills, then
// `writeNextSlot` writes the next (drift-corrected) slot. So a job fires — and an
// agent job CHARGES — at most once per due slot, even if Vercel runs overlapping
// ticks. This replaces ScheduleFacet.recordRun's StaleNextRun guard.
//
// CAVEATS (honest): (1) a recurring job is one commit per fire — fine for
// reminders / low-frequency jobs; for high-frequency recurring load, swap THIS
// module for a KV adapter (cron/endpoint unchanged). (2) lose-not-duplicate: a
// crash between the claim-delete and the next-slot write drops ONE fire rather
// than risking a double-charge. (3) the directory listing caps at 1000 entries —
// bounded by the per-owner job cap; a Git-Trees-API scan is the >1000 upgrade.

const JOBS_REPO = process.env.GH_JOBS_REPO ?? 'compusophy/localharness-jobs';
// Reuse the telemetry PAT if a dedicated jobs token isn't provisioned, so the
// store works the moment this ships (same fallback publish.ts uses).
const GH_TOKEN = process.env.GH_JOBS_TOKEN ?? process.env.GH_TELEMETRY_TOKEN ?? '';
const JOBS_DIR = 'jobs';

// Bounds (mirror the on-chain facet's leashes so an off-chain job can't be
// unboundedly large/long-lived). `task` cap matches the feedback 2048-byte cap.
export const MAX_TASK_BYTES = 2048;
export const MIN_INTERVAL_SECS = 60;
export const MAX_RUNS = 100_000;
// Per-owner active-job cap. Reminder creation is FREE (no meter/gas), so without
// a cap one keypair could flood the shared store and crowd the cron's due scan
// (GitHub's directory listing caps at 1000 entries). Env-overridable.
export const MAX_JOBS_PER_OWNER = ((): number => {
  const n = Number(process.env.SCHEDULER_MAX_JOBS_PER_OWNER ?? '50');
  return Number.isFinite(n) && n > 0 ? Math.min(Math.trunc(n), 1000) : 50;
})();
// How many due jobs one cron tick fires off-chain (on top of the on-chain
// batch). Reminders are cheap (a push); agent jobs are heavier — keep it small
// so the tick fits Edge's wall-clock. Env-overridable.
export const MAX_OFFCHAIN_JOBS_PER_TICK = ((): number => {
  const n = Number(process.env.SCHEDULER_MAX_OFFCHAIN_JOBS_PER_TICK ?? '8');
  return Number.isFinite(n) && n > 0 ? Math.min(Math.trunc(n), 64) : 8;
})();

export type JobKind = 'reminder' | 'agent';

/** One off-chain scheduled job. The authoritative record lives in the file
 * body; the filename mirrors `nextRun`+`id` so the due scan needs no body read. */
export interface OffchainJob {
  /** Stable opaque id (uuid). Distinguishes off-chain jobs from on-chain numeric
   * ids in the shared `?poke` path. */
  id: string;
  /** 0x-lowercase scheduling identity — BILLED (agent jobs debit this meter) and
   * PUSHED (the reminder/result goes to this owner's subscription). */
  owner: string;
  /** `reminder` = web-push the task text, no model call, no charge. `agent` =
   * run the target agent (bounded ping-pong), debit the owner's meter per call. */
  kind: JobKind;
  /** Agent subdomain to run (agent kind). Empty for a reminder. */
  target: string;
  /** tokenId of `target` (decimal string), resolved at create; '0' = none. The
   * push fallback slot + persona lookup key. */
  targetId: string;
  /** Reminder body, or the agent prompt (may carry the `GOAL: ` marker). */
  task: string;
  /** Seconds between fires (>= MIN_INTERVAL_SECS). A one-shot = `runs: 1`. */
  intervalSecs: number;
  /** Remaining fires; 0 ⇒ delete. */
  runsLeft: number;
  /** Unix seconds of the next due fire. */
  nextRun: number;
  /** Unix seconds the job was created (audit). */
  createdAt: number;
}

export function jobStoreConfigured(): boolean {
  return GH_TOKEN.length > 0;
}

// --- GitHub Contents API (commit/list/delete; updates need the prior sha) -----

function ghHeaders(): Record<string, string> {
  return {
    authorization: `Bearer ${GH_TOKEN}`,
    accept: 'application/vnd.github+json',
    'content-type': 'application/json',
    'user-agent': 'localharness-scheduler',
  };
}

// base64 of the UTF-8 BYTES (not `btoa(string)`, which is Latin-1-only and throws
// / corrupts on non-ASCII task text — accents, emoji). Mirrors publish.ts.
function b64encodeUtf8(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let s = '';
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    s += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(s);
}

function b64decodeUtf8(b64: string): string {
  const bin = atob(b64.replace(/\n/g, ''));
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return new TextDecoder().decode(bytes);
}

/** A 12-digit zero-padded nextRun so filenames sort chronologically and the due
 * scan can range-filter by NAME alone. 12 digits covers unix seconds for ~31000
 * years — no overflow. */
function pad(nextRun: number): string {
  return String(Math.max(0, Math.floor(nextRun))).padStart(12, '0');
}

function fileName(job: OffchainJob): string {
  return `${pad(job.nextRun)}__${job.id}.json`;
}

/** Parse `<nextRun>__<id>.json` → { nextRun, id }, or null if it doesn't match. */
function parseName(name: string): { nextRun: number; id: string } | null {
  const m = /^(\d{1,12})__([^/]+)\.json$/.exec(name);
  if (!m) return null;
  return { nextRun: Number(m[1]), id: m[2] };
}

interface GhEntry {
  name: string;
  path: string;
  sha: string;
  type: string;
}

/** List the jobs directory (names + shas, NOT bodies). [] when the dir/repo is
 * empty (404) — the common cold-start case. */
async function ghList(): Promise<GhEntry[]> {
  const res = await fetch(
    `https://api.github.com/repos/${JOBS_REPO}/contents/${JOBS_DIR}?ref=main`,
    { headers: ghHeaders() },
  );
  if (res.status === 404) return [];
  if (!res.ok) throw new Error(`list jobs: ${res.status}`);
  const j = (await res.json()) as GhEntry[];
  return Array.isArray(j) ? j.filter((e) => e.type === 'file') : [];
}

async function ghGetSha(path: string): Promise<string | null> {
  const res = await fetch(`https://api.github.com/repos/${JOBS_REPO}/contents/${path}?ref=main`, {
    headers: ghHeaders(),
  });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`get ${path}: ${res.status}`);
  const j = (await res.json()) as { sha?: string };
  return j.sha ?? null;
}

// Returns the job body AND its blob `sha` — the sha is the optimistic-lock token
// used to CLAIM the fire (a sha-conditional delete). Body and sha come from the
// SAME GET so they're consistent (no TOCTOU between read and claim).
async function ghReadBody(path: string): Promise<{ job: OffchainJob; sha: string } | null> {
  const res = await fetch(`https://api.github.com/repos/${JOBS_REPO}/contents/${path}?ref=main`, {
    headers: ghHeaders(),
  });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`read ${path}: ${res.status}`);
  const j = (await res.json()) as { content?: string; encoding?: string; sha?: string };
  if (!j.content || !j.sha) return null;
  try {
    return { job: JSON.parse(b64decodeUtf8(j.content)) as OffchainJob, sha: j.sha };
  } catch {
    return null;
  }
}

async function ghPut(path: string, body: string, message: string, sha?: string): Promise<void> {
  const res = await fetch(`https://api.github.com/repos/${JOBS_REPO}/contents/${path}`, {
    method: 'PUT',
    headers: ghHeaders(),
    body: JSON.stringify({ message, content: b64encodeUtf8(body), branch: 'main', ...(sha ? { sha } : {}) }),
  });
  if (!res.ok) {
    const d = await res.text();
    throw new Error(`put ${path}: ${res.status} ${d.slice(0, 200)}`);
  }
}

async function ghDelete(path: string, message: string): Promise<void> {
  const sha = await ghGetSha(path);
  if (!sha) return; // already gone — idempotent
  const res = await fetch(`https://api.github.com/repos/${JOBS_REPO}/contents/${path}`, {
    method: 'DELETE',
    headers: ghHeaders(),
    body: JSON.stringify({ message, sha, branch: 'main' }),
  });
  // 404/409 = someone else already removed/moved it; benign for our idempotent
  // delete-on-exhaust + cancel.
  if (!res.ok && res.status !== 404 && res.status !== 409) {
    const d = await res.text();
    throw new Error(`delete ${path}: ${res.status} ${d.slice(0, 200)}`);
  }
}

/**
 * CLAIM a fire by deleting the file with its READ-TIME `sha` (a conditional
 * delete = an optimistic compare-and-swap). This is the off-chain CAS that
 * replaces ScheduleFacet.recordRun's `StaleNextRun` guard: of N overlapping cron
 * ticks that all read the same due file, ONLY ONE delete matches the sha (2xx) —
 * the others get 409/422 (sha mismatch) or 404 (already gone). Returns `true`
 * ONLY for the winner; everyone else (including on a transient 5xx — the file
 * stays, so it re-fires next tick) gets `false` and MUST skip without running or
 * billing. The caller runs + bills ONLY after a true claim, then writes the next
 * slot — so a job is fired (and charged) at most once per due slot.
 */
export async function claimJob(path: string, sha: string): Promise<boolean> {
  const res = await fetch(`https://api.github.com/repos/${JOBS_REPO}/contents/${path}`, {
    method: 'DELETE',
    headers: ghHeaders(),
    body: JSON.stringify({ message: `claim+fire ${path}`, sha, branch: 'main' }),
  });
  return res.ok; // 2xx ⇒ we won the claim; anything else ⇒ lost / transient → skip
}

// --- public store API --------------------------------------------------------

/** Persist a NEW job (filename keyed on its nextRun). */
export async function createJob(job: OffchainJob): Promise<void> {
  await ghPut(
    `${JOBS_DIR}/${fileName(job)}`,
    JSON.stringify(job, null, 0),
    `schedule ${job.kind} ${job.id} (owner ${job.owner.slice(0, 10)})`,
  );
}

/** All jobs due at `now` (nextRun <= now), cheaply: ONE directory listing +
 * body reads ONLY for the due files (capped at `limit`, most-overdue first).
 * Returns `{ job, path, sha }` so the caller can CLAIM (sha-conditional delete)
 * then advance after firing. */
export async function listDue(
  now: number,
  limit: number,
): Promise<{ job: OffchainJob; path: string; sha: string }[]> {
  const entries = await ghList();
  const due = entries
    .map((e) => ({ e, p: parseName(e.name) }))
    .filter((x): x is { e: GhEntry; p: { nextRun: number; id: string } } => x.p !== null)
    .filter((x) => x.p.nextRun <= now)
    .sort((a, b) => a.p.nextRun - b.p.nextRun)
    .slice(0, limit);
  const out: { job: OffchainJob; path: string; sha: string }[] = [];
  for (const { e } of due) {
    try {
      const read = await ghReadBody(e.path);
      if (read) out.push({ job: read.job, path: e.path, sha: read.sha });
    } catch {
      /* skip an unreadable file this tick; it re-lists next tick */
    }
  }
  return out;
}

/** Find a job by id (lists + matches the `__<id>.json` suffix, reads the body). */
export async function findById(id: string): Promise<{ job: OffchainJob; path: string } | null> {
  const entries = await ghList();
  const hit = entries.find((e) => parseName(e.name)?.id === id);
  if (!hit) return null;
  const read = await ghReadBody(hit.path);
  return read ? { job: read.job, path: hit.path } : null;
}

/** Count of a single owner's active jobs (the per-owner create quota). */
export async function countByOwner(owner: string): Promise<number> {
  return (await listByOwner(owner)).length;
}

/** All jobs owned by `owner` (0x-lowercase) — the `list` endpoint backing. */
export async function listByOwner(owner: string): Promise<OffchainJob[]> {
  const entries = await ghList();
  const out: OffchainJob[] = [];
  for (const e of entries) {
    if (!parseName(e.name)) continue;
    try {
      const read = await ghReadBody(e.path);
      if (read && read.job.owner.toLowerCase() === owner.toLowerCase()) out.push(read.job);
    } catch {
      /* skip */
    }
  }
  return out.sort((a, b) => a.nextRun - b.nextRun);
}

/**
 * Write the NEXT slot for a recurring job AFTER it was claimed+fired (the claim
 * already deleted the old file, so there is never a duplicate — the on-chain
 * write-new-then-delete-old double-fire window is gone). Returns the advanced job,
 * or `null` when exhausted (runs spent → no file written, the claim's delete was
 * the teardown).
 *
 * DRIFT-CORRECTED, skip-don't-pile-up — ported verbatim from
 * `ScheduleFacet.recordRun` (the "fires 3 minutes late then drifts further" fix):
 * a LATE fire jumps to the FIRST grid slot strictly after `now`, firing once for
 * all missed slots instead of burst-firing (and burst-BILLING) each.
 */
export async function writeNextSlot(job: OffchainJob): Promise<OffchainJob | null> {
  const runsLeft = job.runsLeft - 1;
  if (runsLeft <= 0) return null; // exhausted — the claim already removed the file
  // Use CURRENT time, not a start-of-run snapshot: an AGENT run can take many
  // seconds, so re-reading `now` here keeps the drift-correction honest (a slow
  // run shouldn't compute a `next` already in the past and re-fire immediately).
  const now = Math.floor(Date.now() / 1000);
  let next = job.nextRun + job.intervalSecs;
  if (next <= now) {
    // intervalSecs >= MIN_INTERVAL_SECS (60), enforced at create — no div-by-zero.
    const missed = Math.floor((now - job.nextRun) / job.intervalSecs);
    next = job.nextRun + (missed + 1) * job.intervalSecs;
  }
  const advanced: OffchainJob = { ...job, runsLeft, nextRun: next };
  const path = `${JOBS_DIR}/${fileName(advanced)}`;
  const msg = `advance job ${job.id} → nextRun ${next} (${runsLeft} left)`;
  // No sha: the claim-winner is the unique writer of this new (fresh-named) slot.
  // A 422 ("already exists") can only be a retry of OUR own advance — idempotent
  // success. Any OTHER failure (transient GitHub 5xx) would STRAND an already-billed
  // recurring job (lose-not-duplicate), so retry the write ONCE before giving up.
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      await ghPut(path, JSON.stringify(advanced, null, 0), msg);
      return advanced;
    } catch (e) {
      const m = String((e as Error).message);
      if (m.includes(' 422 ')) return advanced; // already advanced — idempotent
      if (attempt === 1) throw e; // second failure — drop (never double-bill)
    }
  }
  return advanced;
}

/** Delete a job by its known path (cancel / owner teardown). Idempotent. */
export async function deleteJob(path: string, reason: string): Promise<void> {
  await ghDelete(path, reason);
}
