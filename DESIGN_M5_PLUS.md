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

### L3 — Auth: keys only, no emails

User preference is explicit: **no email auth, ever**. Sign-in is
purely cryptographic.

The trick: the wallet must live at one origin (the apex) but be
usable by every subdomain, because per-origin OPFS isolation means a
wallet stored at `localharness.xyz` is NOT visible to
`john.localharness.xyz`. Three patterns considered:

1. **Wallet per origin** (simplest, worst UX). Every subdomain
   generates its own wallet. No "master identity" possible. User has
   to manage N wallets for N subdomains.
2. **Master wallet at apex, paste private key on each subdomain**
   (medium UX). User generates wallet on first visit to apex, exports
   it, pastes it into each subdomain they own. Works but security
   gross — private key crossing origin boundaries by hand.
3. **Master wallet at apex, sign via iframe** (best UX, most code).
   Apex hosts a `/signer` iframe that holds the wallet. Subdomains
   embed it, send `postMessage` "please sign this hash", get a
   signature back without ever seeing the private key. Same model as
   Phantom / MetaMask Snaps / WebAuthn extensions.

Pattern 3 is the right destination. Pattern 1 is the right interim.
Pattern 2 is a footgun we should not ship.

**Per-subdomain sign-in flow (pattern 3, final):**
1. User visits `john.localharness.xyz`.
2. App reads registry: `ownerOf(idOfName["john"])` → `0xABC...`.
3. App embeds `https://localharness.xyz/signer?nonce=<random>` in a
   hidden iframe.
4. Iframe asks user (in apex-origin UI) to approve signing.
5. Iframe returns signature via postMessage.
6. App verifies signature against `0xABC...`. If valid → owner mode.

No session cookies, no JWTs, no central auth server. Possession of
the private key the master wallet holds IS authentication.

**For state local to this origin** (saving a conversation, editing a
file in OPFS), the wallet is moot — same-origin JS access already
implies write authority. The wallet only matters for registry-state
changes (mint, transfer, metadata) and for proving ownership to
visitors / 3rd-party services.

**Cross-device:** the user can export the master wallet (private key
or seed phrase) from apex on device A, import it to apex on device B,
and instantly have ownership of every subdomain in the registry.
That's the only thing they ever have to back up. (See L7.)

### L4 — Registry contract (Tempo) — track the EIPs

Two recent EIPs cover this exact ground; we should align rather than
invent. Decision is between them, not whether to use one.

**ERC-8004 (Trustless Agents)** — three coupled registries:
- *Identity Registry* (ERC-721 based): each agent is an NFT.
  `register()`, `setAgentURI(uri)`, `setAgentWallet(addr)`.
- *Reputation Registry*: signed feedback values for sorting agents.
- *Validation Registry*: stake-secured re-execution / zkML / TEE.

**ERC-8122 (Minimal Agent Registry)** — single contract:
- `register()`, `registerBatch()`, `ownerOf()`, `setMetadata()`.
- Extends ERC-6909 (multi-token) for ownership.
- Extends ERC-8048 (key-value metadata).
- ERC-7930 interoperable addresses for cross-chain.

**ERC-6551 (Token-Bound Accounts)** — orthogonal but load-bearing:
each NFT gets a deterministic smart contract account via a singleton
registry (create2-derived address). The NFT holder controls the
bound account. Means **every agent NFT automatically has its own
wallet** without us deploying per-agent contracts.

**My read of the trade-off:**
- 8122 is lighter and ships sooner. It's the "minimal" baseline.
- 8004 is the proper standard for the "Shopify-for-agents" endgame
  (discovery + trust + validation), and being ERC-721-based makes it
  6551-compatible out of the box.
- We can start with 8122-shaped storage and migrate to 8004 once
  reputation/validation matter; nothing locks us in either direction.

**Concrete v1 contract:** ERC-8122 surface (`register / ownerOf /
setMetadata`) on Tempo testnet, with metadata key `subdomain` carrying
the chosen name. The `name → owner` reverse lookup we need for "is
this taken?" is just iterating `setMetadata` events, or storing a
secondary mapping. Easier: combine 8122's `agentId` with an extra
`name → agentId` map in our deployment.

**Why "registry on-chain even when reading from the browser":** every
visitor's wasm bundle hits Tempo RPC directly to read; only the OWNER
needs to send a tx (to register/transfer). Reads are free and
permissionless. No backend, no API key, no central failure point.

Sketch (combining 8122 surface + a name index):

```solidity
// LocalharnessRegistry.sol — sketch (combines 8122 shape)
interface IMinimalAgentRegistry {
    function register(address to) external returns (uint256 agentId);
    function ownerOf(uint256 agentId) external view returns (address);
    function setMetadata(uint256 agentId, bytes32 key, bytes calldata value) external;
}

contract LocalharnessRegistry is IMinimalAgentRegistry {
    mapping(uint256 => address) public ownerOfId;
    mapping(string => uint256)  public idOfName;     // the "is it taken" map
    mapping(uint256 => string)  public nameOfId;
    mapping(uint256 => mapping(bytes32 => bytes)) public metadata;
    uint256 public nextId;

    event Registered(uint256 indexed agentId, address indexed owner, string name);
    event MetadataSet(uint256 indexed agentId, bytes32 indexed key, bytes value);

    function registerName(address to, string calldata name) external returns (uint256) {
        require(idOfName[name] == 0, "taken");
        require(_isValid(name), "invalid name");
        uint256 id = ++nextId;
        ownerOfId[id] = to;
        idOfName[name] = id;
        nameOfId[id] = name;
        emit Registered(id, to, name);
        return id;
    }

    function setMetadata(uint256 agentId, bytes32 key, bytes calldata value) external {
        require(msg.sender == ownerOfId[agentId], "not owner");
        metadata[agentId][key] = value;
        emit MetadataSet(agentId, key, value);
    }

    // ... validation, transfer, ERC-6909/8048 conformance per the
    // EIP. The above is the load-bearing surface for "claim a name".
}
```

**ERC-6551 layer:** once `LocalharnessRegistry` is an ERC-721 (or
ERC-6909 implementing the right interfaces), every registered name
automatically gets a deterministic 6551 account via the singleton
6551 registry (already deployed on most chains). That account is the
**agent's wallet** — can hold tokens, sign txs, pay/receive x402 or
MPP. No new deployment per agent.

The original handwritten sketch below is kept for reference but is
*not* what we should ship — the EIP-aligned version is the target.

```solidity

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

Revised after grounding L4 in ERC-8004/8122/6551 and confirming the
no-email constraint.

| M | Surface | Effort | Blocker | Notes |
|---|---------|--------|---------|-------|
| ~~**M5**~~ | ~~DNS + subdomain self-awareness in the app~~ | done | — | Shipped 2026-05-23. `tenant.rs` + apex chrome + per-device claim. |
| ~~**M5.1**~~ | ~~Apex→subdomain query-param hand-off so claim is one click~~ | done | — | Shipped 2026-05-23 — `?claim=1`. |
| ~~**M6 spike**~~ | ~~Compile `alloy` to wasm32~~ | done | — | Pivoted to `k256 + sha3` because alloy-consensus 1.0.22 trips on `serde::__private`. Same primitives under the hood, smaller dep tree. |
| ~~**M6**~~ | ~~Master wallet at apex, BIP-39 seed export~~ | done | — | Shipped 2026-05-23 in 0.8.0. `src/app/wallet_store.rs`. |
| ~~**M7 contract**~~ | ~~`LocalharnessRegistry.sol`~~ | done | — | Deployed 2026-05-23 at `0x42c8D4EaF99bA80F6B6FCA8E163E077D9FC2F9db` on Tempo Moderato. |
| ~~**M7 read-side**~~ | ~~Bundle reads registry via JSON-RPC~~ | done | — | `registry::check_name` + `registry::owner_of_name`. Hand-rolled ABI encoding + JSON-RPC over reqwest. |
| ~~**M7 write-side**~~ | ~~Bundle writes claim tx, signed by master wallet~~ | done | — | `registry::claim_name` with hand-rolled RLP + EIP-155 legacy tx envelope. Faucet bootstrap via `tempo_fundAddress` before first claim. |
| ~~**M8 iframe-signer**~~ | ~~`?signer=1` at apex hosting the wallet~~ | done | — | Shipped 2026-05-23 post-0.8.0 (Vercel only, no crates.io bump). `src/app/signer.rs`. Trusted-origin check `*.localharness.xyz` + `localhost`. Domain-separated digest `localharness-auth-v0:` + nonce. |
| ~~**M8 verify**~~ | ~~Subdomains verify via iframe~~ | done | — | `src/app/verify.rs` creates hidden iframe, postMessage sign-challenge with 5s timeout. `kick_verification` runs after `paint_tenant` and updates the chrome's verify pill. |
| ~~**M8 visitor lockdown**~~ | ~~Hide write affordances for non-owners~~ | done | — | `templates::visitor_banner` swapped into `#input-region` when verify resolves to Visitor. |
| **M9** | ERC-6551 token-bound account exposed on every registered name → that's the agent's wallet | **needs ERC-721 upgrade** | M7 (deployed) | Current registry is **not** ERC-721 — 6551 expects ERC-721 NFTs. Either (a) upgrade the registry to ERC-721 + redeploy + migrate, or (b) deploy a custom token-bound-account derivation. (a) is more standards-aligned. Either way it invalidates the names currently registered against the existing contract. |
| **M10** | x402 or MPP payment hooks: pre-tool-call gate that requires payment | large | M9 + real demand | Whatever the user-tier story turns out to be. |
| **M11** | ERC-8004 expansion: reputation + validation registries on top of M7's identity registry | large | M9 + multi-party usage | When agents start consuming each other's services. |
| **M12** | At-rest encryption (wallet-derived sym key over OPFS contents) | medium | M8 + real threat | Don't ship until OPFS visibility matters. |

**No L7 "cross-device sync server" entry anymore** — the wallet IS
the sync. Export seed phrase from device A, import on device B.
Registry state is on-chain and globally readable. Per-subdomain OPFS
contents (chat history, files) are the only thing that stays local;
those we treat as per-device working state, not authoritative.

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

1. **Which crate stack on wasm32.** `tempo-x402` workspace is mostly
   server-side (actix, tokio, wasmtime); `tempo-x402-identity` likely
   needs porting. `alloy` with `signer-local` is the canonical
   browser-friendly path and is what M6 spike should try first. If
   that fails, `k256` + hand-rolled signing is the fallback (smaller,
   less ergonomic).
2. **Wallet UX for non-crypto users.** "Generate wallet" → "back up
   your phrase" → "actually let me skip" → user loses access on next
   device. Need a forcing function or a clear "you're on your own"
   warning. The Phantom model is the right inspiration.
3. **Bootstrap gas.** First-time user has zero TMP. Options:
   - Operator-funded faucet endpoint we hit on first claim (centralised,
     rate-limit-spammable)
   - Public Tempo faucet — instructions, paste the address yourself
   - Sponsored / meta-transactions where someone else pays gas (EIP-2771).
3. **Squatting.** ERC-8004/8122 don't prevent it. Options: one-per-key
   limit (naive but ships), reserved-names allowlist for early users,
   pay-per-name in mainnet phase. Defer the policy decision.
4. **Apex login.** Today the wallet would live in apex's OPFS. If a
   visitor lands on `john.localharness.xyz` with no apex history,
   they need to bounce through apex to authenticate. iframe-signer
   pattern resolves this without requiring full redirects.
5. **What if Tempo RPC is down on mount?** App should degrade
   gracefully — show "registry unreachable; using cached state" and
   still let the user use the local OPFS data. Optimistic local
   reads with eventual on-chain reconciliation.
6. **Discovery.** How does anyone find John's agent if they don't
   know the subdomain? Eventually: a directory page on the apex,
   reading registry events. Defer.
7. **ERC-8004 vs 8122 final pick.** Ship M7 with 8122 surface; reach
   for 8004 (ERC-721 identity, reputation, validation) when
   agent-to-agent commerce starts mattering. The two are migrate-able
   because metadata fields are extensible.
8. **MPP vs x402.** User stated preference for MPP (Stripe / Tempo
   ecosystem) over x402 (Coinbase / Base). The tempo-x402 workspace
   is named after x402 but built on Tempo; whether the actual MPP
   wire protocol has its own Rust crate is unverified. Resolve when
   M10 starts.

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
