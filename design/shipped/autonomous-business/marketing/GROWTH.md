# GROWTH.md — perpetual marketing agent operating model

> Owner-facing design doc. How a long-running localharness marketing agent operates
> across channels: what it does autonomously, what a human must approve, and how it
> stays inside each platform's Terms of Service. Companion: `CREDENTIALS.template.md`
> (the exact secrets the owner must provide).
>
> **Hard product fact that shapes everything below:** localharness is a niche,
> developer-first, Rust + on-chain-agent product. Its onboarding doc (`web/skill.md`)
> is explicitly written *for autonomous agents* as much as humans. So the highest-ROI
> "channel" is not a social network at all — it is being correctly described and cited
> by LLMs. We prioritize accordingly.

---

## 0. The one ground truth: signup is NOT automatable; posting via official APIs is

Every major platform gates *account creation* behind CAPTCHA, phone/SMS verification,
and behavioral anti-bot checks. There is no ToS-safe way to script signups, and doing
it through CAPTCHA-solvers / SMS farms is a bannable ToS violation on all of them.

**Therefore the division of labour is fixed:**

| Step | Who | Why |
|---|---|---|
| Create each account (phone-verified) | **Human, once** | Signup automation is blocked + against ToS everywhere |
| Create the developer app / get API access | **Human, once** | Requires identity, billing, sometimes manual app-review |
| Hand the agent scoped **API tokens** (never raw passwords where avoidable) | **Human, once** | Tokens are revocable + least-privilege |
| Draft, schedule, and post **own content** to **own accounts** via the official API | **Agent, perpetually** | This is exactly what the APIs are for and is ToS-allowed |
| Reply to / engage real humans, post to communities with strict self-promo rules | **Human-approved queue** | Brand voice + community-norm risk |
| Fake engagement, vote rings, follow/unfollow churn, mass-DM, sockpuppets, identical cross-posting | **NEVER** | Bannable on every platform; also just bad |

The agent is therefore a **content engine + scheduler + analyst**, not an account farm.
It runs forever; humans seed accounts once and approve the small set of high-risk posts.

---

## 1. Channel prioritization

Ranked by fit (audience match × ToS-safe automatability × leverage), not by raw reach.

### Tier 1 — core, highest leverage

| Channel | Why it's Tier 1 | Automatable? |
|---|---|---|
| **AI-discoverability (llms.txt / GEO)** | The product literally targets autonomous agents; LLMs are how the next dev *finds* an "agent SDK". We already ship `llms.txt` + `skill.md`. Being the answer to "Rust agent SDK" / "self-sovereign agent network" in ChatGPT/Claude/Perplexity is the single highest-leverage growth surface and is 100% owned content. | **Fully** (own repo + site) |
| **GitHub** | The SDK's home and where the actual buyer (a Rust/agent dev) already lives. Stars/releases/topics/social-preview are first-party and the API is clean. No self-promo ToS friction — it's your repo. | **Fully** (PAT/App) |
| **Hacker News** | Best single launch surface for a technical, self-sovereign, crypto-adjacent dev tool. "Show HN" fits exactly (a real, try-it-now product). BUT: **automation is forbidden** and vote-rings are detected/shadowbanned. High value, **human-only**. | **No — human posts** |

### Tier 2 — strong fit, mostly automatable

| Channel | Why | Automatable? |
|---|---|---|
| **X / Twitter** | Build-in-public + AI/agents + crypto dev crowd is concentrated here. Posting your *own* content via the API is explicitly allowed; new pay-per-use pricing makes posting cheap. | **Posting: yes.** Following/liking/DM automation: **prohibited** |
| **dev.to (Forem)** | Technical long-form, friendly to canonical cross-posts, clean `api-key` POST endpoint, and the content doubles as feedstock for AI-citation (Tier 1). | **Fully** (api-key) |
| **Reddit** | r/rust, r/LocalLLaMA, r/ethdev, r/AI_Agents, r/SaaS are perfect audiences — but the self-promo bar is brutal (9:1 rule, karma/age gates, identical-content = spam flag). API allows reading own-post metrics + submitting, but posts should be **human-approved**, value-first. | **Read: yes. Submit: human-approved** |

### Tier 3 — opportunistic / credibility / experimental

| Channel | Why lower | Automatable? |
|---|---|---|
| **LinkedIn** | B2B credibility + reaching funders/partners, but not where Rust/agent devs hang out, and API access needs an approval form (Community Management API, `w_member_social`). | Posting own content: yes **after approval** |
| **Instagram** | Off-core audience; only useful for short agent-demo clips. API needs FB Business + Page + IG Pro + app review (2–4 wks) for `instagram_business_content_publish`. High setup cost, low fit. | Yes, **after app review** |
| **TikTok** | Same as IG — demo clips only. Content Posting API works, but **unaudited clients can only post SELF_ONLY (private)** until a manual audit. High friction. | Yes, but **private until audited** |

> **Recommendation:** Stand up Tier 1 + Tier 2 first. Treat IG/TikTok/LinkedIn as a
> *repurposing* layer (one demo video, one B2B post) only once Tier 1/2 is humming —
> their API onboarding cost (app review / audit / approval form) is real and shouldn't
> block launch.

---

## 2. Per-channel operating playbook

Legend: **[AUTO]** = agent acts without per-item human sign-off · **[APPROVE]** =
agent drafts, queues, and a human clicks publish · **[NEVER]** = forbidden.

### 2.1 AI-discoverability (llms.txt / GEO) — Tier 1

- **Setup (human, ~0):** none beyond repo access. The site already serves
  `localharness.xyz/llms.txt`.
- **What the agent does:**
  - **[AUTO]** Keep `web/llms.txt` + `web/skill.md` factually dense, current, and
    extraction-friendly: direct answers up front, real numbers (version, pricing,
    chain id), named entities, no marketing fluff. (Respect the repo's GEN-block SOP —
    facts come from `src/docs_manifest.rs`; edit the manifest, not the GEN blocks.)
  - **[AUTO]** Publish canonical "what is localharness / how do I X" explainers as
    durable, linkable content (dev.to, GitHub README, a docs page) — this is what LLMs
    actually retrieve and cite. Structured headings, code blocks, real stats, one clear
    definition near the top.
  - **[AUTO]** Run a weekly **citation-monitoring panel** (see Experiment 1): ask a
    fixed prompt set to ChatGPT/Claude/Perplexity via API, log whether localharness is
    mentioned, described correctly, and linked.
- **Cadence:** docs reviewed weekly; citation panel weekly; new canonical explainer
  monthly or per release.
- **Honest caveat:** there is **no confirmed evidence** that an `llms.txt` file alone
  lifts citation rates today. The durable win is the *content quality + structure* it
  forces, plus genuine third-party mentions (Reddit/HN/GitHub) that LLMs are known to
  weight. Treat llms.txt as table-stakes hygiene, not a silver bullet.

### 2.2 GitHub — Tier 1

- **Setup (human, once):** confirm repo, reserve the org/handle, add a scoped PAT or
  GitHub App (see CREDENTIALS).
- **What the agent does:**
  - **[AUTO]** Maintain repo **topics**, description, social-preview image, and a
    crisp README hero (within the README-minimal house rule).
  - **[AUTO]** Draft + cut **release notes** on tagged releases (it can read CHANGELOG).
  - **[AUTO]** Pull weekly traffic/stars/clones via the Insights/Traffic API + crates.io
    download stats for the KPI dashboard.
  - **[APPROVE]** Open/triage Discussions or pinned "good first issue" threads aimed at
    contributor growth.
- **Cadence:** metrics weekly; release notes per release; topics/README on change.
- **ToS:** it's your own repo — no self-promo restriction. Standard GitHub Acceptable
  Use applies (no spam, no fake stars — buying/trading stars is a ban-grade violation).

### 2.3 Hacker News — Tier 1, **human-only**

- **Setup (human):** an account with real history; HN throttles new/low-karma accounts.
- **What the agent does:**
  - **[AUTO]** **Draft** the Show HN title + first comment (factual, zero marketing
    language, links to a real try-it-now path), and recommend a posting window.
  - **[NEVER]** Submit, upvote, ask for upvotes, run multiple accounts, or coordinate
    voting. HN has an active **voting-ring detector** that shadowbans domains. The agent
    must not touch HN programmatically at all.
  - **[APPROVE]** Human submits manually and replies in their own voice; the agent can
    suggest reply drafts for the human to edit.
- **Cadence:** one Show HN per major milestone (real, demoable change only). Re-posts of
  a flop are allowed sparingly per HN norms — human-judged.
- **ToS:** "primary use must be curiosity, not promotion"; product must be genuinely
  try-able. This is a once-per-milestone, high-craft, human event.

### 2.4 X / Twitter — Tier 2

- **Setup (human, once):** create + phone-verify `@localharness`; create a dev app in the
  X developer portal; add a payment method (pay-per-use is now default for new devs —
  ~$0.015/post, $0.20 if the post contains a link, billed per call); generate OAuth
  user-context tokens. Label the account's automation in bio per X's bot-labeling rule.
- **What the agent does:**
  - **[AUTO]** Post **own** content to the **own** account via the API: build-in-public
    updates, release threads, short demos, "an agent just claimed its own subdomain"
    moments. Schedule via the agent's own scheduler (no third-party tool needed).
  - **[AUTO]** Pull own-post analytics (impressions, engagement, link clicks) via the API
    for KPIs (cheap "owned reads").
  - **[APPROVE]** Replies to real people / quote-posts in conversations (brand voice).
  - **[NEVER]** Automated following/unfollowing, bulk/aggressive actions, auto-DMs or
    cold-DM outreach, or posting the same/near-same tweet from multiple accounts (X
    treats that as coordinated inauthentic behavior even if you own all the accounts).
- **Cadence:** 1 substantive post/day cap to start (quality > volume), threads per
  release. Avoid link-heavy posts where possible — links cost 13× per call AND get less
  reach; put the link in a reply.
- **Cost note:** posting your own content is cheap; the practical ceiling is X's rate
  limits, not price, at our volume.

### 2.5 dev.to — Tier 2

- **Setup (human, once):** create account, generate an API key in settings.
- **What the agent does:**
  - **[AUTO]** Publish technical long-form (`POST /api/articles`) — deep dives on the
    Rust agent loop, on-chain identity, host::compose, etc. Cross-post canonical content
    with a `canonical_url` back to the site so SEO/AI-citation credit consolidates.
  - **[AUTO]** Pull views/reactions/comments via the Forem API for KPIs.
  - Optionally publish as **draft first** for a human glance, then flip to published — or
    auto-publish own first-party content (low risk).
- **Cadence:** 1 substantive article/1–2 weeks. Don't dump; quality + tags
  (`#rust #ai #webdev #crypto`) drive distribution.
- **ToS:** first-party content on your own account — clean. Just don't spam duplicate
  articles or keyword-stuff.

### 2.6 Reddit — Tier 2, **submit = human-approved**

- **Setup (human, once):** create account; **age it + earn karma manually** (many subs
  gate on 30-day age / 100+ karma); create a "script"-type OAuth app.
- **What the agent does:**
  - **[AUTO]** Read own-post metrics + monitor relevant subs for genuinely on-topic
    threads to *answer helpfully* (draft replies for human review).
  - **[APPROVE]** Any submission or self-referential comment. The agent drafts a
    value-first post; a human posts it from the aged account.
  - **[NEVER]** Cross-posting identical content across many subs, drive-by link drops,
    upvote solicitation, or running multiple promo accounts. Reddit's spam detection +
    mods nuke this fast.
- **The 9:1 / 90:10 rule is the operating constraint:** for every 1 promotional touch,
  the account needs ~9 genuinely useful, non-promotional contributions — measured across
  *all* activity, not per-sub. The agent tracks this ratio and **gates** the next promo
  post until the ratio is healthy.
- **Cadence:** at most 1 self-promo touch per ~2 weeks per relevant sub, only when the
  9:1 budget allows, only where the sub's rules permit, always value-first.

### 2.7 LinkedIn — Tier 3

- **Setup (human, once):** profile/Page; dev app; request **Community Management API**
  access (approval form, Development → Standard tier); obtain `w_member_social` (personal)
  / `w_organization_social` (Page) tokens. Tokens last ~60 days → must be refreshed.
- **What the agent does:** **[AUTO]** post own milestones/B2B framing; **[AUTO]** pull
  post analytics (Social Metadata / org analytics). **[APPROVE]** comments on others.
- **Cadence:** 1–2 posts/week, repurposed from X/dev.to. Lower priority than Tier 1/2.

### 2.8 Instagram — Tier 3, experimental

- **Setup (human, once):** FB Business + Page + IG **Professional** account + Meta app +
  **business verification** + **app review** for `instagram_business_content_publish`
  (2–4 weeks). 60-day long-lived token, refreshable. 50 posts/24h cap.
- **What the agent does:** **[AUTO]** publish short agent-demo reels/images via the
  Content Publishing API (create container → publish). **[AUTO]** pull insights.
- **Cadence:** only once a demo-video pipeline exists; repurpose, don't originate.

### 2.9 TikTok — Tier 3, experimental

- **Setup (human, once):** account + developer app + Content Posting API access; submit
  the **audit** to lift the SELF_ONLY restriction (unaudited clients can only post
  private/self-only videos). ~15 posts/day/creator cap.
- **What the agent does:** **[AUTO]** direct-post demo clips via the API (will be private
  until audited — fine for a soft start). **[APPROVE]** going public post-audit.
- **Cadence:** experimental; repurpose demo footage.

---

## 3. Auto vs human-approved — master matrix

| Action | X | Reddit | HN | dev.to | LinkedIn | GitHub | IG | TikTok | llms.txt/GEO |
|---|---|---|---|---|---|---|---|---|---|
| Post own content via official API | AUTO | APPROVE | NEVER¹ | AUTO | AUTO | AUTO | AUTO | AUTO² | AUTO |
| Pull own analytics via API | AUTO | AUTO | n/a | AUTO | AUTO | AUTO | AUTO | AUTO | AUTO |
| Reply to / engage real humans | APPROVE | APPROVE | APPROVE | APPROVE | APPROVE | APPROVE | APPROVE | APPROVE | n/a |
| Following / DMs / votes | NEVER | NEVER | NEVER | n/a | NEVER | n/a | NEVER | NEVER | n/a |
| Account signup | HUMAN | HUMAN | HUMAN | HUMAN | HUMAN | HUMAN | HUMAN | HUMAN | n/a |

¹ HN is fully manual — the agent only drafts. ² TikTok auto-posts are SELF_ONLY until audited.

**Universal NEVER list (all channels):** fake engagement, bought followers/stars,
upvote/vote-ring solicitation, sockpuppet networks, identical multi-account posting,
mass/cold DMs, CAPTCHA/SMS-farm signups, scraping behind auth, impersonation.

---

## 4. Three growth experiments

### Experiment 1 — AI-citation lift (GEO)
- **Hypothesis:** Publishing structured, factually-dense canonical answers (improved
  `llms.txt` + a dev.to "what is localharness" explainer + a tight README definition)
  measurably increases how often ChatGPT/Claude/Perplexity *correctly describe and link*
  localharness when asked agent-SDK questions.
- **Method:** Fix a 15-prompt panel ("best Rust agent SDK", "self-sovereign AI agent
  platform", "how do agents pay each other on-chain", …). Baseline now via each model's
  API. Ship the content. Re-run weekly.
- **Measure:** (a) mention rate, (b) factual-correctness score (0–2, human/LLM-judge),
  (c) link-present rate; plus (d) AI-referral sessions in web analytics (referrer =
  `chatgpt.com`, `perplexity.ai`, `claude.ai`). Success = mention+correctness+link all up
  vs baseline over 4 weeks.

### Experiment 2 — framing A/B (agent-first vs SDK-first)
- **Hypothesis:** "An agent can claim its own on-chain identity from a shell" (agent-first
  framing) converts better than "Rust agent SDK" (tool-first framing) for our audience.
- **Method:** On X + dev.to (channels we can post to freely), publish matched pairs of
  each framing with distinct UTM-tagged links over 3–4 weeks. (HN is a single manual shot
  — not A/B-able; pick the winning framing for the HN launch.)
- **Measure:** impressions → link CTR → conversions, where conversion =
  `localharness create` identity claims (queryable on-chain via the Diamond) + crate
  downloads (crates.io API), attributed by UTM. Success = one framing wins CTR *and*
  conversion with a clear margin.

### Experiment 3 — cross-post flywheel
- **Hypothesis:** A canonical dev.to deep-dive → distilled X thread → one value-first
  Reddit contribution (9:1-compliant) within 48h drives more qualified identity claims
  than any single channel alone.
- **Method:** Run the 3-channel sequence for 4 milestones; compare to 4 single-channel
  posts of similar substance. Each link UTM-tagged by channel.
- **Measure:** identity claims + crate downloads per sequence vs per single post; also
  which channel UTM seeds the most downstream AI citations (ties back to Exp. 1). Success
  = sequence beats single-channel on conversions per unit of effort.

---

## 5. KPIs per channel + how to pull them

| Channel | Primary KPIs | How the agent pulls them |
|---|---|---|
| **AI-discoverability** | citation/mention rate, correctness, AI-referral sessions | Scripted prompt panel via Anthropic/OpenAI/Perplexity APIs; web-analytics referrer filter; server logs for `llms.txt` fetches |
| **GitHub** | stars, forks, unique clones/visitors, release downloads, **crates.io downloads** | GitHub REST `traffic/views`,`traffic/clones`,`stargazers`; crates.io `/api/v1/crates/localharness` (no auth) |
| **HN** | points, comments, peak rank, referral traffic | Manual + Algolia HN API (`hn.algolia.com`) read-only; analytics referrer |
| **X** | impressions, engagement rate, profile + link clicks | X API v2 own-post metrics (`tweet.fields=public_metrics,non_public_metrics`) — cheap owned reads |
| **dev.to** | views, reactions, comments, followers | Forem API `GET /api/articles/me`, `/api/articles/{id}` |
| **Reddit** | upvotes, comments, post views, **account karma/age health** | Reddit API on own submissions (`/api/info`, `/user/<me>/submitted`); track 9:1 ratio internally |
| **LinkedIn** | impressions, reactions, comments | Social Metadata API / organization analytics (`view=li-lms-…`) |
| **Instagram** | reach, views, saves, profile visits | Graph API `/{ig-media-id}/insights`, `/{ig-user-id}/insights` |
| **TikTok** | views, likes, shares (post-audit) | TikTok Display/Content API video metrics |
| **North-star (cross-channel)** | **identity claims**, **crate downloads**, site sessions | On-chain registration count via the Diamond (read-only RPC); crates.io API; analytics |

**North-star conversion = a new on-chain identity claim** (`localharness create`) — it is
the truest signal because it's an actual activated user, it's free to query on-chain, and
it's hard to fake. Crate downloads are the secondary growth signal. Everything else is a
leading indicator feeding those two.

---

## 6. Operating loop (how "perpetual" actually runs)

The agent is itself a localharness agent, so it uses the platform's own scheduler
(`schedule_task` / off-chain cron) — no third-party social scheduler needed:

1. **Daily:** check the human-approval queue; post any approved items; post the day's
   AUTO content where the cadence cap allows; pull fresh analytics.
2. **Weekly:** run the citation panel; refresh the KPI dashboard; propose next week's
   content + experiment status; flag any channel where cadence/9:1/rate-limit budgets are
   tight.
3. **Per release:** draft GitHub release notes + an X thread + a dev.to deep-dive; queue a
   Show HN draft for human launch if the milestone is big enough.
4. **Always:** never act outside the matrix in §3; when unsure, queue for **[APPROVE]**.

---

## Sources

- X API pricing / pay-per-use: [postproxy](https://postproxy.dev/blog/x-api-pricing-2026/), [sorsa](https://api.sorsa.io/blog/twitter-api-pricing-2026)
- X automation rules: [help.x.com/x-automation](https://help.x.com/en/rules-and-policies/x-automation), [X Developer Policy](https://docs.x.com/developer-terms/policy)
- Reddit self-promo / 9:1 / bot policy: [Responsible Builder Policy](https://support.reddithelp.com/hc/en-us/articles/42728983564564-Responsible-Builder-Policy), [teract 9:1](https://www.teract.ai/resources/reddit-subreddit-marketing-2026), [Postiz API limits](https://postiz.com/blog/reddit-api-limits-rules-and-posting-restrictions-explained)
- HN guidelines / voting rings: [HN Guidelines](https://news.ycombinator.com/newsguidelines.html), [hacker-news-undocumented](https://github.com/minimaxir/hacker-news-undocumented)
- dev.to / Forem API: [developers.forem.com/api/v1](https://developers.forem.com/api/v1)
- LinkedIn Community Management API: [Posts API](https://learn.microsoft.com/en-us/linkedin/marketing/community-management/shares/posts-api), [Increasing Access](https://learn.microsoft.com/en-us/linkedin/marketing/increasing-access)
- Instagram Content Publishing: [Meta docs](https://developers.facebook.com/docs/instagram-platform/content-publishing/)
- TikTok Content Posting API / audit: [TikTok Direct Post](https://developers.tiktok.com/doc/content-posting-api-reference-direct-post), [get-started](https://developers.tiktok.com/doc/content-posting-api-get-started)
- llms.txt / GEO caveats: [GEO guide](https://almcorp.com/blog/how-to-rank-on-chatgpt-perplexity-ai-search-engines-complete-guide-generative-engine-optimization/), [llms.txt wiki](https://agilebrandguide.com/wiki/generative-ai/llms-txt/)
