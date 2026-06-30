# OUTREACH-TEMPLATES.md — cold-email templates

> Three 1:1 outreach templates for the **marketing email once it exists**
> (`PRESS-KIT.md` contact line / `CREDENTIALS.template.md`). Voice per `BRAND.md`:
> technical, terse, honest, value-first — these read like a peer wrote them, not a
> sequencer. **The loop drafts; a human reviews and sends each one individually**
> (`RISKS.md` a.4 / b.1) — there is no auto-send and no blast.
>
> Accuracy rules (re-verified 2026-06-30): crate **0.58.0**; OpenAI/Mock/Gemma are
> **SDK-only** backends (live in-app selector = **Gemini Flash + Claude Opus** only);
> **no diamond/chain address pinned**; x402 settlement is a **mechanism, proven on
> testnet** — no mainnet-live earnings claim; **self-funding is an OPEN problem**; `$LH`
> is a flat usage credit, never a token to pump.
>
> **Personalization slots** are in `[BRACKETS]`. A slot with real, specific content is
> required — if you can't fill `[SPECIFIC_RECENT_THING]` with something genuine, **don't
> send the email**. Faked personalization is worse than none.

---

## AI-disclosure & legal — applies to all three (read before sending)

- **These are AI-drafted, human-reviewed, human-sent.** A 1:1 email a human reads and
  personally sends is not "public AI-generated content," so the platform AI-content labels
  don't apply to the email itself — **but** if a recipient asks whether AI is involved, be
  honest (the brand runs an automated agent; the draft was AI-assisted). Never deny it.
- **Any *public* output that results carries the full disclosure.** If outreach leads to a
  guest post, an AMA, a quoted blurb, or an episode where the brand's own AI agent produces
  public content, that content must carry (a) an AI-generated disclosure, (b) the
  material-connection disclosure (it's the project's own agent), and (c) the platform's
  native AI/bot label — FTC double-disclosure (16 CFR Part 255) + EU AI Act Art. 50
  (`RISKS.md` a.2, guardrail #9).
- **Material connection** is always disclosed: you are the project (or its operator), not a
  neutral third party. Say so in the email.
- **Cold-email law:** comply with CAN-SPAM (truthful subject + "from", a real physical
  mailing address, a working opt-out, honor it promptly) and, for EU/UK recipients,
  GDPR/PECR (legitimate-interest B2B only, easy opt-out). A relevant, individually-written
  email to a public professional address is fine; a scraped-list blast is not.

---

## Template A — dev newsletter / blogger

**Goal:** earn a mention or a "worth a look" in a technical newsletter/blog. Lead with the
one surprising true fact; make it trivially easy to verify.

**Subject:**
```
One Rust crate that's both an agent SDK and a browser-resident agent
```

**Body:**
```
Hi [FIRST_NAME],

I read [NEWSLETTER/BLOG NAME] — [SPECIFIC_RECENT_THING you genuinely read, e.g. "your
write-up on ripping LangChain out of a prod service"]. [ONE honest sentence on why that
makes localharness relevant to your readers, not generic flattery.]

I'm the [maintainer/operator] of localharness, an open-source (Apache-2.0) project. The
short version: it's one Rust crate that's both a model-agnostic agent SDK and — with one
feature flag — a self-sovereign agent that runs in the browser at its own subdomain.

  cargo add localharness   # an agent loop: streaming, tools, hooks, policies, MCP, compaction
                           # the same crate compiles to wasm32 — the loop runs in a browser tab

The bit your readers might find interesting: the same code runs native (tokio) and in the
browser with no server hosting the agent, and each agent is an on-chain identity (its own
name, wallet, persona) that can be hired and paid per call. Gas is sponsored, so onboarding
needs no wallet or seed phrase.

Two live demos that are just URLs, if you want to poke at it before anything else:
  - https://slither.localharness.xyz  (multiplayer game, written in a Rust subset, in-browser)
  - https://fractal.localharness.xyz  (a cartridge running itself, recursively)

Source: https://github.com/compusophy/localharness · Crate: https://crates.io/crates/localharness

No ask beyond: if it's a fit for [NEWSLETTER/BLOG NAME], I'd be glad to answer anything —
the native/wasm seam and the tool dispatch are where the interesting decisions are. If it's
not a fit, no worries and no follow-up.

Honest disclosure: I build this, so I'm not a neutral source. Happy to be the skeptical kind
of read.

[YOUR NAME]
[MARKETING EMAIL] · localharness.xyz
[physical mailing address — CAN-SPAM] · reply "no thanks" and I won't email again
```

**Notes:** keep links in the body (it's email, not a ranked feed). One email, one
recipient, one genuine reason. If they don't reply, **do not** auto-follow-up more than
once, and never on a fixed cadence.

---

## Template B — podcast host

**Goal:** offer a concrete, listener-relevant episode angle — not "have me on." Give them
the hook and the proof.

**Subject:**
```
Episode idea: self-sovereign agents that hold their own keys (open-source, Rust)
```

**Body:**
```
Hi [FIRST_NAME],

[SPECIFIC_RECENT_EPISODE — name it and one honest line about what landed for you, e.g.
"the episode with [GUEST] on agent frameworks — the part about who actually owns the agent
stuck with me."] That's the thread I'd want to pull on.

I run localharness, an open-source (Apache-2.0) project exploring a contrarian take: what
if an AI agent were a real, self-sovereign entity instead of a rented API key behind
someone else's server? Concretely — one Rust crate that's a model-agnostic agent SDK and,
with one flag, a live in-browser agent at its own subdomain, where each agent owns its
name, keys, and wallet on-chain and can be hired and paid per call.

A few angles your audience might actually argue about:
  - "the agent should own its keys" vs the rented-API-key default — and the tradeoffs
  - one crate that compiles native AND to wasm32: how that constrains the design
  - the honest hard part: can a business made of agents pay for its own inference? (Today
    it's seed-funded, not self-funding — I'd rather discuss the open problem than pitch.)

It's a real, try-it-now product (two live demos are just URLs: slither.localharness.xyz,
fractal.localharness.xyz), and I'm a solo, build-in-public maintainer — so it's a builder
conversation, not a corporate one. I can bring specifics and concede what isn't done.

Source: https://github.com/compusophy/localharness · Spec: https://localharness.xyz/llms.txt

Material disclosure: it's my project, so I'm an interested party — flag that to listeners
however you like. If the timing or fit is off, all good.

[YOUR NAME]
[MARKETING EMAIL] · localharness.xyz
[physical mailing address — CAN-SPAM] · reply "no thanks" and I'll stop here
```

**Notes:** pitch a topic, not yourself. Name the open problem (self-funding) — hosts trust
guests who concede limits. If the episode airs, the AI-disclosure rules above apply to any
public AI-generated promo of it.

---

## Template C — community / subreddit mod (AMA / value-first contribution)

**Goal:** ask the mod *first* — propose a value-first contribution that fits the sub's
rules, never a drive-by promo. This is permission-seeking, not posting.

**Subject:**
```
Permission to contribute to r/[SUBREDDIT]: an AMA / write-up on building a Rust agent SDK?
```

**Body:**
```
Hi [MOD_NAME / mod team],

I want to do this the right way and check with you before posting anything.

I maintain localharness (open-source, Apache-2.0) — a Rust-native, model-agnostic agent SDK
that's also a browser-resident, on-chain agent network. I think r/[SUBREDDIT] would have a
genuinely good [discussion / critique] about [SPECIFIC_SUB_RELEVANT_ANGLE — e.g. for r/rust:
"the native↔wasm32 seam: cfg-gating tokio::spawn vs spawn_local and collapsing Send+Sync to
a marker trait"], but I don't want to trip the self-promotion rules or waste anyone's time.

Would any of these be welcome, and on what terms?
  - a technical write-up / "how it's built" post (no link-drop; substance in the body), or
  - an AMA, if that's a format you run, or
  - I simply hang around and answer questions in relevant threads first, and earn the right
    to post later.

I'm aware of the ~9:1 norm and I'd rather over-contribute than promote. Happy to follow
whatever cadence and format you prefer, or to drop it entirely if it's not a fit.

Full disclosure: it's my own project, so I'd be a self-interested poster — I'd state that
plainly in anything I write, and if the brand's automated account were ever involved in
generating content I'd label it as AI-generated per the usual rules. A human (me) writes and
stands behind anything posted to the sub.

Repo if useful for vetting: https://github.com/compusophy/localharness

Thanks for keeping the sub good — I'll wait for your go-ahead before posting anything.

[YOUR NAME]
[MARKETING EMAIL]
```

**Notes:** this is a *request to a mod*, sent before any submission — it is not itself the
post. The actual r/rust and r/ethdev drafts live in `READY-QUEUE.md` (H2/H3) and are
**human-posted from an aged account only when the 9:1 budget is healthy**. Never paste the
same body into two subs (near-duplicate = spam flag).

---

## DO NOT (hard list)

- **No mass-send / no blast.** One individually-written email per recipient. No sequencer
  firing identical copy at a list. A scraped list is out.
- **No fake personalization.** If you can't fill a `[SPECIFIC_…]` slot with something real
  you actually read/heard, don't send it. "I loved your content" is spam.
- **No automated sending.** The loop only *drafts*; a human reviews and sends each email
  (`RISKS.md` a.4). The autonomous path holds no send credentials.
- **Respect unsubscribe / opt-out, immediately and permanently.** Honor "no thanks" on the
  first signal; never re-add. One polite follow-up maximum, never on a fixed cadence.
- **Comply with cold-email law** (CAN-SPAM: truthful subject + sender, real physical
  address, working opt-out; GDPR/PECR for EU/UK B2B). Public professional addresses only.
- **Always disclose the material connection.** You are the project, not a neutral reviewer —
  say so in every email. If asked about AI involvement, answer honestly.
- **Respect each platform's ToS and each community's self-promo rules.** Ask mods first;
  honor the 9:1 norm; no posting until invited where invitation is the norm.
- **No upvote/vote/engagement solicitation, ever** — not from friends, employees, or the
  loop's other agents (voting-ring → domain shadowban; `localharness.xyz` is the brand's
  primary handle, a one-way door — `RISKS.md` a.1).
- **No financial/earnings/investment framing of `$LH`.** It's a usage credit; self-funding
  is an open problem. No "buy `$LH`", no yield, no price talk (`RISKS.md` a.3 / #10).
- **No naming/attacking a named competitor.** Keep contrasts category-level ("a Python
  dependency graph"), never a brand or logo.
```
