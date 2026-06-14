# SolidityLite — Design

> **UPDATE 2026-06-14 (loop tick 1): the #1 gating risk is RESOLVED.** A live probe
> (`examples/tempo_create_probe.rs`) deployed a contract via a **sponsored Tempo 0x76
> CREATE** on Moderato — receipt `status 0x1`, `contractAddress` surfaced
> (`0xe6366e…510b0a`), non-empty `eth_getCode`. `TempoTxBuilder::create()` (RLP-encodes
> the call's `to` as empty `0x80`) shipped + unit-tested, non-breaking. The deploy stage
> is unblocked; next is Installment 0 (child-diamond genesis + facet templates).
>
> **UPDATE 2026-06-14 (loop tick 2): Installment 0 foundation shipped + a real facet deploys.**
> The CounterFacet (`contracts/src/facets/CounterFacet.sol` + `LibCounterStorage`, 9 forge tests)
> and the FacetCut[] ABI encoder (`registry::encode_diamond_cut`, golden-vector tested byte-for-byte
> vs `cast calldata`) are committed. A live probe then deployed the **real 882-byte CounterFacet
> runtime** via sponsored CREATE (`status 0x1`, `getCode` = the runtime at `0x4f3416…49e2`). Because
> solc-0.8.24 emits `PUSH0` (`0x5f`) throughout that bytecode and it deployed + runs, **Moderato
> supports PUSH0** — resolving the §5/§10 open question (the emitter may use PUSH0). Remaining for
> the full E2E: child-diamond genesis + the live `diamondCut` + loupe-verify + call.
>
> **UPDATE 2026-06-14 (loop tick 3): child-diamond GENESIS works live.** Added
> `registry::encode_diamond_constructor_args` (golden-tested vs `cast abi-encode`) and
> `examples/child_diamond_genesis.rs`, which deployed an **agent-owned child Diamond** at
> `0x1e354c…c962` via sponsored CREATE — seeded with the prod Cut/Loupe/Ownership facets
> (reused as stateless code, delegatecalled in the child's storage). Verified: `owner()` ==
> deployer, `facetAddress(diamondCut)` == prod CutFacet → cuttable + loupe-verifiable. This is
> the per-agent SANDBOX diamond the safety model rests on. **Gas note:** genesis OOG'd at a 4M
> limit (the constructor's `diamondCut` does many cold SSTOREs at Tempo's high storage-gas
> rates; `cast run`'s 690k replay number was misleading — `debug_traceTransaction` showed the
> real OOG); 25M succeeded. **Installment 0 E2E is now COMPLETE:** `examples/diamond_cut_e2e.rs`
> cut the CounterFacet into the child diamond (`facetAddress(increment)` → CounterFacet) and
> called `increment()` through the diamond — `countOf` == 1. The full deploy → `diamondCut` →
> call loop works on Moderato; an agent can extend a diamond it owns.
>
> **UPDATE 2026-06-14 (loop tick 4): SolidityLite codegen foundation works on-chain.** Added
> `src/soliditylite/` (the EVM analog of rustlite): a pure-Rust bytecode `asm` (minimal pushes,
> conservative `PUSH1 0x00` zeros, fixed-width `PUSH2` labels with two-pass back-patch,
> CODECOPY/RETURN init wrapper) + `emit_constant_getter` (selector dispatch + body), 14
> golden-byte tests, wasm-clean, clippy-clean. `examples/soliditylite_getter_live.rs` deployed
> the EMITTED getter via sponsored CREATE and `eth_call get()` returned 42 — emitter output
> deploys + executes on the REAL Tempo EVM. DECISION: validate against the live chain (via the
> proven Installment-0 rails), NOT revm — Tempo diverges from standard EVM (gas etc.), so the
> real chain is the authoritative validator and it avoids a heavy dev-dep. Next: the FRONTEND
> (lexer/parser/typecheck forked from rustlite) + storage/ABI codegen, so a real facet compiles
> from SOURCE (then deploy+cut it via the proven path).
>
> **UPDATE 2026-06-14 (loop tick 5): facets compile FROM SOURCE — frontend live.** Added the
> SolidityLite frontend `src/soliditylite/{lexer,ast,parser,codegen}.rs` (recursive-descent,
> rustlite-style, MAX_RECURSION_DEPTH guard, reuses rustlite's `CompileError`/`Span`) + a
> `compile(src) -> CompiledArtifact`. Grammar: `facet { (function name() external view returns
> (uint256) { return <intlit | stateVar>; })+ }`, incl. a `uint256` state-var read → SLOAD at the
> keccak storage slot. GOLDEN GATE: `compile(get→42)` == tick-4's live-proven `emit_constant_getter`
> BYTE-FOR-BYTE. LIVE: compiled a 2-function facet from source → deployed → `a()`==1, `b()`==2 on
> Moderato (multi-function dispatch from source works). 34 soliditylite tests, wasm+clippy clean.
> Next: parameters (calldata decode) + `SSTORE` + `require` + events → compile the full CounterFacet
> from source, then deploy+cut it via the proven path.
>
> **UPDATE 2026-06-14 (loop tick 6): compiled facets WRITE STATE on-chain.** Extended the compiler
> (lexer/ast/parser/codegen) for mutating `function f() external { ... }`, assignment `var = expr;`
> (→ `SSTORE`), and binary `+` (→ `ADD`), reusing the keccak slot derivation + SLOAD read; the
> constant-getter golden gate stays byte-identical. LIVE: compiled `facet Tally { uint256 n;
> function bump() external { n = n + 1; } function get() external view returns (uint256){ return n; } }`
> from source → deployed → `bump()` ×2 → `get()` == 2 on Moderato (compiled SSTORE/SLOAD/ADD persist
> state). 44 soliditylite tests, wasm+clippy clean. Next: function PARAMETERS (calldata decode) +
> mappings (`count[msg.sender]`) + `require` + events → the full CounterFacet from source → cut into
> a child diamond — that closes the SolidityLite MVP.
>
> **UPDATE 2026-06-14 (loop tick 7): mappings + params + msg.sender compile from source — live.**
> Added function PARAMETERS (`CALLDATALOAD(4+32*i)`), `msg.sender` (`CALLER`), and MAPPINGS
> (`m[k]` slot = `keccak256(pad32(key) ++ pad32(baseSlot))`, read + write) to the compiler. LIVE:
> compiled `facet Ledger { mapping(address=>uint256) bal; add(uint256 amt){ bal[msg.sender] =
> bal[msg.sender] + amt } balanceOf(address who) view -> bal[who] }` from source → deployed →
> `add(5)`/`add(3)` → `balanceOf(me)` 0→5→8 on Moderato (per-caller mapping persists; the
> `balanceOf` selector == canonical ERC-20 `0x70a08231`, confirming correct keccak). 59 soliditylite
> tests, wasm+clippy clean. Only `require`/revert + events (LOG) remain before the FULL CounterFacet
> compiles from source.
>
> **UPDATE 2026-06-14 (loop tick 8): the CounterFacet LOGIC compiles from source — require enforced live.**
> Added comparison ops (`> < >= <= ==` → GT/LT/EQ + ISZERO), `require(cond,"msg")` (→ ISZERO + JUMPI to a
> shared `REVERT(0,0)` stub), string literals, and `+=` desugar. LIVE: compiled the CounterFacet-core
> (`mapping count` + `total`; increment / incrementBy(uint256) with `require(n>0)`+`require(n<=100)` /
> countOf(address) / totalCount) FROM SOURCE → deployed → `incrementBy(5)`→countOf 5, `incrementBy(101)`
> **REVERTS** (require enforced, state unchanged at 5), `increment()`→6, totalCount==6. All 4 selectors ==
> canonical keccak. 82 soliditylite tests, wasm+clippy clean. ONLY EVENTS (LOG) now remain for the
> literal full CounterFacet → then deploy+cut into a child diamond = SolidityLite MVP done.
>
> **UPDATE 2026-06-14 (loop tick 9): EVENTS land — the SolidityLite language MVP is COMPLETE.** Added
> `event E(type [indexed] arg, …);` decls + `emit E(...)` → `LOGn` (topic0 = full keccak of the
> canonical event sig; indexed args → topics; non-indexed → ABI-encoded data via MSTORE; exact LOG
> stack order; data staged above the keccak scratch so a mapping-read arg can't corrupt a log word).
> LIVE: the **FULL CounterFacet** (count mapping + total + the `Incremented` event; increment /
> incrementBy(require) / countOf / totalCount) compiled FROM SOURCE → deployed → `increment()` →
> countOf==1, totalCount==1, AND emitted `Incremented(caller, 1, 1)` with topic0 `0xcd5ad702…fdb5`
> (== the topic embedded in tick-2's forge CounterFacet — independent keccak confirmation), indexed
> caller in topic1, data `[1,1]`. 97 soliditylite tests, wasm+clippy clean. **Every CounterFacet
> primitive — mappings, params, msg.sender, arithmetic, require, events — now compiles from a source
> string and executes on the real Tempo EVM.** Next (capstone): compile the full CounterFacet →
> deploy → genesis a fresh child diamond → cut → call through the diamond (the existing child already
> holds those selectors), the full agent-authored-facet-in-its-own-diamond demo.
>
> **UPDATE 2026-06-14 (loop tick 10): CAPSTONE — the SolidityLite MVP is demonstrated END TO END.**
> `examples/soliditylite_mvp_capstone.rs` runs the literal keystone flow live on Moderato: an agent
> (1) WROTE a CounterFacet in source, (2) COMPILED it in-crate (789-byte runtime), (3) DEPLOYED it
> via sponsored CREATE, (4) GENESISed a fresh child diamond it OWNS (`0xe11916…`), (5) CUT the
> compiled facet's 4 selectors into that diamond (loupe `facetAddress(increment)` == the compiled
> facet), and (6) CALLED `increment()` THROUGH the diamond → `countOf`==1, `totalCount`==1, and the
> `Incremented` event fired correctly (topic0 + indexed caller + data). The self-modifying-platform
> keystone is PROVEN: an agent authors, compiles, deploys, and cuts its own facet into a diamond it
> owns. BEYOND-MVP (next): an agent TOOL (browser + CLI) to author/deploy/cut a facet end to end;
> the design §7 safety/immune-system layers (selector-clash + storage-isolation lint, on-chain
> `_init==0` + reserved-selector guard); dynamic types (string/bytes/arrays) for data-heavy facets.

> A hand-rolled, in-browser Solidity/EVM-subset → EVM-bytecode compiler that lets an
> agent **write, compile, deploy, and `diamondCut`** its own facet — the EVM analog of
> `src/rustlite/`. This document is the full-picture design. Status: **design + research,
> nothing built.**

---

## 1. Executive summary + feasibility verdict

SolidityLite is the keystone for a self-modifying agent platform: today an agent can write a
rustlite cartridge that draws pixels in a tab (`src/rustlite/`), but it cannot author the
*on-chain interior parts* of the platform itself. SolidityLite closes that loop — an agent
writes a Solidity-subset facet, the crate compiles it to EVM bytecode in the browser (no
solc), deploys it, and cuts its selectors into a diamond.

**Feasibility verdict: FEASIBLE-WITH-CAVEATS, but NOT as literally framed.**

Three things are true and were verified against this repo:

1. **The compiler frontend is genuinely cheap.** rustlite's pipeline
   (`rustlite::compile` = `lex → parse → typecheck → codegen`, `src/rustlite/mod.rs:19-25`),
   its `CompileError`/`Span` diagnostics, and compile-time keccak (`sha3::Keccak256` in
   `src/registry/abi.rs:1-13`, available on the browser-app/wasm path because `browser-app`
   pulls `wallet` transitively) all port cleanly. ~60% of the frontend is mechanical forking.

2. **The EVM backend is feasible but materially harder than a port.** Three subsystems are
   net-new with **zero rustlite analog**: absolute-PC `JUMPDEST` resolution (rustlite rides
   wasm's *structured, relative* control flow via `extra_depth`, `codegen.rs`), a 4-byte
   selector ABI dispatcher with head/tail encoding, and keccak-namespaced storage slots.

3. **"Self-modify the canonical platform diamond" is NOT feasible and must be reframed.**
   `diamondCut` is unconditionally owner-gated — `DiamondCutFacet` →
   `LibDiamond.enforceIsContractOwner()` = `require(msg.sender == ds.contractOwner)`
   (verified `LibDiamond.sol:57-59`), and the diamond owner key `0x313b16…EF1e` is
   explicitly **NOT in the repo** and is not the sponsor key. An agent therefore **cannot**
   cut the canonical diamond `0x6c31c0…Da30c` by any sponsored path. The keystone is only
   safe against a **per-agent child diamond the agent owns**.

### Recommended approach (decisive)

**Ship in two installments against a per-agent child diamond:**

- **Installment 0 — Parameterized templates.** A curated, hand-verified bytecode skeleton
  (CounterFacet / ArtFacet) with agent-supplied constants. This proves the *entire on-chain
  plumbing* (CREATE → address → `FacetCut[]` encode → cut → loupe-verify → call) with **zero
  codegen-correctness risk**. It is the de-risking move and a shippable artifact on its own.

- **Installment 1 — The real compiler.** A **Yul-shaped Solidity subset** (not raw Solidity)
  lowered to EVM bytecode with **all locals in memory** (Vyper's spill strategy — sidesteps
  stack-too-deep structurally). Add the dynamic-ABI codec and `Replace`/`Remove` cuts only in
  a fenced Phase 2.

**Rejected alternatives:**

- *Full Solidity-subset compiler that parses real Solidity / reuses solc* — solc-js is an
  ~8 MB emscripten port of the C++ compiler; shipping it is the exact "ship rustc to compile
  rustlite" anti-pattern the project already rejected. Parsing real Solidity drags in
  inheritance, modifiers, location inference, and implicit ABI. **No.**
- *rustlite second EVM backend* — the frontend "reuse" is illusory: the type lattice, storage
  model, and ABI are entirely different, and it forces *Rust* syntax onto an EVM target the
  agents already know in Solidity. **No.**
- *Facet-DSL (non-Solidity surface)* — a fine fallback if the Yul-shaped subset proves too
  parser-heavy, but a Solidity-flavored surface is what an LLM writes most reliably. **Fallback only.**

The dominant risk is **not compilation — it is governance and correctness verification.**
EVM bytecode has no self-validator (unlike wasm, which the browser validates), so a
miscompiled storage slot or off-by-one ABI offset is *silently* wrong against a live diamond.
A differential execution harness (revm in `cargo test`) is **non-optional** and must be
installment-1's first deliverable.

---

## 2. What it is + why

### The capability

`rustlite` made agents **authors of in-tab behavior**. SolidityLite makes them **authors of
the platform's on-chain interior** — the facets that *are* the platform. An agent observes a
gap ("there's no per-user streak counter"), writes a facet, compiles it in the same tab,
deploys it, and cuts it into a diamond it owns. The new selectors are live; any caller routed
through that diamond's fallback (`Diamond.sol:28-40`) now hits agent-authored code.

### Why this is the north star

The self-evolving-colony vision (per `MEMORY.md`: agents file their own issues/PRs, evolve
their source, speciate) has a missing rung: agents can change *off-chain* code via the colony
GitHub loop, but the *substrate* — names, $LH, TBAs, escrow — lives in a diamond no agent can
touch. SolidityLite is the substrate-level analog: a **stem-cell** capability where one shared
genome (the diamond pattern) diverges through composable facets an agent grows itself,
exactly as cartridges let a subdomain diverge through composable pixel-buffers.

### Why a hand-rolled compiler (the rustlite precedent)

rustlite proved the bar empirically: a ~7,000-LOC, 8-file, zero-external-dep compiler
(no syn, no cranelift, no wasm-opt) is *enough to write a real cartridge*. SolidityLite sets
the same bar: **enough to write a real facet, nothing that needs a type-inference engine.**
The whole point is that it runs in the user's browser with no toolchain — the same
self-sovereign property that makes rustlite valuable.

---

## 3. Language spec — the v1 subset

**Surface shape: a Yul-flavored Solidity subset.** Yul's grammar is tiny and "compiles to
bytecode in a very regular way" (Yul docs); it is register-based (named variables), which
makes lowering regular. We keep Solidity-familiar keywords (`facet`/`contract`, `function`,
`mapping`, `require`, `emit`) so an LLM writes it fluently, but the semantics are the
restricted Yul-like core.

### Types (v1)

The **only** value types are the four EVM-native 32-byte words:

| Type | Representation |
|------|----------------|
| `uint256` | the word as-is |
| `address` | word, high 12 bytes zero (compiler masks on write/decode) |
| `bool` | `0` / `1` |
| `bytes32` | the word as-is |

Plus **storage-only** `mapping(K => V)` where `K ∈ {uint256, address, bytes32}` and `V` is a
value type **or exactly one nested `mapping`** (depth 1). `event` declarations (multiple
allowed). `bytesN<32`, signed ints, sub-width uints, dynamic arrays, `string`/`bytes` *values*,
in-memory structs, struct-typed storage, inheritance, modifiers, libraries, constructors, and
inline `assembly` are **all excluded** (each a clear `CompileError` with a reason, the rustlite
"exclude-with-a-reason" discipline at `typecheck.rs:294-306`). String literals survive **only**
as `require`/`revert`/`event`-constant operands, never as runtime values.

> **Honest-scope flag.** This means the data-heavy keystone facets — `FeedbackFacet` (string),
> `RegistryFacet.setMetadata(bytes)`, `BountyFacet` (string/custom-errors) — **cannot compile
> in v1.** The v1 target is a CounterFacet then an ArtFacet (uint/address mappings). Dynamic
> types are a fenced Phase 2.

### Statements / expressions

`if`/`else`, `while`, C-style `for`, `let`-locals (value types only), assignment +
compound-assign (`+= -= *= /=`), `require(cond[, "msg"])`, `revert(["msg"])`, `emit Ev(args)`,
`return expr?`, and bare call statements. Operators use rustlite's precedence ladder
(`parser.rs:546-658`) minus `cast`. Compiler-magic globals: `msg.sender`, `msg.value`,
`block.timestamp`, `block.number`, `address(this)`, and named platform constants `LH_TOKEN` /
`DIAMOND` resolved at compile time from `registry::REGISTRY_ADDRESS` (raw address literals are
discouraged — the 2026-06-01 reset abandoned all prior addresses, so pasted literals rot). The
**one** external-call shape is `Type(addr).method(args)` against a small built-in interface
table (initially just `IERC20Min.transferFrom` for $LH), mirroring rustlite's fixed `host::`
table (`typecheck.rs:114-122`).

### Worked example facet (complete, copy-pasteable v1 source)

---

solidity
facet CounterFacet {
    // auto-synthesized at keccak256("localharness.counterfacet.storage.v1")
    mapping(address => uint256) count;   // slot BASE + 0
    uint256 total;                       // slot BASE + 1

    event Incremented(address indexed who, uint256 newCount, uint256 newTotal);

    function increment() external {
        count[msg.sender] = count[msg.sender] + 1;
        total += 1;
        emit Incremented(msg.sender, count[msg.sender], total);
    }

    function incrementBy(uint256 n) external {
        require(n > 0, "zero");
        require(n <= 100, "too big");
        count[msg.sender] += n;
        total += n;
        emit Incremented(msg.sender, count[msg.sender], total);
    }

    function countOf(address who) external view returns (uint256) {
        return count[who];
    }

    function totalCount() external view returns (uint256) {
        return total;
    }
}
```

This exercises every v1 primitive: a mapping + a scalar, an `event` with an `indexed` param +
`emit`, two `require`s, `msg.sender`, compound-assign, comparison/arithmetic, `view` reads, and
a single-value `return`. The compiler **auto-synthesizes** the diamond-storage layout from the
declared state vars — the agent never writes `assembly { s.slot := position }` (the boilerplate
every `Lib*Storage.sol` repeats, e.g. `LibFeedbackStorage.sol:30-35`).

---

## 4. Compiler architecture — pipeline mapped from rustlite

New module `src/soliditylite/`, a sibling of `src/rustlite/`, mirroring its sub-module shape.
The public entry mirrors `rustlite::compile` exactly:

```rust
// src/soliditylite/mod.rs
pub fn compile(source: &str) -> Result<CompiledFacet, CompileError>;

pub struct CompiledFacet {
    pub runtime:   Vec<u8>,      // EVM runtime bytecode (stored on-chain)
    pub selectors: Vec<[u8; 4]>, // for the FacetCut[]
    pub abi:       Vec<FnSig>,   // name + canonical sig (tool feedback / collision check)
}
pub fn wrap_init(runtime: &[u8]) -> Vec<u8>; // CODECOPY/RETURN constructor for CREATE
```

`CompileError`/`Span`/`line_col`/`render_snippet` are **shared with rustlite** (lift them into a
`compile_error` module or `pub use crate::rustlite::{CompileError, Span}`); only new `LHxxxx`
codes are added to `src/error_codes.rs` (a distinct block, e.g. `LH05xx`, so EVM codes are
visually separable): `SELECTOR_COLLISION`, `STORAGE_SLOT_FORBIDDEN`, `STACK_TOO_DEEP`,
`NOT_ABI_ENCODABLE`, `UNSUPPORTED_OPCODE`.

| Stage | rustlite source | Reuse | What changes |
|-------|-----------------|-------|--------------|
| **lexer** | `lexer.rs` (555) | ~85–90% | swap keyword/operator table (`facet`, `function`, `mapping`, `=>` already `FatArrow`, `require`, `emit`, `indexed`, `external`/`view`/`payable`); drop float + char-literal lexing |
| **ast** | `ast.rs` (318) | ~70% of shape | keep `Module`/`Item`/`Expr{kind,span}`/`Stmt`/`Place` + precedence enums; replace `Ty` with the EVM types; add `Item::Facet{state_vars, mappings, events, fns}`, `StateVar`, `Event`; `FnDecl` gains visibility + mutability + `returns` |
| **parser** | `parser.rs` (1777) | ~80% | keep the precedence ladder **and the `MAX_RECURSION_DEPTH=96` guard** (`parser.rs:11-79`) — same browser-tab-abort risk on adversarial LLM input. The `no_struct_literal` flag **vanishes** (Solidity mandates `if (...)` parens, no `v` block). Parse `facet {…}`, visibility/mutability/`returns`, `mapping(K=>V)`, `emit`/`event` |
| **typecheck** | `typecheck.rs` (1318) | ~60% machinery | keep the two-pass `TypeContext` (register sigs → check bodies → source-order independence), the `Vec<HashMap>` scopes, the parallel typed tree. **Rewrite** `resolve_host_fn` → `resolve_builtin` (`msg.sender`→address, `keccak256`→bytes32, `transferFrom`→bool, …). **Add**: storage-vs-memory location rules, `view`/`payable` enforcement (a `view` fn that `SSTORE`s = `LH05xx` error), ABI-encodability checks |
| **codegen** | `codegen.rs` (1544) | ~20% (the discipline) | **fully rewritten EVM emitter** — see §5 |
| **loader → deploy** | `loader.rs` (658) | replaced | not a browser instantiate; an on-chain deploy + cut path (§6) |
| **diagnostics** | `mod.rs:28-179` | ~95% literal | the agent-facing `{error, code, detail, location, snippet, hint}` report (`builtins/compile_rustlite.rs:90-99`) carries over unchanged — the fix-and-recompile loop is the whole point |

**Codegen invariants (write these into the spec NOW — three research dimensions conflicted
on them):**

1. **All locals live in MEMORY** (`MSTORE`/`MLOAD`, bump-allocated from `0x80`), operand stack
   is scratch-only. This *structurally* eliminates stack-too-deep (the 16-deep DUP/SWAP wall)
   and mirrors rustlite's single-page model exactly. **Decided, not optional.**
2. **Labels are ALWAYS `PUSH2`**, single back-patch, no push-size fixed-point in v1 (constant
   +1 byte/jump; gas is sponsored so size matters less than correctness).
3. **`SSTORE`/`SLOAD` ONLY at keccak-namespaced slots.** The emitter NEVER touches slot `0..n`
   and NEVER emits compiler-internal storage. Enforced + statically linted.
4. **A push-data-aware validator runs on every emit** (§7 Layer 1).

---

## 5. EVM target — bytecode / ABI / dispatch / storage emission

### The two-blob model

Codegen emits **runtime** bytecode; `wrap_init` prepends a ~12-byte constant constructor that
`CODECOPY`s the runtime into memory and `RETURN`s it (EVM contract-creation semantics — the one
concept with no wasm analog):

```
; init wrapper (consumed by the CREATE tx; deployed code = the runtime)
PUSH2 <rt_len>  DUP1  PUSH2 <rt_off>  PUSH1 0x00  CODECOPY  PUSH1 0x00  RETURN
; <runtime bytes follow at rt_off>
```

Use `PUSH1 0x00` for zeros, **not `PUSH0`** — `PUSH0` (Shanghai, EIP-3855) availability on
Tempo Moderato is unverified; a pre-Shanghai EVM treats it as an invalid opcode.

### Runtime layout

```
[free-mem-ptr init][calldatasize guard][selector extract][dispatch table][fn bodies][revert stub]
```

### Selector dispatcher

Selectors are `keccak256("name(types)")[0..4]`, computed at **compile time** via the existing
`registry::abi::selector` (verified `src/registry/abi.rs:6-13`; test-pinned
`idOfName(string)==0x127c388a`). The dispatcher loads the selector once and chains `EQ`/`JUMPI`:

```
PUSH1 0x04 CALLDATASIZE LT PUSH2 <FB> JUMPI   ; <4 bytes → fallback revert
PUSH1 0x00 CALLDATALOAD PUSH1 0xE0 SHR        ; selector = calldata[0:32] >> 224
DUP1 PUSH4 <sel_0> EQ PUSH2 <body_0> JUMPI
DUP1 PUSH4 <sel_1> EQ PUSH2 <body_1> JUMPI
FB: JUMPDEST POP PUSH1 0x00 PUSH1 0x00 REVERT ; no match (mirrors Diamond.sol require)
```

A linear `EQ`-chain is O(n) but fine for the handful of fns a facet exposes; a packed jump
table (Huff-style) is the deferred scale path.

### Storage (delegatecall-aware)

A facet runs under **`DELEGATECALL`** from the diamond (`Diamond.sol:30-41`), so every
`SSTORE`/`SLOAD` hits the **diamond's** storage, `CALLER`=the real EOA (correct for auth), and
`ADDRESS`=the diamond (correct for self-calls). The compiler computes `BASE =
keccak256("localharness.<facetname>.storage.v1")` at compile time and lays scalars sequentially
from `BASE` (no packing in v1 — only `bool` wastes space among 32-byte types). Mapping entries:

```
slot(m[k])      = keccak256( pad32(k)  ++ pad32(p) )                 ; p = the var's base slot
slot(m[k1][k2]) = keccak256( pad32(k2) ++ keccak256(pad32(k1) ++ pad32(p)) )
```

emitted at runtime via `MSTORE` the key at `mem[0x00]`, `p` at `mem[0x20]`, then
`PUSH1 0x40 PUSH1 0x00 KECCAK256` → slot. This is the **one mandatory use of memory scratch**
and the single most error-prone primitive (a 1-byte preimage error is a *silent* wrong-slot
read/write, no revert).

### Two-pass label resolution

Pass 1 emits all opcodes with **fixed-width `PUSH2`** placeholders for every body PC and the
fallback PC, recording each `JUMPDEST` byte offset as it walks (a body always starts with
`0x5B`). Pass 2 back-fills the 2 big-endian operand bytes — single pass, no iteration, because
widths never shift. This is the EVM analog of rustlite's `finish()` length-patch
(`codegen.rs:1012`) but *simpler* (fixed width vs LEB recompute). It is also genuinely new work
— rustlite's `extra_depth` relative-branch scheme (`codegen.rs:840,930`) does **not** carry over.

### Worked snippet — `get() returns (uint256)` reading the counter's `total`

```
offset  bytes        mnemonic         note
0000    60 04        PUSH1 0x04
0002    36           CALLDATASIZE
0003    10           LT
0004    61 ????      PUSH2 <FB>       ; placeholder, back-patched pass 2
0007    57           JUMPI
0008    60 00        PUSH1 0x00
000a    35           CALLDATALOAD
000b    60 e0        PUSH1 0xe0
000d    1c           SHR              ; selector on stack
000e    80           DUP1
000f    63 6d4ce63c  PUSH4 sel(get())
0014    14           EQ
0015    61 ????      PUSH2 <Lget>
0018    57           JUMPI
... (revert stub at FB) ...
Lget: 5b            JUMPDEST
      7f <BASE+1>   PUSH32 slot        ; the keccak-derived slot for `total`
      54            SLOAD
      60 00         PUSH1 0x00
      52            MSTORE             ; mem[0..32] = total
      60 20 60 00   PUSH1 0x20 PUSH1 0x00
      f3            RETURN             ; return mem[0..32]
```

### ABI codec (v1 = static words only)

Decode: arg `i` at `CALLDATALOAD(4 + 32*i)`; mask `address` to 20 bytes, normalize `bool` via
`ISZERO ISZERO`. Encode: store each return word at `mem[0x20*k]`, `RETURN(0, 0x20*n)`.
**Dynamic head/tail** (offset-into-tail for `string`/`bytes`/arrays) is deferred — and so is
the one *exception that's on the critical path*: the `diamondCut` calldata is itself a dynamic
tuple-array `FacetCut[]`, strictly harder than any encoder in the repo (which handle only
static words + a single `string`/`bytes`, e.g. `encode_id_of_name`). So a **`FacetCut[]`
encoder must be in v1 regardless** (§6), golden-vector-tested against `cast calldata`.

---

## 6. Deploy + `diamondCut` pipeline

Four stages, riding the existing sponsored plumbing (`tempo_tx.rs` +
`registry/tx.rs::submit_tempo_sponsored` + `app/events/mod.rs::run_sponsored_tempo_call`).

### Stage 1 — DEPLOY (one sponsored 0x76 CREATE call)

**There is no CREATE path today.** Verified: `TempoCall.to` is a fixed `[u8;20]` and its own
doc says *"no contract creation via Tempo tx in this codebase yet"* (`tempo_tx.rs:86-94`). The
fix is small — change `to` to a `TxKind { Address([u8;20]), Create }` enum and have `rlp_call`
emit the empty-bytes `0x80` marker for `Create` (one branch in `tempo_tx.rs:448-454`;
sender/fee-payer hashing is unchanged because they treat the call as opaque RLP). Submit with
the **ROOT key** (apex-iframe `lh-sign-digest`), never an access key — **Tempo rejects
access-key CREATE**. Gas: a length-scaled formula modeled on `set_metadata_gas`
(`registry/tx.rs:41`), e.g. `1.5M + len*200 + ~275k` sponsorship overhead — and `cast
estimate`, never a flat cap (the documented OOG bug class). Read the created address from the
receipt's `contractAddress` (extend `registry/rpc.rs` receipt parsing) — do **not** compute
`keccak(rlp([sender,nonce]))`, which is unverified under Tempo's 2D-nonce/AA model.

> **This stage is the highest unverified risk in the whole design** — see §10.

### Stage 2 — FacetCut[] calldata (new ABI encoder)

`encode_diamond_cut_add(facet, selectors)` →
`selector("diamondCut((address,uint8,bytes4[])[],address,bytes)")` ++ head (offset to `cuts[]`,
`_init=0x0`, `_calldata` offset) ++ the dynamic `cuts[]` tail. Golden-vector-test it.

### Stage 3 — CUT

Submit the Stage-2 calldata. **Signer = whoever owns the target diamond.**

### Stage 4 — VERIFY

For each selector, `eth_call DiamondLoupeFacet.facetAddress(bytes4)` and assert it equals the
Stage-1 address; then call the new fn once. Pre-flight the *same* loupe **before** Stage 3 to
catch selector collisions (`LibDiamond.addFunctions` reverts `"add: function exists"` atomically
after burning gas — verified the diamond is the real prior-collision risk per the
`taskOf`/`bountyTaskOf` precedent).

### The owner-only-cut permission problem — the solution

`diamondCut` requires `msg.sender == ds.contractOwner` (`LibDiamond.sol:57-59`) and the
canonical diamond's owner key is off-repo. **Recommended solution: per-agent CHILD diamonds.**

`Diamond.constructor(address _contractOwner, FacetCut[])` (verified `Diamond.sol:14`) lets the
deployer name **any** owner. So:

1. A new tool `create_my_diamond()` deploys a fresh `Diamond` whose owner is the agent's own
   key/TBA, seeded with the Cut/Loupe/Ownership facets.
2. The agent now passes `enforceIsContractOwner()` on **its** diamond and cuts freely,
   sponsored, with **zero** permission problem.
3. The blast radius of any bad cut is confined to **that agent's toy diamond** — names, $LH,
   TBAs are untouched.
4. Discoverability: record the mapping on-chain under a `keccak256("localharness.child_diamond")`
   metadata key (reuse `setMetadata`), so other agents can find and call an agent's
   self-authored facets.

**Gas optimization with a caveat:** child diamonds can *reuse the production Cut/Loupe/Ownership
facet addresses* (they're stateless code; `DELEGATECALL` runs them in the child's storage),
slashing per-agent deploy cost. But this re-opens a **sponsor-fee griefing vector** (below) —
so meter child-diamond cuts against the **agent's own $LH**, not the sponsor.

**Touching the SHARED diamond** is reserved for an **owner-gated `proposeCut` queue**: any agent
calls `proposeCut(facetAddr, exactFacetCutHash, initHash)`; the off-repo owner key (the human,
or later a `VotingFacet` quorum) calls `approveAndCut(id)` after review. **Never** a
permissioned `AgentCutFacet` that can self-call `diamondCut` on the shared diamond — that is a
total-compromise privilege-escalation surface (a buggy/malicious facet could `Replace`
`DiamondCutFacet`/`OwnershipFacet` and seize everything).

---

## 7. Safety / immune-system model

**Governing principle: never let an agent cut the production diamond.** The blast asymmetry is
total — one shared object, irreversible writes, every name/$LH/TBA/guild-treasury/escrow lives
in one diamond. Defense is layered; **build order is L0 → L1 → L2 → L3** because each lower
layer makes the next lower-stakes, and **L0+L1 alone make agent self-cutting safe-by-construction.**

The non-negotiable architectural lesson from the adversarial review: **off-chain checks are
advisory.** The agent *signs the final `FacetCut[]`/`_init`/`_calldata` tx itself* — it can
hand-craft calldata that never went through SolidityLite. So anything that *must* hold has to be
re-checked **on-chain** at cut time.

### Layer 0 — per-agent sandbox diamonds (build FIRST)

The load-bearing isolation (§6). Converts "the most dangerous capability on the platform" into
"safe by construction." With L0, L1–L3 become a *quality/UX* gate, not a last line of defense.

### Layer 1 — static lint + codegen invariants (in-tab, instant refusals)

A pure, natively-testable `src/cut_guard.rs` (like `confirm.rs`/`turn_flow.rs`):

- **(a) Selector clash** — compute keccak4 of every new sig; reject any already in the live
  `DiamondLoupeFacet.facets()` snapshot. Detect intra-facet keccak4 collisions at compile time.
- **(b) Reserved-selector denylist** — hard-refuse adds/replaces/removes of `diamondCut`
  (`0x1f931c1c`), `transferOwnership` (`0xf2fde38b`), `owner` (`0x8da5cb5b`), the four loupe
  selectors, and (on shared diamonds) every live money/ownership selector. **Critical caveat:**
  `LibDiamond.removeFunction` only protects `address(this)` (verified `LibDiamond.sol:168`) — so
  `DiamondCutFacet`/`OwnershipFacet`/loupe, which live at *separate* addresses, are
  Removable/Replaceable on-chain. This denylist MUST be re-enforced by an on-chain registrar,
  not just the lint.
- **(c) Storage isolation, enforced at codegen** — the emitter is **structurally incapable** of
  emitting `SSTORE`/`SLOAD` to a literal or non-keccak-namespaced slot, OR an arbitrary-preimage
  `KECCAK256` feeding a store. All storage is compiler-managed from `BASE =
  keccak256("localharness.<facetname>.storage.v1")`, `<facetname>` checked unique against the
  live loupe at cut time. This closes the slot-0 (`contractOwner` lives near
  `keccak256("diamond.standard.diamond.storage")`, verified `LibDiamond.sol`) and the
  sibling-slot-forgery vectors (e.g. an agent reproducing `X402` `authState` slots to forge nonce
  state) at the source.
- **(d) `_init` MUST be `address(0)`** — **the single highest-severity vector.**
  `LibDiamond.initializeDiamondCut` does `_init.delegatecall(_calldata)` in the **diamond's
  storage context** (verified `LibDiamond.sol:202-219`). The facet bytecode can pass every lint
  while a *separate* agent-supplied `_init` overwrites `contractOwner` or the credits token in
  one cut. Forbid agent `_init` **mandatorily and on-chain** (the registrar checks it), never as
  an optional allowlist.
- **(e) Forbidden opcodes** — codegen never emits `SELFDESTRUCT`, `DELEGATECALL`, `CALLCODE`, or
  arbitrary `CALL` to non-allowlisted targets in v1.
- **(f) Shape gates** (copied from `loader.rs:85-100`): EIP-170 24,576-byte runtime ceiling,
  non-empty selector arrays, `extcodesize(facet) > 0`.

### Layer 2 — simulation / dry-run gate (network)

**Live-verified feasible on Tempo this session:** `debug_traceCall` returned a full call-frame
tree for the diamond's `facets()`, and `eth_call` accepted a state-override map — both against
`rpc.moderato.tempo.xyz`. Before submitting a real cut: deploy the candidate, `debug_traceCall`
the `diamondCut` as owner (confirm no revert, no `SELFDESTRUCT`/unexpected external call in the
tree), then run a **probe battery** — call every new selector with benign + fuzzed args, and
assert a **canary set** of pre-existing core reads (`nextId`, `ownerOfName`, `owner`, a known TBA)
returns **byte-identical** results post-cut (proves no storage corruption). **Must degrade
gracefully** to L1-only + sandbox if a node drops `debug_`.

> **L2 is a smoke test, never an auto-approve gate for the shared diamond.** It is defeatable by
> input-dependent (`if calldataload(4)==0xC0FFEE drain()`), time-dependent (`block.timestamp`),
> and split-across-two-cuts attacks. A green simulation is evidence of an honest *bug*'s absence,
> not of *malice*'s absence.

### Layer 3 — staging + promotion + reversibility (shared diamond only)

Sandbox cuts stop at L2. Promotion to the shared diamond: cut to a throwaway **staging diamond**
first, soak it (the test-fleet personas exercise it), then require **human/owner sign-off** — the
immune system's "T-cell." **Bind the confirmation to a hash of the EXACT
`(facet bytecode ‖ FacetCut[] ‖ _init ‖ _calldata ‖ target diamond)`** shown to the user, NOT to
the SolidityLite *source* — otherwise `confirm_guard` rubber-stamps whatever the (possibly buggy)
compiler emitted. Every `Replace`/`Remove` snapshots `(selector → prior facet address +
codehash)` via the loupe first, persists it, and exposes `revert_last_cut()` (a deterministic
reverse-cut; `Replace` preserves storage so rollback is clean — verify `extcodesize>0` +
codehash match on the prior facet first, redeploy if dead).

All deploy/cut tools (`create_my_diamond`, `deploy_facet`, `cut_my_facet`, `revert_last_cut`) go
through the existing `confirm_guard` typed-confirmation dispatch gate (`src/confirm.rs`,
`CONFIRM_GATED`) and `tool_allowlist.rs`, exactly like `release_subdomain`/`send_lh`.

### Cross-cutting: sponsor-fee griefing

The cheapest thing to drain is **not** $LH — it's the low-budget sponsor key (AlphaUSD
fee_payer). Unbounded sponsored CREATE+cut loops on throwaway sandbox diamonds DoS *every* user's
sponsored writes. **Mitigation: no sponsor subsidy for facet CREATE/cut — meter against the
agent's own $LH — plus a per-owner cut rate cap.**

---

## 8. `call_agent` x402 finding

**Verdict: mostly GHOST, with one real latent bug.** The "proxied instead of settling on-chain"
intuition is a correct *observation* but a wrong *diagnosis*. There are four call paths; the
proxy-vs-direct routing is **by design and correct**: a payment routes through the proxy only for
a **foreign** agent (the `?rpc=1` iframe is caller-machine-local — a foreign agent has no
key/state on the caller's device, and a static host can't accept HTTP POST). Both proxy-settle
(`proxy/api/mcp.ts::settleOnChain`) and direct-settle (`app/agent_rpc.rs::settle_incoming` →
`settle_x402_sponsored`) call the **same `X402Facet.settle` on the same diamond**, moving real
$LH on-chain. "Proxied" = *who submits the settle tx*, not "settled off-chain." The nonce concern
is also a ghost — every path picks a fresh CSPRNG nonce caller-side; the only nonce-visible
behavior is the deliberate `SettlementUnconfirmedError` "do NOT sign a fresh nonce" guidance,
which is correct anti-double-pay design.

**The real bug:** the **browser-local** pay path (`src/builtins/call_agent.rs::pay_and_build`,
~lines 168-227) does **no wallet pre-flight, no meter→wallet bridge, and no diamond allowance
check** — unlike the proxy fallback (`remote_call.rs:88-133`) and CLI (`mcp.rs::ensure_diamond_
allowance`), which all do. So a caller whose $LH sits only in the **meter** pot, or who lacks a
standing `approve(diamond)`, signs an auth whose `settle` then **silently reverts** callee-side
in `transferFrom` → `"payment: settlement not confirmed"`. This is the exact "has $LH but can't
pay" two-pot symptom (CLAUDE.md "Wallet vs meter").

**Fix (small):** add an app-installed hook (`x402_hook::install_ensure_payable`) that
`pay_and_build` awaits before signing, lifting the **already-live, three-paths-proven** bridge
block from `remote_call.rs:88-133` — (a) pre-flight `token_balance_of`, (b) on shortfall pull
`withdraw_credits_sponsored`, (c) ensure `lh_allowance ≥ value` else `approve_lh_sponsored`. No
new on-chain code; reuses existing `registry::{token_balance_of, withdraw_credits_sponsored,
lh_allowance, approve_lh_sponsored}`. Also unify the divergent per-call caps (browser-local
`MAX_PAY_PER_CALL_WEI = 100 LH` vs proxy-fallback `1 LH`) onto one constant, and surface
`_meta.settlement: "direct"|"proxy"` + tx hash in the result so the user can *see* a "proxy" reply
still settled on-chain. **~40-60 LOC.** Only the next browser-capable agent can E2E it (the local
path runs only in a real two-tab same-device browser).

> This is an **orthogonal** finding — a worthwhile inline fix, but it does not gate SolidityLite.

---

## 9. MVP + phased roadmap

**The MVP is a `CounterFacet` on a child diamond the agent owns**, compiled in-tab, deployed,
cut, and called through loupe routing — proven against a local EVM *before* testnet.

| Phase | Scope | Effort |
|-------|-------|--------|
| **Week 0 — DEPLOY PROBE** | Extend `examples/tempo_tx_live.rs` with one sponsored 0x76 `Create` (empty `to`) on Moderato; confirm it deploys, the receipt yields `contractAddress`, `extcodesize>0`, and that the apex-iframe signer uses the **root** key. Settle `TxKind::Create` RLP (0x80) against `wevm/ox` `TxEnvelopeTempo`. | **~0.5 day — do this FIRST; it can invalidate the deploy model** |
| **Decision #1** | Ratify per-agent child diamonds as the autonomous target; reframe "self-modify the platform" as "self-modify the agent's OWN diamond." | gating decision, no code |
| **Installment 0 — templates** | Parameterized CounterFacet/ArtFacet bytecode skeleton + the full deploy/cut/verify plumbing (CREATE-capable `TempoCall`, `FacetCut[]` encoder, child-diamond genesis, loupe verify). Zero codegen risk. | **~1 week** |
| **Phase 0 — compiler MVP** | lexer/ast/parser/typecheck fork (Yul-shaped subset, the 4 types + depth-1 mappings + `external`/`view` + `if`/`require`/`emit`) + EVM codegen (selector dispatch, keccak storage, all-locals-in-memory, PUSH2 labels, init wrapper) + the push-data-aware validator + **revm differential test harness**. Compiles+deploys+cuts a real CounterFacet. | **~3-4 weeks** (codegen + verification is ~80% of the difficulty) |
| **Phase 1 — ArtFacet** | mappings-of-words richer facet (`mint`/`buy`/`transfer` in $LH via the `IERC20Min.transferFrom` built-in), `Add`-only cuts, L1 static lint + L2 simulation gate. | **+3-4 weeks** |
| **Phase 2 — dynamic + governance** | dynamic-ABI codec (`string`/`bytes`/arrays — unblocks Feedback/setMetadata-class facets), `Replace`/`Remove` cuts behind extra confirmation, `proposeCut`/`approveAndCut` registrar + staging/promotion (L3), on-chain reserved-selector + `_init=0` enforcement. | **+6-10 weeks (dominated by safety)** |

**Calibration:** frontend ≈ a 1-2 week port (~60% reusable from rustlite); the EVM backend +
verification is the real multi-week investment where ~all the correctness risk concentrates. The
EVM emitter is actually *smaller per-construct* than rustlite's wasm emitter (no structured-control
depth bookkeeping, no multi-type numeric matrix) — the only thing genuinely harder than wasm is
absolute jumps, exactly one well-understood algorithm.

---

## 10. Top risks, open questions, and the recommended first step

### Top risks (ranked)

1. **The Tempo CREATE path is unverified and gates the entire deploy stage.** `TempoCall` cannot
   represent CREATE today; whether a sponsored 0x76 empty-`to` tx deploys, how the address is
   surfaced under 2D-nonce/AA, and whether the apex signer uses the root key (access keys can't
   CREATE) are all untested. **Every effort estimate is contingent on this.**
2. **No EVM-semantics validator.** Unlike wasm, "it compiled" proves nothing. A revm differential
   harness in `cargo test` is non-optional — without it, a miscompiled slot/ABI offset's first
   test is a live (even sandbox) diamond.
3. **`_init`-delegatecall is a total-compromise vector** that bypasses every codegen-level
   defense. Mandatory on-chain `_init == address(0)` for agent cuts.
4. **Storage-slot / sibling-slot corruption** bricks-or-drains silently. Enforced at codegen +
   re-checked on-chain; the emitter must be *structurally* incapable of touching low/arbitrary
   slots.
5. **The reserved-selector denylist must be on-chain** — `LibDiamond` itself protects only
   `address(this)`, so cut/ownership/loupe selectors are Removable/Replaceable; an off-chain lint
   is bypassable by a self-signing agent.
6. **Sponsor-fee griefing** — meter cuts against the agent's own $LH, never the shared sponsor.
7. **PUSH0 / hardfork drift** — default to `PUSH1 0x00`; confirm Tempo's EVM version.

### Open questions

- Does a sponsored Tempo 0x76 `Create` deploy, and what is the `TxKind::Create` RLP (0x80 vs a
  sentinel — confirm against `wevm/ox`)?
- Does the apex-iframe `lh-sign-digest` signer use the root key or an access key today?
- Child diamonds: reuse production core-facet addresses (cheap, storage-isolated) or independent
  copies (purer blast radius)?
- v1 arithmetic: skip Solidity-0.8 overflow checks (smaller bytecode, *documented* semantic
  divergence) or emit them (safer, larger, EIP-170 pressure)?
- Is there appetite for agent-authored facets on the SHARED diamond at all, or is the north star
  satisfied by child diamonds + a human-gated promotion path (this decides whether L3 is ever built)?

### THE recommended concrete first step

**Run the Tempo CREATE probe before writing a single compiler line.** Extend
`examples/tempo_tx_live.rs` with one sponsored 0x76 transaction carrying a single `Create` call
(empty `to` / `0x80`, `input` = a trivial known init-code such as the CounterFacet template's
init wrapper) against `rpc.moderato.tempo.xyz`, and confirm: (a) it deploys, (b) the receipt
surfaces a `contractAddress`, (c) `extcodesize > 0` at that address, and (d) the signing path
uses the root key. This ~half-day probe de-risks the single assumption the entire multi-week
investment rests on; if sponsored CREATE doesn't work, the deploy model needs a factory-facet
redesign and the whole roadmap shifts — better to learn that on day zero than week four.
```

---

Key files this design touches or cites (all absolute):

- `src/rustlite/mod.rs` — the `compile()` pipeline + `CompileError`/`Span` to reuse
- `src/rustlite/codegen.rs` — the hand-rolled emitter pattern; `extra_depth` relative-branch scheme that does NOT carry to EVM
- `src/registry/abi.rs:6-13` — `selector()`, reusable verbatim for compile-time selector PUSH4s
- `src/tempo_tx.rs:86-94,448-454` — `TempoCall.to` is `[u8;20]` with no CREATE; the one struct/`rlp_call` change needed
- `contracts/src/libraries/LibDiamond.sol:57-59,168,202-219` — owner-gate, the `address(this)`-only immutable guard, and the `_init.delegatecall` vector (all verified)
- `contracts/src/Diamond.sol:14,28-41` — `constructor(_contractOwner,…)` enables child diamonds; `DELEGATECALL` fallback shaping storage/dispatch
- `src/builtins/call_agent.rs` (~168-227) — the §8 latent bug site
- `src/app/remote_call.rs:88-133` — the bridge block to lift into the fix
- `examples/tempo_tx_live.rs` — where the recommended first-step CREATE probe goes
- `src/confirm.rs`, `src/app/tool_allowlist.rs` — the confirm-gate + allowlist conventions all new tools must use

Suggested destination for the document: `design/soliditylite.md` (per the design/ convention; I did not create it, per the DESIGN-ONLY constraint).
