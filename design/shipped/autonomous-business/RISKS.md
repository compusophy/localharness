# Autonomous business — Risk & Compliance review (adversarial)

> **Verdict up front: do NOT let an autonomous loop post to social media or write
> to `main`/prod with real credentials yet.** The code/marketing loop is safe to
> automate *behind a human gate*; the publish/merge/spend/post legs are not. This
> doc is the gate spec. Reviewer posture is adversarial: assume the loop *will*
> eventually do the dumbest reachable thing, and require that the dumbest thing be
> harmless.
>
> Scope: a 30-minute recurring loop of role-agents doing code + marketing, slated
> to grow into posting on X / Instagram / TikTok / Reddit / Hacker News with real
> account credentials. Grounds in the project's existing conventions (CLAUDE.md:
> one owner of `main`, no worktree deploys, atomic releases, typed-confirmation
> destructive gate, secrets never in repo) and the autonomy-dial model already
> designed in `design/autonomous-loop.md` (observe / exercise / propose, default
> OFF). This is the social-media + production-loop extension of that model.

---

## 0. Threat model in one paragraph

An LLM role-agent fires every 30 min with: (1) write access to a git repo that
publishes a **live crate on crates.io** and a **live web app + on-chain mainnet
diamond**, (2) a wallet that can spend real `$LH` and trigger sponsored gas, and
(3) — the new ask — **API tokens for social accounts that represent the brand**.
The failure modes are not exotic. They are: a duplicated tick double-posts or
double-spends; a confused agent merges its own PR; a marketing agent posts 50
near-identical promos and the account is permanently banned by an *automated*
spam classifier with no human appeal; an agent commits a token into the repo;
an agent impersonates a real person or makes an unqualified earnings/financial
claim about `$LH` that is a securities/FTC problem. Every one of these is
reachable from "run the marketing role-agent on a timer." The guardrails below
make each one either impossible or human-gated.

---

## (a) Autonomous social-media risks

### a.1 Platform ToS for automation — the hard table

Automated *posting of brand content* sits in a different ToS bucket than the
read-only research the loop does today. Summary of what each platform actually
bans, drawn from current (2025–2026) policy:

| Platform | What is allowed | What gets you suspended/banned | Appeal reality |
|----------|-----------------|--------------------------------|----------------|
| **X / Twitter** | Posting via the **official paid API** only; clearly-labeled bot accounts (news/weather/price-style). | Auto follow/unfollow (banned *at any rate*), auto-DMs/bulk DMs, engagement farming (auto like/RT), auto-posting about trending topics, **duplicative or substantially-similar posts on one or across multiple accounts**. AI *reply* bots need **prior written approval from X**. | API access can be rate-limited/terminated independent of the account. |
| **Instagram (Meta)** | Scheduling via the official Graph API / approved partners. | Third-party automation tools that bypass the API, automated likes/follows/comments, bulk/duplicative posting → action-blocks then bans. | Meta automated enforcement; appeals slow and often unresolved. |
| **TikTok** | Scheduling via **official API**, Business Messaging API chatbots, native features. **Branded/AI content must be labeled.** | Automation tools/scripts/"tricks" that bypass systems, fake-engagement bots, artificial engagement. Sept 2025 guidelines tightened AI-content + misinformation rules. | Content removal → account ban. |
| **Reddit** | Genuine participation; the **10% rule** (≤10% of activity self-promotional). Bots must follow the Responsible Builder / API policy. | A bot that "continuously promotes specific products/services in a community or across communities" = spam *by definition*. New/low-rep domain links + high post rate + VPN/proxy → **automated shadowban** (invisible; your posts silently `[dead]`). Vote/karma manipulation. | Shadowban is silent — you won't know. Appeals via modmail/`hn@`-style contact, low success. |
| **Hacker News** | One honest `Show HN`; participate as a person. | **Soliciting upvotes** (incl. from friends/employees/social), voting rings (HN runs a voting-ring detector), astroturfing, repeated self-promo. **Domains get shadowbanned** → every submission instantly `[dead]`, forever, site-wide. | Email hn@ycombinator.com; one shot, and a burned domain is catastrophic for a project whose identity *is* a domain. |

**The cross-platform constant:** every one of these platforms runs **automated**
spam/authenticity classifiers that fire *before* any human sees the content, and
the penalty (shadowban / domain ban / API termination) is often **silent and
hard to reverse**. An autonomous loop optimizing for "post more" is
indistinguishable from a spam bot to those classifiers. Volume, near-duplicate
text, new-domain link-dropping, and burst timing are the exact features they key
on — and they are exactly what a naive 30-min "do marketing" loop produces.

> ⚠️ **Project-specific catastrophe:** `localharness.xyz` (and every
> `*.localharness.xyz` agent) is the brand's primary handle. An HN or Reddit
> **domain-level** shadowban for spam doesn't ban an account you can recreate — it
> poisons the *domain* the whole platform is named after. This is a
> one-way door. Treat domain reputation as an irreplaceable asset.

### a.2 Authenticity & disclosure (this is law, not etiquette)

- **FTC (US).** March 2025 staff guidance + the 2024 amendment to the
  Endorsement Guides (16 CFR Part 255): undisclosed AI-generated review/endorsement
  content not based on real experience is a **per-se deceptive practice** under
  §5. Fabricated/AI testimonials are explicitly covered. The emerging standard is
  **double disclosure** — disclose *both* the material connection (it's the
  brand's own agent) *and* that the content is AI-generated. Penalties run to
  ~$53k **per violation**, and each piece of content is a separate violation. The
  FTC brought its first AI-content enforcement action in late 2025.
- **EU AI Act, Article 50** (transparency obligations, applicable **2 Aug 2026**):
  AI systems that interact with people must tell users they're talking to AI;
  AI-generated text/image/audio/video must be **marked machine-readable** and, for
  text published to inform the public, **disclosed as AI-generated**. Fine tier:
  up to €15M or 3% of turnover. A Code of Practice on labeling (uniform "AI" cue,
  "Generated with AI" text) was published June 2026.
- **Platform-native AI labels.** TikTok, Instagram/Meta, and YouTube require
  creators to flag AI-generated/synthetic media; X requires bot accounts to be
  labeled as automated. Failing to use the native label is itself a ToS breach.

**Net rule:** any autonomously-generated public content must carry (1) an
AI-generated disclosure, (2) a material-connection disclosure where it endorses
the product, and (3) the platform's native AI/bot label. No exceptions, and this
is a *content-generation-time* requirement, not a post-time afterthought.

### a.3 Impersonation / brand-safety / legal exposure

An unsupervised poster can, without intending to:

- **Impersonate a real person** (quote-tweet "as" someone, fabricate a
  testimonial with a real name/face) → impersonation ToS violation + potential
  defamation/right-of-publicity liability.
- **Make a financial/earnings claim about `$LH`.** `$LH` is an on-chain credit
  token with real fiat on-ramp (Stripe, $1 = 100 `$LH`). An agent tweeting
  "buy `$LH`, it'll 10x" or anything investment-flavored is unregistered
  securities-promotion / financial-promotion risk (SEC/FCA/FTC). **Hard-ban this
  topic class for autonomous output.**
- **Pick a fight / post off-brand.** Replying to trolls, taking political
  positions, dunking on competitors — reputational damage with no human judgment
  in the loop. The agent has no concept of "this will look bad on the front page."
- **Leak internals.** Posting an unreleased feature, a private repo path, an
  address, or (worst case) a key fragment that landed in its context.
- **Engagement-bait / astroturf.** Asking the loop's *other* agents to upvote/RT
  is a textbook voting ring → instant HN/Reddit domain ban.

### a.4 Human-approval gates required before ANY social post

Social posting is **`propose`-rung-or-higher** in the autonomy-dial model and
must never be a fully-closed loop. Minimum gates:

1. **Draft-only by default.** The marketing agent writes posts to a review queue
   (a branch, a GitHub issue, a `drafts/` file, or a "pending" store) — it does
   **not** hold post credentials in the autonomous path.
2. **Human approves each post** (or each batch) before it goes live. This is the
   social-media analog of "merge stays human." Approval is an explicit operator
   act, never inferred from the agent's own confidence.
3. **Disclosure + label auto-attached** at draft time and verified at the gate.
4. **Rate/volume budget** enforced *and* spread over time (see d.7) so even an
   approved batch can't burst-post into a spam classifier.
5. **Topic denylist** checked at draft time (financial claims, politics,
   competitor attacks, anything naming a real third party).

---

## (b) Operational risks of a 30-min loop on a LIVE production crate

The repo ships a published crate (crates.io 0.51.x+), a live web bundle, a
mainnet diamond, and a money proxy. A loop with write access to that is a
production-incident generator unless fenced. Each item below maps to an existing
CLAUDE.md convention — the loop must *inherit*, not bypass, them.

### b.1 NEVER auto-merge to `main`
CLAUDE.md: **one owner of `main`**, admin push bypasses the PR rule. The loop's
ceiling is **open a PR** (the `propose` rung in `autonomous-loop.md`). A human
merges. An agent that approves/merges its own PR defeats the only review gate. No
agent gets a token with `contents:write` to `main` or merge rights.

### b.2 NEVER auto-deploy or auto-release
- **No `vercel deploy` / `vercel --prod` from the loop**, and *especially* never
  from a worktree — CLAUDE.md is emphatic that a worktree deploy spawns a stray
  Vercel project and parallel deploys clobber prod (`web` = antig, `proxy` is a
  *separate* deploy). The loop must not hold deploy creds.
- **No `release.sh` / `release.ps1`.** A release is atomic (bump → verify →
  commit → tag → push → `cargo publish` → GH release) and **irreversible**
  (`cargo yank` is not a delete; a tag/published version is forever). Releasing is
  a human act. The loop may *prepare* a CHANGELOG entry; it may not cut a version.
- **No `cargo publish`, no `diamondCut`, no facet upgrade.** These are
  one-way doors. The diamond-owner key is **not in the repo** by design — keep it
  that way; the loop never touches it.

### b.3 NEVER commit secrets
The repo's whole security model assumes the sponsor key is the *only* embedded
key and the diamond-owner/mainnet money keys live outside the repo (env / local
`.lh_*` files). An autonomous `git add -A` is the single most likely way a token
or seed leaks. Required:
- **`.gitignore` covers** `.env`, `*.key`, `.lh_*`, `.qa-sandbox/`, any social
  token file. The loop reads creds from env/secret-store, never from tracked files.
- **A pre-commit secret-scanner** (gitleaks/trufflehog or a regex gate) that
  **hard-fails** the commit. CLAUDE.md already warns: `git add -A` once swept a
  parallel WIP into a broken commit — generalize that scar into a scanner.
- **No `git add -A` / `git add .` in the loop.** Stage explicit paths only.

### b.4 NEVER spend uncontrolled `$LH` or API budget
- **`$LH` / on-chain writes** ride the existing **typed-confirmation
  destructive gate** (`chat::confirm_guard` / `src/confirm.rs`): `send_lh`,
  `release_subdomain`, and every value-moving tool deny the first call and require
  a single-use code echoed in the *latest user message* — which an autonomous loop
  **cannot** satisfy without a human. **Do not weaken this gate for autonomy.** It
  is the load-bearing reason an autonomous agent can't drain a wallet.
- **Per-run + per-day budget ceilings** (count of on-chain writes, `$LH` amount,
  LLM tokens/$). Exceeding either **aborts the tick** and files a finding — it does
  not "try smaller." Mirrors the `budget-exceeded` finding in `autonomous-loop.md`.
- **A funded agent on mainnet can self-pay** a narrow `SELF_PAY_SELECTORS` set
  (transfer/settle/approve/createInvite) via the relay — keep that allowlist
  narrow; everything else stays gated.
- **A global kill switch** (the autonomy dial) defaults **OFF** and is the master
  above all budgets.

### b.5 Branch discipline
- Every loop tick works on a **fresh, named branch off current `main` head**
  (`auto/<role>/<tick-id>`), never directly on `main`, never on a long-lived
  shared branch (avoids two agents colliding — CLAUDE.md: a parallel collision
  made the user "furious"). `git -C <main-clone>` semantics; integrate via PR +
  human merge + full gate on the merged tree.
- **No two agents on one working tree.** CLAUDE.md lesson: two dev agents on one
  tree → `git add -A` swept a parallel WIP into a broken commit. Worktree-per-agent
  or serialize.

### b.6 Idempotency — ticks must not repeat/duplicate
A 30-min cron *will* double-fire (overlap, retry, restart). Without idempotency
the loop double-posts, double-spends, double-PRs, or re-files the same finding.
Required:
- **One-turn-at-a-time** — reuse the existing `TURN_ACTIVE` / `send_when_idle`
  discipline so a tick never races a live turn (already in `autonomous-loop.md`).
- **Idempotency key per unit of work** (e.g. `keccak(role‖date‖task)`). Before
  acting, check a durable store (the off-chain GitHub-job store / on-chain log)
  for that key; skip if present. The off-chain scheduler's **claim-by-delete CAS**
  pattern (`design/offchain-scheduler.md`) is the proven primitive — reuse it so
  exactly one worker claims a job.
- **On-chain dedup** — the triage layer already dedups/clusters identical findings
  (`autonomous-loop.md` §3); extend the same to "have I already posted/PR'd this?"
- **Overlap guard** — a tick that starts while the previous is unfinished
  no-ops, it does not stack.

### b.7 Observability & circuit-breaker
- Every autonomous action emits a structured, queryable record (the on-chain
  feedback log / telemetry repo is the existing bus).
- A **float/error circuit-breaker**: N consecutive failed ticks, or a spend-rate
  spike, trips the dial to OFF and notifies the human (the proxy relay already has
  a "float breaker" precedent). No silent thrash.

---

## (c) Human-in-the-loop vs safe-to-automate

| ✅ Safe to automate (reversible, sandboxed, append-only) | 🔒 Human-in-the-loop required (irreversible / value-moving / reputational) |
|---|---|
| Read-only probing: chain reads, fetch public faces, `cargo test`, clippy, compile rustlite | **Merging to `main`** (PR is the ceiling) |
| Drafting code diffs **on a branch** + a regression test | **`cargo publish` / cutting a release / tagging a version** |
| Drafting marketing copy into a **review queue** (with AI + connection disclosure attached) | **Any social-media post going live** (each post/batch approved) |
| Filing findings to the on-chain/telemetry log (append-only) | **`vercel --prod` / any deploy (web or proxy)** |
| Sandboxed writes to **disposable, jailed** identities within budget (the `exercise` rung) | **`diamondCut` / facet upgrade / any owner-gated admin** |
| Opening a **PR** (`propose` rung) with a linked repro + test | **`send_lh` / spending real `$LH` / any value transfer** (typed-confirm gate) |
| Triage: dedup/cluster/rank existing findings | **`release_subdomain` / burning/transferring a real name** (typed-confirm) |
| Self-paced scheduling of *its own* future read-only ticks | **Committing anything touching secrets / `.env` / keys** |
| Notifying the human / requesting approval | **Posting financial/earnings claims about `$LH`, politics, or naming third parties — banned outright, not even gated** |
| Generating **drafts** of disclosures/labels | **Replying to or engaging real users/competitors unsupervised** |

The dividing line is exactly the one already in CLAUDE.md and
`autonomous-loop.md`: **probing and drafting are cheap and reversible; merging,
deploying, releasing, spending, and publishing-to-the-public are not.** Autonomy
is granted in that order and defaults OFF. The fleet's design goal is to operate
*entirely below the typed-confirmation line* so it never even attempts a gated
action.

---

## (d) Guardrail checklist (numbered, enforceable)

Each item is a hard, checkable control — not advice. "Enforced by" names the
mechanism that makes it real (a gate, a hook, an allowlist), because a guardrail
that's only a prompt instruction is not a guardrail.

**Autonomy & scope**
1. **Autonomy dial defaults OFF** (`observe`). `observe` = read-only tools +
   append-only findings only. `exercise` (sandboxed writes) and `propose` (PR /
   draft post) are deliberate, per-rung operator opt-ins, never inferred.
   *Enforced by:* dial file/flag checked at every write tool; missing/`observe` →
   writes hard-refuse.
2. **Social posting is never a closed loop.** The autonomous ceiling for social is
   **draft → review queue**. The loop holds **no** live post credentials.
   *Enforced by:* post tokens live only in the human-approval service, not in the
   agent's env.

**Production safety**
3. **No agent token can merge, deploy, release, publish, or cut a facet.** The
   loop's GitHub token is PR-only (no merge, no `main` push); no Vercel/crates.io
   /diamond-owner creds in the loop's environment at all. *Enforced by:* token
   scopes + absent creds.
4. **Branch-per-tick off current `main` head**, named `auto/<role>/<tick-id>`;
   never `main`, never a shared branch; worktree-per-agent or serialized.
   *Enforced by:* the harness creates the branch; a guard rejects commits to `main`.
5. **Pre-commit secret-scanner hard-fails** on `.env`/`*.key`/`.lh_*`/token
   patterns; `.gitignore` covers all of them; **no `git add -A`/`.`** — explicit
   paths only. *Enforced by:* git pre-commit hook + lint in the loop runner.

**Spend & value**
6. **Typed-confirmation gate stays intact and unweakened** for every destructive /
   value-moving tool (`send_lh`, `release_subdomain`, …). An autonomous agent
   cannot produce the human-echoed single-use code, so these are *structurally*
   un-automatable. *Enforced by:* `chat::confirm_guard` / `src/confirm.rs` —
   do not add an autonomy bypass.
7. **Per-run AND per-day ceilings** on `$LH` spent, on-chain writes, social posts,
   and LLM token/$ cost. Exceed → **abort the tick + file `budget-exceeded`**, do
   not retry smaller. *Enforced by:* a budget check before every spend/post,
   plus a circuit-breaker that trips the dial OFF on N failures or a spend spike.

**Idempotency**
8. **Idempotency key per unit of work** (`keccak(role‖date‖task)`) checked against
   a durable store before acting; **claim-by-delete CAS** so exactly one worker
   claims a job; an overlapping tick no-ops. Reuse `TURN_ACTIVE` /
   `send_when_idle`. *Enforced by:* the off-chain scheduler store + on-chain dedup.

**Social-media compliance**
9. **Every draft post carries, at generation time:** (a) AI-generated disclosure,
   (b) material-connection disclosure if it endorses the product, (c) the
   platform's native AI/bot label. *Enforced by:* the draft template + a gate check
   that refuses to enqueue a post missing any of the three.
10. **Topic denylist refuses to draft** financial/earnings/investment claims about
    `$LH`, political content, competitor attacks, or anything naming/impersonating
    a real third party. *Enforced by:* a content classifier/keyword gate at draft
    time; flagged drafts go to a human, never the queue.
11. **Per-platform rate + de-dup limits**, spread over time (no bursts):
    e.g. Reddit ≤10% self-promo and never the same domain repeatedly; HN one honest
    `Show HN`, **never** solicit cross-agent upvotes (voting ring = domain ban);
    X no near-duplicate posts across accounts; official APIs only, no bypass tools.
    *Enforced by:* a posting scheduler with per-platform counters + a similarity
    check against recent posts.
12. **No cross-agent engagement.** Loop agents must never like/RT/upvote/comment on
    each other's posts — that's the textbook voting-ring/astroturf pattern that
    triggers automated domain bans. *Enforced by:* the engagement tools are simply
    not granted to the marketing fleet.

**Identity & blast radius**
13. **Disposable, jailed sandbox identities** for any `exercise`-rung write
    (per-run keys under `.qa-sandbox/<run-id>/`, prefixed names, released at run
    end, `Filesystem` rooted at the jail). The loop's destructive surface is
    confined to junk it minted this run; it can never touch the operator's real
    names or keys. *Enforced by:* per-run key gen + name-prefix refusal + rooted FS.
14. **Domain-reputation protection.** Treat `localharness.xyz` as irreplaceable:
    no automated link-dropping of the apex/subdomains into HN/Reddit at volume; a
    domain shadowban is a one-way door. *Enforced by:* item 11 + a human gate on
    any post linking the primary domain.

**Operability**
15. **Structured audit log** of every autonomous action (who/what/tick-id/cost) to
    the existing telemetry/on-chain bus, and a **single kill switch** (the dial)
    that an operator can flip OFF instantly and that the circuit-breaker trips
    automatically. *Enforced by:* the dial + telemetry already in the substrate.

---

## Sources

- [X automation development rules — X Help](https://help.x.com/en/rules-and-policies/x-automation) · [X Authenticity / platform manipulation](https://help.twitter.com/en/rules-and-policies/platform-manipulation)
- [TikTok Terms of Service](https://www.tiktok.com/legal/page/global/terms-of-service/en) · [TikTok Integrity & Authenticity](https://www.tiktok.com/community-guidelines/en/integrity-authenticity)
- [Reddit Spam policy](https://support.reddithelp.com/hc/en-us/articles/360043504051-Spam) · [Reddit Responsible Builder Policy](https://support.reddithelp.com/hc/en-us/articles/42728983564564-Responsible-Builder-Policy)
- [Hacker News Guidelines](https://news.ycombinator.com/newsguidelines.html) · [HN FAQ](https://news.ycombinator.com/newsfaq.html) · [HN undocumented norms (shadowbans/voting rings)](https://github.com/minimaxir/hacker-news-undocumented)
- [FTC Endorsement Guides — 16 CFR Part 255](https://regulations.ai/regulations/RAI-US-NA-FTCENDG-2023) · FTC March 2025 AI advertising staff guidance + 2024 fake-review amendment (per-violation penalty ~$53k)
- [EU AI Act Article 50 — transparency obligations (applies 2 Aug 2026)](https://artificialintelligenceact.eu/article/50/) · [Article 50 practical guide](https://artificialintelligenceact.eu/transparency-rules-article-50/) · [EU Code of Practice on AI-generated content](https://digital-strategy.ec.europa.eu/en/policies/code-practice-ai-generated-content)
- Project conventions: `CLAUDE.md` (one owner of `main`, no worktree deploys, atomic releases, `confirm_guard` typed-confirmation gate, secrets-not-in-repo) · `design/autonomous-loop.md` (autonomy dial: observe/exercise/propose, sandbox jail, never auto-merge) · `design/offchain-scheduler.md` (claim-by-delete CAS idempotency)
