# DECISIONS.md — owner-blocked calls, consolidated

> Every decision the autonomous-business loop has surfaced that **only the human
> owner can make**, pulled from `LEDGER.md`, `BACKLOG.md`, `STRATEGY.md`,
> `ADDRESS-DRIFT.md`, `RISKS.md`, `LOOP-PROTOCOL.md`, `marketing/{CREDENTIALS.template,
> GROWTH}.md`, and `design/road-to-v1.0.md`. Ordered by leverage (most-unblocking
> first). Each is answerable in one short line — see the menu at the bottom.
>
> Nothing here is auto-applied. The loop's standing guardrail is **0 on-chain writes /
> 0 `$LH` / 0 live posts** (`LOOP-PROTOCOL.md` §3); these decisions are exactly the
> levers that raise specific ceilings.

---

## TL;DR — approve all in one glance

| # | Decision | Recommendation (lead option) |
|---|----------|------------------------------|
| 1 | Build-now vs RESET | **Wait for the reset.** Found on testnet now; re-found on the fresh mainnet diamond at the reset. |
| 2 | Testnet dogfood greenlight | **YES — scoped testnet-only exception** (chain 42431, sponsored, 0 mainnet spend). |
| 3 | Social credentials | **Seed Tier 1–2 first** (GitHub PAT + X + dev.to + Reddit + a marketing email); scoped tokens in gitignored `.env.marketing`. |
| 4 | Sponsor float + owner key | **Fund the float + add monitoring now** (do regardless of reset). Owner key stays offline — you run every cut. |
| 5 | MTL / transferable `$LH` | **Commit to non-transferable fiat-origin `$LH` at the reset** (free). Legal read only if you want to sell transferable credits. |
| 6 | CLAUDE.md/AGENTS.md address relabel | **Apply the lockstep relabel now** (free, prevents wrong-chain tx). Reset re-pins anyway. |
| 7 | Code-autonomy ceiling / PR-to-main | **Let the loop open DRAFT PRs to main** (propose rung); you review + merge. Never auto-merge. |
| 8 | Tab-free authority gap | **Co-located CLI host now** (no cut); defer the scheduler-role facet to the reset. |

---

## 1. Build the org NOW vs wait for the pre-1.0 RESET  ·  *keystone — frames 2, 4, 5*

**Context.** The platform is live on the mainnet diamond (`0x8ab4f3a5…f3a77`, chain
4217), so the business guild *could* be founded today. But a single atomic **pre-1.0
RESET** is planned (`road-to-v1.0.md` §3): fresh diamond + token + audited economy
ladder + I6/I7 re-cuts + dead-weight removal + off-chain notifications + sybil
economics + one canonical address set, all in one deploy. Anything founded on the
current diamond gets orphaned and must be re-instantiated after the reset.

**Options.** (A) Wait for the reset; found the org on the fresh diamond when it lands.
(B) Found on the current mainnet diamond now and re-found after the reset. (C) Found on
testnet now, mainnet only after the reset.

**Recommendation: ▶ (C) Wait for the reset for *mainnet*; found on *testnet* now.**
Founding is cheap to redo but it sets the dogfooding clock, and the reset wipes
mainnet state regardless. Proving the full pipeline on testnet (Decision 2) captures
all the dogfood learning at zero mainnet spend and zero throwaway gas, leaving mainnet
founding as a clean one-shot on the post-reset diamond.

**Cost/risk.** A = idle org-spine work until the reset ships (slowest dogfood start,
but zero wasted writes). B = real sponsor gas spent on state that gets thrown away +
the address churn the reset is meant to end. **C = best of both: real E2E proof now,
no mainnet waste** (only cost: testnet ≠ mainnet fee-token/relay path, so a thin
mainnet smoke is still needed post-reset).

**Unblocks.** The org-spine backlog (#1 create_guild, #2 role roster, #7 payroll) and
sequences Decisions 4 and 5 — answering "wait" lets both defer cleanly to the reset.

---

## 2. Greenlight REAL on-chain founding on Moderato TESTNET  ·  *biggest immediate unblock for the loop*

**Context.** The loop's hard guardrail is **0 on-chain writes** (`LOOP-PROTOCOL.md`
§3). The `found_company` write pipeline + `company_status` read-back are shipped and
wasm/drift-verified on the branch but have **never been run** — the create→read cycle
is unproven. Moderato testnet (chain 42431, diamond `0x6c31c01e…Da30c`) is fully
sponsored: zero mainnet `$LH`, zero real money, fully reversible.

**Options.** (A) Grant a **scoped testnet-only** exception: the loop may execute real
`found_company` → `company_status` on chain 42431, capped (e.g. ≤1 founding + reads
per tick), mainnet writes still hard-0. (B) Keep 0-writes everywhere; you run the
dogfood by hand. (C) Stay design-only until the reset.

**Recommendation: ▶ (A) Grant the scoped testnet exception.** This is the single
highest-leverage unblock for autonomous progress: it lets the loop prove the entire
product end-to-end (CLI twin → found → read → later escrow→claim→work→judge→pay→attest)
and file each rough edge as a real bug, with **no mainnet risk and no human in the
tick**. The typed-confirm gate, the mainnet 0-write ceiling, and the no-merge/no-deploy
stops all stay intact — this widens exactly one ceiling on one throwaway chain.

**Cost/risk.** A = a confused tick could spam testnet names (mitigate: per-tick cap +
prefixed throwaway names + it's testnet, so blast radius is junk identities). B =
correct but stalls the loop on you. C = safest, slowest — the pipeline stays unproven
into the reset. **Risk of A is bounded to testnet garbage; reward is the whole QA loop.**

**Unblocks.** BACKLOG NEXT-TICK "Dogfood the full create→read cycle on TESTNET" +
backlog #3 (headless escrow→…→attest cycle) — the loop's next 2–3 ticks of real work.

---

## 3. Social credentials — which platforms first + secure delivery  ·  *unblocks all marketing execution*

**Context.** Marketing is fully **prepared** (BRAND, CONTENT, GROWTH, READY-QUEUE,
DEVTO-ARTICLE) but stuck in *prepare* mode — the loop holds **no** post credentials by
design (`RISKS.md` a.4). Account signup is human-only everywhere (CAPTCHA/SMS/ToS);
the agent only runs accounts you seed, via official APIs. `GROWTH.md` ranks channels
by fit × ToS-safe automatability.

**Options (which to seed first).** (A) **Tier 1–2 only:** GitHub PAT (you likely have),
a dedicated marketing email, X, dev.to, Reddit. (B) Tier 1–2 **+ Tier 3** (LinkedIn/IG/
TikTok) up front. (C) GEO + GitHub only (no social accounts yet).
**Delivery:** scoped API tokens (never passwords) dropped in a gitignored
`~/.lh_marketing_secrets` (preferred — outside the tree) or `.env.marketing` (already
covered by `.gitignore`'s `.env.*`).

**Recommendation: ▶ (A) Seed Tier 1–2 first, deliver as scoped tokens in
`~/.lh_marketing_secrets`.** dev.to + GitHub auto-publish cleanly (first-party content,
no self-promo friction); X posts own content via the official API; Reddit stays
draft→human-approved (9:1 rule). Skip Tier 3 for now — IG/TikTok/LinkedIn need 2–4-week
app reviews and are off-core audience; add them as a repurposing layer once Tier 1–2
hums. **HN stays human-only forever** (programmatic = domain shadowban; one-way door).

**Cost/risk.** A = fast, low-ToS-risk, covers the real dev audience; minor: X is
pay-per-use (~$0.015/post). B = weeks of setup cost up front for low-fit channels,
delays nothing useful. C = safest but leaves the whole social arm dark. Even under A,
**live posting stays per-post human-approved** (`LOOP-PROTOCOL.md` §7) until you flip it.

**Unblocks.** Moves marketing from *prepare* to *execute*: AUTO dev.to/GitHub publishing,
the X build-in-public cadence, the citation-monitoring panel, and the READY-QUEUE
assets. Delivers the loop's north-star instrument (on-chain identity claims via UTM).

---

## 4. Sponsor-float funding + the diamond owner (cut/admin) key  ·  *gates mainnet ops + new facets*

**Context.** Every sponsored write rides one low-budget sponsor float (~**1.46 USDC.e**
today, **no monitor**) — the single drainable SPOF behind all mainnet activity. Any new
facet (scheduler-role authority, sybil bond) needs the **diamond owner key**, which is
**not in the repo** by design and never should be. Both are owner-only.

**Options.** (A) Fund the float to a real runway + stand up balance/error monitoring +
alerting **now**, and keep the owner key strictly offline (you personally run every
`diamondCut`). (B) Defer both to the reset window. (C) Fund + monitor now; pre-stage any
facet cuts to execute *during* the reset.

**Recommendation: ▶ (C) Fund the float + add monitoring now; batch facet cuts into the
reset.** Float funding + a monitor are needed *regardless* of the reset (mainnet is live
and a drained float silently breaks the zero-balance UX), so do them immediately and
cheaply. New facets, though, should ride the reset's audited ladder rather than a
one-off cut — so pre-stage them and execute under your key in that one window. **The
owner key never enters the loop's environment** (`LOOP-PROTOCOL.md` §6 hard-stop).

**Cost/risk.** A = float safe + monitored, but ad-hoc cuts fragment the reset. B = the
float stays a ~$1.46 SPOF with no alarm until the reset — a live UX risk. **C = float
de-risked today, cuts done once, audited.**

**Unblocks.** Backlog #4 (fund + monitor the sponsor SPOF), the always-on ops half of
the architecture, and any facet-dependent item (#5 tab-free authority, sybil breaker).

---

## 5. MTL / legal on transferable `$LH`  ·  *gates real external revenue*

**Context.** `$LH` is open-loop today (`settle`/`withdrawCredits`/`send_lh`), so
fiat-bought *transferable* `$LH` is likely stored-value / money-transmitter (MSB)
territory (`road-to-v1.0.md` H2). External paying callers above inference cost are the
*only* path to a net-positive business — but selling transferable credits for money is
the legal exposure.

**Options.** (A) Commit to making **fiat-origin `$LH` permanently non-transferable**
(a spend-on-compute balance class) at the reset — sidesteps MSB entirely, free to
implement in the reset. (B) Get a formal legal/MTL read before selling transferable
credits. (C) Defer; keep `$LH` internal-credit-only with no external sale.

**Recommendation: ▶ (A) Commit to non-transferable fiat-origin `$LH` at the reset;
optionally start (B) in parallel only if you specifically want to sell transferable
credits.** (A) removes the blocker at zero legal spend and is "free at the reset" per
the road-to-v1 plan; agents still earn/spend internal `$LH`, you just can't cash
fiat-origin credits back out. Pursue the legal read only if a transferable-credit market
is a deliberate business goal — it's slow and lawyer-gated.

**Cost/risk.** A = closes the compliance hole cheaply; cost: no fiat cash-out story
(fine for a compute-credit). B = unlocks transferable credits but is slow + costs legal
fees + carries real regulatory risk. C = zero revenue path.

**Unblocks.** External-revenue backlog (#15 advertise a per-call price above cost) and
the honest "net-positive" thesis — without it, external paid demand can't be safely
priced.

---

## 6. CLAUDE.md / AGENTS.md address-table relabel  ·  *prevents wrong-chain transactions*

**Context.** `ADDRESS-DRIFT.md` proved (on-chain) there's **no code bug** — the drift is
pure labeling: CLAUDE.md/AGENTS.md present a table headed "Canonical addresses
(post-reset)" whose rows are the **Moderato testnet** set (`0x6c31c01e…`), with no
"(testnet)" qualifier, while the live platform is mainnet (`0x8ab4f3a5…`). A reader
trusting that table targets the wrong chain. The source of truth (`chain.rs`,
`docs_manifest`, llms.txt, skill.md) is already correct.

**Options.** (A) Apply the proposed lockstep relabel now (mainnet rows primary, testnet
rows explicitly marked) — hand-edit both files (drift guard requires lockstep). (B) Hold
until the reset re-pins one canonical set anyway.

**Recommendation: ▶ (A) Apply the relabel now.** It's a doc-only edit with a concrete
correctness payoff (an agent or human can't accidentally sign a mainnet tx against the
testnet diamond), the loop has the exact diff staged in `ADDRESS-DRIFT.md`, and the
reset re-pin is weeks out. The only catch is the lockstep edit + the 40K CLAUDE.md cap —
both checked by `cargo test`. *(I flagged this to you rather than auto-applying because
it's a user-curated core spec that ties to the reset; one word from you and the loop
lands it.)*

**Cost/risk.** A = ~10 lines, drift-guard-checked, eliminates a real footgun. B =
the footgun persists for weeks for zero benefit (the reset edit happens either way).

**Unblocks.** BACKLOG "Relabel CLAUDE.md/AGENTS.md address table" (NEEDS OWNER OK).

---

## 7. Code-autonomy ceiling — when does the loop open a PR to `main`?  ·  *gets shipped code into the product*

**Context.** The loop lands all work on the `autonomous-business` branch and **never
merges** (`LOOP-PROTOCOL.md` §6). Three verified, compiling slices now sit on that
branch unused by the product: `found_company`, `company_status`/`set_role`/`attest`,
and the loop guardrail tooling. The question the protocol leaves open (§8.5 "Open a PR…
never self-merge"): should the loop actually open PRs to `main`, and is its current
**propose-rung** ceiling (PR-only, no merge/deploy/release/cut) the right one?

**Options.** (A) Let the loop open **draft PRs to `main`** per code branch; you review +
merge; ceiling otherwise unchanged. (B) Keep everything on the branch; you cherry-pick
when you want it. (C) Raise the ceiling further (auto-merge after green CI).

**Recommendation: ▶ (A) Let the loop open draft PRs; you merge.** This is the designed
`propose` rung — it surfaces accumulating work for review without weakening the only
gate that matters (a human merges `main`). The loop's token stays PR-only (no merge, no
`main` push, no deploy/release/cut creds) per `RISKS.md` b.1/d.3. **(C) is a hard
no** — auto-merge defeats the single review gate and is explicitly a hard-stop.

**Cost/risk.** A = visible review queue, zero new privilege; minor: PR noise (mitigate:
one PR per coherent slice). B = work stays invisible/unshipped (the "unpushed = invisible"
scar). C = removes the load-bearing human gate — unacceptable.

**Unblocks.** Getting `found_company` + the company tools reviewed and into the real
product instead of stranded on a branch.

---

## 8. Tab-free authority gap — scheduler-role facet vs co-located CLI host  ·  *gates always-on ops*

**Context.** The off-chain scheduler tick exposes only ~4 tools — a no-tab PM/marketing
agent **cannot `post_bounty` or `spend_treasury`** (`STRATEGY.md` §b). So "fully
autonomous always-on ops" can't move value without either a new **scheduler-role facet**
(`post_bounty`/`spend_treasury` callable by the scheduler key) or a **co-located CLI
host** that holds a funded agent key and runs the value-moving tools on a timer.

**Options.** (A) Co-located CLI host now (no cut, reuses the shipped CLI + relay
self-pay selectors). (B) Cut a scheduler-role facet (needs the owner key + audit). (C)
Defer always-on value-moving ops entirely.

**Recommendation: ▶ (A) Co-located CLI host now; defer the facet to the reset.** It
needs no `diamondCut` (so no owner-key exposure and no ad-hoc cut fragmenting the reset),
reuses already-shipped machinery, and proves the always-on pattern. If it proves out,
fold a proper scheduler-role facet into the reset's audited ladder (ties to Decision 4C).

**Cost/risk.** A = a host you operate (a real but bounded op surface; keep its key
funded-narrow, relay self-pay only). B = cleanest long-term but owner-key + audit cost,
best batched into the reset. C = the always-on half of the architecture stays dormant.

**Unblocks.** Backlog #5 (close the tab-free authority gap) — the oggoel phase-2 blocker
for always-on PM/marketing/payroll ops. *(Lower near-term leverage: the testnet dogfood
in Decision 2 does not need it.)*

---

## Reply menu (answer in one line)

```
1) build-now-vs-reset .......... A wait-mainnet-only | B build-now | [C] testnet-now/mainnet-at-reset
2) testnet dogfood greenlight .. [A] yes-scoped | B no, manual | C design-only
3) social creds ................ [A] Tier1-2 (X+devto+reddit+email+gh) | B +Tier3 | C GEO+GitHub only
4) sponsor float + owner key ... A fund+monitor now | B defer | [C] fund+monitor now, cuts at reset
5) MTL / transferable $LH ...... [A] non-transferable at reset | B legal read | C internal-only
6) address relabel ............. [A] apply now | B hold for reset
7) PR-to-main ceiling .......... [A] draft PRs, you merge | B branch-only | C auto-merge (not recommended)
8) tab-free authority .......... [A] CLI host now | B scheduler facet | C defer
```

Bracketed letters are the recommendation. A bare "approve all recommendations" greenlights
the bracketed column.
