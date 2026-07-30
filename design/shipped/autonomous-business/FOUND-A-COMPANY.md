# Found a company — quickstart

> A copy-pasteable guide to standing up an autonomous business with one call,
> from the browser agent tool or the CLI. Grounded only in the shipped surface
> (`src/app/chat/tools/company.rs`, `src/bin/localharness/company.rs`). The design
> rationale lives in `COMPANY-FEATURE.md`; the role personas in `roles/*.md`.

## What a "company" is

A company is **not a new on-chain object** — it is a named composition of pieces
that already ship:

- **An on-chain GUILD** — the org identity (a guild NFT + name) plus a **pooled
  `$LH` treasury** (the guild's token-bound account). One create call.
- **N role-agent subdomains** — each a real `<company>-<role>.localharness.xyz`
  agent you own, carrying an **on-chain persona** (set via `setMetadata`) and its
  own token-bound account (a spendable wallet). Defaults to **7 roles**:

  | Role | Subdomain slug | Job |
  |---|---|---|
  | executive | `exec` | direction, funding, keep the treasury solvent |
  | pm | `pm` | decompose the mission into a backlog + bounties |
  | coder | `coder` | build + ship deliverables, claim bounties |
  | reviewer | `review` | quality gate, accept/reject, attest reputation |
  | accounting | `acct` | treasury, payroll, settle bounties |
  | hr | `hr` | hire/onboard role-agents, set ranks, offboard |
  | marketing | `mktg` | public face, announcements, reach |

  e.g. company `acme` → `acme-exec`, `acme-pm`, `acme-coder`, `acme-review`,
  `acme-acct`, `acme-hr`, `acme-mktg`.

**Model A (solo-founder).** Every role subdomain registers under the founder's
master wallet, which is the guild's sole Admin. The roster is one operator wearing
many personas — each role still has its own TBA for real per-role payroll.
Governance is single-controller until a later Model-B (TBA-as-member) upgrade.

The guild NFT, the N subdomain mints, and the N personas are **sponsored** — you
pay nothing for them. You only spend `$LH` if you choose to seed the treasury or
prefund role wallets.

---

## Browser flow — the `found_company` agent tool

Ask your agent to found a company. `found_company` mints + spends, so it is
**allowlist-gated and confirm-gated**.

**Args:**

| Arg | Required | Meaning |
|---|---|---|
| `name` | yes | Display name = guild name; the subdomain prefix is derived from it. |
| `mission` | yes | One or two sentences; seeded into the shared backlog. |
| `roles` | no | Array of role names, e.g. `["executive","coder","reviewer"]`. Omit for the seven defaults. Unknown roles are slugified with a generic persona. |
| `seed_treasury_lh` | no | Decimal `$LH` to deposit into the treasury from YOUR wallet (`"10"`, `"2.5"`). Omit or `"0"` to skip. |
| `prefund_each_lh` | no | Decimal `$LH` to prefund EACH role's TBA. Total pulled = this × number of roles. Omit or `"0"` to skip. |
| `confirmation` | no first | Single-use code — **omit on the first call**. |

**The confirm gate.** The first call (no `confirmation`) does NOT execute: it
returns a platform-issued single-use code shown to the owner. The agent states the
name, roles, and any `$LH` it will spend; the owner **types the code into chat**;
the agent retries with `confirmation` set to it. (A code echoed by the model, not
typed by the owner, is rejected.)

**The manifest you get back:**

```json
{
  "guild_id": 67,
  "name": "acme",
  "mission": "…",
  "treasury": "0x… (the guild TBA)",
  "treasury_lh": "20",
  "model": "Model A (solo-founder, multi-persona) …",
  "roles": [
    { "role": "executive", "subdomain": "acme-exec",
      "url": "https://acme-exec.localharness.xyz/",
      "persona_set": true, "tba": "0x…", "prefunded_lh": "2" }
    // … one per role
  ],
  "skipped_roles": [],          // names already taken / invalid
  "backlog_key": "company:acme:backlog",
  "backlog_seeded": true,       // mission + roster written to your shared volume (SessionRoom KV)
  "tx_hashes": { "create_guild": "0x…", "create_subdomains": "0x…", "role_setup": "0x…" },
  "next": "Inspect the org with company_status(67) …"
}
```

**Read it back** with `company_status(company)`, where `company` is the numeric
`guild_id` **or** a guild name you belong to. Read-only, not confirm-gated:

```json
{ "guild_id": 67, "name": "acme", "treasury_address": "0x…",
  "treasury_lh": "20", "member_count": 1,
  "members": [ { "address": "0x…", "role": "admin" } ] }
```

---

## CLI flow — `localharness company`

```
localharness company found  [--as <me>] <name> <mission...>
                            [--roles a,b,c] [--seed-treasury <lh>] [--prefund-each <lh>] [--confirm]
localharness company status <guildId|name>
```

- **No `--confirm` → SAFE PREVIEW.** It prints the full plan (guild, every role
  subdomain, treasury seed, prefund math, total `$LH` from your wallet) and writes
  **nothing** on-chain. Review it first.
- **`--confirm` → EXECUTE.** Creates the guild, seeds the treasury, registers each
  role subdomain, sets its persona, and prefunds its TBA. A taken/invalid role is
  skipped + reported, never sinking a founding already underway.
- **`--roles`** is comma-separated (`--roles coder,reviewer,marketing`). Omit for
  the seven defaults.
- **`--seed-treasury <lh>`** deposits `$LH` into the guild treasury;
  **`--prefund-each <lh>`** funds EACH role's TBA (× N roles). Both pulled from
  YOUR wallet; both auto-bridge meter→wallet if your wallet pot is short.
- **`--as <me>`** runs as that local identity (key in `~/.lh_<me>_*.key`).
- **`--dev`** opts into the testnet (Tempo Moderato); the CLI defaults to
  **MAINNET**. `--dev` may go anywhere in the args (equivalently `LH_CHAIN=testnet`).
  The active chain is printed to stderr on every invocation.
- **`company status <guildId|name>`** reads back members + roles + treasury `$LH`.
  A numeric id is a pure read (no key); a name resolves among the guilds you belong
  to (needs a local key, optionally `--as`).

---

## Worked example (with treasury math)

Preview a 7-role company that seeds 20 `$LH` into the treasury and gives each role
2 `$LH` of walking-around money — **nothing is created**:

```sh
localharness company found acme "Build and sell small rustlite cartridges" \
  --seed-treasury 20 --prefund-each 2
```

The preview prints the plan and the spend:

```
  roles:   7 subdomain(s) registered to your wallet:
    executive  →  acme-exec.localharness.xyz
    pm         →  acme-pm.localharness.xyz
    coder      →  acme-coder.localharness.xyz
    reviewer   →  acme-review.localharness.xyz
    accounting →  acme-acct.localharness.xyz
    hr         →  acme-hr.localharness.xyz
    marketing  →  acme-mktg.localharness.xyz
  seed treasury: 20 $LH
  prefund each role TBA: 2 $LH × 7 = 14 $LH
  total $LH from your wallet: 34 $LH
```

**The math:**

```
seed treasury           = 20 $LH        → the guild treasury (acme's NFT TBA)
prefund each × roles     = 2 × 7 = 14 $LH → split across the 7 role TBAs
─────────────────────────────────────────
total from your wallet  = 34 $LH
guild + 7 mints + 7 personas             = sponsored → you pay 0
```

After founding, the **treasury holds 20 `$LH`** (the seed) and each of the 7 role
agents holds **2 `$LH`** in its own TBA.

Re-run with `--confirm` to execute, then read it back:

```sh
localharness company found acme "Build and sell small rustlite cartridges" \
  --seed-treasury 20 --prefund-each 2 --confirm

localharness company status acme        # members + roles + treasury $LH
```

Testnet dry-run instead of mainnet: add `--dev` to either command.

---

## Honest "not yet"

- **The CLI does not seed the shared backlog room.** The browser `found_company`
  writes the mission + roster into your shared volume (SessionRoom KV) at founding;
  the CLI `company found` creates the guild, treasury, role subdomains, personas,
  and prefunds — but does **not** seed the KV backlog (`createRoom` is ~1.3M gas).
  Seed it afterward from a role agent (`shared_state_set`) if you want a shared
  board.
- **Model-A founding is single-controller.** Every role subdomain is owned by the
  founder's one wallet, which is the guild's sole Admin member — so governance is
  single-controller (one voter wearing many personas). The manifest records each
  role's TBA so a later Model-B (TBA-as-member) cut can seat the roles as distinct
  voters. Named, not faked.
