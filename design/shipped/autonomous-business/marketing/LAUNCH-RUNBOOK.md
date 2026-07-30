# LAUNCH-RUNBOOK.md — the one turnkey go-live runbook

> **Purpose.** The single operator checklist that makes launch *mechanical* the moment
> the owner drops credentials into `.env.marketing`. It consolidates `CALENDAR.md`
> (3-week schedule) + `CAMPAIGN-01.md` (`whoami`) + `CAMPAIGN-02.md` (`git log`) into
> **one ordered fire-sequence**, and it is the operational front-end to `READY-QUEUE.md`
> (exact copy), `CREDENTIALS.template.md` (the secrets), `GROWTH.md` (the playbook), and
> `RISKS.md` (the gate spec). Execute it **top-to-bottom**.
>
> **Who runs it:** an operator — a human, or the loop *once an operator has deliberately
> greenlit it*. **The loop only ever *enqueues*; a human flips each live item.** The
> autonomous path holds **no live post credentials** (`RISKS.md` a.4 / guardrail #2).
>
> **Accuracy + compliance lock (binding on every step, re-verified 2026-06-30 vs source):**
> crate **0.58.0**; OpenAI / Mock / Gemma are **SDK-only backends** (the live in-app
> selector is **Gemini Flash + Claude Opus** only — never frame the others as a live
> in-app model); **no diamond/chain address is pinned** (facets churn via `diamondCut`);
> **x402 settlement is testnet-only** — no mainnet-live / earnings assertion;
> **self-funding is the OPEN problem** — zero earnings / yield / investment claims, ever;
> `$LH` is a flat usage credit (1 `$LH`/message). HN + Reddit are **human-posted**; no
> cross-agent engagement; FTC/EU-AI-Act disclosure on **every** post.

---

## 0. FIRST 5 ACTIONS WHEN CREDS LAND (day-one needs no thinking)

Do these five, in order, the moment `.env.marketing` is populated. Nothing else required to start.

1. **Load + validate `.env.marketing`** (it is gitignored by the `.env.*` rule — confirm
   it is NOT tracked, then run the secret-scanner). Verify the **core** creds resolve:
   `GITHUB_TOKEN` (PR-only scope, no merge/admin), `X_API_KEY/SECRET` +
   `X_ACCESS_TOKEN/SECRET` + `X_BEARER_TOKEN`, `DEVTO_API_KEY`. **Missing Tier-3
   (LinkedIn / IG / TikTok) is NOT a blocker** — those channels are simply "disabled" and
   a human posts their copy manually until approval lands.
2. **Run the GO / NO-GO gate** (§3, all 8 criteria). Any **NO** → fix before firing. The
   one exception: GitHub asset **#1** is first-party, zero-risk and may proceed alone the
   moment its own cred + accuracy check pass, even if other channels lag.
3. **Fire Asset #1 — GitHub** repo description + topics + the `whoami` social-preview
   still (§5, D1). Instant, zero-risk, own property. This is the safe first move.
4. **Set the X account to "Automated account"** (X's native bot label, linked to the human
   operator — a one-time account setting), confirm the **payment method** is on the dev
   account, then stage the **D0 `whoami` teaser** (`CAMPAIGN-01` §5.1) with its first-reply
   disclosure into the human-approval queue. **Publish `whoami.localharness.xyz`** (the
   hero landing — a human act).
5. **Confirm the human approver is on call** and the **HN + Reddit aged/karma accounts**
   are ready with a healthy 9:1 budget; sign off the **accuracy lock**. Then begin the
   day-by-day fire sequence (§5) at **D0 → D1**.

---

## 1. PRE-FLIGHT — credentials, accounts, handles

### 1.1 `.env.marketing` — required for the CORE launch (Tier 1 + 2)

Per `CREDENTIALS.template.md`. The launch **cannot fire a given channel** until its line is
real (a placeholder = "channel disabled"). Core = everything needed for Weeks 1–6 except the
gated Tier-3 channels.

| ☐ | Env key(s) | Channel | Gate note |
|---|---|---|---|
| ☐ | `MARKETING_EMAIL` | shared | The single registration/recovery inbox; agent does NOT need its password. |
| ☐ | `GITHUB_TOKEN` | GitHub #1 | **Fine-scoped PAT/App: repo metadata + `contents` for release notes + read traffic. PR-only — NO merge, NO `admin`, NO org-wide write** (`RISKS.md` guardrail #3). |
| ☐ | `X_API_KEY`,`X_API_SECRET`,`X_ACCESS_TOKEN`,`X_ACCESS_TOKEN_SECRET`,`X_BEARER_TOKEN` | X #3/#4/#6/#8 + pool + NEW beats | OAuth user-context scoped to `@localharness` write + app-only bearer for own-metric reads. **Post/own-analytics ONLY — never follow/like/DM endpoints.** |
| ☐ | *(human action)* X **payment method** + **Automated-account** label ON | X | Posting is billed per call (~$0.015; $0.20 with a link → put links in a reply). Automated label is a ToS requirement for automated posting. |
| ☐ | `DEVTO_API_KEY` | dev.to #2/#2b/#2c + git-log canonical | `POST /api/articles` with `published:false`; a human flips live. |
| ☐ | `REDDIT_CLIENT_ID`,`REDDIT_CLIENT_SECRET`,`REDDIT_REFRESH_TOKEN`(preferred),`REDDIT_USER_AGENT` | Reddit metrics | **Reads own metrics only.** H2/H3 **submissions are HUMAN** from an aged account — the real prereq is the aged/karma account, not the token. |
| ☐ | *(human action)* **HN account** with real history/karma | Hacker News H1 + git-log Show HN | **No token, ever** — HN is touched only by a human, never programmatically. |
| ☐ | `ANTHROPIC_API_KEY`,`OPENAI_API_KEY`,`PERPLEXITY_API_KEY` *(optional, recommended)* | GEO citation panel | Weekly Experiment 1; reuse existing model access. |
| ☐ | `ANALYTICS_API_KEY` *(optional)* | KPIs | UTM / AI-referrer attribution; skip if reading server logs. |

### 1.2 `.env.marketing` — Tier-3, MAY LAG (NOT a launch blocker)

These gate specific later beats; until they land a human posts the same copy manually.
**Do not block launch on them.**

| ☐ | Env key(s) | Channel | What it gates / lag |
|---|---|---|---|
| ☐ | `LINKEDIN_CLIENT_ID/SECRET`,`LINKEDIN_ACCESS_TOKEN`(`w_member_social`),`LINKEDIN_REFRESH_TOKEN`,`LINKEDIN_ORG_ID` | LinkedIn #5 (D8), #7 (D36) | **Community Management API approval (manual review).** AUTO only after approval; else human posts. ~60-day token → refresh. |
| ☐ | `META_APP_ID/SECRET`,`IG_USER_ID`,`IG_LONG_LIVED_ACCESS_TOKEN` | IG Reels (VISUAL-BRIEFS, sustain tail) | `instagram_business_content_publish` **app review: 2–4 wks.** |
| ☐ | `TIKTOK_CLIENT_KEY/SECRET`,`TIKTOK_ACCESS_TOKEN`,`TIKTOK_REFRESH_TOKEN`,`TIKTOK_OPEN_ID` | TikTok (VISUAL-BRIEFS, sustain tail) | Content-Posting **audit** to leave SELF_ONLY; private until audited. |

### 1.3 Accounts + handles reserved (human, one-time — the agent CANNOT do these)

| ☐ | Item | Note |
|---|---|---|
| ☐ | **`@localharness` reserved identically** on X, dev.to, Reddit, GitHub, LinkedIn, IG, TikTok | Reserve even on channels not yet used (`CREDENTIALS.template.md` §1). |
| ☐ | Each account **phone/CAPTCHA-verified by a human**, dev app created | Signup automation is impossible AND against ToS (`GROWTH.md` §0). |
| ☐ | **`whoami.localharness.xyz` claimed + live** | The `CAMPAIGN-01` hero landing; must render before D1. |
| ☐ | **`autonomous-business` branch staged for public push** | The `CAMPAIGN-02` "open the books" hero. Publishing it = **Decision 7** (human), curated for no secrets / no diamond address / testnet-vs-mainnet nuance intact, BEFORE Week 4. |
| ☐ | HN + Reddit accounts **aged, karma-bearing, 9:1 budget healthy** | Human-post prerequisite; never the loop. |

---

## 2. DISCLOSURE & NATIVE-LABEL MATRIX (attach to every step)

Mandated by `RISKS.md` a.2 (FTC 16 CFR Part 255 double disclosure) + EU AI Act Art. 50
(applies 2026-08-02) + platform-native labels (guardrail #9). Every AUTO post carries **all
three**: (a) AI-generated disclosure, (b) material-connection disclosure, (c) the platform's
native AI/bot label. The gate **refuses to enqueue** a post missing any one.

**Canonical disclosure line (reuse verbatim):**
```
AI-generated, human-reviewed. Posted by localharness's own automated account
(the project's own AI agent). #AI
```

| Code | Platform | Disclosure to attach | Native AI/bot label toggle |
|---|---|---|---|
| **[X]** | X / Twitter | Short canonical line `AI-generated, human-reviewed. Posted by localharness's own automated account. #AI` as the **immediate FIRST reply** (single posts) / **FINAL reply** (threads). Link in a reply, never the post body. | **Account-level "Automated account"** setting ON (set once, linked to the human operator). |
| **[DEV]** | dev.to | The article's **footer disclosure paragraph** (verbatim, already in each `DEVTO-ARTICLE*.md`). | **No native toggle → the footer text IS the label.** Ship `published:false`; a human flips live. |
| **[GH]** | GitHub | **No native label** (first-party repo metadata, not a "post"). When drafting release notes, the footer line `Release notes drafted by localharness's automated agent and human-reviewed before publishing.` | n/a |
| **[LI]** | LinkedIn | `Disclosure: AI-generated and human-reviewed; posted by localharness's own automated account.` appended to the post body. | **No strong native label → the appended footer IS the label.** |
| **[HUM]** | HN / Reddit | **Human authorship — no automated-bot label.** A human posts in their own voice, from their own aged account, and **personally owns every line.** No automation, no upvote solicitation, no identical cross-posting. | n/a (genuine human post). |
| **[WEB]** | First-party property (hero landing / receipts branch) | The carrying social post keeps its mandated [X] disclosure; the film's "the thing that told you agents can own themselves — is one" / "the company wrote the receipts" punchline is **additive, never a substitute** (`CAMPAIGN-01` §5.2 / `CAMPAIGN-02` §3). | n/a |
| **[IG/TT]** *(tail only)* | IG / TikTok | Caption disclosure (`VISUAL-BRIEFS.md` shared block). | **Native AI-content label toggled at upload** (TikTok "AI-generated content"; IG "AI info") — hard-required if any synthetic voice/avatar/B-roll. |

---

## 3. GO / NO-GO GATE (all 8 must be YES, or No-Go)

Check immediately before firing. Any **NO** halts launch until fixed (GitHub #1 may proceed
alone once its own line passes).

| ☐ | Criterion | Source |
|---|---|---|
| ☐ | **1. Autonomy posture set.** Social ceiling is **draft → review queue**; the loop holds **NO** live post credentials; a human flips each item. Dial defaults OFF; the operator has deliberately greenlit. | `RISKS.md` guardrail #1/#2, a.4 |
| ☐ | **2. Core creds present + validated** — `GITHUB_TOKEN` confirmed PR-only; X **Automated-account label ON** + payment method; `DEVTO_API_KEY` valid. | §1.1 |
| ☐ | **3. Handles + properties live** — `@localharness` reserved everywhere; **`whoami.localharness.xyz` live and rendering**. | §1.3 |
| ☐ | **4. Disclosure + native label on every queued asset** — the 3-part disclosure attached at generation time; gate refuses any post missing one. | §2; guardrail #9 |
| ☐ | **5. Accuracy lock signed off** — crate 0.58.0; OpenAI/Mock/Gemma SDK-only; no diamond/chain address pinned; x402 testnet-only; self-funding OPEN, no earnings claim; `$LH` = usage credit. | top lock; guardrail #10 |
| ☐ | **6. Human approver on call** for every HUMAN-GATED + brand-risk item; HN/Reddit aged accounts ready, 9:1 healthy; **hero film + landing approved**. | §1.3; a.4 |
| ☐ | **7. Secret hygiene** — `.env.marketing` gitignored + secret-scanner active; **no token in tree**; no `git add -A`. | guardrail #5; b.3 |
| ☐ | **8. Abort owner + kill switch known** — the autonomy dial (single kill switch, defaults OFF) and the circuit-breaker are armed; the operator can flip OFF instantly. | guardrail #15; §4 |

---

## 4. ABORT / ROLLBACK — triggers + who halts

**Master halt:** the **autonomy dial** is the single kill switch (defaults OFF; any operator
flips it instantly; the circuit-breaker trips it automatically). Because the loop holds no
live post creds, "halt" in practice = **stop approving + flip the dial OFF**. The marketing
role-agent **cannot self-clear** any of these — a human re-greenlights.

| Trigger | Signal to watch | Who/what halts + action |
|---|---|---|
| **Shadowban (per-channel)** | HN submission instantly `[dead]` / absent from `/newest`; Reddit post `[removed]` / invisible when logged out; X reach collapses or an automation-label enforcement warning lands; engagement falls to ~zero. | **Halt that channel immediately** (do NOT post more — volume deepens an automated ban). Human investigates; for **HN/Reddit DOMAIN** shadowban (`localharness.xyz` = one-way door) **STOP all link-drops**, escalate to owner, contact `hn@ycombinator.com` / modmail. |
| **Bad take / off-brand reply going viral** | An approved or human reply is escalating badly; a thread turning hostile. | The loop **never** replies to humans unsupervised (APPROVE-only). The human **stops engaging — does not double down**; corrects plainly only if warranted; deletes only if defamatory/false (deletion can amplify — operator judgment). |
| **Inaccuracy shipped** | A post implies OpenAI/Gemma is a live in-app model; pins a diamond/chain address; makes a `$LH` earnings/price claim; asserts mainnet x402; leaks an internal/secret. | **IMMEDIATE correction protocol:** (a) flip dial OFF / freeze the queue; (b) **correct or delete** the offending post + publish a plain correction; (c) **re-run the accuracy lock against ALL queued assets** before resuming; (d) file a finding. A human re-greenlights. |
| **Budget / spend spike or N failed ticks** | Per-run or per-day ceiling hit (posts, `$LH`, LLM token/$); consecutive tick failures. | **Circuit-breaker trips the dial OFF automatically** + notifies the human; the tick **aborts + files `budget-exceeded`** — it does not retry smaller. |

**Reversibility note:** the only irreversible-ish artifacts in scope are a **live social post**
(deletable/correctable — prefer a correction over a silent delete unless unsafe) and the
**Decision-7 receipts publish** (curate BEFORE pushing — caches persist even if re-privatized).
This runbook's loop **never** releases the crate, deploys, or cuts a facet — there is no
production rollback to perform (`RISKS.md` b.1/b.2).

---

## 5. THE FIRE SEQUENCE — one ordered timeline (D-offset from Launch Monday = D1)

> Three waves on one spine. **Wave 1** (`whoami` + `CALENDAR` Weeks 1–3) = D1–D21.
> **Week 4** = breather + the Decision-7 receipts staging. **Wave 2** (`git log`) = a
> fresh 2-week window, D29–D42. Each row cites the exact asset by `READY-QUEUE` id or
> campaign beat (copy + per-asset accuracy guard live there). **Lane:** AUTO = loop
> enqueues, human flips live · HUMAN = a human posts in their own voice · WEB = a human
> publishes a property. **Disc/Label** code → §2. Brand-risk rows carry an **approval ☐**.

### Wave 1 — `whoami` (D0–D21)

| D | Day | Channel | Asset (id / beat) | Lane | Approve ☐ | Disc/Label |
|---|---|---|---|---|---|---|
| **D0** | Sun (pre) | X | `whoami` teaser — `CAMPAIGN-01` §5.1 (NEW) | AUTO | — | [X] |
| **D1** | W1 Mon | GitHub | **#1** repo desc + topics + `whoami` social-preview still | AUTO | — | [GH] |
| **D1** | W1 Mon | Web | **`whoami.localharness.xyz` hero landing goes live** (NEW) | WEB | ☐ owner publishes | [WEB] |
| **D1** | W1 Mon | X | **#3** launch announce **+ HERO FILM** (NEW), **pinned**; landing link in reply 1 | AUTO | ☐ film + post approved (hero) | [X] |
| **D1** | W1 Mon | dev.to | **#2** article #1 — self-sovereign agent in Rust | AUTO | ☐ human flips `published` | [DEV] |
| **D2** | W1 Tue | Hacker News | **H1** Show HN — "an AI agent that owns itself" | HUMAN | ☐ human posts, own voice | [HUM] |
| **D3** | W1 Wed | X | **#4** technical hook — SDK loop + backend seam | AUTO | — | [X] |
| **D4** | W1 Thu | Reddit r/rust | **H2** model-agnostic agent SDK (native+wasm seam) | HUMAN | ☐ aged acct, 9:1 healthy | [HUM] |
| **D5** | W1 Fri | X | **#6** build-in-public thread — "the autonomous business" | AUTO | — | [X] |
| **D6–7** | Sat–Sun | — | Monitor + reply to HN/Reddit as a human; pull launch analytics | — | — | — |
| **D7** | W1 Sun | X | pool **B2-6** (slither demo, live URL) | AUTO | — | [X] |
| **D8** | W2 Mon | LinkedIn | **#5** launch post (embed hero film) | AUTO¹ | ☐ if API unapproved, human posts | [LI] |
| **D9** | W2 Tue | X | **#8** founder-story thread — "why self-sovereign, not rented" ² | AUTO | — | [X] |
| **D10** | W2 Wed | dev.to | **#2b** article #2 — x402 + EIP-6551 token-bound accounts | AUTO | ☐ human flips `published` | [DEV] |
| **D11** | W2 Thu | Reddit r/ethdev | **H3** on-chain architecture (distinct body from H2) | HUMAN | ☐ aged acct, 9:1, ≠ H2 body | [HUM] |
| **D12** | W2 Fri | X | pool **B2-8** (pay-per-call mechanism) | AUTO | — | [X] |
| **D13–14** | Sat–Sun | — | KPI refresh (claims, downloads, stars) | — | — | — |
| **D14** | W2 Sun | X | `whoami` recap quote-post — `CAMPAIGN-01` §5.3 (NEW) | AUTO | ☐ verify `{N}` on-chain; if 0/unflattering → demo-clip recap | [X] |
| **D15** | W3 Mon | LinkedIn | *(no post — #7 reserved for Wave 2)* ³ | — | — | — |
| **D16** | W3 Tue | X | pool **B2-2** (identity hook) | AUTO | — | [X] |
| **D17** | W3 Wed | dev.to | **#2c** article #3 — rustlite cartridge compiler | AUTO | ☐ human flips `published` | [DEV] |
| **D18** | W3 Thu | X | pool **B2-3** (fair comparison vs frameworks) ⁴ | AUTO | — | [X] |
| **D19** | W3 Fri | — | Weekly GEO citation panel (`GROWTH` Exp. 1); review drivers | — | — | — |
| **D20** | W3 Sat | X | pool **B2-1** (remaining filler) ⁴ | AUTO | — | [X] |
| **D21** | W3 Sun | — | Queue Week-4; continue pool drips | — | — | — |

### Week 4 — breather + receipts staging (D22–D28)

| D | Channel | Action | Lane | Approve ☐ |
|---|---|---|---|---|
| **D22–27** | — | Monitor; KPI; drip remaining pool **B2-7 / B2-9 / B2-10** into gaps (≥1 day apart, rotate angle, similarity-gated) | AUTO | — |
| **D22–27** | GitHub/Web | **DECISION 7 (human):** curate + push the `autonomous-business` branch public / open the draft PR — no secrets, no diamond address, testnet-vs-mainnet nuance intact. Stages the `git log` hero. | WEB | ☐ **owner Decision-7 gate** |
| **D28** | X | `git log` teaser — `CAMPAIGN-02` §5.1 (NEW) | AUTO | — | [X] |

### Wave 2 — `git log` (D29–D42)

| D | Day | Channel | Asset (id / beat) | Lane | Approve ☐ | Disc/Label |
|---|---|---|---|---|---|---|
| **D29** | gl-W1 Mon | GitHub/Web | **RECEIPTS GO LIVE** — branch + pinned `LEDGER.md` public; preview = `0 writes·0 $LH·0 posts·0 merges` still | WEB | ☐ owner Decision-7 flip | [WEB] |
| **D29** | gl-W1 Mon | X | **anchor thread** — `CAMPAIGN-02` §5.2 (NEW), **pinned**; receipts link in reply 1 | AUTO | ☐ approved (hero) | [X] |
| **D30** | gl-W1 Tue | Hacker News | **Show HN** — `CAMPAIGN-02` §5.5 (NEW, distinct from H1) | HUMAN | ☐ human posts, own voice | [HUM] |
| **D31** | gl-W1 Wed | X | pool **B2-4** (autonomous-business "what it is") | AUTO | — | [X] |
| **D32** | gl-W1 Thu | dev.to | **anchor long-form** — `CAMPAIGN-02` §5.6 (NEW canonical) | AUTO | ☐ human flips `published` | [DEV] |
| **D33** | gl-W1 Fri | X | pool **B2-5** (honest scope / burn) — **highest denylist care**: ESTIMATE-labeled, cost framing, no earnings | AUTO | ☐ number = cost/open-problem only | [X] |
| **D34–35** | Sat–Sun | — | Monitor + reply to HN/dev.to as a human; analytics | — | — | — |
| **D36** | gl-W2 Mon | LinkedIn | **#7** autonomous-business vision ³ | AUTO¹ | ☐ if API unapproved, human posts | [LI] |
| **D37** | gl-W2 Tue | — | Community replies / monitor (no post — #8 fired D9) ² | — | — | — |
| **D38** | gl-W2 Wed | Reddit r/rust *or* r/AI_Agents | **loop-architecture** post — `CAMPAIGN-02` §5.5 (NEW, distinct body) | HUMAN | ☐ aged acct, 9:1, ≠ H2/H3 | [HUM] |
| **D39** | gl-W2 Thu | X | **guardrails single** — `CAMPAIGN-02` §5.3 (NEW): "0 merges, 0 $LH, 0 posts" | AUTO | — | [X] |
| **D40** | gl-W2 Fri | — | KPI refresh (claims, stars, clones, downloads) | — | — | — |
| **D41** | gl-W2 Sat | — | Plan sustain tail | — | — | — |
| **D42** | gl-W2 Sun | X | `git log` recap quote-post — `CAMPAIGN-02` §5.4 (NEW) | AUTO | ☐ verify `{N}` on-chain; if 0/unflattering → receipts-scroll clip | [X] |

¹ **LinkedIn AUTO only after Community Management API approval** lands; until then a human
posts the same copy manually (the one beat that may lag — `GROWTH.md` §2.7).

² **Reconciliation — asset #8 fires ONCE.** Both `CALENDAR` (W2 Tue) and `CAMPAIGN-02` (its
W2 Tue) claim the **#8** founder-story thread. It is fired in **Wave 1 (D9)** where `whoami`
frames it; Wave 2's slot (D37) becomes a monitor day. Never re-post #8 (near-duplicate =
CIB/domain-ban risk).

³ **Reconciliation — asset #7 fires ONCE.** Both `CALENDAR` (W3 Mon) and `CAMPAIGN-02` (gl-W2
Mon) claim the **#7** LinkedIn vision long-form. It is fired in **Wave 2 (D36)** — its natural
autonomous-business home — so the Wave-1 W3 LinkedIn slot (D15) is empty. This keeps LinkedIn
long-forms ≥1 week apart (#5 D8 → #7 D36) and never two Page long-forms back-to-back.

⁴ **Reconciliation — pool posts (B2-x) each fire ONCE.** `CALENDAR` + both campaigns
independently over-allocated the shared `X-POSTS-BATCH-2` pool. This runbook is the
authoritative single allocation: **B2-6**→D7, **B2-8**→D12, **B2-2**→D16, **B2-3**→D18,
**B2-1**→D20, **B2-7/B2-9/B2-10**→Week-4 filler, **B2-4**→D31, **B2-5**→D33 (the two
autonomous-business posts reserved for the `git log` wave per `CAMPAIGN-02` §8). No B2-x is
posted twice anywhere in the program.

---

## 6. CROSS-CUTTING RULES AT EVERY FIRE (always on)

From `RISKS.md` (a.1 / guardrails #9–#14) + `GROWTH.md` §3 — these bind on every row above:

- **Disclosure + native label at generation time** on every AUTO post (§2); gate refuses
  any post missing one.
- **The loop only enqueues; a human flips each AUTO item live.** No live post creds in the
  autonomous path.
- **HN + Reddit stay human-posted** from aged/karma accounts, distinct bodies, value-first,
  9:1-gated; **no automation, no upvote solicitation, no identical cross-posting.** A
  voting-ring → **domain** shadowban is a one-way door for `localharness.xyz`.
- **No cross-agent engagement** — the loop's other role-agents never like/RT/upvote/comment
  on these posts (textbook astroturf → automated domain ban); the engagement tools are not
  granted to the fleet.
- **Topic denylist** — no `$LH` earnings/yield/price/investment framing; no politics; no
  naming/showing a named competitor (keep contrasts category-level).
- **Spacing / no bursts** — ≤1 substantive X post/day; X threads never same-day/adjacent;
  dev.to long-forms ≥1 week apart; links in a reply, not the post body; similarity-check
  gates the next post.
- **Accuracy guards ride each asset** — every cited `READY-QUEUE` id keeps its own
  re-verified guard; NEW campaign beats are pinned to the accuracy lock at the top of this
  file.
