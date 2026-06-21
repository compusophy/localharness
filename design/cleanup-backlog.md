# Cleanup backlog — legacy / unused / dead-weight to remove

A running list for the **"huge pass on removing unused stuff"** (legacy systems,
old crap we don't need). Likely paired with a larger refactor once **Fable 5**
lands. Append as cruft is found; clear lines as removed.

## On-chain (needs a diamondCut)

- [ ] **`CreditMeterFacet.chargeFromWallet`** (facet `0x7575FEF84A28EEb38Ca0AAF848DbEF7f7eCB6d72`, mainnet)
  — added 2026-06-20 (commit `6a53a20`) for a wallet-primary-billing direction we
  then **rejected** (no approvals; keep the meter). It is **inert** (needs a diamond
  approval to pull, and nothing approves the diamond), so it's harmless dead weight.
  Remove the selector in the cleanup cut (deploy a clean CreditMeterFacet, Replace
  the live selectors + drop `chargeFromWallet`). Not urgent.
- [ ] **`fiatLocked` / chargeback-lock machinery** (`MintGateFacet` + `CreditMeterFacet`
  lock-aware `withdrawCredits`/`meter`/`withdrawableOf`) — we decided: no chargebacks,
  no 90-day lock. The lock just adds branches that never matter. Strip it when the
  billing path is next touched.
- [ ] **`SessionFacet`** — coarse time-boxed sessions; SHELVED (the per-message meter
  is the live path). Candidate for removal if nothing reads it.

## Code / surfaces (candidates — confirm unused before cutting)

- [ ] **OpenAI backend** — shipped but PARKED (no plan; key/selector never wired).
  Keep or delete? (`src/backends/openai/`, `gpt-*` ids in CLI models list.)
- [x] **PairingFacet** — archived 2026-06-21 → `contracts/archive/` (source +
  Add/AddV2/Remove scripts). On-chain already removed; QR seed-adoption superseded it.
- [x] **Flat registry + legacy deploy** — `LocalharnessRegistry.sol` + `Deploy.s.sol`
  archived 2026-06-21 → `contracts/archive/`. Pre-diamond, abandoned after the reset.
- [x] **BootstrapFaucet** — `BootstrapFaucet.sol` + `DeployBootstrapFaucet.s.sol`
  archived 2026-06-21 → `contracts/archive/`. Dormant since Tempo sponsorship.

## Done — restore the warning signal (report Phase 1)

- [x] **CLI `#[allow(unused_imports)] use crate::*`** — DONE 2026-06-21. All 27
  `src/bin/localharness/*.rs` modules converted to explicit `use crate::{…}`
  (test-only helpers imported inside the test modules). Zero glob imports / zero
  `allow(unused_imports)` left in the CLI; un-reasoned `allow()` in `src/` fell
  53 → 26. `scripts/audit-tech-debt.sh` tracks the residual count.
- [x] **Chain-config drift guard (report §3)** — DONE 2026-06-21. Added the
  `proxy_chain_ts_defaults_match_moderato` cargo test: the proxy `_chain.ts`
  testnet fallbacks must mirror Rust `MODERATO`, caught on every `cargo test`
  (gates releases via verify.sh). Residual 26 allows are mostly legit (cfg-gated,
  wire structs, test helpers) — a future tick can reason-annotate or remove them.
- [x] **Model-catalog drift guard (report §2)** — DONE 2026-06-21. Added the
  `proxy_price_table_matches_cli_models` cargo test: the proxy `_prices.ts`
  per-model price table must price EXACTLY the non-Gemini ids in CLI `MODELS`
  (itself pinned to the backend wire constants), so a renamed model can't silently
  fall to the proxy's unknown-model default tier. Remaining §2 surfaces (browser
  selector, `_usage.ts` token rates, docs) still drift-by-hand — a fuller catalog
  is a larger follow-up.
- [x] **allow() suppressions are now a HARD GATE** — DONE 2026-06-21. The original
  "26 un-reasoned" count was a crude single-line grep; the TRUE bare count was 4
  (the rest carry `///`/`//` reasons on the line above, or are cfg_attr-conditional).
  Annotated those 4 (MCP/RPC wire-completeness fields) and rewrote
  `audit-tech-debt.sh` stage 5 to recognize a reason on the same OR preceding line
  (and skip cfg_attr / string literals), then flipped it to FAIL on any bare
  allow(dead_code|unused_imports|deprecated). The trash can't creep back.
- [x] **data-action → Action::parse guard (report §8)** — DONE 2026-06-21. Added
  `tests/data_action_dispatch.rs`: a native source-level cross-check that every
  `data-action="…"` literal under `src/app` has a `=> Action::…` arm in
  `Action::parse` (no dead buttons). `src/app` is wasm32-only so it can't unit-test
  parse directly; the text cross-check runs in every `cargo test`. Reverse
  direction (every parsed Action has a dispatch arm) is a possible follow-up.
- [x] **AGENTS.md / CLAUDE.md sync guard (report §4)** — DONE 2026-06-21. The two
  563-line maps had drifted: a blanket Claude→Codex replace turned the factual
  "Claude Messages API" into a nonexistent "Codex Messages API" (×4) in AGENTS.md.
  Fixed those, then added `tests/agents_claude_in_sync.rs`: AGENTS.md with its
  intentional agent-name substitutions undone must equal CLAUDE.md byte-for-byte,
  so they can't drift again. (Full generation per the report would be a larger
  follow-up; the guard captures the value cheaply.)

## Needs a product decision (flagged, NOT auto-resolved)

- [ ] **Pricing default drift (possible billing bug).** `proxy/api/_prices.ts`
  defaults `COST_PER_REQUEST_WEI` to **1 $LH** but `fetch.ts`, `notify.ts`, and
  `scheduler.ts` each default to **0.01 $LH** — and `main.rs`/`proxy/README.md`
  disagree too. In prod these are likely all env-overridden, but the code defaults
  are inconsistent. Fix = one shared `_metering.ts` table, but the *value* (are
  fetch/notify 1 or 0.01?) is a billing call — needs the user, not a 4am guess.
  Full analysis: `design/tech-debt-unused-code-report-2026-06-21.md` §1.

## Reference

The full audit (SSOT drift, model-catalog fragmentation, chain-config split,
AGENTS/CLAUDE near-dup, proxy auth/meter copy-paste, registry ABI boilerplate,
large-file hotspots, tooling gaps) lives in
`design/tech-debt-unused-code-report-2026-06-21.md`. This file is the *actionable
queue*; the report is the *analysis*.

## Notes

- Fable 5 is world-class → likely a large refactor soon; batch the structural
  cleanup with it rather than churning twice.
- The "4 wallets" confusion is really 3 pots (owner wallet / meter / agent TBA);
  the simplification is UI-only — show wallet+meter as ONE balance, leave the TBA
  as the agent's separate economy wallet. Not a removal, just a display merge.
