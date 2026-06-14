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
//! ## What's here (Installment 1 foundation)
//!
//! - [`asm`] — the EVM bytecode assembler: raw opcodes, minimal-width `push`,
//!   two-pass absolute-jump label resolution, and [`asm::init_wrapper`] (the
//!   `CODECOPY`/`RETURN` contract-creation constructor).
//! - [`emit_constant_getter`] — a worked emitter that mirrors design §5's
//!   dispatcher + getter snippet exactly, producing a [`CompiledArtifact`] whose
//!   `init_code` is directly deployable via a CREATE transaction.
//!
//! The real compiler pipeline (`lex → parse → typecheck → codegen`, mirroring
//! [`crate::rustlite::compile`]) is the next layer and is NOT yet built.

/// EVM bytecode assembler: opcodes, minimal-width push, two-pass label
/// resolution, and the init-wrapper constructor.
pub mod asm;

use asm::{op, Asm};

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
pub fn emit_constant_getter(selector: [u8; 4], value_be32: [u8; 32]) -> CompiledArtifact {
    let mut a = Asm::new();
    let fb = a.new_label();
    let body = a.new_label();

    // --- calldatasize guard: `< 4 bytes → fallback revert` ---
    // PUSH1 0x04 CALLDATASIZE LT PUSH2 <FB> JUMPI
    a.push_u64(0x04)
        .emit(op::CALLDATASIZE)
        .emit(op::LT)
        .push_label(fb)
        .emit(op::JUMPI);

    // --- selector extract: `calldata[0:32] >> 224` ---
    // PUSH1 0x00 CALLDATALOAD PUSH1 0xE0 SHR
    a.push_u64(0x00)
        .emit(op::CALLDATALOAD)
        .push_u64(0xE0)
        .emit(op::SHR);

    // --- dispatch: `DUP1 PUSH4 <sel> EQ PUSH2 <BODY> JUMPI` ---
    a.emit(op::DUP1)
        .push(&selector) // PUSH4 (selectors are 4 significant bytes; non-zero high byte)
        .emit(op::EQ)
        .push_label(body)
        .emit(op::JUMPI);

    // --- fallback: no match → REVERT(0, 0) ---
    // FB: JUMPDEST PUSH1 0x00 PUSH1 0x00 REVERT
    a.jumpdest(fb)
        .push_u64(0x00)
        .push_u64(0x00)
        .emit(op::REVERT);

    // --- body: MSTORE(0, value) ; RETURN(0, 0x20) ---
    // BODY: JUMPDEST PUSH32 <value> PUSH1 0x00 MSTORE PUSH1 0x20 PUSH1 0x00 RETURN
    a.jumpdest(body)
        .push32(&value_be32) // PUSH32 — the full 32-byte word, per design §5
        .push_u64(0x00)
        .emit(op::MSTORE)
        .push_u64(0x20)
        .push_u64(0x00)
        .emit(op::RETURN);

    let runtime = a.finish();
    let init_code = asm::init_wrapper(&runtime);
    CompiledArtifact { init_code, runtime }
}

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
