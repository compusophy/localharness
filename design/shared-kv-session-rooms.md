# lh session: shared encrypted KV rooms (GitHub #22)

## Problem

When two+ agents collaborate over `call_agent`/`mcp-call`/`--pay`, the only way to carry shared state today is to re-send it in every call payload (full context in each prompt). That bloats every metered request, costs $LH per byte of re-sent context, and has no shared mutable surface — there is no "scratchpad both agents read and write." Issue #22 asks for an **encrypted, ephemeral, shared key-value store** that a small set of invited peer agents sync against, so a call carries a *room id + a few keys* instead of the whole state. API surface requested: `lh session create / join / set / get`.

## What already exists (honest reuse audit)

The repo has a **near-complete P2P substrate**, but it is purpose-built for "one owner syncs their OWN devices," not "distinct agents share a room." Reusable vs. not:

**Reusable as-is**
- `SignalingFacet` (`contracts/src/facets/SignalingFacet.sol`): the **room/topic model already exists** — `roster[topic]` (presence) + `inbox[addr]` (append-only mailbox). A topic IS a room. `announce`/`peersOf`/`postSignal`/`inboxOf`/`clearInbox`/`leave`.
- `TeamFacet` (`contracts/src/facets/TeamFacet.sol`): **consent-gated membership** (`createTeam`/`invite`/`accept`/`isMember`/`membersOf`/`teamsOf`). This is exactly the room ACL for `create`/`join`. Topic for a team = `keccak256("localharness.team" ‖ teamId)` (already in `registry::team_topic`).
- `src/app/webrtc.rs` (`Peer`): pure WebRTC transport (STUN, non-trickle ICE, negotiated data channel). Identity-agnostic — fine for cross-owner peers.
- `src/signaling_seal.rs`: native-tested **signed+sealed envelope** (`seal_envelope`/`open_envelope`, sender-auth + recipient-binding + replay protection) — reusable verbatim for any blob, not just SDP.
- `src/app/encryption.rs`: ECIES (`ecies_seal`/`ecies_open`) and raw-key AES-256-GCM (`seal_with_raw_key`/`open_with_raw_key`).
- `src/sharedfs_reconcile.rs`: the **convergent-merge pattern** (deterministic, symmetric, native-tested). Directly adaptable from files→KV entries.
- `src/registry/signaling.rs`: ABI codecs for announce/postSignal/inbox/peers + `decode_addr_ts_bytes_array`.

**NOT reusable / blocking gaps**
- `src/app/shared_fs.rs` is **seed-keyed at rest** (`sharedfs_key_from_entropy`) and lives in **apex OPFS** — it only works for ONE owner's own data. A KV room shared with *another* agent (different seed) cannot use this key or this store.
- `src/app/teams_sync.rs` hard-assumes **one shared seed across peers** (`master == owner_key`, the devices topic). Cross-agent rooms have no shared seed.
- `SignalingFacet.announce` **only owner-gates the *devices* topic**; for team topics it enforces mere self-consistency (`recover==ephemeral`) — explicitly "full member-gating is a follow-up." A KV room must close this (announce gated by `TeamFacet.isMember`).
- The **entire P2P teams stack is "COMPILE-VERIFIED ONLY"** — never run end-to-end across two real browsers (stated in every module doc). Building on it means we are the first to exercise it live.
- **WebRTC requires both peers online simultaneously.** Headless CLI agents (`localharness call`) and the "carry a room id across async calls" use-case need a store that is **readable when the counterparty is offline** — pure P2P cannot provide that.

## Core design decision (options-first)

**Option A — P2P-only (WebRTC, both online).** Room = a TeamFacet team + signaling topic; state lives only in each peer's OPFS, reconciled over the data channel when both are connected. Reuses the most code. **Rejected as the primary path:** breaks the headless/async use-case that motivates #22 (the point is to *not* need a live co-session).

**Option B — new `KvRoomFacet` (on-chain durable op-log).** A thin facet stores per-room **encrypted CRDT ops** so any member can `get` the latest state without the other being online. State is end-to-end encrypted under a room key; the chain sees only ciphertext. Gas-bounded like FeedbackFacet.

**Option C (RECOMMENDED) — SignalingFacet inbox as the op-log + TeamFacet ACL, NO new facet.** Reuse the existing `inbox[topic-address]` as an append-only **encrypted op log addressed to the room**, with `TeamFacet` as the ACL and a per-room symmetric key distributed via ECIES on join. `set` = append a sealed op; `get` = fold the inbox through the convergent reducer. This adds **zero new on-chain surface** (only needs the *announce member-gate* follow-up on SignalingFacet), is durable/async (inbox persists), and matches "existing infra before new" — the credit proxy/chain is already the bridge.

I recommend **C**, with **B's `KvRoomFacet` as a fallback only if** inbox semantics prove too coarse (e.g. we need per-room `clearInbox` without nuking a device's SDP inbox — see Risks). The rest of this doc specs C.

## Design (Option C)

### Room model
- A **room** is a `TeamFacet` team. `lh session create <name>` → `createTeam`; returns `teamId`. The creator is the first member.
- **Room key**: on create, generate a random 32-byte AES key `K_room`. The creator stores it locally (`.lh_session/<teamId>.key`). On `invite`, the inviter ECIES-seals `K_room` to the invitee's identity pubkey and posts it to the invitee's signaling inbox (a `KeyGrant` op). On `accept`, the invitee opens it and persists `K_room`. This is the ONLY thing that gates read access — TeamFacet gates *who can be granted*; `K_room` gates *who can decrypt*.
- **Room address** (the inbox the op-log is addressed to): a deterministic non-EOA address `room_addr = address(keccak256("localharness.kvroom" ‖ teamId))`. All members `postSignal(room_addr, sealed_op)` and `inboxOf(room_addr, cursor)` to read. (Reuses `inbox` as a multi-writer log; existing inbox is per-recipient, this just uses a synthetic recipient.)

### Op format (CRDT — LWW-element map)
Each KV op is a sealed blob in the inbox:
```
op = { key: String, value: Vec<u8> | TOMBSTONE, lamport: u64, writer: addr20 }
```
- `value` AES-256-GCM-sealed under `K_room` (`encryption::seal_with_raw_key`); the op envelope is `signaling_seal::seal_envelope`-wrapped (writer-authenticated, room-bound) so a non-member can't forge an op even if they learn `room_addr`.
- **Conflict resolution = LWW by `(lamport, writer)`** — pure, deterministic, symmetric (same shape as `sharedfs_reconcile`'s greater-hash tiebreak: higher lamport wins; tie → lexicographically greater `writer` wins). This is a new pure module `src/kv_reduce.rs`, native-tested exactly like `sharedfs_reconcile.rs`. `get(key)` = fold the inbox ops for that key through the reducer; `list` = fold all.
- **lamport clock**: each writer keeps a local counter; on `set` it uses `max(seen)+1`. Convergence holds because the reducer is order-independent over the op set.

### Encryption / consistency summary
- **Confidentiality**: chain sees only `seal_envelope(seal_with_raw_key(K_room, value))`. Key distribution is ECIES-to-member.
- **Authenticity**: every op is signed by the writer's key, recipient-bound to `room_addr` (replay-proof across rooms) via the existing envelope.
- **Authorization**: read = possess `K_room` (only granted to accepted members); write = TeamFacet membership (enforced when we add the member-gate to `announce`/optionally to a future `postSignal` check; for v1 the envelope-signature + `isMember` client check suffices, since a forged op without `K_room` is undecryptable garbage and is dropped by the reducer).
- **Consistency**: eventual; LWW-map CRDT → all members converge to the same map after seeing the same op set. No locking, no online requirement.
- **Ephemerality**: ops carry a TTL (the room is "ephemeral"); the reducer ignores ops older than the room's TTL, and a creator-only `lh session close` posts a tombstone-all marker + `leave`s. (Hard inbox reclamation is the one place a `KvRoomFacet` would be cleaner — see Risks.)

## File-by-file plan

**Contracts (minimal)**
- `contracts/src/facets/SignalingFacet.sol` — add the deferred **team-topic member-gate**: for a team topic, recover the announce sig and require `TeamFacet.isMember(teamId, signer)` (the facet can read team storage in-diamond). Needed so a room roster is trustworthy. NO new facet for v1.
- (Fallback B only, if adopted) `contracts/src/facets/KvRoomFacet.sol` + `LibKvRoomStorage.sol` + `script/AddKvRoomFacet.s.sol` — `appendOp(teamId,bytes)`, `opsOf(teamId,fromIndex)`, `clearRoom(teamId)` gated by `isMember`. Mirrors SignalingFacet shape.

**Pure cores (native-testable, no wasm/no chain)**
- `src/kv_reduce.rs` (NEW) — `KvOp`, `reduce(ops) -> BTreeMap<String,Vec<u8>>`, LWW `(lamport,writer)` tiebreak, tombstones, TTL filter. Convergence/symmetry/idempotence unit tests (clone the structure of `sharedfs_reconcile.rs` tests).
- `src/kv_room.rs` (NEW, `feature=wallet`) — op encode/decode + `seal_op`/`open_op` (compose `seal_with_raw_key` + `seal_envelope`), `room_address(teamId)`, `key_grant_seal`/`open`. Native round-trip + tamper tests.
- `src/lib.rs` — `pub mod kv_reduce; #[cfg(feature="wallet")] pub mod kv_room;`.

**Registry (`feature=wallet`)**
- `src/registry/kvroom.rs` (NEW) — `room_address`, sponsored `post_op`(=postSignal to room_addr), `read_ops`(=inboxOf), `grant_key`(postSignal to member), team helpers re-exported. Reuse `signaling.rs` codecs.
- `src/registry/mod.rs` — re-export.

**CLI (`src/bin/localharness/`)**
- `session.rs` (NEW) — `lh session create <name>` / `join <teamId>` (accept invite + open key grant) / `invite <teamId> <agent>` (createTeam invite + seal `K_room`) / `set <teamId> <key> <value>` / `get <teamId> <key>` / `list <teamId>` / `close <teamId>`.
- `main.rs` — add `mod session;`, dispatch `Some("session")`, USAGE block, and add `"session"` to the `usage_documents_every_command` test list.

**Browser (`src/app/`)** — phase 3, optional
- `src/app/session_room.rs` (NEW) — owner-side `set`/`get` over the same registry path; an admin-panel KV viewer (HTML-template + innerHTML swap, no DOM).
- Wire a `share_state(key,value)` / `read_shared(key)` agent tool in `chat/tools/` so a running agent reads/writes the room mid-turn instead of re-sending context (this is the payload-bloat win).

**Docs (the 5-surface SOP)**
- `web/llms.txt` (room API for agents), `web/skill.md`, `CLAUDE.md` (on-chain + CLI lines), `CHANGELOG.md`, `contracts/README.md` (SignalingFacet member-gate).

## Risks / open questions
- **Untested substrate.** The P2P teams stack has never run live. Even Option C leans on `SignalingFacet` semantics that work but are unexercised at scale. Mitigation: Option C avoids WebRTC entirely (pure inbox + chain), so we sidestep the most fragile part (ICE/NAT) — a real win over reusing `teams_sync`.
- **Inbox reclamation.** `SignalingFacet.clearInbox` is **caller==recipient only**; a synthetic `room_addr` has no key, so a room's op-log can't be cleared on-chain (only TTL-filtered client-side). It grows unbounded. This is the strongest argument for the `KvRoomFacet` fallback (B), which gets a member-gated `clearRoom`. Decide before scaling.
- **Gas per op.** Inbox writes are SSTOREs (~7.6k gas/byte heuristic from CLAUDE.md). Keep values small; the *point* of KV is small shared state, not blobs. Document a byte cap.
- **Key rotation / revocation.** Removing a member can't un-share `K_room` they already hold. v1 accepts this (rooms are ephemeral); real revocation = rotate `K_room` + re-grant to remaining members (Phase 4).
- **announce member-gate cut.** Adding the `isMember` check to SignalingFacet is a `diamondCut` (owner-only) — the assistant runs it; coordinate with the live "sync my devices" path (devices topic unchanged).
- **lamport monotonicity across a wiped CLI cache** — a member who loses local state restarts its counter; collisions resolve via the `writer` tiebreak, so it's safe but may briefly lose to a stale-high counterpart. Acceptable for ephemeral rooms.

## Phased build order
1. **Pure core**: `src/kv_reduce.rs` + tests (CRDT convergence, native). No chain, no wasm. This is the load-bearing correctness proof.
2. **Crypto/wire**: `src/kv_room.rs` (seal/open op + key-grant) + tests. Reuse `signaling_seal` + `encryption`.
3. **Registry**: `src/registry/kvroom.rs` over existing SignalingFacet codecs. Sponsored `post_op`/`read_ops`/`grant_key`.
4. **Contract cut**: add the team-topic member-gate to `SignalingFacet` (and ship `KvRoomFacet` only if the reclamation risk forces B).
5. **CLI**: `session.rs` (`create/invite/join/set/get/list/close`) + dispatch + docs. **Dogfood E2E** between two real on-chain identities (the assistant's `claude.localharness.xyz` + a fleet persona) — this is also the first live exercise of the teams substrate.
6. **Browser + agent tool**: `session_room.rs` + `share_state`/`read_shared` tools so a running agent uses the room instead of re-sending context. 5-surface doc sync + CHANGELOG; routine release.