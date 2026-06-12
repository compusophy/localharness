# Unified Roadmap — compose, autonomy, economy

> **STATUS: SHIPPED (historical sequencing artifact).** This is the plan that got
> the project from compose (0.21) to the colony (0.32). The *dependency graph* and
> *risk taxonomy* are sound and worth keeping; the *phase clock is obsolete* —
> Track C (economy) is massively shipped (bounty/guild/voting-DAO/reputation/colony,
> 0.27–0.32), Phases 1–4 of compose+autonomy landed. STILL OPEN: Phase 0a (the
> present-contract display-spine refactor) and Phase 5 (the unattended fix-agent /
> `dial=propose` rung). For forward work read `host-compose.md` + `autonomous-loop.md`.

Status: build plan. Synthesizes the three frontier designs
([host-compose](../host-compose.md), [autonomous-loop](../autonomous-loop.md),
[economy-reputation](economy-reputation.md)) and their adversarial critiques into
one sequenced, dependency-ordered plan. Every claim below was verified against
the live tree (`display.rs:568` present-contract, `registry.rs:534` persona
seam, `LibX402Storage.sol:13` authState shape, `agent.rs:426` write-guard).

The organizing principle: **three tracks share ONE spine — the sandbox/isolation
boundary.** host::compose isolates untrusted *module code*; the autonomous loop
isolates untrusted *execution*; the economy makes both *cost money*, which makes
every isolation hole a financial exploit. We build the isolation primitive once,
at the bottom, and every track stacks on it.

---

## 1. Dependency graph

```
                         ┌─────────────────────────────────────────────┐
                         │  SPINE: the isolation/sandbox boundary        │
                         │  (present-contract + per-child runtime ctx +  │
                         │   dispatcher-level policy hook)               │
                         └─────────────────────────────────────────────┘
                            │                    │                    │
          ┌─────────────────┘                    │                    └──────────────────┐
          ▼                                       ▼                                       ▼
  TRACK A: host::compose              TRACK B: autonomous loop            TRACK C: economy/reputation
  (module code isolation)             (execution isolation)               (value + trust, mainnet)
          │                                       │                                       │
  A0 viewport+present refactor         B0 read-only probe (sync,             C0 capability descriptor seam
     (NO-OP, test-pinned)                  no triggers, dispatcher              (hash-committed, verified)
          │                                  policy hook)                              │
  A1 second instance, hardcoded rect           │                            (everything below = 1.0/2.0,
          │                            B1 fleet-only feedback channel           DO NOT BUILD on testnet)
  A2 async fetch + Loading tile          + name-janitor (prereqs)                    │
          │                                       │                          C1 module revenue
  A3 pointer focus/routing             B2 sandboxed exercise (dial=exercise)     (compose settle-before-mount)
          │                              + spend ceiling at SIGNING boundary         │   needs: A3 + C0 + B-sandbox
  A4 host_compose ABI + budgets          + fs jail over CUSTOM tools                 │
     (max children/bytes/memory,                  │                          C2 X402 authState extension
      net allowlist, fuel)              B3 triage agent (dedup, on-chain          (bind from→to→value)
          │                                resolve facet)                            │   FATAL PREREQ for reputation
  A5 delete iframe compose.rs/embed.rs           │                          C3 ReputationFacet (proof-of-tx gated)
                                        B4 fix-agent / PR arc                        │
                                          (separate trust grant,            C4 sybil bond + marketplace index
                                           NOT on sandbox substrate)                 │
                                                                            C5 ValidationFacet (stake/slash) — 2.0
```

### The hard edges (what must precede what)

1. **The present-contract decision precedes everything in Track A.** Today
   `start_frame_loop` (display.rs:568) only calls `frame.call1()`; the cartridge
   itself calls `present()`. host::compose's core invariant ("only the host
   presents, once/frame") *inverts who presents* — and that inversion changes the
   single-cartridge path, not just the multi-child path. A0 must settle this and
   pin it with a real wasm-instantiation render test, or A1+ silently break
   existing fullscreen cartridges.

2. **Per-child runtime context precedes child isolation.** `set_pixel` etc.
   (display.rs:234) clip to the `FB_W/FB_H` *globals* and close over the
   thread-local `POINTER`/`STATE`. A "viewport param" does NOT give a child its
   own 64-slot state or focus-gated pointer. `build_host_display` must be
   re-architected to take a per-child *runtime context* (viewport + state cells +
   focus flag + parent-memory handle), not just a rect. This is the load-bearing
   refactor, larger than the design's "gains a viewport param" framing.

3. **host::compose unlocks the module marketplace — but ONLY the iframe-free
   version does.** The economy doc's §1.2 settle-before-mount is written against
   `compose.rs`'s *iframe* path, which the no-iframes rule forbids. Module revenue
   (C1) therefore depends on Track A landing first (A3 minimum: a real composited,
   pointer-routed module), not on the iframe path the economy doc inherits.

4. **The autonomous loop needs the execution-tool SANDBOX before ANY unattended
   action — and the sandbox must be enforced at the dispatcher, not the prompt.**
   agent.rs:426 only inspects `effective_tools()` (the BuiltinTool set); custom
   `ClosureTool`s (every `qa_*` tool) bypass `has_write`, so the runtime's own
   safety guard passes with zero policy. B0 must register a real pre-tool-call
   policy hook at the `ToolRunner` level *before* any write tool exists.

5. **Reputation needs real usage signal AND a storage change the economy doc
   wrongly calls "no change".** `authState` (LibX402Storage.sol:13) is
   `mapping(from => mapping(nonce => bool))` — it records that `from` consumed a
   nonce, NOT the `to`/`value`. The "only a paid counterparty can rate" gate (the
   doc's single most important sybil defense) is **satisfiable by 1-wei
   self-payment**. C3 (ReputationFacet) therefore depends on C2 (an X402 storage
   extension `authMeta[from][nonce] = {to, value}` + a `settle()` write), which is
   a real facet change — not the additive-only path the doc claims.

6. **The economy is mainnet-gated.** Per the locked 0.x→1.0→2.0 grammar and the
   no-double-digit versioning rule, NOTHING in Track C settles real value on
   testnet. C0 (the descriptor seam) is the only Track-C item that ships now,
   because it's purely additive, network-free, and forecloses nothing.

7. **The fix-agent arc (B4) breaks every isolation property of B0–B3.** It needs
   `gh` + repo write + the operator's git identity on a real machine — a
   categorically larger trust grant than jailed probes. It does NOT live on the
   sandbox substrate and must be sequenced last, behind an explicit operator gate.

---

## 2. Sequenced plan (phases, each shippable, each validated by the live feedback loop)

The now-live on-chain feedback loop (`submit_feedback` → `list_feedback`) is the
validation harness for every phase: each phase ends by exercising the new surface
and confirming the expected on-chain/visible result. Phases are ordered so each
is independently shippable and de-risks the next.

### Phase 0 — Spine: pin the contracts that all three tracks share (this week + next)

Three small, independently-shippable, mostly network-free changes that unblock
all three tracks and force the load-bearing decisions into the open.

- **0a. Present-contract + viewport refactor (Track A foundation).** See §3 — this
  is the FIRST BUILD. Introduce `ModuleViewport`, thread a per-child runtime
  context through `build_host_display`, decide present-ownership, pin with a wasm
  render smoke test. NO-OP at runtime for single cartridges.
- **0b. Dispatcher-level policy hook (Track B foundation).** Close the
  `agent.rs:426` gap: make the write-guard (or a new `qa_*` pre-tool-call hook)
  inspect custom `ClosureTool` names, not just `effective_tools()`. A network-free
  unit test asserts an unlisted tool name is hard-denied at dispatch. This is the
  safety boundary B0 stands on.
- **0c. Capability-descriptor seam (Track C foundation).** Add
  `CAPABILITY_LABEL = b"localharness.capability"`, `capability_descriptor_of`,
  `encode_set_capability`, modeled byte-for-byte on the existing
  `PERSONA_LABEL`/`persona_of`/`encode_set_persona` trio (registry.rs:534).
  **Store keccak256(payload), not the payload**, plus `verify_descriptor(token_id,
  served_payload)` that recomputes and returns mismatch — closing the
  payee-swap/freshness hole before any settle path can consume it. Network-free
  ABI-layout test mirroring `encode_set_persona_abi_layout`.

*Validation:* existing cartridges render byte-identical (0a); a denied custom tool
files no on-chain write (0b); a tampered served descriptor fails `verify_descriptor`
(0c). All three ship in normal commits + a deploy; no version bump.

### Phase 1 — Track A walk: composited children, no ABI, no agent surface

Stack on 0a in numbered increments, each independently testable.

- **1a.** Second `Instance`+`Memory` in a `MODULES` table, ticked in the
  compositor loop with a **hardcoded** bitmask rect (no fetch yet — embed test
  bytes). Implement the **snapshot-handles-then-tick + deferred-mutation-queue**
  pattern so a child `frame()` that mutates `MODULES` can't double-borrow the
  `RefCell` (the most likely first crash per the critique).
- **1b.** Async `app_wasm_of` fetch + `Loading`/`Failed` placeholder tiles.
  WASM_CACHE keyed by **content hash, not tokenId** (so an on-chain republish
  isn't silently masked).
- **1c.** Pointer focus hit-test (in the existing delegated `mousedown` listener,
  exposed as a `pub(crate) hit_test_focus`) + rect-local, focus-gated
  `pointer_x/y/down`.

*Validation:* claude.localharness.xyz composites the real bitmask `app.wasm`
byte-for-byte; clicking the panel toggles bits; clicking chrome doesn't leak.

### Phase 2 — Track B observe: trigger-free read-only probe

The autonomous loop's revised-first-step, hardened.

- **2a. `localharness probe` as a SYNCHRONOUS one-shot subcommand** (NOT a
  trigger/daemon — `triggers.rs` needs a standing `Connection`/dispatcher loop
  that a one-shot CLI exit kills before `every()` ever fires; defer triggers until
  there's a standing host). One generalist agent at `autonomy=observe` with
  read-only custom `ClosureTool`s registered via `with_tool` alongside
  `enabled_tools: Some(Vec::new())`.
- **2b. Tools that touch NO secrets and write NOTHING but feedback:** `qa_compile`
  (rustlite::compile on known-good + known-bad sources, assert ok/err), `qa_fetch`
  **restricted to an allowlist** (`<name>.localharness.xyz`, `llms.txt`,
  `skill.md`), `qa_chain` limited to registry fns that **provably exist**
  (`owner_of_name`, `public_face_of`, `persona_of`, `list_feedback`) — explicitly
  NOT `facet_count`, which is unimplemented. On failure, ONE `qa/v1 source=qa-smoke
  …` envelope via `submit_feedback_sponsored` under a `--as` fleet key.
- **2c.** The dispatcher hook from 0b hard-denies any tool not on the read-only
  allowlist — safety in the dispatcher, not the prompt.

*Validation:* the probe finds a real bug (e.g. a rustlite source that panics the
compiler instead of erroring) and the `qa/v1` envelope appears in `list_feedback`.

### Phase 3 — Track A run + Track B prereqs (parallel)

- **3a (A).** `host_compose` import module + rustlite host-call signatures
  (resolve through the existing `HostCall`/`register_import` path; wire the
  parent-memory handle for string reads exactly as `host_net` does). **v1
  resource gates, NOT deferred:** max-children cap, per-child wasm byte cap,
  aggregate-Memory budget, per-frame fuel/time box, and a `host_net` URL allowlist
  per child (network-identity collapse: a composed module otherwise beacons under
  the COMPOSITOR's origin). Slot reclamation (free `None` slots) so a long-lived
  window-manager compositor doesn't leak.
- **3b (A).** Delete `compose.rs`/`embed.rs`, repoint `?compose=` to a synthetic
  iframe-free compositor parent.
- **3c (B).** Fleet-only feedback channel (separate metadata key or facet) so
  `qa_report` can't flood the human-facing log — the sponsor pays the fleet's gas,
  so "gas is the spam filter" does NOT apply to it. Plus a prefix-keyed
  name-janitor (`enumerate-by-prefix` → release leaked `qa-<run>-*` names), since
  a sponsored `release` can OOG mid-run and a `finally` won't run on a kill.

*Validation:* an agent-authored compositor mounts 3 named modules; a 4th over the
cap is refused; the fleet writes to its own channel, not the human log.

### Phase 4 — Track B exercise + triage (dial past observe)

- **4a.** `dial=exercise`: sandboxed writes (`qa_create`/`qa_publish`/`qa_call`)
  against disposable names. **Spend ceiling enforced at the sponsor-SIGNING
  boundary** (sponsor.rs), not as a pre-submit check the tool performs — a buggy
  or looping fuzz tool otherwise drains the low-budget sponsor key. **fs jail must
  wrap the custom tools' raw `std::fs`/`reqwest`/`registry` calls**, not only the
  8 builtins (the design's jail fences a surface the qa tools don't use).
- **4b.** `qa-triage` agent + an on-chain `resolve(entryIndex)` facet so fixed
  bugs stop re-surfacing every triage pass (recurrence-as-signal is otherwise
  corrupted by un-expirable history). Envelope-shape validation before any repro
  string is consumed (an attacker can plant a crafted `qa/v1 … repro:` in the
  permissionless feedback log).

*Validation:* exercise finds a gas/publish bug; triage dedups it to one ranked
item; `resolve` clears it; the next pass doesn't re-list it.

### Phase 5 — Track B fix-agent (separate trust grant, gated)

- **5a.** `dial=propose`: fix-agent reproduces a repro **inside the jail**, writes
  a diff + regression test, runs `cargo test` + the wasm guardrail, opens a PR via
  `gh`. **Never merges.** This leg needs repo write + operator git identity and
  does NOT live on the sandbox substrate — it ships behind an explicit, separate
  operator opt-in, sequenced last.

*Validation:* a `qa/v1` finding produces a PR with a failing-then-passing
regression test; a human merges.

### Phase 6+ — Track C (mainnet-gated, post-1.0, post-pilot)

Build ONLY when (a) 1.0 shipped on mainnet (value exists) AND (b) the economy
pilot shows someone cares. Order: C1 module revenue (needs A3+C0) → C2 X402
authState extension → C3 ReputationFacet → C4 sybil bond + marketplace index →
C5 ValidationFacet (stake/slash, 2.0). C2 is a FATAL prerequisite for C3 and must
be done append-only (cross-facet storage coupling is an upgrade footgun).

---

## 3. The single highest-leverage FIRST BUILD (this week)

**Phase 0a: the present-contract + per-child-runtime-context refactor in
`src/app/display.rs`, shipped as a runtime NO-OP, pinned by a real
wasm-instantiation render test.**

Why this one, concretely:

- It is the load-bearing mechanism **every** Track-A increment depends on, and
  Track A is what unlocks the module marketplace (the economy's first value flow).
- It is independently buildable and testable against current code with zero
  on-chain/registry/economy surface touched.
- It forces the decision the design *hides*: today `start_frame_loop`
  (display.rs:568) calls only `frame.call1()`; the cartridge presents itself. The
  "viewport param" framing makes step 1 look identity-preserving — but it only is
  because it leaves present alone, and the moment a second child is added the
  present-ownership MUST invert, silently changing the single-cartridge path. Do
  the risky refactor first, in the open, under test.

The build, concretely:

1. Introduce `struct ModuleViewport { ox, oy, w, h }` and a per-child runtime
   context (viewport + own state cells + focus flag + memory handle). Thread it
   through `build_host_display` (display.rs:213) so `set_pixel`/`fill_rect`/
   `draw_char`/`draw_number`/`width`/`height` translate+clip to the viewport and
   `pointer_*`/`state_*` close over the per-child cells — NOT the `FB_W/FB_H`
   globals and the thread-local `POINTER`/`STATE` they close over today
   (display.rs:234, 335). The parent passes the full-screen viewport (identity
   transform).
2. In the SAME change, decide and implement present-ownership: move `present()`
   into `start_frame_loop` after `frame()`, make the `present` import a no-op, and
   prove existing fullscreen cartridges still render.
3. Add a wasm-instantiate + render smoke test (node/headless) that loads a real
   cartridge through the refactored builder with a full-screen viewport and
   asserts the framebuffer bytes equal the pre-refactor output. The cargo suite
   never instantiates wasm today (per the rustlite-codegen-validation memo), so a
   pure unit test cannot catch a viewport or present regression.

Do NOT touch `MODULES`, pointer focus, the `host_compose` import, or any registry
code in this step. This is the smallest increment that is both independently
buildable AND forces the present-ownership decision into the open instead of
deferring it to where it silently breaks single-cartridge rendering.

---

## 4. Cross-cutting risks (the security/sandbox spine)

The three tracks fail at the same seam: **an isolation boundary that holds for
memory/framebuffer but leaks on network, money, or storage.**

1. **Network-identity collapse (A + C).** A composed child's `host_net` sockets
   open from the COMPOSITOR's origin with no URL allowlist — a malicious
   on-chain `app.wasm` beacons/exfiltrates under e.g. claude.localharness.xyz's
   identity. The iframe at least isolated network origin per module; this design
   is *less* contained on network. **Gate: per-child `host_net` URL allowlist (or
   origin tagging) is a v1 requirement of A4, not deferred**, the moment
   `spawn_module` accepts an attacker-chosen name. The same allowlist discipline
   covers `qa_fetch` (SSRF/exfil if the probe is prompt-injected by content it
   fetches).

2. **Sponsor-key drain (A→C + B).** Every per-edge compose settle and every
   sandboxed `qa_create` is paid by the single low-budget sponsor key
   (`0x0AFf88…`, ~275k gas overhead/tx). An attacker composes a deep/wide graph of
   their own modules, or a looping fuzz persona mints names in a tight loop, to
   force N sponsorships. **Gate: spend velocity + a balance circuit-breaker
   enforced at the sponsor-SIGNING boundary** (sponsor.rs), not as a pre-submit
   check the tool/host performs. This is the one drainable resource and it is
   uncapped in all three designs.

3. **Custom-tool policy bypass (B).** agent.rs:426 checks only `effective_tools()`
   (BuiltinTool set); every `qa_*` `ClosureTool` is invisible to `has_write`, so
   the runtime's safety guard passes with zero enforcement, leaving the safety
   story as prompt-level honor-system the model can be talked past. **Gate:
   register a real pre-tool-call policy hook at the `ToolRunner` level (Phase 0b),
   prefix-checking sandbox names and dial-checking writes.**

4. **RefCell re-entrancy (A).** A child `frame()` (or async spawn completion) that
   mutates `MODULES` while the compositor loop holds a borrow double-borrow-panics
   the whole tab (single-threaded wasm can't deadlock but can double-borrow).
   **Gate: snapshot-handles-then-tick + a deferred mutation queue for
   spawn/close/move issued during a frame** (Phase 1a).

5. **Stale-bytes / unverified-descriptor trust window (A + C).** WASM_CACHE keyed
   by tokenId masks an on-chain republish (good OR malicious) until reload; the
   capability descriptor stores only a hash while the payload is served mutably
   off-chain, so a settle path reading price/payee from served bytes can be
   drained by a payee swap. **Gate: content-hash cache key (A); hash-committed
   descriptor + mandatory `verify_descriptor` before any settle consumes it (C0).**

6. **Untrusted repro as code-execution channel (B).** The feedback log is
   permissionless; a planted `qa/v1 … repro: <payload>` is executed by the
   autonomous fix-agent at `dial=propose`. **Gate: envelope-shape validation +
   the repro runs only inside the jail; the fix-agent leg is the separate trust
   grant of Phase 5, never on the probe substrate.**

7. **Broken anti-astroturf gate (C, fatal).** `authState` (LibX402Storage.sol:13)
   stores `(from, nonce)` only — the "only a paid counterparty can rate" gate is
   satisfiable by 1-wei self-payment to an attacker-controlled address. Any
   reputation built on current X402 storage is gameable from block one. **Gate: C3
   is hard-blocked on C2 (an X402 `authMeta[from][nonce] = {to, value}` storage +
   `settle()` write), done append-only — the economy doc's "X402Facet, no change"
   is wrong.**

8. **Live-diamond fuzzing (B).** A `qa-security` persona tasked to find
   destructive-tool/trust-boundary bypasses does so against the LIVE production
   diamond — there is no testnet-fork isolation. **Gate: scope qa-security to
   read-only + disposable-name targets until a fork target exists; never point a
   destructive-probe persona at production identities.**

The through-line: **build the isolation boundary once at the spine (Phase 0),
enforce money/network/storage limits at the lowest possible boundary (signing,
dispatcher, content-hash), and never let a higher track (economy) consume a lower
track's output (a composed module, a descriptor, a repro) without verifying it.**

---

## 5. Folded-in revised first steps (per critique)

- **host::compose →** Keep the viewport refactor but make it an explicit
  runtime-NO-OP, test-only change AND settle the `present()` contract in the same
  step (move present into `start_frame_loop`, make the import a no-op), proving
  existing fullscreen cartridges render byte-identically via a real
  wasm-instantiation smoke test — not a pure unit test, which can't catch a
  viewport/present regression. Touch no MODULES/pointer/ABI/registry code.
  **→ This is Phase 0a / the FIRST BUILD (§3).**

- **autonomous-loop →** Ship `localharness probe` as a SYNCHRONOUS one-shot
  subcommand (defer triggers/daemon), one generalist at `autonomy=observe` with
  read-only custom tools (`qa_compile`, allowlisted `qa_fetch`, `qa_chain` over
  provably-existing fns — NOT `facet_count`), a dispatcher-level pre-tool-call hook
  that hard-denies anything off the read-only allowlist (closing the
  custom-tool-policy-bypass), writing one `qa/v1` envelope on failure. Resolve a
  fleet-only feedback channel and a prefix-keyed name-janitor as prereqs before the
  exercise/fix arcs. **→ Phase 0b + Phase 2; prereqs in Phase 3c.**

- **economy-reputation →** Ship ONLY the capability-descriptor metadata seam,
  self-verifying: `CAPABILITY_LABEL` + `capability_descriptor_of` +
  `encode_set_capability` modeled byte-for-byte on the existing
  `PERSONA_LABEL`/`persona_of`/`encode_set_persona` trio, store
  keccak256(payload) (NOT the payload) with a `verify_descriptor` helper that
  recomputes and returns mismatch, plus ABI-layout + key-distinctness tests
  mirroring the persona tests. STOP there — do NOT touch X402Facet, add
  Reputation/Validation facets, or wire settle-before-mount; each depends on a
  missing escrow facet or the X402 storage change the doc wrongly calls "no
  change". Purely additive, network-free, forecloses nothing. **→ Phase 0c.**
