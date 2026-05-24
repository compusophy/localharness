# `localharness` — platform-layer design (subdomains, wallets, registry)

> Written 2026-05-23 as a planning doc; the layers it sketched
> shipped in 0.8.0 → 0.10.0. Now serves as the architecture
> reference for the platform stack on top of the SDK (0.2.x–0.6.x
> plan lives in `DESIGN.md`). The phase table at the bottom marks
> what's done; the "What's actually next" section after it covers
> the frontier.

## Goal in one sentence

`localharness` is a self-sovereign agent platform: each user owns
their own subdomain, their own data, and their own keys — with the
operator running zero per-user infrastructure beyond a thin name
registry contract deployed once.

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

## What shipped

Through 0.10.0 — released 2026-05-23, live at
[`localharness.xyz`](https://localharness.xyz/). Diamond proxy
address (immutable for the project's lifetime):
`0xed7a2d170ab2d41721c9bd7368adbff6df0c656d`.

| Layer | Where it lives | Notes |
|-------|----------------|-------|
| Wildcard subdomains | `src/app/tenant.rs` + Vercel DNS | `Apex` / `Tenant(name)` / `Other(raw)` classification on mount. Per-origin OPFS = per-subdomain sandbox, free. |
| Apex `?claim=1` hop | `src/app/events.rs::run_apex_claim` | One-click apex→subdomain. |
| Master wallet | `src/app/wallet_store.rs` + `src/wallet.rs` | k256 + sha3 + BIP-39. Persisted to apex OPFS at `.lh_wallet` as the 12-word phrase. Show/hide + import flow on apex. |
| Diamond proxy | `contracts/src/Diamond.sol` + facets | EIP-2535. Cut/Loupe/Ownership/Registry/ERC721/TBA facets. New facets add without churning the address constant. |
| Registry contract | `contracts/src/facets/LocalharnessRegistryFacet.sol` | `register / ownerOfName / setMetadata / …` + on-chain name validation. Mints emit ERC-721 Transfer events. |
| Read-side RPC client | `src/registry.rs` (public, `feature = "wallet"`) | Hand-rolled JSON-RPC + ABI encoding. `check_name`, `owner_of_name`, `tba_of_name`, `list_owned_tokens`. |
| Write-side (claim) | `src/registry.rs::claim_name` | RLP + EIP-155 legacy tx. Faucet bootstrap via `tempo_fundAddress` before first claim. |
| Iframe signer | `src/app/signer.rs` (apex `?signer=1`) | postMessage signing service. Domain-separated `keccak256("localharness-auth-v0:" || nonce)`. Trusted origins: `*.localharness.xyz` + `localhost`. |
| Subdomain verify | `src/app/verify.rs` (`kick_verification`) | Hidden iframe → sign-challenge → recover address → compare to `ownerOfName`. 5s timeout. Pill in header reflects state. |
| Visitor lockdown | `src/app/templates.rs::visitor_banner` | `#input-region` swap when verify resolves to Visitor. Read-only browsing. |
| ERC-721 facet | `contracts/src/facets/ERC721Facet.sol` | Every name is an NFT. Standard surface + Metadata extension. `tokenURI` → `https://<name>.localharness.xyz/`. |
| ERC-6551 stack | `contracts/src/erc6551/` + `contracts/src/facets/TbaFacet.sol` | Vendored reference registry + CALL-only account impl. `tokenBoundAccount(id)` + `tokenBoundAccountByName(name)` on the diamond. Every name's wallet is counterfactual + deterministic. |
| "Your agents" panel | `src/app/templates.rs::agents_list` | Iterates `1..nextId`, filters by `ownerOf`, surfaces name + tokenId + TBA. |
| TBA pill in tenant chrome | `src/app/templates.rs::tba_pill` | 💰 link to the agent's wallet on the block explorer. |
| Public SDK surface | `pub mod wallet` + `pub mod registry` (0.10.0) | Off-bundle consumers can query/claim from native too. |

**No L7 "cross-device sync server."** The wallet IS the sync.
Export seed phrase from device A, import on device B. Registry
state is on-chain and globally readable. Per-subdomain OPFS contents
(chat history, files) are per-device working state, not authoritative.

## What's actually next

In rough order of leverage:

1. **MPP / x402 payment hooks.** Pre-tool-call gate that requires a
   payment to the agent's TBA before the LLM call runs, or an
   agent-pays-agent flow over Stripe's MPP (preferred) or Coinbase's
   x402. Either fits behind the existing `Hook` trait. The on-chain
   plumbing exists; this is wiring + UX. Open question: where do
   funds come from on first-call (faucet, master wallet sweep, both)?
2. **TBA-driven actions in the bundle.** UI for "let your agent
   send this transaction from its TBA." Master wallet signs a
   TBA.execute payload via the existing iframe signer; bundle wires
   the RPC. Contract surface is done.
3. **ERC-8004 reputation + validation facets.** Two more facets cut
   into the diamond — `ReputationFacet` (signed feedback storage)
   and `ValidationFacet` (validator stake escrow + re-execution
   requests). The standard's mostly-finalised; vendoring the
   reference impl into `contracts/src/erc8004/` is the path.
4. **Second backend** (Anthropic, OpenAI, local). The
   `Connection` / `ConnectionStrategy` abstractions have been
   waiting for a non-Gemini implementation to validate them. Should
   be straightforward (the abstraction is exactly the right shape).
5. **Tool-call activity in restored transcripts.** `TranscriptEntry`
   drops FunctionCall / FunctionResponse on replay — the agent's
   context is correct but the user can't see prior tool use.
   Either project tool turns into the transcript or surface them
   as collapsed "(N tool calls)" stubs.
6. **At-rest encryption.** Wallet-derived sym key over OPFS contents
   so an XSS-equivalent attack on an origin can't trivially
   exfiltrate. Adds UX friction; don't ship until the threat is real.

---

## Apex marketing site

Shipped differently than originally sketched. There's only one
`web/index.html` — the same bootstrap shell loads on both apex and
every subdomain. The wasm bundle classifies the hostname on mount
(`src/app/tenant.rs`) and renders one of three chrome variants
from the same `src/app/templates.rs`:

- `Host::Apex` → `templates::apex(host, wallet_addr)` — wallet
  panel, claim form with live availability check, "your agents"
  list, footer.
- `Host::Tenant(name)` → `templates::chrome(host)` if the local
  owner marker exists; `templates::unclaimed(host, name)` otherwise.
- `Host::Other(_)` → `templates::chrome(host)` (Vercel previews,
  localhost — full app, no verification).

No Vercel host-based rewrite needed; same `index.html` for everything.

---

## Open questions (still open)

1. **Bootstrap gas at scale.** Today the apex form auto-faucets the
   master wallet via `tempo_fundAddress` before the first claim. Fine
   for testnet (which the operator runs the faucet for); on mainnet
   somebody has to pay. Options when that day comes:
   - Operator-funded faucet endpoint (centralised, rate-limit-spammable)
   - Public faucet with instructions
   - Sponsored / meta-transactions via EIP-2771
2. **Wallet UX for non-crypto users.** Today the seed-phrase reveal
   is collapsed behind a confirm — easy to skip, and skipping costs
   you everything on the next device. Need a forcing function or a
   sharper warning before the first claim succeeds.
3. **Squatting.** No anti-squatting mechanism today (was just
   one-per-address until ERC-721 dropped that constraint). Options:
   reserved-names allowlist for early users, pay-per-name on mainnet,
   reputation-weighted reclaim periods. Policy decision, not a code
   one.
4. **Tempo RPC outage.** Bundle errors out today if the RPC is down.
   Should degrade gracefully — show "registry unreachable, using
   cached state" and trust the legacy local-OPFS marker as fallback.
5. **Discovery.** No directory page yet — you have to know a
   subdomain to visit it. A future apex page could read registry
   events (`Registered` topic) and render a public agent listing.
6. **MPP vs x402.** User-stated preference: MPP (Stripe ecosystem)
   over x402 (Coinbase). The `tempo-x402` workspace mostly hosts
   server-side Rust; whether MPP has its own Rust crate worth
   reusing is unverified. Resolve when the payment-hook design
   starts.

Each step ships on its own. M5 = steps 1–3. M6 = step 4. M7 = steps
5–6. M8 = steps 7–8.

---

## What I will NOT do without explicit go-ahead

- Deploy a NEW smart contract to mainnet — testnet deploys via the
  scratch deployer wallet are fine and have shipped already.
- Buy / register / DNS-modify any domain (you control localharness.xyz).
- Add a "premium" tier that uses an operator-funded LLM API key.
- Pull in a heavy framework (Leptos, Yew) for the wallet UI — same
  rule as the rest of the app, maud + HTMX swaps.
- Persist anything beyond the user's own OPFS without their consent.
- Run any background process or scheduled job.

Once a direction is greenlit I run it autonomously per
[[feedback-execution-autonomy]] — but the items above need individual
approval because each one is a meaningful architectural or
operational commitment.
