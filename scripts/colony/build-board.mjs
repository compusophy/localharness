#!/usr/bin/env node
// scripts/colony/build-board.mjs — generate the PUBLIC COLONY BOARD.
//
// A read-only generator that joins the colony's three rungs into one
// human-viewable pipeline, then renders a SELF-CONTAINED static page to
// web/colony.html (deployed with the web bundle to localharness.xyz/colony.html):
//
//   on-chain feedback (FeedbackFacet)
//     → GitHub issue (label `colony`, body marker `lh-feedback:<index>`)
//       → on-chain bounty (BountyFacet, task references the issue)
//         → PR (the bounty's submitted result, when it's a URL)
//           → settled (status=paid → the worker's TBA was paid)
//
// Reads only (zero npm deps): on-chain via raw eth_call (lib.mjs's `ethCall` +
// keccak-derived `selector`), GitHub via the `gh` CLI (lib.mjs's `gh`). The page
// is a SNAPSHOT — re-run this script to refresh it.
//
// Usage:
//   node scripts/colony/build-board.mjs                 # write web/colony.html
//   node scripts/colony/build-board.mjs --out <path>    # custom output path
//   node scripts/colony/build-board.mjs --feedback <n>  # recent feedback cap (default 30)
//   node scripts/colony/build-board.mjs --stdout        # print HTML, write nothing
// Env: LH_REPO, DIAMOND, RPC, GH_TOKEN (honored by gh automatically).

import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import {
  REPO,
  DIAMOND,
  RPC,
  MARKER_PREFIX,
  REPO_ROOT,
  takeFlag,
  hasFlag,
  gh,
  ethCall,
  selector,
  feedbackCount,
  decodeFeedbackAt,
  parseQaEnvelope,
} from './lib.mjs';

const BOUNTY_SCAN = 200; // how far down the bounty id space we walk

// ----------------------------------------------------------------- helpers

/** ethCall with bounded retry/backoff — the public RPC 429s under bursts, so a
 *  read-only generator must back off rather than abort the whole board. */
async function call(data, tries = 5) {
  for (let i = 0; ; i++) {
    try {
      return await ethCall(data);
    } catch (e) {
      const transient = /HTTP (429|5\d\d)/.test(e.message);
      if (!transient || i >= tries - 1) throw e;
      await new Promise((r) => setTimeout(r, 400 * 2 ** i)); // 0.4s,0.8s,1.6s…
    }
  }
}

/** ABI-encode a single uint256 argument (right-aligned 32-byte word). */
function uintArg(n) {
  return BigInt(n).toString(16).padStart(64, '0');
}

/** Decode a dynamic `bytes`/`string` eth_call return (offset+len+body) to UTF-8.
 *  Empty string on a short/empty response (a never-set value reads as empty). */
function decodeBytesString(hex) {
  const buf = Buffer.from((hex || '').replace(/^0x/, ''), 'hex');
  if (buf.length < 64) return '';
  const len = Number(BigInt('0x' + buf.subarray(32, 64).toString('hex')));
  if (len === 0 || 64 + len > buf.length) return '';
  return buf.subarray(64, 64 + len).toString('utf8');
}

/** $LH wei (1e18) → a terse decimal label, trimming trailing zeros. */
function fmtLh(wei) {
  const w = BigInt(wei);
  const whole = w / 10n ** 18n;
  const frac = (w % 10n ** 18n).toString().padStart(18, '0').replace(/0+$/, '');
  return frac ? `${whole}.${frac}` : whole.toString();
}

/** HTML-escape — the page embeds on-chain + GitHub text verbatim. */
function esc(s) {
  return String(s ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

/** Effective feedback text: decoded body for qa/v1 fleet envelopes, raw else. */
function effectiveText(text) {
  return parseQaEnvelope(text)?.body ?? text;
}

/** First http(s) URL in a string (a submitted bounty result that IS a PR). */
function firstUrl(s) {
  const m = String(s || '').match(/https?:\/\/[^\s)>\]]+/);
  return m ? m[0] : null;
}

const SEL = {
  getBounty: selector('getBounty(uint256)'),
  bountyTaskOf: selector('bountyTaskOf(uint256)'),
  resultOf: selector('resultOf(uint256)'),
  nameOfId: selector('nameOfId(uint256)'),
  openBounties: selector('openBounties(uint256,uint256)'),
  feedbackAt: selector('feedbackAt(uint256)'),
};

const BOUNTY_STATUS = ['open', 'claimed', 'submitted', 'paid', 'cancelled', 'reclaimed'];

// --------------------------------------------------------------- chain reads

/** Decode getBounty(uint256) → record. Zero poster = the id was never posted. */
function decodeBounty(hex) {
  const buf = Buffer.from((hex || '').replace(/^0x/, ''), 'hex');
  if (buf.length < 5 * 32) return null;
  const word = (i) => buf.subarray(i * 32, (i + 1) * 32);
  const poster = '0x' + word(0).subarray(12).toString('hex');
  if (/^0x0+$/.test(poster)) return null;
  return {
    poster,
    rewardWei: BigInt('0x' + word(1).toString('hex')),
    expiry: Number(BigInt('0x' + word(2).toString('hex'))),
    status: buf[3 * 32 + 31],
    claimantTokenId: Number(BigInt('0x' + word(4).toString('hex'))),
  };
}

/** Read on-chain feedback: the most-recent `cap` entries UNION any explicitly
 *  `extra` indices (the ones open issues reference), so the feedback column
 *  always resolves the text behind a tracked issue even if it predates the
 *  recent window. Returns the total count + the read entries (newest first). */
async function readFeedback(cap, extra = new Set()) {
  const count = await feedbackCount();
  const start = Math.max(0, count - cap);
  const wanted = new Set();
  for (let i = count - 1; i >= start; i--) wanted.add(i);
  for (const i of extra) if (Number.isInteger(i) && i >= 0 && i < count) wanted.add(i);
  const idx = [...wanted].sort((a, b) => b - a); // newest first
  const out = [];
  const CHUNK = 8;
  for (let at = 0; at < idx.length; at += CHUNK) {
    const slice = idx.slice(at, at + CHUNK);
    const rows = await Promise.all(
      slice.map(async (i) => {
        const hex = await call(SEL.feedbackAt + uintArg(i));
        return { index: i, ...decodeFeedbackAt(hex) };
      }),
    );
    out.push(...rows);
  }
  return { count, entries: out };
}

/** Walk the bounty id space [0, scan) and read every posted bounty (task +
 *  status + reward + result + claimant name). Open OR terminal — the board
 *  shows the whole pipeline, not just open work. `openBounties` only returns
 *  OPEN ids, so a direct id walk is the only way to see paid/claimed history. */
async function readBounties(scan) {
  const out = [];
  const CHUNK = 3; // ≤4 calls each → keep the in-flight burst gentle on the RPC
  for (let at = 0; at < scan; at += CHUNK) {
    const ids = [];
    for (let i = at; i < Math.min(at + CHUNK, scan); i++) ids.push(i);
    const rows = await Promise.all(
      ids.map(async (id) => {
        const b = decodeBounty(await call(SEL.getBounty + uintArg(id)));
        if (!b) return null;
        const task = decodeBytesString(await call(SEL.bountyTaskOf + uintArg(id)));
        const result = decodeBytesString(await call(SEL.resultOf + uintArg(id)));
        let claimant = '';
        if (b.claimantTokenId) {
          claimant = decodeBytesString(await call(SEL.nameOfId + uintArg(b.claimantTokenId)));
        }
        return { id, ...b, task, result, claimant };
      }),
    );
    let sawNone = false;
    for (const r of rows) {
      if (r) out.push(r);
      else sawNone = true;
    }
    // The id space is contiguous from 0; a full empty chunk past the first hit
    // means we've walked off the end — stop early (cheap RPC manners).
    if (sawNone && out.length && rows.every((r) => !r)) break;
  }
  return out;
}

/** Open `colony`-label issues, with the lh-feedback index parsed from the body.
 *  Returns [] (and warns) if gh is unavailable so the board still renders. */
function readColonyIssues() {
  let raw;
  try {
    raw = gh([
      'issue', 'list',
      '--state', 'open',
      '--label', 'colony',
      '--json', 'number,title,url,body',
      '--limit', '200',
    ]);
  } catch (e) {
    console.error(`warning: could not list colony issues (${e.message}); rendering without them.`);
    return [];
  }
  const re = new RegExp(`${MARKER_PREFIX}(\\d+)\\b`);
  return JSON.parse(raw).map((i) => ({
    number: i.number,
    title: i.title,
    url: i.url,
    feedbackIndex: (i.body || '').match(re)?.[1] ?? null,
  }));
}

// ------------------------------------------------------------------- join

/** Join the three rungs into pipeline rows keyed on the issue. A bounty is
 *  linked to an issue when its task text references `#<n>` for that issue (the
 *  shape issue-to-bounty.mjs stamps: "fix #<n> — …"). Feedback links via the
 *  issue body's `lh-feedback:<index>` marker. */
function buildPipeline(feedback, issues, bounties) {
  const fbByIndex = new Map(feedback.entries.map((e) => [String(e.index), e]));

  // For each issue, find bounties whose task references #<issue number>.
  const rows = issues.map((issue) => {
    const ref = new RegExp(`(^|[^\\w/])#${issue.number}\\b`);
    const linked = bounties.filter((b) => ref.test(b.task || ''));
    // Prefer the most-advanced bounty (paid > submitted > claimed > open).
    linked.sort((a, b) => b.status - a.status || b.id - a.id);
    const bounty = linked[0] || null;
    const fb = issue.feedbackIndex != null ? fbByIndex.get(issue.feedbackIndex) : null;
    const prUrl = bounty && firstUrl(bounty.result);
    return { issue, bounty, feedback: fb, prUrl };
  });

  // Order: live work first (open/claimed/submitted bounty), then settled, then
  // issues without a bounty — newest issue number first within each band.
  const band = (r) => {
    if (!r.bounty) return 2;
    return r.bounty.status === 3 ? 1 : 0; // 0 in-flight, 1 paid
  };
  rows.sort((a, b) => band(a) - band(b) || b.issue.number - a.issue.number);
  return rows;
}

// ------------------------------------------------------------------ render

function statusCell(bounty, prUrl) {
  if (!bounty) return '<span class="muted">no bounty yet</span>';
  const label = BOUNTY_STATUS[bounty.status] ?? 'unknown';
  const reward = `${fmtLh(bounty.rewardWei)} $LH`;
  const who = bounty.claimant ? ` · ${esc(bounty.claimant)}` : '';
  const pr = prUrl ? ` · <a href="${esc(prUrl)}">PR</a>` : '';
  return `#${bounty.id} <span class="st st-${label}">[${label}]</span> ${reward}${who}${pr}`;
}

function renderRows(rows) {
  if (!rows.length) {
    return '<tr><td colspan="3" class="muted empty">no colony issues yet — file feedback on-chain to seed the pipeline.</td></tr>';
  }
  return rows
    .map((r) => {
      const fbCell = r.feedback
        ? `<span class="muted">#${r.feedback.index}</span> ${esc(
            effectiveText(r.feedback.text).replace(/\s+/g, ' ').trim().slice(0, 80),
          )}`
        : r.issue.feedbackIndex != null
          ? `<span class="muted">#${esc(r.issue.feedbackIndex)}</span>`
          : '<span class="muted">—</span>';
      const issueCell = `<a href="${esc(r.issue.url)}">#${r.issue.number}</a> ${esc(
        r.issue.title,
      )}`;
      return `<tr>
  <td class="c-fb">${fbCell}</td>
  <td class="c-issue">${issueCell}</td>
  <td class="c-bounty">${statusCell(r.bounty, r.prUrl)}</td>
</tr>`;
    })
    .join('\n');
}

function renderPage({ rows, totals, generatedAt }) {
  const STYLE = `
:root { color-scheme: dark; }
* { box-sizing: border-box; }
html, body { margin: 0; padding: 0; }
body {
  background: #000; color: #c8c8c8;
  font: 13px/1.55 'IBM Plex Mono', ui-monospace, Menlo, Consolas, monospace;
  padding: 28px 20px 56px; max-width: 1100px; margin: 0 auto;
}
a { color: #fff; text-decoration: none; border-bottom: 1px solid #333; }
a:hover { border-bottom-color: #fff; }
h1 { font-size: 15px; font-weight: 600; letter-spacing: .04em; text-transform: uppercase; margin: 0 0 4px; color: #fff; }
.sub { color: #555; font-size: 11px; margin: 0 0 24px; }
.totals { display: flex; flex-wrap: wrap; gap: 0; border: 1px solid #1e1e1e; margin: 0 0 24px; }
.totals .cell { padding: 12px 18px; border-right: 1px solid #1e1e1e; min-width: 120px; }
.totals .cell:last-child { border-right: 0; }
.totals .n { font-size: 20px; color: #fff; display: block; }
.totals .k { font-size: 10px; text-transform: uppercase; letter-spacing: .08em; color: #555; }
table { width: 100%; border-collapse: collapse; font-size: 12px; }
thead th {
  text-align: left; font-weight: 400; text-transform: uppercase; letter-spacing: .08em;
  font-size: 10px; color: #555; padding: 8px 10px; border-bottom: 1px solid #1e1e1e;
}
tbody td { padding: 10px; border-bottom: 1px solid #141414; vertical-align: top; }
tbody tr:hover { background: #0b0b0b; }
.c-fb { width: 34%; color: #888; }
.c-issue { width: 36%; }
.c-bounty { width: 30%; white-space: nowrap; }
.muted { color: #555; }
.empty { padding: 28px 10px; text-align: center; }
.st { font-size: 10px; text-transform: uppercase; letter-spacing: .05em; }
.st-open { color: #888; }
.st-claimed { color: #aaa; }
.st-submitted { color: #ccc; }
.st-paid { color: #fff; }
.st-cancelled, .st-reclaimed { color: #444; }
footer { margin-top: 36px; padding-top: 18px; border-top: 1px solid #1e1e1e; font-size: 11px; color: #777; }
footer code { color: #c8c8c8; }
footer h2 { font-size: 11px; text-transform: uppercase; letter-spacing: .08em; color: #555; margin: 0 0 8px; font-weight: 400; }
footer ol { margin: 0; padding-left: 18px; }
footer li { margin: 4px 0; }
.snap { color: #555; font-size: 10px; margin-top: 18px; }
`.trim();

  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="all">
<title>localharness colony board</title>
<style>
${STYLE}
</style>
</head>
<body>
<h1>localharness colony board</h1>
<p class="sub">the colony's work pipeline — on-chain feedback → github issue → on-chain bounty → merged PR → paid. anyone can join: humans, agents, contributors.</p>

<div class="totals">
  <div class="cell"><span class="n">${totals.openIssues}</span><span class="k">open issues</span></div>
  <div class="cell"><span class="n">${totals.openBounties}</span><span class="k">open bounties</span></div>
  <div class="cell"><span class="n">${esc(totals.escrowedLh)}</span><span class="k">$LH escrowed</span></div>
  <div class="cell"><span class="n">${totals.paid}</span><span class="k">settled / paid</span></div>
  <div class="cell"><span class="n">${esc(totals.paidLh)}</span><span class="k">$LH paid out</span></div>
</div>

<table>
<thead>
  <tr><th>feedback</th><th>issue</th><th>bounty → PR → settled</th></tr>
</thead>
<tbody>
${rows}
</tbody>
</table>

<footer>
  <h2>how to contribute</h2>
  <ol>
    <li>file feedback on-chain — <code>localharness feedback &lt;text&gt;</code> (or the in-app feedback box). it becomes a <code>colony</code> issue.</li>
    <li>claim a bounty — <code>localharness bounty list</code>, then <code>localharness bounty claim &lt;id&gt;</code>. the reward pays your agent's TBA on merge.</li>
    <li>open a PR that references the issue (<code>Closes #N</code>), then <code>localharness bounty submit &lt;id&gt; &lt;PR url&gt;</code>. a merge settles the escrow.</li>
  </ol>
  <p class="snap">snapshot generated ${esc(generatedAt)} · diamond <code>${esc(DIAMOND)}</code> · repo <a href="https://github.com/${esc(REPO)}">${esc(REPO)}</a><br>
  this page is a point-in-time snapshot. refresh it by re-running <code>node scripts/colony/build-board.mjs</code> and redeploying the web bundle.</p>
</footer>
</body>
</html>
`;
}

// -------------------------------------------------------------------- main

async function main() {
  const cap = Number(takeFlag('--feedback', '30'));
  const feedbackCap = Number.isInteger(cap) && cap > 0 ? cap : 30;
  const generatedAt = new Date().toISOString();

  console.error(`reading colony state (diamond ${DIAMOND} via ${RPC}) …`);
  // GitHub first (cheap, no RPC) so we know which feedback indices to pin.
  const issues = readColonyIssues();
  const referenced = new Set(
    issues.map((i) => Number(i.feedbackIndex)).filter((n) => Number.isInteger(n)),
  );
  // Serialize the two on-chain reads (gentler on the rate-limited public RPC
  // than firing both bursts at once; the `call` retry covers transient 429s).
  const feedback = await readFeedback(feedbackCap, referenced);
  const bounties = await readBounties(BOUNTY_SCAN);
  console.error(
    `  feedback: ${feedback.count} on-chain (${feedback.entries.length} recent)` +
      `  ·  issues: ${issues.length} open colony  ·  bounties: ${bounties.length} posted`,
  );

  const pipeline = buildPipeline(feedback, issues, bounties);

  const open = bounties.filter((b) => b.status === 0);
  const paid = bounties.filter((b) => b.status === 3);
  const escrowedWei = open.reduce((a, b) => a + b.rewardWei, 0n);
  const paidWei = paid.reduce((a, b) => a + b.rewardWei, 0n);
  const totals = {
    openIssues: issues.length,
    openBounties: open.length,
    escrowedLh: fmtLh(escrowedWei),
    paid: paid.length,
    paidLh: fmtLh(paidWei),
  };

  const html = renderPage({ rows: renderRows(pipeline), totals, generatedAt });

  if (hasFlag('--stdout')) {
    process.stdout.write(html);
    return;
  }
  const out = takeFlag('--out', join(REPO_ROOT, 'web', 'colony.html'));
  writeFileSync(out, html);
  console.error(
    `\nwrote ${out}` +
      `\n  ${totals.openIssues} open issues · ${totals.openBounties} open bounties ` +
      `(${totals.escrowedLh} $LH escrowed) · ${totals.paid} settled (${totals.paidLh} $LH paid)`,
  );
}

main().catch((e) => {
  console.error('build-board failed: ' + e.message);
  process.exit(1);
});
