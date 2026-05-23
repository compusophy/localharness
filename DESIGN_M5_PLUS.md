# `localharness` — M5+ design doc (multi-tenant, self-sovereign)

> This is a planning doc, not a spec — written 2026-05-23 after the
> 0.7.x line landed. The 0.7.x plan (Phases 1–10) lives in `DESIGN.md`;
> this doc picks up where that one stops. **Nothing here is shipped.**
> The plan exists so we can debate the shape before writing code.

## Goal in one sentence

Turn `localharness` from a single-tenant browser demo into a
multi-tenant, self-sovereign agent platform where each user owns their
own subdomain, their own data, and their own keys — with the operator
(you) running zero per-user infrastructure beyond a thin name registry.

## Constraints we keep

These come from prior sessions and user-stated preferences, encoded
in memory as `feedback-*` and `project-*`:

- **No central API key.** Users bring their own LLM keys. The operator
  is not a custodian of credentials, conversation data, or money.
- **No central server.** Static deploy on Vercel + a smart contract or
  two on Tempo testnet. No backend processes the operator has to babysit.
- **HTMX-style UI, Rust-driven.** Same rule as M4: every interaction is
  a fragment swap targeted at a fixed `id=`. No DOM walking. See
  [[feedback-ui-no-dom]].
- **Per-origin sandboxing is the security primitive.** OPFS per
  subdomain is "free" multi-tenancy.
- **The operator doesn't run terminal commands.** Plans must be either
  self-executing (CI, Vercel auto-deploy) or one-button manual.
- **Atomic releases.** Each milestone ships as one cohesive piece. See
  [[feedback-release-atomicity]] and [[feedback-no-phase-fragmentation]].

## What ships first: a name to point at

Before anything else lands, `*.localharness.xyz` needs to route to the
same Vercel deployment as the apex. DNS is propagating now. Once it's
live, **no code change is needed for per-subdomain isolation** — the
browser already gives us per-origin OPFS, per-origin sessionStorage,
per-origin cookies. The wasm bundle's only new responsibility is
reading `window.location.hostname` and surfacing it.

That's M5. Everything else is on top.

---

## Layers, top to bottom

### L1 — Subdomain as identity boundary

- Apex `localharness.xyz` = marketing site + signup. Static HTML +
  a "claim your name" form. Anyone can read; nobody is authenticated.
- `<name>.localharness.xyz` = one user's home. Same wasm bundle served
  by Vercel; the bundle reads the hostname and decides what to do.
- Per-origin OPFS means each subdomain has its own private storage
  with no work from us. `john.localharness.xyz` cannot read
  `jane.localharness.xyz`'s OPFS even if both are open in the same tab.

**Trust model:** anyone can visit any subdomain. Auth is the next
layer (L3). The subdomain is just a name + a sandbox.

### L2 — Wallet in browser

User identity = a keypair generated client-side on first visit.

- Wallet bytes live in OPFS at `.lh_wallet.json`. Same security model
  as the Gemini key (per-origin sandbox; not encrypted at rest yet).
- App auto-generates one on first visit if missing. User never has to
  know what a private key is until they want to back it up or move
  devices.
- Export/import via copy-paste of a hex-encoded seed phrase or raw
  key. (Tempo-compatible — use whatever scheme tempo-x402 expects.)
- This wallet is the identity for the registry contract (L4), the
  signer for any auth challenge (L3), and eventually the payer/payee
  for x402 transactions (L5).

**Why client-side wallet, not OAuth or magic-link:** OAuth makes a
Big Tech company the identity provider (contradicts self-sovereign);
magic-link needs email infra (operator side, recurring cost). Wallet
gen has a learning-curve cost paid once; after that the user owns
their identity.

**Cost:** added wasm dep — likely `alloy` or a tempo-specific crate.
Bundle grows; gate behind a feature so the SDK side stays clean.

### L3 — Auth, in-app

For the home origin (`john.localharness.xyz`):

- App reads registry contract: "owner of `john` is `0xABC...`"
- App checks: does the local OPFS wallet's pubkey match `0xABC`?
  - **Yes** → unlock "owner mode" — show send/edit/save/wipe affordances.
  - **No** → "this is John's public profile" — read-only; show "import
    wallet" affordance for the case where John is on a new device.

No session cookies, no JWTs, no auth server. Possession of the
private key for the registered pubkey IS authentication. Every
state-changing action that touches the registry contract is signed.

**For state local to this origin** (saving a conversation, editing a
file in OPFS), the wallet is moot — same-origin JS access already
implies write authority. The wallet only matters for things the
registry contract enforces, or for at-rest encryption (L6 / future).

### L4 — Registry contract (Tempo)

The minimum: `subdomain → ownerPubkey` mapping, on Tempo testnet,
deployed via Foundry.

```solidity
// LocalharnessRegistry.sol — sketch
contract LocalharnessRegistry {
    mapping(string => address) public ownerOf;
    mapping(address => string) public nameOf;
    event Registered(string indexed name, address indexed owner);
    event Transferred(string indexed name, address indexed from, address indexed to);

    function register(string calldata name) external {
        require(ownerOf[name] == address(0), "taken");
        require(bytes(nameOf[msg.sender]).length == 0, "already own one");
        require(_isValid(name), "invalid name");
        ownerOf[name] = msg.sender;
        nameOf[msg.sender] = name;
        emit Registered(name, msg.sender);
    }

    function transfer(string calldata name, address to) external {
        require(ownerOf[name] == msg.sender, "not owner");
        require(bytes(nameOf[to]).length == 0, "recipient already owns one");
        delete nameOf[msg.sender];
        ownerOf[name] = to;
        nameOf[to] = name;
        emit Transferred(name, msg.sender, to);
    }

    function _isValid(string memory name) internal pure returns (bool) {
        // ASCII a-z, 0-9, dash; min 3 chars, max 32. Done in Solidity
        // to keep the wasm side simple — also matches DNS label limits.
        bytes memory b = bytes(name);
        if (b.length < 3 || b.length > 32) return false;
        for (uint i = 0; i < b.length; i++) {
            bytes1 c = b[i];
            bool ok = (c >= 0x30 && c <= 0x39)  // 0-9
                   || (c >= 0x61 && c <= 0x7a)  // a-z
                   || c == 0x2d;                  // dash
            if (!ok) return false;
        }
        // No leading/trailing dash.
        if (b[0] == 0x2d || b[b.length-1] == 0x2d) return false;
        return true;
    }
}
```

**Decisions baked in (each open for debate):**
- One subdomain per pubkey. Forces conscious choice; defeats squatting
  via key-shuffling. Loosen later if needed.
- Free registration on testnet. Mainnet would charge gas; that's the
  natural anti-spam. Optional: small fee that goes to a treasury.
- No expiry. ENS-style yearly renewal could come later if squatting
  becomes a real problem.
- Names mirror DNS label rules. The contract validates so the wasm
  side doesn't have to.

**Why not fork ENS:** ENS is overkill for v1 — its multi-resolver
architecture, reverse records, and TTL machinery aren't needed yet.
The contract above is ~50 lines and does what we need. We can adopt
ENS conventions later (reverse records, resolver interface) without
breaking this.

**Operational reality:**
- You deploy once via `forge create`. I write the script; you click run.
- Contract address goes into the wasm bundle as a const.
- Vercel deploys are independent of the contract — the contract is
  a separate operational concern that only changes when we want it to.

### L5 — Payments (x402, eventually)

Only sketched here — defer until L1-L4 prove the platform shape.

Possible flows:
- **User pays operator** for managed services (nothing yet, but a
  "premium" tier could pay for default-key access).
- **Agent pays agent** (Shopify-for-agents): one user's agent calls
  another user's published agent → x402 transfer settles automatically.
- **User pays themselves** for compute (charges their own wallet on
  every LLM call, as a budget mechanism).

x402 fits cleanly because every agent action already passes through
the tool runner — adding a pre-tool-call hook that requires payment is
trivial. The hard part is UX: how do users top up, see balances, audit
spend? That's a whole sub-design.

### L6 — Encryption at rest (defer)

Same-origin = same JS access = can read OPFS. If an attacker can run JS
in `john.localharness.xyz`, they can read John's data — including the
wallet private key. That's the threat model.

Mitigation, if it ever becomes load-bearing:
- Wallet exists; derive a sym key from it; encrypt all sensitive OPFS
  contents (history, key, wallet itself with a password gate).
- Non-owner visitors see ciphertext; even an XSS in the bundle can't
  trivially exfiltrate without the wallet.
- Adds UX friction (password on every reload? or session-cached?) so
  not free.

**Don't ship until there's a real reason.** OPFS sandboxing is enough
for a personal demo; encryption matters when multi-device or
multi-user-on-same-machine becomes real.

### L7 — Cross-device sync (defer further)

If a user opens `john.localharness.xyz` on phone vs laptop, they see
different OPFS contents — there's no sync layer. Solutions all have
trade-offs:

- **IPFS pinning service** — distributed, costly, slow.
- **A dedicated sync server you run** — back to centralization.
- **On-chain storage** — gas costs make this absurd for chat history.
- **Pin a CRDT to the wallet** — interesting but research-grade.

Don't pretend to solve sync until users actually feel the lack.

---

## Phase plan

| M | Surface | Effort | Blocker | Notes |
|---|---------|--------|---------|-------|
| **M5** | DNS + subdomain self-awareness in the app | small | DNS propagation | App reads `hostname`, shows "this is X's space", switches OPFS just by virtue of being on a different origin. No identity yet. |
| **M6** | Browser wallet (gen + import + export); `.lh_wallet.json` in OPFS | medium | M5 | Add wasm-compatible crypto dep (alloy, tempo-x402, or hand-rolled). Wallet exists but isn't checked against anything. |
| **M7** | `LocalharnessRegistry` deployed on Tempo testnet; app reads it on mount | medium | M6 + you running `forge create` once | Address goes into the bundle as a const. Contract is immutable; bundle re-reads on every load. |
| **M8** | Owner-gated UX (sign/verify against registry on mount) | small | M7 | Lock down write actions when wallet doesn't match. Show "import wallet" UX for new device. |
| **M9** | x402 payment hooks | large | M8 + a real use case | Don't build until there's a flow that needs payment. |
| **M10** | At-rest encryption | medium | M8 + a real threat | Don't build until OPFS visibility actually matters. |
| **M11** | Cross-device sync | large | M8 + user demand | Don't build until users complain about device drift. |

The first three (M5–M7) are sequenced. M8+ branches — pick based on
what hurts.

---

## Apex marketing site

`localharness.xyz` (the apex, no subdomain) needs a separate page
than `*.localharness.xyz`. Probably:

```
web/
├── index.html         currently the IDE
├── apex.html          NEW: marketing / signup
└── pkg/               wasm (unchanged)
```

Vercel routing — either two separate projects, or one project with
host-based rewrites in `vercel.json`:

```json
{
  "rewrites": [
    { "source": "/", "has": [{ "type": "host", "value": "localharness.xyz" }], "destination": "/apex.html" }
  ]
}
```

Content of apex: explain what this is, "claim your name" form (calls
the registry contract), link to source on GitHub. Static, no wasm
needed (or maybe a tiny wasm bundle for the registry call only).

---

## Open questions

1. **Tempo crate.** Does `tempo-x402` compile on wasm32 today? If not,
   how much porting is needed? Worth a 30-minute spike before
   committing to the dep.
2. **Wallet UX for non-crypto users.** "Generate wallet" → "back up
   your phrase" → "actually let me skip" → user loses access on next
   device. Need a forcing function or a clear "you're on your own"
   warning. The Phantom model is the right inspiration.
3. **Squatting.** First-come-first-served + one-per-pubkey is naive
   but ships. Reserved names list? Anti-bot via a small testnet fee?
4. **Apex login.** Should the apex page require a wallet to claim, or
   should claim happen on the subdomain after redirect? Latter is
   simpler (the subdomain is the canonical place for the wallet).
5. **What if Tempo is down / RPC fails on mount?** App should
   degrade gracefully — show "registry unreachable" but still let the
   user use the local OPFS data. Identity check can be deferred.
6. **Discovery.** How does anyone find John's agent if they don't know
   the subdomain? Eventually: a directory page on the apex. Defer.

---

## What I'd build next, in order

Assuming you green-light each:

1. **DNS sanity check** — once propagation completes, hit
   `https://test.localharness.xyz/` and confirm the bundle loads.
   (No code change needed — Vercel wildcards just work.)
2. **Surface the subdomain in the app's chrome.** Tiny change to
   `templates::chrome()` — read `hostname` via web-sys and show it.
   "you are on john.localharness.xyz · per-origin OPFS active".
3. **Apex page.** Static HTML at `web/apex.html`, Vercel routing tweak.
   Just text + a link to "go to your subdomain" (no form yet).
4. **Wallet module.** `src/app/wallet.rs` — gen, store in OPFS, expose
   getter. No UI yet beyond a "your pubkey: 0xABC" line.
5. **Registry contract + Foundry project.** `contracts/` directory,
   forge.toml, the contract above, a deploy script. You run
   `forge create` once; I tell you what to paste where.
6. **Registry read in the app.** On mount, fetch
   `ownerOf[hostname.split('.')[0]]`. Show "owner: 0x..." in chrome.
7. **Claim flow.** "claim this name" button if unowned and wallet
   exists → builds & sends tx → waits for confirmation → reloads.
8. **Owner-gated UX.** Hide send/save actions if wallet doesn't match
   registered owner. Add a "this is read-only" banner.

Each step ships on its own. M5 = steps 1–3. M6 = step 4. M7 = steps
5–6. M8 = steps 7–8.

---

## What I will NOT do without explicit go-ahead

- Deploy any smart contract anywhere (you control the keys).
- Buy / register / DNS-modify any domain (you control localharness.xyz).
- Add a "premium" tier that uses an operator-funded API key.
- Pull in a heavy framework (Leptos, Yew) for the wallet UI — same
  rule as the rest of the app, maud + HTMX swaps.
- Persist anything beyond the user's own OPFS without their consent.
- Run any background process or scheduled job.

Once a step is greenlit I run it autonomously per
[[feedback-execution-autonomy]] — but the steps above need individual
approval because each one is a meaningful architectural commitment.
