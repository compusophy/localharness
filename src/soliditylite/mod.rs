//! SolidityLite — a hand-rolled, in-browser Solidity/EVM-subset → EVM-bytecode
//! compiler (the EVM analog of [`crate::rustlite`]). This module is the
//! FOUNDATION: a bytecode assembler ([`asm`]) plus a worked emitter that proves
//! the dispatch/init scaffolding end-to-end against a deployable artifact.
//!
//! Full design: `design/soliditylite.md` (§4 compiler architecture, §5 EVM
//! target). Like rustlite, this is PURE Rust with no new dependencies and
//! compiles on BOTH native and `wasm32` (no `std::fs`, no I/O, no platform
//! calls) — the same self-sovereign property that lets it run in the user's
//! browser with no toolchain.
//!
//! ## What's here (Installment 1)
//!
//! - [`asm`] — the EVM bytecode assembler: raw opcodes, minimal-width `push`,
//!   two-pass absolute-jump label resolution, and [`asm::init_wrapper`] (the
//!   `CODECOPY`/`RETURN` contract-creation constructor).
//! - [`lexer`] / [`ast`] / [`parser`] — the FRONTEND: a `facet { fn+ }` source
//!   string → tokens → a [`ast::Facet`] tree, in the recursive-descent style of
//!   [`crate::rustlite`] (same `MAX_RECURSION_DEPTH` guard, same shared
//!   [`crate::rustlite::CompileError`]/[`crate::rustlite::Span`] diagnostics).
//! - [`codegen`] — the EVM emitter: a typed [`ast::Facet`] → runtime bytecode,
//!   wrapped into deployable init code. Owns the SHARED dispatch/body emission.
//! - [`compile`] — the top-level `&str → CompiledArtifact` pipeline, mirroring
//!   [`crate::rustlite::compile`].
//! - [`emit_constant_getter`] — the worked single-function emitter, refactored to
//!   drive [`codegen::assemble`] so its bytes are byte-IDENTICAL to the
//!   source-compiled path for the constant-getter case (the golden gate). Its
//!   `init_code` is the same shape already proven deployable on the live Tempo
//!   EVM (design `soliditylite.md` §4 update, loop tick 4).

/// EVM bytecode assembler: opcodes, minimal-width push, two-pass label
/// resolution, and the init-wrapper constructor.
pub mod asm;
/// SolidityLite AST — the parsed shape of the v1 facet subset.
pub mod ast;
/// EVM codegen — a typed facet → runtime bytecode + the shared dispatch/body emit.
pub mod codegen;
/// Byte-level lexer for the v1 Solidity-subset surface.
pub mod lexer;
/// Recursive-descent parser (with the rustlite recursion guard).
pub mod parser;

/// A compiled contract: the deployed runtime bytecode plus the full init code
/// (constructor + runtime) to hand to a CREATE transaction.
///
/// `init_code` is what you deploy; the chain runs it once and stores `runtime`
/// as the contract's deployed code. `init_code == asm::init_wrapper(&runtime)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledArtifact {
    /// The contract-creation INIT code (constructor prelude + runtime). This is
    /// the blob a CREATE tx carries.
    pub init_code: Vec<u8>,
    /// The deployed runtime bytecode (what `eth_getCode` returns after deploy).
    pub runtime: Vec<u8>,
    /// The 4-byte function selectors this facet dispatches, in declaration order
    /// — i.e. the `FacetCut.functionSelectors` needed to `diamondCut` it into a
    /// diamond. (`emit_constant_getter` yields the single getter's selector.)
    pub selectors: Vec<[u8; 4]>,
}

/// Emit a minimal single-function contract: a constant getter.
///
/// The runtime dispatches on a 4-byte selector and, on a match, returns a fixed
/// 32-byte big-endian word. It mirrors design §5's dispatcher + getter snippet
/// exactly:
///
/// ```text
/// PUSH1 0x04 CALLDATASIZE LT PUSH2 <FB> JUMPI   ; calldata < 4 bytes → revert
/// PUSH1 0x00 CALLDATALOAD PUSH1 0xE0 SHR        ; selector = calldata[0:32] >> 224
/// DUP1 PUSH4 <selector> EQ PUSH2 <BODY> JUMPI   ; if selector matches → body
/// FB: JUMPDEST PUSH1 0x00 PUSH1 0x00 REVERT     ; no match → revert
/// BODY: JUMPDEST
///       PUSH32 <value> PUSH1 0x00 MSTORE        ; mem[0..32] = value
///       PUSH1 0x20 PUSH1 0x00 RETURN            ; return mem[0..32]
/// ```
///
/// `value_be32` is returned verbatim as the 32-byte ABI-encoded `uint256`/word.
/// The all-zero placeholders for `<FB>`/`<BODY>` are back-patched by the
/// assembler in pass 2. Zeros use `PUSH1 0x00`, never `PUSH0`.
///
/// This now drives the SHARED [`codegen::assemble`] (a one-function call), so its
/// output is byte-IDENTICAL to [`compile`]'ing a single `return <intlit>;` facet
/// — the golden invariant the source-driven path is gated against.
pub fn emit_constant_getter(selector: [u8; 4], value_be32: [u8; 32]) -> CompiledArtifact {
    codegen::assemble(&[(selector, codegen::BodyValue::Const(value_be32))])
}

/// Compile a SolidityLite source string into a deployable [`CompiledArtifact`].
///
/// Pipeline (mirroring [`crate::rustlite::compile`]): `lex → parse → codegen`. The
/// v1 subset is `facet <Ident> { <stateVar>* <fn>+ }`:
/// - state vars: scalar `uint256 <name>;` (slot `BASE+i`) or
///   `mapping(<key> => <value>) <name>;` (entry slot
///   `keccak256(pad32(key) ++ pad32(BASE+i))`);
/// - functions: `function <name>(<params>) external [view] [returns (<ty>)] { … }`,
///   bodies of `return <expr>;` or `{ (<var>|<map>[<key>]) = <expr>; }*`;
/// - expressions: int literals, scalar reads (`SLOAD`), parameter reads
///   (`CALLDATALOAD(4+32*i)`), `msg.sender` (`CALLER`), `<map>[<key>]`
///   (keccak-slot read/write), and left-associative `+`.
///
/// Selectors are `keccak256("<name>(<types>)")[..4]` (the full ABI signature).
/// Errors are the shared [`crate::rustlite::CompileError`] (`LH0xxx` codes), each
/// pinned to a source span — `err.render(source)` shows the offending line + a
/// caret. Gated on `wallet` because selector + storage-slot keccak live there.
#[cfg(feature = "wallet")]
pub fn compile(source: &str) -> Result<CompiledArtifact, CompileError> {
    let tokens = lexer::lex(source)?;
    let facet = parser::parse(&tokens)?;
    codegen::compile(&facet)
}

#[cfg(feature = "wallet")]
use crate::rustlite::CompileError;

#[cfg(test)]
mod tests {
    use super::asm::op;
    use super::{emit_constant_getter, CompiledArtifact};

    /// `get()` selector = keccak256("get()")[0..4] = 0x6d4ce63c.
    const GET_SELECTOR: [u8; 4] = [0x6d, 0x4c, 0xe6, 0x3c];

    /// The 32-byte big-endian encoding of decimal 42 (0x00..002a).
    fn word_42() -> [u8; 32] {
        let mut w = [0u8; 32];
        w[31] = 42;
        w
    }

    #[test]
    fn dispatcher_prelude_matches_design_section_5() {
        let CompiledArtifact { runtime, .. } = emit_constant_getter(GET_SELECTOR, word_42());
        // The prelude bytes are FIXED (no label patching affects them except the
        // two PUSH2 placeholders, which we check by value below). Mirror the §5
        // worked snippet byte-for-byte.
        //
        // offset 0000  60 04        PUSH1 0x04
        // offset 0002  36           CALLDATASIZE
        // offset 0003  10           LT
        // offset 0004  61 ?? ??     PUSH2 <FB>
        // offset 0007  57           JUMPI
        // offset 0008  60 00        PUSH1 0x00
        // offset 000a  35           CALLDATALOAD
        // offset 000b  60 e0        PUSH1 0xe0
        // offset 000d  1c           SHR
        // offset 000e  80           DUP1
        // offset 000f  63 6d4ce63c  PUSH4 get()
        // offset 0014  14           EQ
        // offset 0015  61 ?? ??     PUSH2 <BODY>
        // offset 0018  57           JUMPI
        assert_eq!(&runtime[0..2], &[op::PUSH1, 0x04]);
        assert_eq!(runtime[2], op::CALLDATASIZE);
        assert_eq!(runtime[3], op::LT);
        assert_eq!(runtime[4], op::PUSH2);
        // 5..7 = FB operand (checked for validity below)
        assert_eq!(runtime[7], op::JUMPI);
        assert_eq!(&runtime[8..10], &[op::PUSH1, 0x00]);
        assert_eq!(runtime[10], op::CALLDATALOAD);
        assert_eq!(&runtime[11..13], &[op::PUSH1, 0xE0]);
        assert_eq!(runtime[13], op::SHR);
        assert_eq!(runtime[14], op::DUP1);
        // PUSH4 (0x63) followed by the 4 selector bytes
        assert_eq!(runtime[15], op::PUSH1 + 3); // PUSH4 == 0x63
        assert_eq!(&runtime[16..20], &GET_SELECTOR);
        assert_eq!(runtime[20], op::EQ);
        assert_eq!(runtime[21], op::PUSH2);
        // 22..24 = BODY operand
        assert_eq!(runtime[24], op::JUMPI);

        // The two PUSH2 operands point at real JUMPDESTs.
        let fb = u16::from_be_bytes([runtime[5], runtime[6]]) as usize;
        let body = u16::from_be_bytes([runtime[22], runtime[23]]) as usize;
        assert_eq!(runtime[fb], op::JUMPDEST, "FB must land on a JUMPDEST");
        assert_eq!(runtime[body], op::JUMPDEST, "BODY must land on a JUMPDEST");
        // FB comes before BODY (fallback stub is emitted first).
        assert!(fb < body);
    }

    #[test]
    fn fallback_stub_is_revert_0_0() {
        let CompiledArtifact { runtime, .. } = emit_constant_getter(GET_SELECTOR, word_42());
        let fb = u16::from_be_bytes([runtime[5], runtime[6]]) as usize;
        // FB: JUMPDEST PUSH1 0x00 PUSH1 0x00 REVERT
        assert_eq!(
            &runtime[fb..fb + 6],
            &[op::JUMPDEST, op::PUSH1, 0x00, op::PUSH1, 0x00, op::REVERT]
        );
    }

    #[test]
    fn body_returns_the_32_byte_value() {
        let value = word_42();
        let CompiledArtifact { runtime, .. } = emit_constant_getter(GET_SELECTOR, value);
        let body = u16::from_be_bytes([runtime[22], runtime[23]]) as usize;
        // BODY: JUMPDEST PUSH32 <value> PUSH1 0x00 MSTORE PUSH1 0x20 PUSH1 0x00 RETURN
        assert_eq!(runtime[body], op::JUMPDEST);
        assert_eq!(runtime[body + 1], op::PUSH1 + 31); // PUSH32
        assert_eq!(&runtime[body + 2..body + 34], &value);
        assert_eq!(
            &runtime[body + 34..body + 42],
            &[op::PUSH1, 0x00, op::MSTORE, op::PUSH1, 0x20, op::PUSH1, 0x00, op::RETURN]
        );
        // the runtime ends exactly at the RETURN
        assert_eq!(body + 42, runtime.len());
    }

    #[test]
    fn init_code_wraps_the_runtime() {
        let artifact = emit_constant_getter(GET_SELECTOR, word_42());
        // init_code is exactly init_wrapper(runtime): 13-byte prelude + runtime.
        assert_eq!(
            artifact.init_code,
            super::asm::init_wrapper(&artifact.runtime)
        );
        assert_eq!(artifact.init_code.len(), 13 + artifact.runtime.len());
        // the prelude encodes the runtime's length in its first PUSH2
        let rt_len = artifact.runtime.len() as u16;
        assert_eq!(
            &artifact.init_code[0..3],
            &[op::PUSH2, rt_len.to_be_bytes()[0], rt_len.to_be_bytes()[1]]
        );
        assert_eq!(&artifact.init_code[13..], &artifact.runtime[..]);
    }

    /// Cross-check the hard-coded `get()` selector against a live keccak256 (the
    /// `registry::abi::selector` helper this compiler will reuse for compile-time
    /// PUSH4s). Gated on `wallet` because that's where `sha3` / `registry` live.
    #[cfg(feature = "wallet")]
    #[test]
    fn get_selector_matches_keccak() {
        assert_eq!(crate::registry::selector("get()"), GET_SELECTOR);
    }

    /// Golden hex of the FULL `init_code` for `emit_constant_getter(get(), 42)` —
    /// the exact blob deployed via CREATE for a live `eth_call get()` == 42. A
    /// byte-for-byte pin so any accidental opcode/offset drift is caught here.
    #[test]
    fn init_code_golden_hex_for_get_42() {
        let artifact = emit_constant_getter(GET_SELECTOR, word_42());
        // Runtime = 73 bytes (0x49); prelude = 13 bytes; init_code = 86 bytes.
        assert_eq!(artifact.runtime.len(), 73, "runtime length pin");
        assert_eq!(artifact.init_code.len(), 13 + 73);

        let hex = to_hex(&artifact.init_code);

        // The exact deploy blob, byte-for-byte. Annotated layout:
        //   PRELUDE (13B): 61 0049  80  61 000d  60 00  39  60 00  f3
        //     PUSH2 0x0049(rt_len) DUP1 PUSH2 0x000d(rt_off) PUSH1 0 CODECOPY PUSH1 0 RETURN
        //   RUNTIME (73B), FB JUMPDEST at runtime-offset 0x19, BODY at 0x1f:
        //     60 04 36 10 61 0019 57          guard: calldatasize<4 → FB
        //     60 00 35 60 e0 1c               selector = calldata[0:32] >> 224
        //     80 63 6d4ce63c 14 61 001f 57    DUP1 PUSH4 get() EQ → BODY
        //     5b 60 00 60 00 fd               FB: REVERT(0,0)
        //     5b 7f <32-byte 0x..2a> 60 00 52 60 20 60 00 f3   BODY: MSTORE+RETURN
        let expected = "0x\
            6100498061000d6000396000f3\
            6004361061001957\
            60003560e01c\
            80636d4ce63c1461001f57\
            5b60006000fd\
            5b7f000000000000000000000000000000000000000000000000000000000000002a6000526020600 0f3"
            .replace([' ', '\n'], ""); // readability-split literal; whitespace stripped
        assert_eq!(hex, expected, "init_code drifted: {hex}");
    }

    // ── FRONTEND (source → bytecode) tests ──────────────────────────────────

    /// THE GOLDEN GATE: the source-driven path produces the SAME bytecode as the
    /// hand-built emitter (which is already proven to deploy + run live). If this
    /// holds, `compile`'s output for the constant-getter case inherits the live
    /// proof for free.
    #[cfg(feature = "wallet")]
    #[test]
    fn compile_get_42_equals_emit_constant_getter() {
        let from_source =
            super::compile("facet C { function get() external view returns (uint256) { return 42; } }")
                .expect("floor-grammar source must compile");
        let from_emitter = emit_constant_getter([0x6d, 0x4c, 0xe6, 0x3c], word_42());
        assert_eq!(
            from_source.init_code, from_emitter.init_code,
            "source-compiled init_code must be byte-identical to the hand-built emitter"
        );
        assert_eq!(from_source.runtime, from_emitter.runtime);
    }

    /// A 2-function facet compiles; each function's body returns its own constant
    /// when reached via its own selector. Spot-check the structure: two dispatch
    /// arms (each `DUP1 PUSH4 <sel> EQ PUSH2 <body> JUMPI`) before the fallback.
    #[cfg(feature = "wallet")]
    #[test]
    fn compile_two_function_facet() {
        let art = super::compile(
            "facet Two { function a() external view returns (uint256) { return 1; } \
             function b() external view returns (uint256) { return 2; } }",
        )
        .expect("a 2-function facet must compile");
        let rt = &art.runtime;
        // Selectors: a() and b().
        let sel_a = crate::registry::selector("a()");
        let sel_b = crate::registry::selector("b()");
        // Both PUSH4 selector immediates appear in the dispatch region.
        let push4_a: Vec<u8> = std::iter::once(op::PUSH1 + 3).chain(sel_a).collect();
        let push4_b: Vec<u8> = std::iter::once(op::PUSH1 + 3).chain(sel_b).collect();
        assert!(
            rt.windows(push4_a.len()).any(|w| w == push4_a.as_slice()),
            "a() selector PUSH4 must be present"
        );
        assert!(
            rt.windows(push4_b.len()).any(|w| w == push4_b.as_slice()),
            "b() selector PUSH4 must be present"
        );
        // The constants 1 and 2 are each returned by a PUSH32 body.
        let mut one = [0u8; 32];
        one[31] = 1;
        let mut two = [0u8; 32];
        two[31] = 2;
        assert!(rt.windows(32).any(|w| w == one), "constant 1 must be embedded");
        assert!(rt.windows(32).any(|w| w == two), "constant 2 must be embedded");
        // init_code wraps the runtime.
        assert_eq!(art.init_code, super::asm::init_wrapper(rt));
    }

    /// `if`/`else`/`else if` control flow + `==`/`!=` compile (the branch
    /// stretch). One facet exercises every new shape: an `if (==)`, an
    /// `else if (!=)` chain, a `require` NESTED inside a branch (which must still
    /// allocate the shared revert stub), and a plain `else`. Behavior is proven
    /// live on-chain (see the loop-tick report); this pins that the front-to-back
    /// pipeline accepts the grammar and lowers the nested branch without error.
    #[cfg(feature = "wallet")]
    #[test]
    fn compile_if_else_and_neq() {
        let art = super::compile(
            "facet Gate { uint256 v; \
             function set(uint256 x) external { \
                 if (x == 0) { v = 1; } \
                 else if (x != 5) { require(x < 100, \"hi\"); v = x; } \
                 else { v = 5; } \
             } \
             function get() external view returns (uint256) { return v; } }",
        )
        .expect("if/else/else-if + ==/!= + nested require must compile");
        assert_eq!(art.selectors.len(), 2, "set + get");
        // The branch emits an unconditional JUMP over the else, and the nested
        // require keeps the REVERT stub.
        assert!(art.runtime.contains(&op::JUMP), "an if/else must JUMP over the else branch");
        assert!(art.runtime.contains(&op::REVERT), "a require nested in a branch keeps the revert stub");
        assert_eq!(art.init_code, super::asm::init_wrapper(&art.runtime));
    }

    /// Bad source returns a clean `CompileError` (not a panic), with a coded,
    /// span-pinned diagnostic that `render`s a caret.
    #[cfg(feature = "wallet")]
    #[test]
    fn bad_source_is_a_clean_compile_error() {
        // Missing the trailing `;` after the return.
        let src = "facet C { function get() external view returns (uint256) { return 42 } }";
        let err = super::compile(src).expect_err("missing semicolon must fail cleanly");
        assert!(err.code.is_some(), "error must carry an LH code");
        assert!(err.to_string().starts_with("LH0"), "surfaced: {err}");
        // The rendered diagnostic points at a source location.
        assert!(err.render(src).contains("line "), "{}", err.render(src));
        // An empty facet is also a clean error, never a panic.
        assert!(super::compile("facet C { }").is_err());
        // A stray byte the floor grammar can't begin a token with.
        assert!(super::compile("facet C { @ }").is_err());
    }

    /// THE STATEFUL TARGET (Installment 1 storage-write MVP): the `Tally` facet —
    /// a mutating `bump()` (`n = n + 1`) plus a view `get()` — compiles, both
    /// selectors are dispatched, and `init_code` wraps the runtime. The exact
    /// bump() opcode sequence is pinned in `codegen::tests::tally_bump_*`.
    #[cfg(feature = "wallet")]
    #[test]
    fn compile_tally_facet_with_a_storage_write() {
        let art = super::compile(
            "facet Tally { uint256 n; \
             function bump() external { n = n + 1; } \
             function get() external view returns (uint256) { return n; } }",
        )
        .expect("the Tally facet (storage write) must compile");
        let rt = &art.runtime;

        // bump() = keccak256("bump()")[..4]; get() = 0x6d4ce63c.
        let sel_bump = crate::registry::selector("bump()"); // = 0x68110b2f
        let sel_get = crate::registry::selector("get()");
        assert_eq!(sel_bump, [0x68, 0x11, 0x0b, 0x2f], "bump() selector pin");
        assert_eq!(sel_get, [0x6d, 0x4c, 0xe6, 0x3c], "get() selector pin");

        // Both dispatch arms (PUSH4 <sel>) are present.
        for sel in [sel_bump, sel_get] {
            let push4: Vec<u8> = std::iter::once(op::PUSH1 + 3).chain(sel).collect();
            assert!(
                rt.windows(push4.len()).any(|w| w == push4.as_slice()),
                "selector PUSH4 {sel:02x?} must be dispatched"
            );
        }
        // The runtime stores (SSTORE) — the new capability — and reads (SLOAD).
        assert!(rt.contains(&op::SSTORE), "Tally must SSTORE (the storage write)");
        assert!(rt.contains(&op::SLOAD), "Tally must SLOAD (reads n)");
        assert!(rt.contains(&op::ADD), "Tally must ADD (n + 1)");

        // init_code wraps the runtime.
        assert_eq!(art.init_code, super::asm::init_wrapper(rt));
    }

    /// THE INSTALLMENT-1 TARGET: the `Ledger` facet — a `mapping(address =>
    /// uint256)`, a mutating `add(uint256 amt)` that writes `bal[msg.sender]`, and a
    /// view `balanceOf(address who)` — compiles end-to-end. Both selectors (which
    /// now INCLUDE the param types) are dispatched, and the runtime exercises the
    /// three new primitives: CALLDATALOAD (params), CALLER (msg.sender), KECCAK256
    /// (mapping slot derivation).
    #[cfg(feature = "wallet")]
    #[test]
    fn compile_ledger_target_facet() {
        let art = super::compile(
            "facet Ledger { mapping(address => uint256) bal; \
             function add(uint256 amt) external { bal[msg.sender] = bal[msg.sender] + amt; } \
             function balanceOf(address who) external view returns (uint256) { return bal[who]; } }",
        )
        .expect("the Ledger TARGET facet must compile");
        let rt = &art.runtime;

        // Selectors are over the FULL ABI signature (with param types).
        let sel_add = crate::registry::selector("add(uint256)");
        let sel_balance_of = crate::registry::selector("balanceOf(address)");
        for sel in [sel_add, sel_balance_of] {
            let push4: Vec<u8> = std::iter::once(op::PUSH1 + 3).chain(sel).collect();
            assert!(
                rt.windows(push4.len()).any(|w| w == push4.as_slice()),
                "selector PUSH4 {sel:02x?} must be dispatched"
            );
        }
        // The three new primitives are all present in the runtime.
        assert!(rt.contains(&op::CALLDATALOAD), "params decode via CALLDATALOAD");
        assert!(rt.contains(&op::CALLER), "msg.sender emits CALLER");
        assert!(rt.contains(&op::KECCAK256), "mapping slot derivation uses KECCAK256");
        assert!(rt.contains(&op::SSTORE), "add() writes via SSTORE");
        assert!(rt.contains(&op::SLOAD), "balanceOf reads via SLOAD");
        // init_code wraps the runtime.
        assert_eq!(art.init_code, super::asm::init_wrapper(rt));
    }

    /// THE INSTALLMENT-1 CounterFacet TARGET (require + comparisons): the full
    /// `Counter` facet (minus the event) compiles end-to-end via [`compile`], all
    /// four canonical selectors are dispatched, and the runtime exercises the new
    /// relational/guard primitives (GT, GT+ISZERO for `<=`, and the require
    /// ISZERO/JUMPI branch to a REVERT stub).
    #[cfg(feature = "wallet")]
    #[test]
    fn compile_counter_target_facet() {
        const SRC: &str = "facet Counter { mapping(address => uint256) count; uint256 total; \
             function increment() external { count[msg.sender] = count[msg.sender] + 1; total = total + 1; } \
             function incrementBy(uint256 n) external { require(n > 0, \"zero\"); require(n <= 100, \"too big\"); \
             count[msg.sender] = count[msg.sender] + n; total = total + n; } \
             function countOf(address who) external view returns (uint256) { return count[who]; } \
             function totalCount() external view returns (uint256) { return total; } }";
        let art = super::compile(SRC).expect("the CounterFacet TARGET must compile");
        let rt = &art.runtime;

        // The four canonical selectors (pinned in the task).
        for (sig, want) in [
            ("increment()", [0xd0u8, 0x9d, 0xe0, 0x8a]),
            ("incrementBy(uint256)", [0x03, 0xdf, 0x17, 0x9c]),
            ("countOf(address)", [0xf8, 0x97, 0x7e, 0x96]),
            ("totalCount()", [0x34, 0xea, 0xfb, 0x11]),
        ] {
            assert_eq!(crate::registry::selector(sig), want, "selector pin for {sig}");
            let push4: Vec<u8> = std::iter::once(op::PUSH1 + 3).chain(want).collect();
            assert!(
                rt.windows(push4.len()).any(|w| w == push4.as_slice()),
                "{sig} selector must be dispatched"
            );
        }
        // The new primitives are present.
        assert!(rt.contains(&op::GT), "comparisons emit GT");
        assert!(rt.windows(2).any(|w| w == [op::GT, op::ISZERO]), "`<=` → GT ISZERO");
        assert!(
            rt.windows(5).any(|w| w[0] == op::ISZERO && w[1] == op::PUSH2 && w[4] == op::JUMPI),
            "require → ISZERO/JUMPI branch"
        );
        // init_code wraps the runtime.
        assert_eq!(art.init_code, super::asm::init_wrapper(rt));
    }

    /// THE INSTALLMENT-1 MVP CAPSTONE: the FULL `CounterFacet` (mappings + scalars +
    /// require + comparisons + an `event` declaration + two `emit`s) compiles
    /// end-to-end via [`compile`], all four canonical selectors are dispatched, and
    /// the `Incremented` LOG2 fires with the correct full-keccak `topic0`. This is
    /// the exact target source pinned in the task.
    #[cfg(feature = "wallet")]
    #[test]
    fn compile_full_counter_facet_with_event() {
        const SRC: &str = "facet CounterFacet { mapping(address => uint256) count; uint256 total; \
             event Incremented(address indexed who, uint256 newCount, uint256 newTotal); \
             function increment() external { count[msg.sender] = count[msg.sender] + 1; total = total + 1; \
             emit Incremented(msg.sender, count[msg.sender], total); } \
             function incrementBy(uint256 n) external { require(n > 0, \"zero\"); require(n <= 100, \"too big\"); \
             count[msg.sender] = count[msg.sender] + n; total = total + n; \
             emit Incremented(msg.sender, count[msg.sender], total); } \
             function countOf(address who) external view returns (uint256) { return count[who]; } \
             function totalCount() external view returns (uint256) { return total; } }";
        let art = super::compile(SRC).expect("the FULL CounterFacet (with events) must compile");
        let rt = &art.runtime;

        // The four canonical selectors (pinned in the task).
        for (sig, want) in [
            ("increment()", [0xd0u8, 0x9d, 0xe0, 0x8a]),
            ("incrementBy(uint256)", [0x03, 0xdf, 0x17, 0x9c]),
            ("countOf(address)", [0xf8, 0x97, 0x7e, 0x96]),
            ("totalCount()", [0x34, 0xea, 0xfb, 0x11]),
        ] {
            assert_eq!(crate::registry::selector(sig), want, "selector pin for {sig}");
            let push4: Vec<u8> = std::iter::once(op::PUSH1 + 3).chain(want).collect();
            assert!(
                rt.windows(push4.len()).any(|w| w == push4.as_slice()),
                "{sig} selector must be dispatched"
            );
        }
        // The Incremented topic0 (full keccak, NOT the 4-byte selector) is PUSH32'd.
        let topic0 = super::codegen::event_topic0("Incremented(address,uint256,uint256)");
        let mut push32: Vec<u8> = vec![op::PUSH1 + 31];
        push32.extend_from_slice(&topic0);
        assert!(
            rt.windows(33).any(|w| w == push32.as_slice()),
            "the Incremented event topic0 must be PUSH32'd"
        );
        // init_code wraps the runtime.
        assert_eq!(art.init_code, super::asm::init_wrapper(rt));
    }

    /// A require with a true constant compiles (codegen-shape); ISZERO of a truthy
    /// constant is 0 so the branch is never taken at runtime — proving a passing
    /// guard does not revert.
    #[cfg(feature = "wallet")]
    #[test]
    fn compile_require_true_constant_is_well_formed() {
        let art = super::compile(
            "facet C { function f() external { require(1 == 1, \"never\"); } }",
        )
        .expect("require(true) must compile");
        assert_eq!(art.init_code, super::asm::init_wrapper(&art.runtime));
    }

    /// Breadth must NOT trip the depth guard: a facet with far more functions than
    /// `MAX_RECURSION_DEPTH` still compiles, proving the guard counts NESTING (per
    /// parse path), not the number of declared functions. (The guard's depth-cap
    /// trigger itself is unit-tested directly in `parser::tests::recursion_guard_*`,
    /// since the floor grammar's flat expressions can't naturally nest 96 deep.)
    #[cfg(feature = "wallet")]
    #[test]
    fn breadth_does_not_trip_the_depth_guard() {
        let mut src = String::from("facet Many {");
        for i in 0..300 {
            src.push_str(&format!(
                " function f{i}() external view returns (uint256) {{ return {i}; }}"
            ));
        }
        src.push('}');
        assert!(super::compile(&src).is_ok(), "breadth (many fns) must not trip the depth guard");
    }

    /// 0x-prefixed lowercase hex of a byte slice (test helper).
    fn to_hex(bytes: &[u8]) -> String {
        use core::fmt::Write;
        let mut s = String::with_capacity(2 + bytes.len() * 2);
        s.push_str("0x");
        for b in bytes {
            let _ = write!(s, "{b:02x}");
        }
        s
    }
}
