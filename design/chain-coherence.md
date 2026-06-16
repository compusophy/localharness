# Chain coherence — the CLI is testnet, the live platform is mainnet

> Surfaced 2026-06-16 (autonomous loop). NOT a bug I should fix unsupervised —
> it's an architectural decision for the owner. Flagging with the full picture.

## The split

`src/registry/chain.rs`: `ACTIVE = MODERATO` by default; `--features mainnet`
swaps in `MAINNET`. Only the **web bundle** is built `--features mainnet`. So:

| Surface | Chain | Build |
|---|---|---|
| **Published CLI** (`cargo install localharness`) | **Moderato testnet** (42431, diamond `0x6c31…`) | default features |
| **Web app** (`*.localharness.xyz`) | **mainnet** (4217, diamond `0x8ab4…`) | `--features mainnet` |
| **Credit proxy** (metering + on-ramp) | **mainnet** (live env flips `_chain.ts` → 4217; `/prices` confirms chainId 4217) | Vercel env |
| **`llms.txt` / `skill.md`** (agent docs) | **testnet** constants (the `llms_txt` test pins `registry::REGISTRY_ADDRESS` = the testnet diamond) | stamped from the default build |

## Why it matters (concrete consequences)

1. **CLI `create` is FREE; the web charges 1 $LH.** The testnet diamond has no
   `registrationCost` function at all (`cast` → "function not found"); the 1 $LH
   sybil guard exists ONLY on the mainnet diamond. So the whole "subdomains cost
   1 $LH" change applies to the **web/human** population, not CLI agents.
2. **CLI `call` hits the mainnet-metering proxy.** The CLI signs an auth token for
   its testnet address; the live proxy checks `creditOf`/session on **mainnet**.
   A testnet identity funded via testnet `redeem`/`send` has **no mainnet `$LH`**,
   so `call` 402s — unless the agent funds its address with **mainnet** `$LH`
   (e.g. via `buy`, whose on-ramp mints on mainnet). So a CLI agent's identity is
   testnet but its spendable metering balance must be mainnet — incoherent.
3. **The canonical agent docs advertise testnet addresses on the mainnet domain.**
   `llms.txt` (served at the mainnet `localharness.xyz`) is stamped with the
   **testnet** diamond/token/RPC, because the `llms_txt` test + `build-web` read
   the default (testnet) build's constants. An agent that reads `llms.txt` for the
   registry address and queries it gets **testnet** state, not the live mainnet
   platform's.
4. **Earlier doc drift (now corrected):** the subdomain-pricing pass edited
   `llms.txt`/CLI hints to say "claiming costs 1 $LH / buy first" — true for
   mainnet, **wrong** for the testnet CLI those docs describe. Reverted to the
   testnet-accurate "free" this pass, with a one-line chain note.

## Options (owner's call)

- **A. Move the CLI default to mainnet** (`ACTIVE = MAINNET`, or flip the default
  feature). Unifies CLI + web + proxy + docs on mainnet; the 1 $LH guard, fiat
  ramp, and x402 metering then apply to agents too. Biggest change (a release;
  the embedded sponsor + every CLI flow now mainnet; testnet becomes opt-in via a
  feature). Most coherent end state.
- **B. Keep the split, document it explicitly.** CLI/testnet = free sandbox for
  dev; mainnet web = the real economy. Then `llms.txt`/`skill.md` must clearly
  say "CLI = testnet sandbox" and the proxy should meter testnet for testnet
  callers (or the CLI can't `call` the live proxy without mainnet `$LH`). Cheapest
  but leaves two economies.
- **C. Hybrid:** CLI gains a `--mainnet` flag / `LH_CHAIN` env so an agent can opt
  into the real economy, default staying testnet. Proxy must then gate per the
  caller's declared chain.

The proxy-metering-chain ↔ CLI-chain mismatch (point 2) is the most urgent: today
a vanilla `cargo install` agent likely **cannot complete a paid `call`** against
the live proxy. Worth deciding before promoting the CLI onboarding path widely.
