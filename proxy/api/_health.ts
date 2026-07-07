// _health.ts — the proxy's hourly SELF-CHECK (road-to-v1 step 2: zero
// monitoring/alerting on the SPOF). The scheduler cron already fires every
// minute; at minute==0 it runs these checks and a failing set files ONE deduped
// GitHub issue (the same telemetry rails ops already watch) plus a best-effort
// owner web-push (LH_ALERT_OWNER → push store; unset ⇒ the issue IS the alert).
// Decision logic is pure + unit-tested (test/health-check.mjs); the I/O reads
// are 5s-capped so a health pass can't eat the tick's wall-clock.

import { missingEnv } from './_env';
import { fileIssueDeduped } from './_ghissue';

export interface HealthCheck {
  name: string;
  ok: boolean;
  detail: string;
}

/** Sponsor-float headroom (pure): warn at `headroom`× the circuit-breaker floor
 * (default 10×) — the LH_RELAY_MIN_FLOAT_WEI breaker only trips when the relay
 * is already dead. `minFloatWei === 0` = breaker disabled → nothing to warn on. */
export function floatHealth(floatWei: bigint, minFloatWei: bigint, headroom = 10n): HealthCheck {
  const threshold = minFloatWei * headroom;
  const ok = minFloatWei === 0n || floatWei >= threshold;
  return {
    name: 'sponsor-float',
    ok,
    detail: ok
      ? `float ${floatWei} >= ${threshold} (${headroom}x breaker floor)`
      : `sponsor float ${floatWei} below ${headroom}x breaker floor (${minFloatWei}) — top up the fee-token float`,
  };
}

/** Env dry-run over the cross-route critical set (pure over process.env). */
export function envHealth(required: string[], anyOf: string[][] = []): HealthCheck {
  const missing = missingEnv(required, anyOf);
  return {
    name: 'env',
    ok: missing.length === 0,
    detail: missing.length === 0 ? 'all critical env present' : `missing ${missing.join(', ')}`,
  };
}

/** Stable dedupe signature for a failing set — one incident = one open issue
 * (telemetry.ts convention: the signature rides in the title). Order-free. */
export function healthSignature(failing: HealthCheck[]): string {
  return 'proxy-health-' + failing.map((c) => c.name).sort().join('-');
}

/** ERC-20 balanceOf via raw eth_call (viem-free; 5s-capped). */
export async function erc20Balance(rpc: string, token: string, holder: string): Promise<bigint> {
  const data = '0x70a08231' + holder.toLowerCase().replace(/^0x/, '').padStart(64, '0');
  const res = await fetch(rpc, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'eth_call',
      params: [{ to: token, data }, 'latest'],
    }),
    signal: AbortSignal.timeout(5000),
  });
  if (!res.ok) throw new Error(`RPC ${res.status}`);
  const j = (await res.json()) as { result?: string; error?: { message?: string } };
  if (j.error || !j.result) throw new Error(`eth_call: ${j.error?.message ?? 'no result'}`);
  return j.result === '0x' ? 0n : BigInt(j.result);
}

/** The float check with its read: an RPC failure is itself a failing check. */
export async function sponsorFloatHealth(
  rpc: string,
  feeToken: string,
  sponsor: string,
  minFloatWei: bigint,
): Promise<HealthCheck> {
  try {
    return floatHealth(await erc20Balance(rpc, feeToken, sponsor), minFloatWei);
  } catch (e) {
    return { name: 'sponsor-float', ok: false, detail: `float read failed: ${(e as Error).message}` };
  }
}

/** ONE cheap authenticated read against a GitHub-store repo (5s-capped). */
export async function githubHealth(repo: string, token: string): Promise<HealthCheck> {
  try {
    const res = await fetch(`https://api.github.com/repos/${repo}`, {
      headers: {
        authorization: `Bearer ${token}`,
        accept: 'application/vnd.github+json',
        'user-agent': 'localharness-health',
      },
      signal: AbortSignal.timeout(5000),
    });
    return {
      name: 'github-store',
      ok: res.ok,
      detail: res.ok ? `repo ${repo} reachable` : `GET repos/${repo} -> ${res.status}`,
    };
  } catch (e) {
    return { name: 'github-store', ok: false, detail: `GET repos/${repo} failed: ${(e as Error).message}` };
  }
}

/** File ONE deduped issue for the failing set + best-effort owner push when a
 * push mapping exists (`pushOwner` unset ⇒ the issue alone is the alert). The
 * console.error is the last-resort alarm when even filing fails. Never throws. */
export async function alertHealth(
  failing: HealthCheck[],
  deps: {
    repo: string;
    token: string;
    pushOwner?: string;
    push?: (owner: string, title: string, body: string) => Promise<boolean>;
  },
): Promise<IssueOutcome> {
  const sig = healthSignature(failing);
  const names = failing.map((c) => c.name).join(', ');
  const title = `[health] proxy self-check failing: ${names} (${sig})`;
  const body =
    `Hourly proxy health self-check (api/scheduler.ts) found:\n\n` +
    failing.map((c) => `- **${c.name}**: ${c.detail}`).join('\n') +
    `\n\nOne deduped issue per failing-set signature; close it once fixed to re-arm.` +
    `\n\n---\n*localharness proxy health · road-to-v1 step 2*`;
  console.error(`[health] FAILING: ${failing.map((c) => `${c.name} (${c.detail})`).join('; ')}`);
  let outcome: IssueOutcome = { filed: false };
  if (deps.token) {
    try {
      const r = await fileIssueDeduped({
        repo: deps.repo,
        token: deps.token,
        title,
        body,
        signature: sig,
        labels: ['health'],
      });
      outcome = { filed: r.filed, deduped: r.deduped, url: r.url };
    } catch {
      /* filing failed — the console.error above is the alert of last resort */
    }
  }
  if (deps.pushOwner && deps.push) {
    try {
      await deps.push(deps.pushOwner, 'proxy health check failing', names);
    } catch {
      /* best-effort */
    }
  }
  return outcome;
}

export interface IssueOutcome {
  filed: boolean;
  deduped?: boolean;
  url?: string;
}
