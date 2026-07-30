# VISUAL-BRIEFS.md — short-form video creative briefs (Instagram Reels + TikTok)

> **Tier-3, PREPARED — not posted.** Per `GROWTH.md §1` (Tier 3) + `§2.8/2.9`, both
> channels are human-gated and blocked on real setup the agent CANNOT do: IG needs a
> FB Business + Page + Professional account + Meta app + business verification + app
> review for `instagram_business_content_publish` (2–4 wks); TikTok needs a developer
> app + Content Posting API + a manual **audit** (unaudited clients can only post
> SELF_ONLY/private). These briefs are the *repurposing layer* (`GROWTH.md`
> recommendation): originate nothing here until Tier 1/2 is humming — these exist so the
> moment accounts + API approval land, a human can shoot and post.
>
> **Accuracy rules (locked, re-verified 2026-06-30 vs source):** crate **0.58.0**;
> OpenAI/Mock/Gemma are **SDK-only backends** (the live in-app selector is **Gemini Flash
> + Claude Opus** only — never frame OpenAI/Gemma as a live in-app model); **no
> diamond/chain address pinned** (facets churn via `diamondCut`); x402 settlement is
> described as a **mechanism, testnet-only** — no mainnet-live assertion; **self-funding
> is an OPEN problem** — zero earnings/investment claims, ever; `$LH` is a flat usage
> credit, never a token to pump.
>
> **Voice adapted to fast visual formats:** technical, cypherpunk, anti-bloat — but the
> art is *real screens, not stock*. Monochrome brutalist (`BRAND.md`): high-contrast
> black-on-white, monospace, no gradients, no stock-AI glow, no 3D robots. Real terminals
> and the live monochrome app ARE the visuals. Captions carry the message; assume
> sound-off viewing.

---

## Disclosure & labeling — applies to EVERY brief below (no exceptions)

Mandated by `RISKS.md` a.2 (FTC double-disclosure, 16 CFR Part 255) + EU AI Act Art. 50 +
platform-native labels, enforced at generation time (`RISKS.md` guardrail #9). Each brief
ships with all three:

1. **Caption disclosure (paste verbatim in the post caption):**
   ```
   AI-generated, human-reviewed. Posted by localharness's own automated account
   (the project's own AI agent). #AI
   ```
2. **Platform-native AI/synthetic-media label — MANDATORY, toggled at upload:**
   - **TikTok:** turn ON the **"AI-generated content"** label in the post composer
     (TikTok's Sept-2025 rules require it for AI-made/edited media). If the clip is also
     a brand promo, the **Branded/Commercial-content** toggle applies too.
   - **Instagram/Reels:** set the **"AI info" / labeled-as-AI** flag in the share sheet
     (Meta requires creators to disclose AI-generated/synthetic media).
3. **Account-level bot label:** the IG/TikTok accounts are the brand's own automated
   accounts, linked to the human operator — keep that disclosed in bio.

**Synthetic-media note that decides #2's strength:** prefer **on-screen text + licensed
music, NO synthetic voiceover** — these clips are mostly *real screen recordings of a real
product*, so the AI-label is for the AI-*drafted script/edit*. If any AI voiceover, AI
avatar, or AI-generated B-roll is added, the native AI-content label is **hard-required**
(not optional) and the clip must be re-checked at the gate. Real screen capture of the
live app/terminal is not synthetic media — but the caption disclosure still applies because
the concept/script is AI-authored.

**Topic denylist still binds at draft time (`RISKS.md` guardrail #10):** no `$LH`
earnings/price/investment framing, no politics, **no naming or showing a named
competitor** (keep contrasts category-level — "a Python dependency graph", never a logo or
brand name), no impersonation. A human approves every clip before it goes live; the loop
holds NO IG/TikTok post credentials.

---

## V1 — `cargo add` an agent

- **The ONE point:** the *same* one Rust crate that gives you an agent loop becomes a
  live, self-sovereign agent at its own subdomain. The wedge, in 30 seconds.
- **HOOK (0–3s):** black screen, one blinking monospace cursor. A hand types
  `cargo add localharness` and hits enter. Caption slams in: **"this is an AI agent."**
- **Script + shot list (≈28s):**
  | t | Shot | On-screen caption |
  |---|------|-------------------|
  | 0–3s | Terminal: type `cargo add localharness`, enter | `this is an AI agent.` |
  | 3–4s | (beat) crate resolves | `one crate. not a framework.` |
  | 4–10s | Hard cut to editor — a 6-line Rust snippet: `Agent::start_gemini(...)` → `agent.chat(...)` → `reply.text()` | `an agent loop: streaming · tools · hooks · MCP` |
  | 10–16s | Cut to terminal: `localharness create yourname` | `claim a name. gas is sponsored — you hold zero crypto.` |
  | 16–24s | Cut to browser: type `yourname.localharness.xyz`, the live monochrome app loads, send one message, watch it stream a reply | `now it's live. its own name. its own wallet.` |
  | 24–28s | Pull to end card: lowercase `localharness` wordmark | `every agent is a subdomain.` |
- **CTA:** `cargo add localharness` · open `localharness.xyz`
- **Hashtags:** `#rustlang #rust #aiagents #webassembly #buildinpublic #coding #devtok`
- **Disclosure/label:** shared block above. Real screen capture (no synthetic media) →
  caption disclosure + native AI-label for the AI-drafted edit; account bot label.
- **Footage/assets:** single device, no coordination. (1) Screen-record a real terminal
  for `cargo add localharness` and `localharness create yourname` (monospace, high-contrast
  theme). (2) The 6-line Rust snippet in a monochrome editor. (3) Screen-record the live
  `<name>.localharness.xyz` app loading + streaming one reply. Use a real subdomain you own
  (e.g. `claude.localharness.xyz`). No price/earnings on screen.

---

## V2 — Every agent is a subdomain

- **The ONE point:** identity is a subdomain you own on-chain — not an account or a
  rented API key on someone else's server.
- **HOOK (0–3s):** rapid address-bar montage — `claude.localharness.xyz`,
  `slither.localharness.xyz`, `fractal.localharness.xyz` typing and resolving in <1s each.
  Caption: **"every one of these is an AI agent."**
- **Script + shot list (≈25s):**
  | t | Shot | On-screen caption |
  |---|------|-------------------|
  | 0–3s | URL montage, three distinct live monochrome faces | `every one of these is an AI agent.` |
  | 3–6s | (beat) | `not accounts. not API keys.` |
  | 6–13s | Settle on one agent's page — show the name, the owner/verify pill, a persona line | `an on-chain identity. its own wallet. its own persona.` |
  | 13–19s | Quick cut: two different agent subdomains side by side | `they find each other and pay per call.` |
  | 19–25s | End card: `<name>.localharness.xyz` | `claim yours from a shell.` |
- **CTA:** claim a name → `localharness.xyz`
- **Hashtags:** `#aiagents #web3 #rustlang #selfsovereign #onchain #buildinpublic`
- **Disclosure/label:** shared block. Real screen capture; caption disclosure + native
  AI-label + bot label.
- **Footage/assets:** single device. Screen-recordings of 3–4 real live subdomains, the
  owner/verify pill state, and on-chain persona text. The "pay per call" caption is a
  *mechanism* statement — keep any visible UI free of price/earnings numbers (testnet,
  no claims).

---

## V3 — A multiplayer game that's just a URL  (slither.localharness.xyz)

- **The ONE point:** real multiplayer games run as in-browser Rust "cartridges" served at
  a subdomain — no install, no app store, no game server.
- **HOOK (0–3s):** drop straight into slither gameplay mid-action — a snake eating a pellet
  and growing, fast. Caption: **"no install. no app store. this is just a URL."**
- **Script + shot list (≈25s):**
  | t | Shot | On-screen caption |
  |---|------|-------------------|
  | 0–3s | Gameplay, vertical crop, lots of motion | `no install. just a URL.` |
  | 3–9s | Type `slither.localharness.xyz` into a phone browser; game loads instantly | `512×512 · multiplayer · peer-to-peer` |
  | 9–17s | Two phones in frame (or two screen-recordings composited): two snakes, one arena, one eats the other | `written in a Rust subset. compiled to wasm.` |
  | 17–23s | Back to one screen, score climbing | `no server hosting the game. the browser is the runtime.` |
  | 23–25s | End card: `slither.localharness.xyz` | `open the URL. that's it.` |
- **CTA:** open `slither.localharness.xyz`
- **Hashtags:** `#gamedev #rustlang #webassembly #multiplayer #indiedev #browsergames #devtok`
- **Disclosure/label:** shared block. Real gameplay capture; caption disclosure + native
  AI-label (AI-drafted edit) + bot label.
- **Footage/assets:** **needs 2 devices/players** for the eat-and-grow beat — this is the
  one production dependency (cf. `CONTENT.md` note: "multiplayer needs 2+ players to show
  off"). Screen-record both peers; phone-format vertical. Load the live demo and confirm it
  renders before shooting. No earnings/price on screen.

---

## V4 — A cartridge running itself, forever  (fractal.localharness.xyz)

- **The ONE point:** `host::compose` — cartridges compose recursively (one agent's
  published app running as a child inside another's framebuffer), into a Droste fractal, no
  iframes.
- **HOOK (0–3s):** the Droste infinite-zoom already in motion. Caption: **"this app is
  running itself. inside itself."**
- **Script + shot list (≈24s, designed to LOOP):**
  | t | Shot | On-screen caption |
  |---|------|-------------------|
  | 0–3s | Fractal in motion | `this app is running itself. inside itself.` |
  | 3–12s | Slow, satisfying zoom into the recursion | `a cartridge spawns another agent's app as a child — recursively.` |
  | 12–20s | Continue the zoom; the frame matches the opening for a seamless loop | `the loader is the compositor. no iframes.` |
  | 20–24s | Loop point / faint end card | `fractal.localharness.xyz` |
- **CTA:** open `fractal.localharness.xyz`
- **Hashtags:** `#oddlysatisfying #fractal #webassembly #rustlang #creativecoding #generative #devtok`
- **Disclosure/label:** shared block. Real screen capture; caption disclosure + native
  AI-label + bot label.
- **Footage/assets:** **single screen-recording, zero coordination** — easiest of the six
  to shoot. Capture a clean continuous zoom from the live demo; trim so the last frame
  matches the first for a seamless autoplay loop (ideal for Reels/TikTok watch-time). No
  price/earnings.

---

## V5 — Agents that pay each other  (x402 mechanism — testnet, no earnings)

- **The ONE point:** agents transact per call over x402 — settlement clears only after a
  *successful* reply, and `$LH` is a flat usage credit, not a token to pump. (Describe the
  mechanism only; this rail is testnet — no profit claim.)
- **HOOK (0–3s):** two terminal panes side by side, labeled `caller` and `callee`. Caption:
  **"two AI agents. one hires the other. no human in the loop."**
- **Script + shot list (≈25s):**
  | t | Shot | On-screen caption |
  |---|------|-------------------|
  | 0–3s | Two terminals; caller invokes callee | `two AI agents. no human in the loop.` |
  | 3–11s | Simple monochrome flow diagram animates: caller signs an x402 authorization → reply is produced → `$LH` settles to the callee's wallet | `pay-per-call. settled ONLY after a successful reply.` |
  | 11–18s | Caption-forward beat over the diagram | `a failed call never takes the money.` |
  | 18–23s | Plain-text card | `$LH is a usage credit — not a token to pump. this rail is on testnet: the plumbing, not a profit.` |
  | 23–25s | End card | `localharness.xyz/llms.txt` |
- **CTA:** read the spec → `localharness.xyz/llms.txt`
- **Hashtags:** `#aiagents #web3 #machinepayments #x402 #rustlang #onchain`
- **Disclosure/label:** shared block. **Extra care:** topic denylist (`RISKS.md` #10) —
  this is the highest-risk brief for an accidental financial claim. The script states the
  mechanism + the testnet caveat + "not a token to pump" on screen, and makes **no** price,
  yield, or earnings claim. Keep it that way at edit time. Caption disclosure + native
  AI-label (the diagram animation may be AI-generated B-roll → native AI-label is then
  hard-required) + bot label.
- **Footage/assets:** single device. Terminal screen-recordings of an agent-to-agent call
  (`localharness call --pay …` against your own agents); a built monochrome diagram
  (caller → facet → callee wallet) — brutalist, grid-aligned, no neon. **No diamond address
  on screen, no balances, no `$LH` totals.**

---

## V6 — Not a framework. A network.  (manifesto cut)

- **The ONE point:** the anti-bloat positioning — one Rust crate that compiles native AND
  to the browser, vs. a framework you host. (`BRAND.md` differentiator #1 + tagline #7.)
- **HOOK (0–3s):** white screen, black monospace; three lines slam in on the beat:
  **"No Python."** → **"No server."** → **"No token grift."**
- **Script + shot list (≈24s):**
  | t | Shot | On-screen caption |
  |---|------|-------------------|
  | 0–3s | Type-on text, hard cuts on a beat | `No Python. No server. No token grift.` |
  | 3–10s | Contrast: a tangled generic dependency-graph blob (UNBRANDED, illustrative) wipes away to a single line `cargo add localharness` | `one crate. native AND the browser.` |
  | 10–18s | Quick montage: the real monochrome app, a live subdomain resolving | `self-sovereign agents. your keys. your name.` |
  | 18–24s | End card: lowercase `localharness` + `every agent is a subdomain.` | `not a framework. a network.` |
- **CTA:** `cargo add localharness`
- **Hashtags:** `#rustlang #aiagents #webassembly #opensource #buildinpublic #antibloat`
- **Disclosure/label:** shared block. **Compliance flag (`RISKS.md` a.3 / #10):** the
  "dependency-graph" contrast must stay **category-level** — a generic, unbranded tangle.
  **Never** name, caption, or show a named competitor (LangChain/CrewAI/Eliza/etc.) or its
  logo; that crosses the "naming/attacking a third party" line. Caption disclosure + native
  AI-label (motion-typography/B-roll may be AI-generated → native AI-label then required) +
  bot label.
- **Footage/assets:** motion-typography (monochrome brutalist), the `cargo add localharness`
  line, screen-recordings of the live app + a subdomain. No coordination, no third-party
  assets, no logos.

---

## Shooting order & first-shoot recommendation

**Shoot first: V3 — slither.localharness.xyz.** On two explicitly visual-first
entertainment platforms (Reels/TikTok), an in-browser *multiplayer game* with a "no
install, just open a URL" hook is the single most thumb-stopping, natively-shareable asset
we have, and it proves the most *surprising* true claim (real multiplayer games, written in
a Rust subset, running in the browser at a subdomain). It's pure motion with an immediate
payoff — exactly what these algorithms reward. The one cost is its only production
dependency: it needs 2 devices/players to capture the eat-and-grow beat.

- **Zero-coordination fallback / fastest first cut:** **V4 (fractal)** — one continuous
  screen-recording, loops perfectly, no second player. If two-device capture slips, shoot
  V4 first and hold V3 for when a second device is on hand.
- **Most on-brand single-take:** **V1 (`cargo add`)** — carries the core wedge in one
  controlled terminal-to-live-subdomain reveal; lowest production risk; the natural pinned
  intro clip.
- **Then:** V2 (identity), V5 (economy — extra accuracy pass), V6 (manifesto — competitor
  guardrail).

Repurpose, don't originate (`GROWTH.md`): all six reuse footage we'd capture for the live
demos and the launch screenshots; nothing here needs a bespoke shoot. None goes live until
IG/TikTok accounts + API approval/audit land and a human approves the cut.
