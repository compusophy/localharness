// _ghissue.ts — the shared GitHub-issue filer with signature dedupe, factored
// out of telemetry.ts (one incident = one OPEN issue — the "23 identical 429
// issues" fix) so the scheduler's health alerts ride the exact same rails.
// Convention: the caller embeds `(<signature>)` in the title; dedupe matches it.

export interface IssueResult {
  filed: boolean;
  deduped?: boolean;
  url?: string;
  number?: number;
  status?: number;
  detail?: string;
}

/** Search open issues for `(signature)` in the title; hit → dedupe (no create),
 * miss/hiccup → create. Non-2xx create → `{filed:false, status, detail}`. */
export async function fileIssueDeduped(opts: {
  repo: string;
  token: string;
  title: string;
  body: string;
  signature?: string;
  labels?: string[];
}): Promise<IssueResult> {
  const ghHeaders = {
    authorization: `Bearer ${opts.token}`,
    accept: 'application/vnd.github+json',
    'content-type': 'application/json',
    'user-agent': 'localharness-telemetry',
  };
  if (opts.signature) {
    try {
      const q = encodeURIComponent(
        `repo:${opts.repo} is:issue is:open in:title ${opts.signature}`,
      );
      const sres = await fetch(`https://api.github.com/search/issues?q=${q}&per_page=5`, {
        headers: ghHeaders,
      });
      if (sres.ok) {
        const found = (await sres.json()) as {
          items?: Array<{ number: number; html_url: string; title: string }>;
        };
        const hit = found.items?.find((i) => i.title.includes(`(${opts.signature})`));
        if (hit) return { filed: true, deduped: true, url: hit.html_url, number: hit.number };
      }
    } catch {
      /* search hiccup — fall through to a normal create */
    }
  }
  const res = await fetch(`https://api.github.com/repos/${opts.repo}/issues`, {
    method: 'POST',
    headers: ghHeaders,
    body: JSON.stringify({ title: opts.title, body: opts.body, labels: opts.labels ?? [] }),
  });
  if (!res.ok) {
    return { filed: false, status: res.status, detail: (await res.text()).slice(0, 200) };
  }
  const issue = (await res.json()) as { html_url?: string; number?: number };
  return { filed: true, url: issue.html_url, number: issue.number };
}
