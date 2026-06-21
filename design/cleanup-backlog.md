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
