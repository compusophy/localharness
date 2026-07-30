# STATUS — state of the business

> One-glance snapshot for the owner's return. Ground truth as of **Tick 10
> (2026-06-30)**, branch `autonomous-business`. Detail lives in `LEDGER.md`
> (per-tick log), `ARCHITECTURE.md` (the map), and `DECISIONS.md` (your open calls).

## TL;DR

An autonomous software **business-of-agents** (coder/reviewer/PM/exec/accounting/HR/
marketing), built on and *for* localharness. Over 11 ticks the loop has shipped a
complete, **inspectable, read-only PREVIEW product** — found → status → plan → payroll →
books → day → forecast, five tested pure cores, a runnable example, and a fully-stocked marketing
library — all on the branch, **nothing executed on-chain, nothing posted, nothing merged.**
**The one thing to do:** answer the 8 calls in `DECISIONS.md` (one line each, recommendations
bracketed) — they're the only thing gating the jump from *preview* to *live*.

---

## What the system can do TODAY  ·  *all read-only / preview — zero writes, zero broadcast*

**`company` CLI** (`src/bin/localharness/company.rs` — honors `LH_CHAIN`):

| Command | What | Mode |
|---|---|---|
| `company found <name> <mission> [--roles]` | Stands up a whole company (guild + treasury + N role subdomains + personas + KV backlog). **Without `--confirm` it's a broadcast-free PREVIEW** of the plan. | preview (write needs `--confirm`) |
| `company status <guild\|name>` | Roster (members + roles) + pooled `$LH` treasury. | read-only |
| `company plan <guild\|name>` | Dry-runs ONE work cycle off live chain reads → prints the planned assign/judge/pay/attest `Action`s. | read-only preview |
| `company payroll <guild\|name> [--fraction] [--by-rep]` | Treasury + per-role TBA balances + a suggested even/rep-weighted split. **No transfers.** | read-only |
| `company books <guild\|name> [--period-cost/-revenue/--seed/--calls]` | Net position / runway / breakeven / self-funding vs relies-on-seed. Only the treasury is on-chain; rest clearly labeled ESTIMATE. | read-only |
| `company day <guild\|name>` | One-shot daily glance composing status + plan + payroll + books under a "PREVIEW ONLY" banner. | read-only |
| `company forecast <guild\|name> [--cycles/--cost-per-cycle/--revenue-per-accepted/--submit-quality]` | Runs `simulation` forward over the live board → per-cycle treasury/throughput projection + runway-exhaustion verdict. Model inputs labeled. | read-only |

**Browser agent tools** (`src/app/chat/tools/company.rs`, registered both backends):
- `company_status` — read-only roster + treasury (mirror of the CLI). **read-only**
- `found_company` / `set_role` / `attest` — the write tools; **confirm-gated + allowlist-gated, never run** (loop holds the 0-write line).

**Pure decision cores** (`src/*.rs` — native + wasm clean, fully unit-tested, zero I/O):
- `work_cycle` — the claim→work→judge→pay→attest cycle as DATA; emits `Action` descriptors. *(13 tests)*
- `work_cycle_runtime` — `Reader` trait → `plan_cycle` → `CyclePlan`; **previews `Action`s, never executes.** *(7 tests)*
- `accounting` — honest seed-vs-earned net position, runway, breakeven, margin. *(15 tests)*
- `hiring` — role-fit ranking; the canonical ranker `work_cycle` now delegates to. *(11 tests)*
- `simulation` — multi-cycle forward forecast (treasury/throughput/runway-exhaustion). *(10 tests)*

**Runnable example:** `cargo run --example autonomous_company` — pure end-to-end demo (HR staffs
a coder, work_cycle previews 7 actions, accounting prints the honest read: net −23, *relies on
seed, NOT yet self-funding*).

> The full dry path is CLI-runnable end-to-end. The single missing layer to a *live* company is
> the **greenlight-gated Action→tx executor** (mapping documented per-variant, not built — by design).

---

## Marketing library  ·  *all PREPARED, NONE posted (loop holds no post credentials)*

| Asset type | Count / files | State |
|---|---|---|
| **dev.to articles** | 3 — `DEVTO-ARTICLE.md`, `-2` (x402+6551), `-3` (rustlite) | publish-ready, source-pinned |
| **X / Twitter** | ~30+ — `CONTENT.md` (10 posts + 10-post thread), `X-POSTS-BATCH-2.md` (10 evergreen), + READY-QUEUE launch/hook/build-in-public/founder threads | drafted, ≤280 verified |
| **LinkedIn** | 2 — launch post + autonomous-business vision (READY-QUEUE #5/#7) | drafted, gated on API approval |
| **Reddit / HN** | 3 human-gated — Show HN, r/rust, r/ethdev (+ CONTENT drafts) | **human-posts-only** (9:1 / no-automation) |
| **Visual briefs** | 6 — `VISUAL-BRIEFS.md` (IG/TikTok short-form concepts) | concept + shot list |
| **Press kit** | 1 — `PRESS-KIT.md` (boilerplate + 8 facts) | quote placeholder unapproved |
| **Outreach** | 3 templates — `OUTREACH-TEMPLATES.md` (newsletter/podcast/mod) + CAN-SPAM/GDPR notes | ready to send |
| **SEO / GEO** | 1 — `SEO-LANDING.md` (answer-engine copy, Apache-2.0 verified) | ready |
| **Calendar** | 1 — `CALENDAR.md` (3-week day×platform fire schedule keyed to READY-QUEUE) | sequenced |
| **Supporting** | `BRAND.md`, `GROWTH.md` (`[AUTO]/[APPROVE]/[NEVER]` matrix), `CREDENTIALS.template.md` | reference |

`READY-QUEUE.md` = 9 AUTO-lane assets + 3 human-gated, each carrying FTC + EU-AI-Act disclosure.
**Nothing fires until you seed credentials** (Decision 3) — and live posting stays per-post
human-approved even then.

---

## Verified / quality

Every tick re-runs the gates before committing — **branch-only, no merge, no deploy, no cut:**

- **wasm guard** — `cargo check --no-default-features --target wasm32-...` (all cores native+wasm clean)
- **doc drift** — `no_doc_drift` (generated GEN blocks vs `docs_manifest`)
- **tests** — 41 CLI tests + a 42-assertion golden integration test locking the whole `company`
  surface; 56 pure-core tests (work_cycle 13 · runtime 7 · accounting 15 · hiring 11 · simulation 10)
- **secret-scan** — `loop-secret-scan.sh` commit gate; explicit paths only (no `git add -A`)
- **license** — Apache-2.0 verified against `Cargo.toml` before any marketing claim

Standing guardrail held all 11 ticks: **0 on-chain writes · 0 `$LH` moved · 0 live posts · 0 merges.**

---

## The gated frontier  ·  *what's left needs YOU — see `DECISIONS.md` for the one-line asks*

Non-blocked work has narrowed to polish/forecasting. The substantive next leaps are owner-gated:

1. **Live execution** — the loop has never run `found_company` on-chain; the create→read cycle is
   unproven. → **Decision 2** (scoped testnet dogfood greenlight) + **Decision 1** (build-now vs reset).
2. **Real marketing** — the library is full but dark; no post creds by design. → **Decision 3** (seed Tier 1–2).
3. **Ship the code** — `found_company` + company tools sit verified-but-stranded on the branch. → **Decision 7** (draft PRs to `main`).
4. **Mainnet ops** — the sponsor float is a ~$1.46 SPOF with no monitor; new facets need the owner key. → **Decision 4**.
5. **Revenue & always-on** — transferable-`$LH` legal call (**Decision 5**), tab-free authority host (**Decision 8**), address relabel (**Decision 6**).

`DECISIONS.md` has all 8 in a one-glance table with recommendations bracketed —
**"approve all recommendations" greenlights the whole column.**
