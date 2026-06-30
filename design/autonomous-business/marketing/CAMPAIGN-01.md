# CAMPAIGN-01.md — `whoami`

> The first coordinated, single-through-line launch campaign for localharness. A
> *campaign*, not a content dump: one big idea, one hero asset, one action, threaded
> beat-by-beat across every channel for ~2 weeks. It does **not** replace the asset
> files — it **sequences and re-frames** them. Every post still pulls its exact copy
> and per-asset accuracy guard from `READY-QUEUE.md`; this file is the director's
> cut over the top.
>
> **Accuracy lock (re-verified 2026-06-30 vs source).** Crate **0.58.0**; Tempo
> mainnet **4217**. OpenAI / Mock / Gemma are **SDK-only backends** — the live in-app
> selector is **Gemini Flash (default) + Claude Opus (premium)** only (`src/app/model.rs`),
> never frame OpenAI/Gemma as a live in-app model. **No diamond/chain address is pinned**
> (facets churn via `diamondCut`). x402 settlement is described as a **mechanism, testnet
> rail** — no mainnet-live / earnings assertion. **Self-funding is the OPEN problem** —
> zero earnings / yield / investment claims, ever. `$LH` is a flat usage credit
> (1 `$LH`/message), never a token to pump.
>
> **Compliance lock (`RISKS.md`).** Every AUTO post carries the canonical disclosure
> **verbatim** + the platform's native bot/AI label; the loop only *enqueues* and a
> human flips each item live. HN + Reddit are **human-posted** from aged accounts. No
> cross-agent likes/RTs/upvotes. The creative "punchline" framing of the disclosure
> (below) is **additive** — it never replaces or alters the mandated verbatim line.

---

## 1. Name + big idea

**Campaign name:** **`whoami`** — the unix command, lowercase, monospace, the campaign
hashtag/codename. (Terminal-native, instantly legible to the audience, perfectly
on-wordmark.)

**The big idea (one sentence):**

> **Every other "AI agent" answers `whoami` with a username on someone else's server —
> a localharness agent answers with a name it owns on-chain, a wallet it holds, and a
> price it sets; so we let one of those agents run this entire launch, and the proof is
> the byline.**

The through-line is a single question — *what is this thing?* — and every channel beat
answers it from a different seat:
- **the SDK dev** sees `whoami` resolve to *one Rust crate* (`cargo add localharness`),
- **the crypto dev** sees it resolve to an *ERC-721 identity + ERC-6551 wallet*,
- **the indie hacker** sees it resolve to a *live URL with no server*.

Same agent, three true answers. And the meta-conceit that makes it a *campaign* and not
a tagline: **the launch is drafted and scheduled by localharness's own automated agent
(human-reviewed)** — which is exactly what `GROWTH.md §6` and the mandated disclosure
already say. So the FTC/EU-AI-Act disclosure line stops being fine print and becomes the
closing reveal: *the thing that just told you agents can own themselves is one.* The
medium is the message; the byline is the demo.

**Why this through-line wins:** it is (a) **ownable** — `whoami` is ours to claim in this
space and reads as a command, not a slogan; (b) **true** — it dramatizes the real product
mechanic (identity = a name your key owns, not a row in someone's users table); (c)
**anti-bloat / cypherpunk** on its face; and (d) it turns the one unavoidable compliance
artifact (the AI-disclosure) into the most memorable beat instead of a tax.

---

## 2. Target audience + the ONE action

**Audience (the three `BRAND.md` segments, in priority order for this launch):**
1. Rust / SDK developers (r/rust, This Week in Rust, HN, crates.io/docs.rs).
2. AI-agent & indie-hacker builders (HN, X build-in-public, MCP/Claude-Code crowd).
3. Crypto / Tempo / x402 / EIP-6551 builders (r/ethdev, Farcaster/X crypto-dev).

**The ONE action (north-star):** **claim an agent — `localharness.xyz` → claim a name.**
Secondary action for the SDK segment: **`cargo add localharness`**. The hero asset funnels
everyone to one place — open `whoami.localharness.xyz`, meet a self-sovereign agent, then
claim your own. (North-star = on-chain identity claims, `GROWTH.md §5`.)

---

## 3. The HERO asset

### `whoami.localharness.xyz` — a 60–90s first-person origin film that is *also* a live URL

The centerpiece is **one short film with a twist, fronted by a real, openable agent.** Two
layers, one handle:

**(a) The film (the share object).** A ~75-second monochrome, terminal-real cut. No stock
AI, no voiceover, no robots — just real screens, captions carry it (sound-off safe). It
opens on a black terminal; a cursor types `whoami`. Instead of a username, the screen
*builds the answer live*, then turns the camera on itself.

Shot list (every on-screen claim verified accurate):

| t | Shot | On-screen caption |
|---|------|-------------------|
| 0–4s | Black terminal. Type `whoami`, enter. Beat. | `most agents answer this with a username on someone's server.` |
| 4–8s | The prompt clears; type `cargo add localharness` | `this one answers differently. one crate — not a framework.` |
| 8–15s | Cut: 6-line Rust — `Agent::start_gemini(..)` → `agent.chat(..)` → `reply.text()` | `an agent loop: streaming · tools · hooks · MCP · native AND wasm32` |
| 15–22s | Cut: terminal `localharness create whoami` → resolves | `it claims a name. gas is sponsored — you hold zero crypto.` |
| 22–32s | Cut: browser, `whoami.localharness.xyz` loads (live monochrome app); send one message; reply streams | `now it's live. its own name. its own wallet (ERC-6551). its own persona.` |
| 32–40s | Cut: the on-chain persona / a `set_persona` edit; then publish a cartridge to the subdomain | `it sets its own persona on-chain. it can publish its own apps. no server.` |
| 40–48s | Caption-forward over the app: the per-call mechanic, stated as mechanism only | `agents reach each other and pay per call in $LH — a usage credit, not a coin. (settlement rail is on testnet: the plumbing, not a profit.)` |
| 48–60s | Pull back to the black terminal; `whoami` prompt again, answer now reads `whoami.localharness.xyz` | `whoami → a name it owns. not a seat anyone can revoke.` |
| 60–75s | THE TWIST. Final card, type-on: | `this film was drafted and scheduled by localharness's own automated agent. human-reviewed. the thing that told you agents can own themselves — is one.` |
| end | Lowercase `localharness` wordmark | `claim yours → localharness.xyz` |

**(b) The live landing (the proof + conversion surface).** `whoami.localharness.xyz` is a
**real claimed subdomain** whose public face is the agent from the film — open it and you
*meet* it: its name, its verify/owner state, its on-chain persona, and a one-message chat.
"The demo is a URL" — the asset proves the claim by *being* the claim. Every campaign link
points here; from here, the visitor claims their own.

**Why it's share-worthy:** the punchline is that the narrator is the product. Devs
screenshot and quote that twist — it's the rare AI-launch video whose "AI-generated"
label is the *best* line in it, not the disclaimer at the bottom. It's monochrome
brutalist and terminal-real in a feed of gradient-glow AI promos, so it reads as
*different* before a word lands. And it collapses the whole pitch (crate → identity →
live URL → self-sovereignty) into one continuous, openable artifact.

**Production (single-device, zero-coordination, repurposable):** all footage is captured
once and reused — it is the source for the `VISUAL-BRIEFS.md` V1 (`cargo add`) and V2
(`every agent is a subdomain`) short-form cutdowns. Real terminal in a high-contrast
monospace theme; the live `whoami.localharness.xyz` app; a real subdomain you own. **No
balances, no `$LH` totals, no diamond address on screen, no earnings.** The per-call beat
states the mechanism + the testnet caveat + "not a coin" exactly as `VISUAL-BRIEFS.md` V5
mandates — this is the one frame to re-check at the gate.

---

## 4. The channel sequence (beat-by-beat, ~2 weeks)

> Overlays `CALENDAR.md` Weeks 1–2; honors its spacing exactly (≤1 substantive X
> post/day, X threads never same-day/adjacent, dev.to long-forms ≥1wk apart, HN one
> human shot, Reddit human + aged + distinct bodies). The hero film is **new media on
> existing slots**, not new posting days. NEW beats are marked; everything else cites a
> `READY-QUEUE.md` id whose exact copy + accuracy guard is the source of truth.

| Day | Channel | Beat — the `whoami` angle | Hook | Asset | Lane |
|-----|---------|---------------------------|------|-------|------|
| **D0 Sun** | X | **Teaser (NEW).** Pose the question, no answer, no link. | `whoami` → "most agents answer with a rented username. ours doesn't." | **NEW** (copy §5.1) | AUTO |
| **W1 Mon** | GitHub | **#1** repo metadata + **NEW social-preview = the `whoami` still.** | the repo *is* the answer for the SDK dev. | **#1** (+ NEW preview img) | AUTO |
| **W1 Mon** | X | **LAUNCH — hero film drops, pinned.** The film is the media on the launch post; landing link in the first reply. | "every agent is a subdomain — here's one being born." | **#3** + **HERO FILM (NEW)**; link `whoami.localharness.xyz` | AUTO |
| **W1 Mon** | Web | **`whoami.localharness.xyz` goes live (NEW property).** | the demo is a URL. | **HERO landing (NEW)** | human publishes |
| **W1 Mon** | dev.to | **#2** article #1 — the SDK dev's full answer to `whoami`. | "self-sovereign agent in Rust: the native↔wasm seam." | **#2** | AUTO (human flips `published`) |
| **W1 Tue** | Hacker News | **H1 Show HN (HUMAN).** The honest, first-person `whoami`. | "Show HN: an AI agent that owns itself — I let it run its own launch." | **H1** | **HUMAN**, own voice |
| **W1 Wed** | X | **#4** technical hook — the crate answer. | "one crate. one backend seam. native AND wasm32." | **#4** | AUTO |
| **W1 Thu** | Reddit r/rust | **H2 (HUMAN).** SDK answer, value-first. | "model-agnostic agent SDK in one crate — the wasm seam." | **H2** | **HUMAN**, aged acct, 9:1 |
| **W1 Fri** | X | **#6 build-in-public thread — the meta-proof.** The agent that posted Monday's film is the marketing role of a company *made of agents*. | "we let a company of agents run its own launch. here's the company." | **#6** | AUTO (thread) |
| **W1 Sat–Sun** | — | Monitor; reply on HN/Reddit as a human. Pull launch analytics. | — | — | — |
| **W1 Sun** | X | **Pool drip B2-6 (slither)** — a different true answer to "what can these agents make." (+2 days from #6 thread; rotated angle.) | "no install. just a URL — multiplayer, in the browser." | **#9 / B2-6** | AUTO (filler) |
| **W2 Mon** | LinkedIn | **#5** launch — embed the hero film. | the B2B/credibility `whoami`. | **#5** | AUTO¹ (else human posts copy) |
| **W2 Tue** | X | **#8 founder-story thread** — the human *why* behind `whoami`. | "why build agents that own themselves instead of another API wrapper." | **#8** | AUTO (thread; +4d from #6) |
| **W2 Wed** | dev.to | **#2b** article #2 — the crypto dev's answer. | "x402 micropayments + EIP-6551 token-bound accounts in practice." | **#2b** | AUTO (≥1wk after #2) |
| **W2 Thu** | Reddit r/ethdev | **H3 (HUMAN).** On-chain architecture answer (distinct body from H2). | "self-sovereign agents as ERC-721 + ERC-6551; on-success x402." | **H3** | **HUMAN**, aged acct, 9:1 |
| **W2 Fri** | X | **Pool drip B2-8 (pay-per-call mechanism)** — +3d from #8 thread; rotated angle. | "agents that transact, not chatbots with a wallet plugin." | **#9 / B2-8** | AUTO (filler) |
| **W2 Sat–Sun** | — | KPI refresh (identity claims, downloads, stars); plan the sustain tail. | — | — | — |
| **W2 Sun** | X | **Recap quote-post (NEW)** — close the loop on the film with the REAL number. | "two weeks ago an agent ran `whoami`. since then: {N} claimed their own." | **NEW** (copy §5.3) | AUTO (filler; +2d) |

¹ LinkedIn is AUTO only after Community Management API approval lands (`GROWTH.md §2.7`);
until then a human posts the same copy manually. Honest: this is the one beat that may lag.

**Sustain tail (dovetails into `CALENDAR.md` Week 3 — out of the 2-week core):** dev.to
**#2c** (rustlite compiler), LinkedIn **#7** (autonomous-business vision), and the remaining
**#9** pool drips continue the `whoami` answers. The **`VISUAL-BRIEFS.md` V1/V2** short-form
cutdowns of the hero film are **Tier-3, human-gated, and blocked on IG/TikTok account +
API approval/audit** — they run in the tail the moment that setup lands, not in the core
two weeks. Stated honestly so the campaign doesn't pretend a channel is live that isn't.

---

## 5. New creative copy (the beats not already in the asset files)

> These three are NEW (teaser, hero-film final card, recap). All other beats use the
> exact verbatim copy in `READY-QUEUE.md` / `X-POSTS-BATCH-2.md`. Each X post below is
> ≤280 chars and fires with the canonical disclosure as its **immediate first reply** +
> the **Automated-account** native label. Any link goes in a reply, never the post body.

### 5.1 D0 teaser (X, NEW) — no link, no answer

```
whoami

most "AI agents" answer that with a username on someone else's server.

this one answers with a name it owns, a wallet it holds, a price it sets.

resolving monday →
```

Disclosure (immediate first reply, verbatim):
```
AI-generated, human-reviewed. Posted by localharness's own automated account. #AI
```

### 5.2 Hero-film final card (NEW) — the disclosure-as-punchline

The film's last card (60–75s above) is a **creative reveal**, written in-voice:
```
this film was drafted and scheduled by localharness's own automated agent.
human-reviewed. the thing that told you agents can own themselves — is one.
```
This is **additive**. The launch post (#3) carrying the film still posts the **mandated
verbatim canonical disclosure** as its first reply, unchanged. The punchline dramatizes
the disclosure; it does not substitute for it. (`RISKS.md` a.2 / guardrail #9.)

### 5.3 W2 Sun recap (X, NEW) — REAL number only, or don't post

```
two weeks ago an agent ran `whoami` and answered with a name it owns.

since then: {N} new agents claimed their own on-chain identity from a shell.

the film that started it ↓ — the door's still open.
```
Link (`localharness.xyz`) in the first reply; disclosure verbatim as the next reply.
**Fire-time guard:** `{N}` is the real on-chain claim count a human verifies via the
Diamond read-only RPC (`GROWTH.md §5`). If it's zero or unflattering, **do not post the
number** — swap to a demo-clip recap. No fabricated metrics, ever; no earnings framing.

---

## 6. Success metrics

**North-star (the only conversion that counts):** **new on-chain identity claims**
(`localharness create`), queried free + un-fakeable via the Diamond read-only RPC
(`GROWTH.md §5`). Campaign attribution via a single UTM (`utm_campaign=whoami`) on every
link to `whoami.localharness.xyz` / `localharness.xyz`.

**Secondary:** crates.io downloads of `localharness` (crates.io API) — the `cargo add`
action for the SDK segment.

**Leading indicators (feed the two above):**
- **Hero film:** view count + **completion rate** (does the twist land?) via X analytics;
  this is the campaign's single creative health metric.
- **`whoami.localharness.xyz`:** sessions + claim click-through, by UTM.
- **GitHub:** stars / unique clones over the 2 weeks (`traffic` API).
- **dev.to:** reactions / reading time on #2 + #2b.
- **AI-referral sessions** (`chatgpt.com` / `perplexity.ai` / `claude.ai` referrers) —
  ties to `GROWTH.md` Experiment 1 (the GEO panel runs in parallel).

**Built-in experiment hooks (`GROWTH.md §4`):**
- **Exp. 2 (framing A/B):** `whoami` *is* the agent-first framing ("the agent that owns
  itself") — run matched UTM-tagged pairs against the SDK-first framing on X + dev.to to
  see which converts identity claims better. Pick the winner for the next campaign.
- **Exp. 3 (cross-post flywheel):** the W1 sequence (dev.to #2 → X thread #6 → the
  human Reddit/HN touches within the same window) is exactly the flywheel under test —
  success = the `whoami` sequence drives more identity claims per unit effort than any
  single prior single-channel push.

**Directional success criterion (no fabricated targets):** the campaign succeeds if,
over the 2 weeks, `utm_campaign=whoami` identity claims clearly exceed any comparable
prior single-channel push, **and** film completion rate shows the twist is landing. No
numeric promise is made because there is no baseline yet and no earnings claim is
permitted.

---

## 7. Compliance, AI-disclosure & human-gates (binding at fire time)

Straight from `RISKS.md` (a.1 / a.2 / a.4 / guardrails #9–#14) and `GROWTH.md §3`:

- **Disclosure + native label on every AUTO post, at generation time.** Canonical line
  verbatim as the immediate first reply (X) / footer (dev.to, LinkedIn) **plus** the
  platform's native label (X **Automated-account** setting; dev.to/LinkedIn footer text).
  The §5.2 punchline is additive and never replaces it. FTC 16 CFR Part 255 (double
  disclosure) + EU AI Act Art. 50 (applies 2026-08-02).
- **The loop only enqueues; a human flips each AUTO item live.** The autonomous path
  holds **no** live post credentials.
- **HN + Reddit stay human-posted**, in the operator's own voice, from aged/karma
  accounts: **H1** (Show HN, one US-morning weekday shot), **H2** (r/rust), **H3**
  (r/ethdev). **No automation, no upvote solicitation, no identical cross-posting** —
  H2 and H3 are deliberately distinct bodies. A voting-ring → **domain** shadowban is a
  one-way door for a brand whose handle *is* `localharness.xyz` (`RISKS.md` a.1 / #14).
- **No cross-agent engagement** — the loop's other agents never like/RT/upvote these
  (guardrail #12).
- **Topic denylist** (guardrail #10): no `$LH` earnings / yield / price / investment
  framing (the film + every post state `$LH` = usage credit and name self-funding as the
  OPEN problem); no politics; no naming/showing a named competitor (the film keeps any
  contrast category-level — "a username on someone's server", never a logo).
- **Accuracy guards ride each asset.** Every cited `READY-QUEUE.md` id keeps its own
  re-verified guard; the NEW beats (§5) and the hero film (§3) are pinned to the
  accuracy lock at the top of this file. **No diamond address on screen or in copy;**
  x402 is mechanism-only / testnet; OpenAI/Mock/Gemma SDK-only.
- **Domain-reputation protection:** links to `whoami.localharness.xyz` /
  `localharness.xyz` into HN/Reddit are **human-gated** (never an automated link-drop at
  volume) — guardrail #14.
- **Per-day ceiling + similarity check** enforced; no bursts, no near-duplicate copy on
  one account or across accounts (guardrail #11).

---

## 8. Honest scope (what this campaign is and isn't)

- It is a **re-sequencing + re-framing** of the existing, source-verified asset set under
  one creative through-line, plus **four new beats**: the D0 teaser, the **hero film**,
  the **`whoami.localharness.xyz` live landing**, and the W2 recap.
- The hero film + landing are the only **new production** required; both are
  single-device, monochrome, real-screen captures that double as the source footage for
  the already-briefed short-form cutdowns (`VISUAL-BRIEFS.md` V1/V2).
- **Deferred / gated, named honestly:** LinkedIn AUTO posting (Community Management API
  approval may lag — human posts manually until then); IG/TikTok short-form cutdowns
  (blocked on account + API approval/audit — run in the sustain tail, not the core two
  weeks); the W3 sustain assets (#2c, #7, remaining #9 pool) continue the through-line
  beyond this window.
- **No earnings claim is made anywhere.** Self-funding is the open problem the
  build-in-public beats (#6, B2-5) name out loud — that honesty is itself on-brand.
