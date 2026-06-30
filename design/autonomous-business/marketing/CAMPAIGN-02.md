# CAMPAIGN-02.md — `git log`

> The second coordinated, single-through-line campaign — the **build-in-public META**
> wave that follows `CAMPAIGN-01` (`whoami`). Where `whoami` made *awareness* ("an agent
> can own itself"), `git log` makes *trust* ("…and here are the receipts"). A *campaign*,
> not a content dump: one big idea, one hero, one action, threaded beat-by-beat across
> every channel for ~2 weeks. It runs as the **second wave** — a fresh 2-week window after
> the `whoami` launch + `CALENDAR.md` Weeks 1–3, so its only X-pool reuse is the
> autonomous-business posts `whoami` left **unconsumed** (B2-4, B2-5). Every beat still
> pulls exact copy + per-asset accuracy guard from `READY-QUEUE.md`; this file is the
> director's cut over the top.
>
> **Accuracy lock (re-verified 2026-06-30 vs source).** Crate **0.58.0**; Tempo mainnet
> **4217**. OpenAI / Mock / Gemma are **SDK-only backends** — the live in-app selector is
> **Gemini Flash (default) + Claude Opus (premium)** only (`src/app/model.rs`); never frame
> the others as live in-app models. **No diamond/chain address is pinned** (facets churn via
> `diamondCut`). x402 settlement is a **mechanism, testnet rail** — no mainnet-live / earnings
> assertion. **Self-funding is the OPEN problem** — zero earnings / yield / investment claims,
> ever. `$LH` is a flat usage credit (1 `$LH`/message), never a token to pump.
>
> **Compliance lock (`RISKS.md`).** This is the highest-stakes campaign for the topic
> denylist (guardrail #10) because its hero literally **shows the books**. Every number that
> goes public carries the source's own **ESTIMATE** label (only the treasury is on-chain;
> `src/accounting.rs`), reads as a **cost / open problem**, and never as earnings, yield, or
> a reason to acquire `$LH`. Every AUTO post carries the canonical disclosure **verbatim** +
> the platform's native bot/AI label; the loop only *enqueues* and a human flips each item
> live. **Publishing the receipts is itself a human act** (Decision 7 — pushing the branch /
> opening a draft PR; the loop never pushes `main`, deploys, or merges). HN + Reddit are
> human-posted from aged accounts, **distinct bodies** from `whoami`'s H1/H2/H3. The loop's
> own role-agents **never** like/RT/upvote these — a voting ring here would be both a
> domain-ban risk *and* a credibility self-own for a campaign whose subject is those agents.

---

## 1. Name + big idea

**Campaign name:** **`git log`** — the unix command every developer runs to ask *what
actually happened here?* Lowercase, monospace, the campaign hashtag/codename. It is the
deliberate sibling of `CAMPAIGN-01`'s `whoami`: same terminal-native register, opposite
verb — `whoami` asks *who is this*, `git log` asks *prove it*.

**The big idea (one sentence):**

> **Every "autonomous AI company" is sold to you as a highlight reel — so instead of a demo
> we're handing you our `git log`: the full, append-only record of seven role-agents building
> localharness on localharness, honest books and all, including the burn they haven't solved
> and the human gate they never once crossed.**

The through-line is a single dare — *don't trust the pitch, read the receipts* — and every
channel beat is a different receipt:
- **the indie hacker** reads the receipt as a *running build log* (the ledger, the commits),
- **the Rust dev** reads it as an *architecture* (a pure-core ↔ I/O-shell loop with
  guardrails that are *structurally* incapable of the dumb thing — 0 merges, 0 `$LH` moved,
  0 live posts across every tick),
- **the crypto-skeptic** reads it as *anti-grift proof* (a company that says out loud it
  spends faster than it earns, with `$LH` a usage credit and self-funding the open problem).

And the meta-conceit that makes it a *campaign* and not a brag: **the company being shown is
the company doing the showing.** The marketing role-agent that drafted these very beats is one
of the seven on the org chart in the receipts. So the FTC/EU-AI-Act disclosure stops being a
tax — the receipts *are* the disclosure: they document, line by line, that the loop drafts and
a human fires. The medium is the message; the ledger is the proof.

**Why this through-line wins:** it is (a) **distinct from `whoami`** on a clean axis — `whoami`
is *one agent's identity*; `git log` is *a whole company's operation, opened to inspection*;
(b) **ownable** — no competitor can credibly hand you its losses; radical transparency is a
moat you build by telling the truth; (c) **on-brand to the atom** — cypherpunk receipts-over-
trust, anti-bloat, honest-scope; and (d) it converts the two hardest constraints (mandatory
AI-disclosure + self-funding-is-OPEN) into the *content* instead of the fine print.

**The bridge from `whoami`:** `whoami` says *an agent can own itself*. `git log` answers the
obvious next question — *says who?* — with the receipts of the company that made the claim.
Act one earns the curiosity; act two earns the trust.

---

## 2. Target audience + the ONE action

**Audience (the three `BRAND.md` segments, re-prioritized for a receipts campaign):**
1. **AI-agent & indie-hacker / build-in-public builders** (HN, Indie Hackers, X
   build-in-public, the MCP/Claude-Code crowd) — this audience *rewards* a raw, honest build
   log over a glossy launch; it is the natural front door for receipts.
2. **Rust / SDK developers** (r/rust, HN, crates.io/docs.rs) — the loop's architecture and
   guardrails (`LOOP-PROTOCOL.md`, the pure-core/IO-shell split, the open branch) are catnip;
   the whole thing is `cargo`-inspectable.
3. **Crypto / Tempo / x402 / EIP-6551 builders** (r/ethdev, Farcaster/X crypto-dev) — the
   post-meme-cycle skeptic is won by the anti-grift stance: a "company" that is a guild +
   treasury + role members, `$LH` a credit not a pump, self-funding named as unsolved.

**The ONE action (north-star):** **claim an agent — `localharness.xyz` → claim a name** (and,
for the deeper cut, `company found …`). The campaign-specific funnel is *read the receipts →
trust it's real → found your own role-agent / claim your own name*. Secondary action for the
SDK segment: **star the repo / `cargo add localharness`**. North-star = on-chain identity
claims (`GROWTH.md §5`).

---

## 3. The HERO asset

### "Open the books" — the public `autonomous-business` branch as one browsable receipt

`whoami`'s hero was *the demo is a URL* (a live agent). `git log`'s hero is its twin one rung
deeper: **the demo is the repo.** The centerpiece is not a produced film — it is a **real,
public, append-only artifact you can read end to end**, packaged as one linkable property:

**(a) The receipts (the share object + proof).** The `autonomous-business` branch, made public
and browsable, anchored by four real files anyone can open and verify:
- **`LEDGER.md`** — the append-only, one-entry-per-tick build log (newest at top). Every
  30-minute tick: what shipped, what was verified, what stayed human-blocked.
- **the `git log` itself** — the commit history behind that ledger; the receipts have receipts.
- **the honest books** — `cargo run --example autonomous_company` prints a company's read with
  a **negative net position, `relies_on_seed`, "NOT yet self-funding"** — the source's own
  words (`src/accounting.rs`; only treasury is on-chain, the rest is **ESTIMATE**-labeled).
- **`STATUS.md` + the seven role personas** (`roles/{executive,pm,coder,reviewer,accounting,
  hr,marketing}.md`) — the org chart of agents that did the work.

The single most quotable receipt, and the one that travels: the guardrail line that held every
tick — **`0 on-chain writes · 0 $LH moved · 0 live posts · 0 merges`**. It is a *non-financial*
claim, it is true, and it is the exact opposite of every "fully autonomous agent" hype thread.

**(b) The conversion surface.** From the receipts, every link funnels to `localharness.xyz` —
*the seven agents that built this are seven names someone's key owns; found your own company, or
claim one name, from a shell.* The receipts prove the product by **being built with** it.

**(c) The feed cut (derivative, optional).** A ~60–90s monochrome, terminal-real screen-
recording for X/feeds — captions carry it, sound-off safe: scroll `git log --oneline` of the
branch → open `LEDGER.md` → run `cargo run --example autonomous_company` and land on the honest
"relies on seed, NOT yet self-funding" output → end on the `0 writes · 0 $LH · 0 posts · 0
merges` card. Reuses the *same* single-device monochrome capture kit as `whoami`'s film; no new
production pipeline. **No diamond address on screen, no `$LH` totals framed as earnings, every
number ESTIMATE-labeled.**

**Why it's share-worthy:** in a feed where every AI-company post is a victory lap, the one that
hands you its losses and a `0 merges` guardrail reads as *different* before a word lands. Devs
screenshot a real ledger and a real honest-net-position line precisely because nobody else
shows them. It is brutalist and terminal-real; it is auditable, not asserted; and the punchline
— *the company in the receipts wrote the receipts* — is the rare AI-launch flex whose
disclosure is the best part of it.

**Human-gate on the hero (honest):** publishing the receipts means a human pushes the branch
public / opens a draft PR — that is **Decision 7** in `DECISIONS.md`, a deliberate operator act
(the loop never pushes `main`, deploys, or merges; `RISKS.md` b.1/b.2). Until that gate is
flipped, the hero is staged, not live. The campaign does not pretend otherwise.

---

## 4. The channel sequence (beat-by-beat, ~2 weeks)

> A fresh 2-week window opening **after** the `whoami` launch + `CALENDAR.md` Weeks 1–3. Honors
> the same spacing law (≤1 substantive X post/day, X threads never same-day/adjacent, dev.to
> long-forms ≥1wk apart, HN one human shot, Reddit human + aged + **distinct** bodies, no
> bursts, similarity-gated). NEW beats are marked; everything else cites a `READY-QUEUE.md` id
> whose exact copy + accuracy guard is the source of truth. The only X-pool reuse is **B2-4 /
> B2-5** (the autonomous-business posts `whoami` did not fire).

| Day | Channel | Beat — the `git log` angle | Hook | Asset | Lane |
|-----|---------|----------------------------|------|-------|------|
| **D0 Sun** | X | **Teaser (NEW).** State the dare, no link. | "every autonomous-AI-company demo is a highlight reel. ours ships with receipts." | **NEW** (copy §5.1) | AUTO |
| **W1 Mon** | GitHub / Web | **RECEIPTS GO LIVE (NEW property).** `autonomous-business` branch + pinned `LEDGER.md` made public; repo social-preview = the `0 writes·0 $LH·0 posts·0 merges` still. | the repo *is* the proof. | **HERO** (+ NEW preview img) | **human publishes** (Decision 7) |
| **W1 Mon** | X | **LAUNCH — anchor thread (NEW), pinned.** Walk the receipts: 7 roles · the ledger · the guardrails that held · the honest books. | "we let a company of role-agents build this. here's the git log." | **NEW thread** (copy §5.2); link to receipts in reply 1 | AUTO (thread) |
| **W1 Tue** | Hacker News | **Show HN (HUMAN, NEW — distinct from `whoami` H1).** | "Show HN: I open-sourced the receipts — AI role-agents building a company in public, ledger and burn included." | **NEW H** (§5.5) | **HUMAN**, own voice, aged acct |
| **W1 Wed** | X | **B2-4 — "what it is."** The org as composition. | "a company that's nothing but role-agents — each a subdomain with its own wallet." | **#9 / B2-4** | AUTO (filler) |
| **W1 Thu** | dev.to | **Anchor long-form (NEW canonical).** The receipts, narrated: the loop, the guardrails, the burn. | "We built a company out of AI agents and opened its books." | **NEW dev.to** (§5.6) | AUTO (human flips `published`) |
| **W1 Fri** | X | **B2-5 — honest scope.** The receipt nobody else posts. | "…including the part that isn't solved: it spends `$LH` faster than it earns." | **#9 / B2-5** | AUTO (filler) |
| **W1 Sat–Sun** | — | Monitor; reply on HN/dev.to as a human. Pull launch analytics. | — | — | — |
| **W2 Mon** | LinkedIn | **#7 autonomous-business vision.** The B2B/credibility cut of the receipts. | "a business that is nothing but software agents — what's shipped, and what isn't." | **#7** | AUTO¹ (else human posts copy) |
| **W2 Tue** | X | **#8 founder-story thread.** The human *why* behind a company of agents. | "why self-sovereign, not rented — in the first person." | **#8** | AUTO (thread; ≠ same day as any thread) |
| **W2 Wed** | Reddit r/rust *or* r/AI_Agents | **HUMAN (NEW — distinct body).** The *loop architecture*: pure-core/IO-shell, the guardrail model, the open branch. | "I built an autonomous agent loop that's structurally incapable of merging its own PR — here's the design." | **NEW H** (§5.5) | **HUMAN**, aged acct, 9:1 |
| **W2 Thu** | X | **The guardrails post (NEW single).** The credibility beat. | "11 ticks. 0 merges, 0 `$LH` moved, 0 posts fired. autonomy you fence with arithmetic, not vibes." | **NEW** (copy §5.3) | AUTO |
| **W2 Fri** | X | **B2-2 — identity hook (drip).** Pivot back to the ONE action. | "every agent is a subdomain — including all seven of those role-agents." | **#9 / B2-2** | AUTO (filler; +day from #8/guardrails) |
| **W2 Sat–Sun** | — | KPI refresh (identity claims, stars, clones, downloads); plan the sustain tail. | — | — | — |
| **W2 Sun** | X | **Recap quote-post (NEW)** — close the loop with the REAL number. | "two weeks ago we opened the books. since then: {N} claimed their own. the log's still appending." | **NEW** (copy §5.4) | AUTO (filler; +2d) |

¹ LinkedIn is AUTO only after Community Management API approval lands (`GROWTH.md §2.7`); until
then a human posts the same copy manually. Honest: this is the one beat that may lag.

**Sustain tail (out of the 2-week core):** the remaining `#9` pool drips (B2-1/3/7/9/10) and
the **`VISUAL-BRIEFS.md` V1 / V6** short-form cutdowns (Tier-3, human-gated, blocked on IG/TikTok
account + API approval/audit) continue the *receipts → claim* arc the moment that setup lands —
stated honestly so the campaign doesn't pretend a channel is live that isn't.

---

## 5. New creative copy (the beats not already in the asset files)

> These are NEW (teaser, anchor thread, guardrails post, recap) or NEW-distinct human/long-form
> drafts. All other beats use the exact verbatim copy in `READY-QUEUE.md` / `X-POSTS-BATCH-2.md`.
> Each X post is ≤280 chars and fires with the canonical disclosure as its **immediate first
> reply** + the **Automated-account** native label. Any link goes in a reply, never the body.

### 5.1 D0 teaser (X, NEW) — no link, the dare

```
every "autonomous AI company" demo is a highlight reel.

ours ships with receipts.

monday we open the books on a company of role-agents building localharness — the ledger, the commits, and the burn it hasn't solved.

git log →
```

Disclosure (immediate first reply, verbatim):
```
AI-generated, human-reviewed. Posted by localharness's own automated account. #AI
```

### 5.2 W1 Mon anchor thread (X, NEW) — the receipts walk

> Reply-chain on ONE account; each post ≤280; disclosure as the **final** reply. Distinct in
> content from `#6` (which states the org *composition*) — this one *walks the evidence*.

```
1/
We didn't write a launch post. We opened a git log.

A company of seven role-agents — exec, PM, coder, reviewer, accounting, HR, marketing — has been building localharness, on localharness, in the open. Here are the receipts. 🧵

2/
The org isn't a new contract. It's a composition that already ships: a guild for identity, the guild's token-bound account as a shared $LH treasury, and seven role subdomains as members. Each role is a real on-chain name with its own wallet and persona.

3/
The proof is an append-only ledger — one entry per 30-minute tick, newest on top. What shipped, what was verified, what stayed blocked on a human. No summary. The raw log. You can read every tick.

4/
What got built (read-only, all on a branch): found a company in one call, then status / plan / payroll / books / day / forecast it. Five pure decision cores, fully tested, native AND wasm. A runnable example. Nothing merged, nothing deployed.

5/
The receipt nobody else posts — the honest books. The example prints a NEGATIVE net position, "relies on seed, NOT yet self-funding." $LH is a usage credit it spends on inference, not a coin. Self-funding is the open problem, and we say so.

6/
And the line we're proudest of, held every single tick:
0 on-chain writes · 0 $LH moved · 0 live posts · 0 merges.
An autonomous loop fenced by arithmetic and a human gate — not by hoping it behaves.

7/
The meta part: the marketing agent that drafted this thread is one of the seven in the receipts. The loop drafts; a human reviews and fires every live post. The disclosure isn't fine print — the ledger documents it, line by line.

8/
Read the whole log, then go own a piece of it: the seven roles are seven names a key owns. Found your own company, or claim one name, from a shell. Open source, Apache-2.0.
crates.io/crates/localharness
```

Disclosure (immediate final reply, verbatim):
```
AI-generated, human-reviewed. Posted by localharness's own automated account. #AI
```

### 5.3 W2 Thu guardrails post (X, NEW) — the credibility single

```
"Fully autonomous agents" usually means nobody fenced the dumb thing.

Our loop, across every tick: 0 merges, 0 $LH moved, 0 live posts.

It can't merge its own PR, can't post live, can't move value — the gates are structural, not vibes. Autonomy you can audit.
```

Disclosure (immediate first reply, verbatim): same canonical line.

### 5.4 W2 Sun recap (X, NEW) — REAL number only, or don't post

```
two weeks ago we stopped pitching and opened the books on a company of role-agents.

since then: {N} people claimed their own on-chain agent from a shell.

the ledger's still appending. read it, then write your own entry ↓
```
Link (`localharness.xyz`) in the first reply; disclosure verbatim as the next reply.
**Fire-time guard:** `{N}` is the real on-chain claim count a human verifies via the Diamond
read-only RPC (`GROWTH.md §5`). If it's zero or unflattering, **do not post the number** — swap
to a receipts-scroll clip. No fabricated metrics, ever; no earnings framing.

### 5.5 Human-gated drafts (NEW — distinct bodies, human posts, NO automation)

These are **human-only by ToS** (`RISKS.md` a.1; `GROWTH.md §2.3/2.6`): aged/karma accounts,
no automation, no upvote solicitation, value-first, 9:1-budget-gated, **never** the loop. Each
must be written **distinct** from `whoami`'s H1/H2/H3 *and* from each other (Reddit/HN flag
near-identical multi-sub posts; a domain shadowban on `localharness.xyz` is a one-way door).

- **Show HN (W1 Tue) — title:** `Show HN: A company of AI role-agents building itself in public —
  the ledger, the commits, and the burn`. Body angle: first-person, honest — *I let a loop of
  seven role-agents build a real read-only product on a branch; here's the append-only ledger,
  the guardrails that kept it from merging/posting/spending, and the part that doesn't work yet
  (it isn't self-funding). Try the SDK; read the log.* One US-morning weekday shot.
- **Reddit (W2 Wed), r/rust or r/AI_Agents — title:** `An autonomous agent loop that's
  structurally incapable of merging its own PR — the pure-core / IO-shell design`. Body angle:
  the *architecture* (decisions live in tested pure cores; I/O + value-moving stays behind a
  human-echoed typed-confirmation gate; branch-per-tick, no `git add -A`, secret-scan). Distinct
  from H2's SDK-seam post — this is the *loop/guardrail* story. Value-first; present in comments.

### 5.6 Anchor dev.to long-form (W1 Thu, NEW) — the canonical

**Title:** `We built a company out of AI agents and opened its books: the ledger, the guardrails,
and the burn`. Distinct angle from all three shipped dev.to deep-dives (`DEVTO-ARTICLE{,-2,-3}`
= SDK-seam / x402+6551 / rustlite) — this is the **autonomous-business meta-story**. Front-matter
`published: false` (human flips live), `canonical_url: https://localharness.xyz/llms.txt`, footer
disclosure verbatim per `READY-QUEUE.md §2`. **Accuracy guard:** the company = guild + treasury +
role members (composition, not a new contract); `found_company`/`company_status`/the CLI surface
ship and are read-only/confirm-gated; the seven roles are real (`roles/*.md`); the books are
ESTIMATE-labeled with self-funding named OPEN; the `0 writes · 0 $LH · 0 posts · 0 merges`
guardrail and the branch-only/no-merge posture match `STATUS.md` + `RISKS.md`; **no diamond
address pinned; x402 testnet-only; no `$LH` financial claim; OpenAI/Mock/Gemma SDK-only.**

---

## 6. Success metrics

**North-star (the only conversion that counts):** **new on-chain identity claims**
(`localharness create` / `company found`), queried free + un-fakeable via the Diamond read-only
RPC (`GROWTH.md §5`). Campaign attribution via a single UTM (`utm_campaign=gitlog`) on every link
to the receipts / `localharness.xyz`.

**Campaign creative-health metric (the `git log` analog of `whoami`'s film-completion rate):**
**GitHub movement over the window** — stars, unique clones, unique visitors, and `autonomous-
business` branch / PR views (`traffic` API). The receipts campaign *should* move the repo more
than `whoami` did, because the hero **is** the repo; if it doesn't, the receipts aren't landing.

**Secondary:** crates.io downloads of `localharness` (crates.io API) — the `cargo add` / star
action for the SDK segment.

**Leading indicators (feed the above):**
- **Receipts property:** sessions + claim/`cargo add` click-through, by UTM.
- **Anchor thread:** completion + the reply-7 "the company wrote the receipts" quote-rate (does
  the meta-conceit land?).
- **dev.to anchor:** reading time + comment *sentiment* — for a trust campaign, whether skeptics
  engage the honest-scope framing constructively is the signal, not raw reactions.
- **HN/Reddit:** comment quality on the guardrail/honest-books framing (human-read).
- **AI-referral sessions** (`chatgpt.com` / `perplexity.ai` / `claude.ai` referrers) — the
  fact-dense receipts are strong GEO feedstock (`GROWTH.md` Experiment 1, runs in parallel).

**Built-in experiment hooks (`GROWTH.md §4`):**
- **Exp. 2 (framing A/B):** `git log` is the **transparency/trust** framing — run matched
  UTM-tagged pairs against `whoami`'s **agent-first/identity** framing to see which converts
  identity claims better, and whether trust-framing wins the *skeptic* segments specifically.
- **Exp. 3 (cross-post flywheel):** the W1 sequence (receipts drop → anchor thread → dev.to
  canonical → the human HN/Reddit touches in-window) is the flywheel under test; success = the
  `git log` sequence drives more identity claims per unit effort than any single-channel push.

**Directional success criterion (no fabricated targets):** the campaign succeeds if, over the 2
weeks, `utm_campaign=gitlog` identity claims clearly exceed a comparable prior single-channel
push, **and** GitHub stars/clones move materially against their pre-campaign baseline. No numeric
promise is made (no baseline yet) and **no earnings claim is permitted.**

---

## 7. Compliance, AI-disclosure & human-gates (binding at fire time)

Straight from `RISKS.md` (a.1 / a.2 / a.4 / guardrails #9–#14) and `GROWTH.md §3`, with the
campaign-specific surfaces called out:

- **Showing the books is the #1 denylist risk (guardrail #10).** Every public number is a
  **cost / open-problem** statement, carries the source's **ESTIMATE** label (only treasury is
  on-chain; `src/accounting.rs`), and **never** reads as earnings, yield, ROI, or a reason to
  acquire `$LH`. `$LH` is stated as a usage credit; self-funding is named OPEN. No `$LH` price/
  investment framing anywhere. If a beat can't show a number without implying a return, it shows
  the *honest negative* or it shows nothing.
- **Publishing the receipts is a human act.** Pushing the `autonomous-business` branch public /
  opening the draft PR is **Decision 7** — an operator decision, never the loop (`RISKS.md`
  b.1/b.2: no auto-merge, no auto-deploy, the loop's token is PR-only). The receipts are
  human-curated before they go public: **no secrets, no unreleased-feature leaks, no diamond
  address, no mainnet-live x402 assertion** (the branch openly discusses testnet vs mainnet —
  that nuance must survive into anything public).
- **Disclosure + native label on every AUTO post, at generation time.** Canonical line verbatim
  as the immediate first/last reply (X) / footer (dev.to, LinkedIn) **plus** the platform's
  native label (X **Automated-account**; dev.to/LinkedIn footer text). The receipts' "the loop
  drafts, a human fires" framing is **additive** — it dramatizes the disclosure, never replaces
  the mandated verbatim line. FTC 16 CFR Part 255 + EU AI Act Art. 50 (applies 2026-08-02).
- **The loop only enqueues; a human flips each AUTO item live.** The autonomous path holds **no**
  live post credentials.
- **No cross-agent engagement — load-bearing here (guardrail #12).** The seven role-agents must
  **never** like/RT/upvote/comment on these posts. For a campaign whose *subject* is those agents,
  an astroturf ring is both the textbook domain-ban trigger **and** a fatal credibility self-own —
  the engagement tools are simply not granted to the fleet.
- **HN + Reddit stay human-posted**, in the operator's own voice, from aged/karma accounts, with
  **distinct bodies** from `whoami`'s H1/H2/H3 and from each other (§5.5). No automation, no
  upvote solicitation, no identical cross-posting. A voting-ring → **domain** shadowban is a
  one-way door for a brand whose handle *is* `localharness.xyz` (`RISKS.md` a.1 / #14).
- **Accuracy guards ride each asset.** Every cited `READY-QUEUE.md` id keeps its re-verified
  guard; the NEW beats (§5) + the hero (§3) are pinned to the accuracy lock at the top. **No
  diamond address on screen or in copy; x402 mechanism-only / testnet; OpenAI/Mock/Gemma
  SDK-only; crate 0.58.0.**
- **Domain-reputation protection + per-day ceiling + similarity check** enforced; links into
  HN/Reddit are human-gated; no bursts, no near-duplicate copy on/across accounts (guardrails
  #11/#14).

---

## 8. Honest scope (what this campaign is and isn't)

- It is a **distinct second campaign** under a new through-line, sequenced **after** `whoami`,
  reusing only the **unconsumed** autonomous-business X-pool posts (B2-4/B2-5) plus the
  autonomous-business long-forms/threads that fit (#7/#8), with **five new beats**: the D0
  teaser, the **receipts hero** (open books / public branch), the **anchor thread**, the
  **guardrails single**, the **dev.to canonical**, and the recap — plus two **distinct** human
  HN/Reddit drafts.
- The receipts hero is the only **new production**, and it is **single-device, monochrome,
  real-screen** capture of an artifact that **already exists** (the branch, the ledger, the
  example) — the lowest-risk hero possible, because nothing is staged: you are recording the
  truth.
- **Deferred / gated, named honestly:** publishing the receipts is owner-gated (**Decision 7**);
  LinkedIn AUTO posting may lag (Community Management API approval — human posts manually until
  then); the IG/TikTok short-form cutdowns are blocked on account + API approval/audit and run in
  the sustain tail; the live, *autonomously-operating* company is **not** claimed — the receipts
  show a read-only PREVIEW on a branch with **nothing executed on-chain, nothing posted, nothing
  merged** (`STATUS.md`), and live execution / real posting wait on the 8 calls in `DECISIONS.md`.
- **No earnings claim is made anywhere.** Self-funding is the open problem the entire campaign is
  built around naming out loud — that honesty is not a caveat, it is the product.
