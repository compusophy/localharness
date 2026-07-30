# CALENDAR.md — 3-week launch content schedule

> The day×platform plan for firing the `READY-QUEUE.md` asset set. **References every
> asset by its READY-QUEUE id** (the queue is the source of truth for exact copy +
> per-asset accuracy guards; this file only schedules them). Spacing obeys the rules
> in `READY-QUEUE.md` ("Posting order & spacing") and `RISKS.md` (a.1 / guardrails
> #9–#12): no two long-forms back-to-back, no two X threads in a day, HN + Reddit are
> human-gated, every AUTO post carries its disclosure + native bot/AI label.
>
> **Assumption:** launch is the **Monday of Week 1**. Re-verified 2026-06-30 against
> source (crate 0.58.0; OpenAI/Mock/Gemma SDK-only, live selector = Gemini Flash +
> Claude Opus; no diamond/chain address pinned; x402 settlement testnet-only;
> self-funding OPEN — no earnings claims).

## The asset set this calendar covers

| Bucket | READY-QUEUE ids | Lane |
|---|---|---|
| GitHub (repo metadata) | **#1** | AUTO (first-party property) |
| dev.to long-form ×3 | **#2** (self-sovereign agent in Rust), **#2b** (x402 + EIP-6551), **#2c** (rustlite compiler — NEW) | AUTO (own api-key) |
| X / Twitter ×4 — 2 single posts + 2 threads | **#3** launch (post), **#4** technical hook (post), **#6** build-in-public (thread), **#8** founder-story (thread) | AUTO (own account, Automated-account label) |
| LinkedIn ×2 long-form | **#5** launch, **#7** autonomous-business vision | AUTO **after** Community Management API approval (else human posts copy manually) |
| Reddit ×2 + Show HN ×1 | **H2** r/rust, **H3** r/ethdev, **H1** Show HN | **HUMAN-GATED** — a human posts in their own voice; the loop holds no creds |
| X / Twitter evergreen pool | **#9** (`X-POSTS-BATCH-2.md` B2-1…B2-10) | AUTO **filler** — drip one at a time between the dated assets; not date-fixed |

> Note on counts: there are **4** X assets (two single posts #3/#4, two reply-chain
> threads #6/#8), not three — this calendar schedules all four. "AUTO" = the loop
> *enqueues* and a human flips each item live (RISKS a.4 / guardrail #2); the loop
> never auto-posts and never holds live post credentials.

---

## Week 1 — launch week (Tier-1 surfaces first)

| Day | Platform | Asset (READY-QUEUE id) | Lane | Notes |
|-----|----------|------------------------|------|-------|
| **W1 Mon** | GitHub | **#1** repo description + topics | AUTO | Instant, zero-risk first-party metadata. Fire first. |
| **W1 Mon** | dev.to | **#2** article #1 — self-sovereign agent in Rust | AUTO | Flip `published: true` after human review. Anchor long-form; the canonical the X/Reddit posts distill. |
| **W1 Mon** | X | **#3** launch announce (single post) | AUTO | Pin to profile. Link (if any) goes in a reply, not the post. |
| **W1 Tue** | Hacker News | **H1** Show HN | **HUMAN** | US-morning weekday (PT). Human submits manually, replies in own voice. **No automation, no upvote solicitation** (domain-ban risk). |
| **W1 Wed** | X | **#4** technical hook — SDK loop + backend seam (single post) | AUTO | ≥1 day after #3; never the same hour. Distinct copy. |
| **W1 Thu** | Reddit r/rust | **H2** model-agnostic agent SDK (native + wasm seam) | **HUMAN** | Aged/karma account; only when the 9:1 self-promo budget is healthy. Value-first; be present in comments. |
| **W1 Fri** | X | **#6** build-in-public thread — "the autonomous business" | AUTO | Spaced from #3/#4 (another day later). Reply-chain on one account; disclosure as final reply. |
| **W1 Sat–Sun** | — | (no post) | — | Monitor + reply to HN/Reddit threads as a human. Pull launch-day analytics. |

## Week 2 — deepen (second long-forms + second thread)

| Day | Platform | Asset (READY-QUEUE id) | Lane | Notes |
|-----|----------|------------------------|------|-------|
| **W2 Mon** | LinkedIn | **#5** launch post | AUTO¹ | Long-form. ¹Fire via API once Community Management approval lands; until then a human posts the same copy manually. |
| **W2 Tue** | X | **#8** founder-story thread — "why self-sovereign, not rented" | AUTO | ≥1 day after #6; **never two X threads in one day** (#6 was 4 days prior). Distinct first-person angle. |
| **W2 Wed** | dev.to | **#2b** article #2 — x402 + EIP-6551 token-bound accounts | AUTO | **≥1 week after #2** (W1 Mon → W2 Wed = 9 days). Distinct angle (payments/identity). Not adjacent to LinkedIn #5 (2 days prior). |
| **W2 Thu** | Reddit r/ethdev | **H3** on-chain self-sovereign agents (distinct body from H2) | **HUMAN** | Aged account; 9:1 budget; **distinct body from r/rust** — never the same text in two subs. ~1 week after H2. |
| **W2 Fri** | X | **#9** pool — **B2-3** (fair comparison vs hosted-API-key frameworks) | AUTO | Filler. +3 days from the #8 thread (W2 Tue) — not adjacent. Distinct copy + disclosure reply + Automated-account label. |
| **W2 Sat–Sun** | — | (no post) | — | Community replies; KPI refresh (stars, crate downloads, identity claims); plan Week 3. |

## Week 3 — sustain (vision long-form + compiler deep-dive)

| Day | Platform | Asset (READY-QUEUE id) | Lane | Notes |
|-----|----------|------------------------|------|-------|
| **W3 Mon** | LinkedIn | **#7** autonomous-business vision | AUTO¹ | Long-form. **≥several days from #5** (1 week apart — never two Page long-forms back-to-back). ¹Same approval gate as #5. |
| **W3 Tue** | X | **#9** pool — **B2-2** ("every agent is a subdomain" / identity hook) | AUTO | Filler. No X thread/single in W3; +4 from the W2 Fri pool post. Disclosure reply + Automated-account label. |
| **W3 Wed** | dev.to | **#2c** article #3 — rustlite cartridge compiler (NEW) | AUTO | **≥1 week after #2b** (W2 Wed → W3 Wed = 7 days). Distinct angle (compiler/runtime). Not adjacent to LinkedIn #7 (2 days prior). |
| **W3 Thu** | X | **#9** pool — **B2-6** (slither demo CTA, live URL) | AUTO | Filler. +2 days from the W3 Tue pool post; rotated angle (identity → demo). Disclosure reply + Automated-account label. |
| **W3 Fri** | — | (no post) | — | Run the weekly AI-citation panel (GROWTH Exp. 1); review which assets drove identity claims/downloads. |
| **W3 Sat** | X | **#9** pool — **B2-4** (autonomous-business build story) | AUTO | Filler. +2 days from W3 Thu; rotated angle (demo → build-story). ~12 days after the #8 thread — far enough to avoid any similarity. Disclosure reply + Automated-account label. |
| **W3 Sun** | — | (no post) | — | Queue Week-4 cadence; continue dripping the remaining `#9` pool posts (B2-1/5/7/8/9/10) as filler. |

---

## Spacing-compliance check (why this layout is safe)

- **dev.to long-forms ≥1 week apart, never back-to-back:** #2 (W1 Mon) → #2b (W2 Wed,
  +9d) → #2c (W3 Wed, +7d). Three distinct angles (agent-in-Rust / payments+identity /
  compiler+runtime) — no near-duplicates, no substantially-similar trap.
- **LinkedIn long-forms spaced:** #5 (W2 Mon) → #7 (W3 Mon, +7d) — never two Page
  long-forms back-to-back.
- **No two long-forms back-to-back across platforms:** the closest pair is LinkedIn
  Mon ↔ dev.to Wed (2 days apart) in W2 and W3 — never consecutive days.
- **X threads never collide:** #6 (W1 Fri) and #8 (W2 Tue) are 4 days apart; never the
  same day, never adjacent — two threads in a day reads as automation churn.
- **X posts ≥1 day apart, no bursts:** #3 (W1 Mon), #4 (W1 Wed), #6 (W1 Fri), #8
  (W2 Tue) — every pair ≥2 days; ≤1 substantive X post/day; link in a reply, not the post.
- **X evergreen-pool filler (#9) never collides with a dated X asset:** the four scheduled
  here — B2-3 (W2 Fri), B2-2 (W3 Tue), B2-6 (W3 Thu), B2-4 (W3 Sat) — are each ≥2 days from
  any X thread/single and ≥2 days from each other, with rotated angles (comparison → identity
  → demo → build-story) and no near-duplicate of a thread (B2-4 lands ~12 days after #8). The
  remaining six pool posts drip into later gaps under the same rules.
- **HN: one honest shot,** US-morning weekday, human-only (H1). Never solicit upvotes.
- **Reddit: two posts, two subs, distinct bodies,** ~1 week apart (H2 W1 Thu, H3 W2 Thu),
  human-posted from an aged account only when the 9:1 budget allows. No identical
  cross-posting.

## Cross-cutting rules at fire time (RISKS.md)

1. **Disclosure + native label on every AUTO post** at generation time (guardrail #9):
   the canonical line in READY-QUEUE + the platform's bot/AI label (X Automated-account
   setting; dev.to/LinkedIn footer text).
2. **No cross-agent engagement** (guardrail #12): the loop's other agents never
   like/RT/upvote/comment on these posts — that's the voting-ring/astroturf pattern that
   triggers domain bans.
3. **Topic denylist** (guardrail #10): no `$LH` financial/earnings/investment claims, no
   politics, no naming/attacking third parties.
4. **A human approves each item before it goes live** (RISKS a.4 / b.1); the loop only
   enqueues. HN/Reddit are posted by a human in their own voice, never the loop.
5. **Per-day post ceiling + similarity check** enforced (guardrail #11): no near-duplicate
   text on one account or across accounts.
