//! EVM bytecode assembler — the hand-rolled emitter for SolidityLite's EVM
//! target (the analog of [`crate::rustlite::codegen`]'s wasm emitter, but for an
//! ABSOLUTE-jump machine instead of wasm's structured/relative control flow).
//!
//! Discipline mirrors rustlite: a single struct accumulates a `Vec<u8>`, opcodes
//! are named consts, and a final pass produces the bytes. Pure Rust, no deps, no
//! I/O — compiles on native AND `wasm32` exactly like rustlite.
//!
//! ## Two-pass label resolution (design `soliditylite.md` §4 invariant 2 + §5)
//!
//! EVM jumps take an ABSOLUTE program-counter operand, so a forward jump's target
//! is unknown when the jump is emitted. We resolve in two passes, with NO width
//! fixpoint:
//!
//! - **Pass 1** ([`Asm::push_label`]) emits a FIXED-WIDTH `PUSH2 0x0000`
//!   placeholder for every label reference and records its operand byte offset.
//!   [`Asm::jumpdest`] emits the `0x5B JUMPDEST` opcode and records that label's
//!   byte offset. Because every reference is always exactly `PUSH2` (3 bytes),
//!   emitting a `jumpdest` never shifts any prior offset.
//! - **Pass 2** ([`Asm::finish`]) back-patches each recorded reference's 2
//!   big-endian operand bytes with its label's resolved offset. One pass, no
//!   iteration to a fixpoint.

/// EVM opcodes used by the assembler + the worked emitter. Named consts in the
/// rustlite style (see `src/rustlite/codegen.rs`); `_`-prefixed ones are part of
/// the documented instruction set but not yet emitted.
pub mod op {
    /// Halt + revert, returning `mem[offset..offset+len]` (`REVERT(off, len)`).
    pub const REVERT: u8 = 0xFD;
    /// Halt + return `mem[offset..offset+len]` as the call's output.
    pub const RETURN: u8 = 0xF3;
    /// Copy `len` bytes of THIS contract's code into memory (`CODECOPY`).
    pub const CODECOPY: u8 = 0x39;
    /// Load a 32-byte word from memory (`MLOAD(off)`). Not emitted by codegen, but
    /// the diff-harness interpreter supports it, so it lives here (asm.rs is the
    /// opcode SSOT — no stray literals in the interpreter).
    pub const MLOAD: u8 = 0x51;
    /// Store a 32-byte word into memory (`MSTORE(off, word)`).
    pub const MSTORE: u8 = 0x52;
    /// Load a 32-byte word from storage (`SLOAD(slot)`).
    pub const SLOAD: u8 = 0x54;
    /// Store a 32-byte word into storage (`SSTORE(slot, word)`).
    pub const SSTORE: u8 = 0x55;
    /// Size of the call's calldata in bytes (`CALLDATASIZE`).
    pub const CALLDATASIZE: u8 = 0x36;
    /// Load a 32-byte word from calldata at `off` (`CALLDATALOAD(off)`).
    pub const CALLDATALOAD: u8 = 0x35;
    /// Copy `len` calldata bytes into memory (`CALLDATACOPY(destOff, srcOff, len)`),
    /// zero-extending past the end of calldata (mirrors the EVM). Used to bulk-copy a
    /// dynamic `string`/`bytes` argument's `[length ‖ data]` tail into memory for an
    /// ABI re-encode (the param-echo path, no per-word loop).
    pub const CALLDATACOPY: u8 = 0x37;
    /// Unsigned less-than (`a < b`).
    pub const LT: u8 = 0x10;
    /// Unsigned greater-than (`a > b`).
    pub const GT: u8 = 0x11;
    /// Equality (`a == b`).
    pub const EQ: u8 = 0x14;
    /// Logical NOT: `1` if the top item is `0`, else `0` (`ISZERO(x)`). Used to
    /// invert a comparison (`a <= b` = `ISZERO(a > b)`) and to branch on a failed
    /// `require` condition (`cond ISZERO … JUMPI` → revert when the cond is false).
    pub const ISZERO: u8 = 0x15;
    /// Logical right shift (`SHR(shift, value)`).
    pub const SHR: u8 = 0x1C;
    /// Bitwise AND (`a & b`). Used to test a dynamic-string slot's low bit
    /// (`slot & 1` → short/long discriminator) and to isolate the SHORT-layout
    /// length byte (`slot & 0xff`).
    pub const AND: u8 = 0x16;
    /// Addition.
    pub const ADD: u8 = 0x01;
    /// Subtraction — `SUB` computes `μs[0] - μs[1]` (top minus next), wrapping mod
    /// 2^256 on underflow (NO 0.8-style revert in v1).
    pub const SUB: u8 = 0x03;
    /// Multiplication — wraps mod 2^256 on overflow (no 0.8 revert in v1).
    pub const MUL: u8 = 0x02;
    /// Integer division `μs[0] / μs[1]` — yields 0 when the divisor is 0 (EVM, no revert).
    pub const DIV: u8 = 0x04;
    /// Modulo `μs[0] % μs[1]` — yields 0 when the divisor is 0 (EVM, no revert).
    pub const MOD: u8 = 0x06;
    /// Keccak-256 of `mem[offset..offset+len]` (`KECCAK256(offset, len)`, aka SHA3) —
    /// used to derive a mapping-entry storage slot from `key ++ baseSlot`.
    pub const KECCAK256: u8 = 0x20;
    /// The 20-byte caller address, left-padded to a 32-byte word (`CALLER`, aka
    /// `msg.sender`).
    pub const CALLER: u8 = 0x33;
    /// The current block's unix timestamp as a word (`TIMESTAMP`, `block.timestamp`).
    pub const TIMESTAMP: u8 = 0x42;
    /// The current block height as a word (`NUMBER`, `block.number`).
    pub const NUMBER: u8 = 0x43;
    /// Duplicate the top stack item (`DUP1`).
    pub const DUP1: u8 = 0x80;
    /// Duplicate the 2nd-from-top stack item (`DUP2`). Reaches a loop induction
    /// variable / frame slot sitting under the top — the dynamic-string copy loop
    /// keeps a `[dataSlot0, count, i]` frame and reads beneath `i`.
    pub const DUP2: u8 = 0x81;
    /// Duplicate the 3rd-from-top stack item (`DUP3`). Reaches `count`/`dataSlot0`
    /// in the copy loop's 3-deep frame each iteration.
    pub const DUP3: u8 = 0x82;
    /// Swap the top two stack items (`SWAP1`). Reorders operands (e.g. `slot - 1`
    /// for the LONG length decode) without re-deriving from storage.
    pub const SWAP1: u8 = 0x90;
    /// Pop the top stack item (`POP`).
    pub const POP: u8 = 0x50;
    /// Unconditional absolute jump (`JUMP(dest)`).
    pub const JUMP: u8 = 0x56;
    /// Conditional absolute jump (`JUMPI(dest, cond)`).
    pub const JUMPI: u8 = 0x57;
    /// Valid jump target marker (`JUMPDEST`).
    pub const JUMPDEST: u8 = 0x5B;
    /// `LOG0(offset, len)` — emit a log with 0 topics over `mem[offset..offset+len]`.
    pub const LOG0: u8 = 0xA0;
    /// `LOG1(offset, len, topic0)` — a log with 1 topic.
    pub const LOG1: u8 = 0xA1;
    /// `LOG2(offset, len, topic0, topic1)` — a log with 2 topics.
    pub const LOG2: u8 = 0xA2;
    /// `LOG3(offset, len, topic0, topic1, topic2)` — a log with 3 topics.
    pub const LOG3: u8 = 0xA3;
    /// `LOG4(offset, len, topic0..topic3)` — a log with 4 topics (the EVM max).
    pub const LOG4: u8 = 0xA4;
    /// `PUSH1` base; `PUSH<n>` = `PUSH1 + (n - 1)`.
    pub const PUSH1: u8 = 0x60;
    /// `PUSH2` (push 2 bytes) — the fixed width used for every label reference.
    pub const PUSH2: u8 = 0x61;
}

/// A label: a named jump target whose absolute PC is resolved in pass 2.
///
/// Obtained from [`Asm::new_label`], placed with [`Asm::jumpdest`], and
/// referenced with [`Asm::push_label`]. A label may be referenced before OR
/// after it is placed (forward and back jumps), and may be referenced any number
/// of times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Label(usize);

/// The EVM bytecode assembler. Build a program with the `emit*`/`push*`/`jumpdest`
/// methods, then call [`Asm::finish`] to back-patch labels and get the bytes.
#[derive(Debug, Default)]
pub struct Asm {
    /// The accumulating bytecode (with `PUSH2 0x0000` placeholders for labels).
    code: Vec<u8>,
    /// `dests[label.0]` = the byte offset of that label's `JUMPDEST`, or `None`
    /// until [`Asm::jumpdest`] places it.
    dests: Vec<Option<usize>>,
    /// Pending back-patches: `(operand_byte_offset, label)`. The operand offset
    /// points at the FIRST of the 2 big-endian bytes following a `PUSH2`.
    refs: Vec<(usize, Label)>,
}

impl Asm {
    /// A fresh, empty assembler.
    pub fn new() -> Self {
        Self::default()
    }

    /// The current byte offset (the PC of the next emitted byte). A `JUMPDEST`
    /// emitted now would live at this offset.
    pub fn here(&self) -> usize {
        self.code.len()
    }

    /// Emit a single raw opcode byte (no operand). Use the [`op`] consts.
    pub fn emit(&mut self, opcode: u8) -> &mut Self {
        self.code.push(opcode);
        self
    }

    /// Emit several raw opcode bytes in order.
    pub fn emit_all(&mut self, opcodes: &[u8]) -> &mut Self {
        self.code.extend_from_slice(opcodes);
        self
    }

    /// Push a big-endian integer using the MINIMAL `PUSH<n>` that fits.
    ///
    /// `bytes` is interpreted big-endian; leading zero bytes are stripped before
    /// selecting the push width. The all-zero value emits `PUSH1 0x00` (NOT
    /// `PUSH0`/EIP-3855 — we treat its availability conservatively per design §5,
    /// even though a later live probe found Moderato supports it). Inputs wider
    /// than 32 bytes are rejected via a debug assertion (an EVM word is at most
    /// 32 bytes); in release the trailing 32 significant bytes are used.
    pub fn push(&mut self, bytes: &[u8]) -> &mut Self {
        // Strip leading zeros to find the minimal significant width.
        let first = bytes.iter().position(|&b| b != 0);
        let sig: &[u8] = match first {
            None => &[0u8], // all-zero (or empty) → PUSH1 0x00
            Some(i) => &bytes[i..],
        };
        debug_assert!(sig.len() <= 32, "EVM PUSH operand exceeds 32 bytes");
        let sig = if sig.len() > 32 { &sig[sig.len() - 32..] } else { sig };
        let n = sig.len() as u8; // 1..=32
        self.code.push(op::PUSH1 + (n - 1));
        self.code.extend_from_slice(sig);
        self
    }

    /// Push a `u64` constant (minimal width, big-endian) — a convenience over
    /// [`Asm::push`] for the small integer operands the emitter needs
    /// (memory/calldata offsets, lengths).
    pub fn push_u64(&mut self, value: u64) -> &mut Self {
        self.push(&value.to_be_bytes())
    }

    /// Push a full 32-byte word with `PUSH32`, NO leading-zero stripping.
    ///
    /// Use this where the FULL word is semantically required — a keccak-derived
    /// storage slot or a 32-byte return value — and the design §5 snippets show a
    /// literal `PUSH32`. (Plain [`Asm::push`] would minimize the width, which is
    /// equally correct for `MSTORE`'s left-padding but does not match the worked
    /// dispatcher/getter bytecode.)
    pub fn push32(&mut self, word: &[u8; 32]) -> &mut Self {
        self.code.push(op::PUSH1 + 31); // PUSH32
        self.code.extend_from_slice(word);
        self
    }

    /// Allocate a fresh, unplaced label.
    pub fn new_label(&mut self) -> Label {
        let id = self.dests.len();
        self.dests.push(None);
        Label(id)
    }

    /// Place `label` at the current offset: emits `0x5B JUMPDEST` and records
    /// this offset as the label's resolved PC. A label must be placed exactly
    /// once; a second placement is a programming error (debug-asserted).
    pub fn jumpdest(&mut self, label: Label) -> &mut Self {
        debug_assert!(self.dests[label.0].is_none(), "label placed twice");
        self.dests[label.0] = Some(self.code.len());
        self.code.push(op::JUMPDEST);
        self
    }

    /// Emit a FIXED-WIDTH `PUSH2 0x0000` placeholder referencing `label`, to be
    /// back-patched in pass 2. Pushes the label's absolute PC onto the stack;
    /// follow with `JUMP`/`JUMPI`.
    pub fn push_label(&mut self, label: Label) -> &mut Self {
        self.code.push(op::PUSH2);
        // Record the operand offset (the byte AFTER the PUSH2 opcode), then emit
        // the 2-byte placeholder.
        self.refs.push((self.code.len(), label));
        self.code.push(0x00);
        self.code.push(0x00);
        self
    }

    /// Pass 2: back-patch every label reference and return the final bytecode.
    ///
    /// Each recorded reference's 2 placeholder bytes are overwritten with its
    /// label's resolved offset (big-endian). Panics if a referenced label was
    /// never placed (a forward jump with no `jumpdest`) or if a resolved offset
    /// exceeds `u16::MAX` (a >64KB program — far past any facet's EIP-170 limit).
    pub fn finish(mut self) -> Vec<u8> {
        for (operand_off, label) in &self.refs {
            let dest = self.dests[label.0]
                .expect("referenced label was never placed (missing jumpdest)");
            let dest =
                u16::try_from(dest).expect("jump target exceeds 64KB (u16) — program too large");
            let be = dest.to_be_bytes();
            self.code[*operand_off] = be[0];
            self.code[*operand_off + 1] = be[1];
        }
        self.code
    }
}

/// Prepend the constant init wrapper (the contract-creation constructor) to a
/// runtime blob and return the full INIT code for a CREATE transaction.
///
/// The wrapper `CODECOPY`s the trailing runtime into memory and `RETURN`s it as
/// the contract's deployed code (EVM creation semantics — the one concept with no
/// wasm analog; design §5). It uses `PUSH1 0x00` for zeros, never `PUSH0`.
///
/// Layout of the emitted prelude (then `runtime` bytes follow):
/// ```text
/// PUSH2 <rt_len>  DUP1  PUSH2 <rt_off>  PUSH1 0x00  CODECOPY  PUSH1 0x00  RETURN
/// ```
/// `rt_off` is the length of the prelude itself (where the runtime bytes begin in
/// this contract's own code). The prelude is a fixed 13 bytes (design §5's
/// "~12-byte" estimate; the exact count is 3+1+3+2+1+2+1), so `rt_off` is the
/// constant `0x000D` and `PUSH2` keeps the width fixed regardless of `rt_len`.
pub fn init_wrapper(runtime: &[u8]) -> Vec<u8> {
    let rt_len =
        u16::try_from(runtime.len()).expect("runtime exceeds 64KB (u16) — far past EIP-170");
    // Prelude is exactly 13 bytes (3+1+3+2+1+2+1 = 13). The runtime begins right
    // after it, so the CODECOPY source offset is the prelude length.
    const PRELUDE_LEN: u16 = 13;
    let len_be = rt_len.to_be_bytes();
    let off_be = PRELUDE_LEN.to_be_bytes();
    let mut out = Vec::with_capacity(PRELUDE_LEN as usize + runtime.len());
    // PUSH2 <rt_len>            ; stack: [len]
    out.extend_from_slice(&[op::PUSH2, len_be[0], len_be[1]]);
    // DUP1                      ; stack: [len, len]
    out.push(op::DUP1);
    // PUSH2 <rt_off>            ; stack: [len, len, off]
    out.extend_from_slice(&[op::PUSH2, off_be[0], off_be[1]]);
    // PUSH1 0x00                ; stack: [len, len, off, 0]  (dest mem offset)
    out.extend_from_slice(&[op::PUSH1, 0x00]);
    // CODECOPY                  ; mem[0..len] = code[off..off+len]; stack: [len]
    out.push(op::CODECOPY);
    // PUSH1 0x00                ; stack: [len, 0]            (return mem offset)
    out.extend_from_slice(&[op::PUSH1, 0x00]);
    // RETURN                    ; return mem[0..len] as deployed code
    out.push(op::RETURN);
    debug_assert_eq!(out.len(), PRELUDE_LEN as usize);
    out.extend_from_slice(runtime);
    out
}

#[cfg(test)]
mod tests {
    use super::op;
    use super::{init_wrapper, Asm};

    #[test]
    fn push_then_add_emits_exact_bytes() {
        // The canonical sanity program from the task: push(1) push(2) ADD.
        let mut asm = Asm::new();
        asm.push(&[1]).push(&[2]).emit(op::ADD);
        assert_eq!(asm.finish(), vec![0x60, 0x01, 0x60, 0x02, 0x01]);
    }

    #[test]
    fn push_size_boundaries() {
        // 0 → PUSH1 0x00 (NOT PUSH0).
        let mut a = Asm::new();
        a.push(&[0]);
        assert_eq!(a.finish(), vec![op::PUSH1, 0x00]);
        // empty slice is also zero → PUSH1 0x00.
        let mut a = Asm::new();
        a.push(&[]);
        assert_eq!(a.finish(), vec![op::PUSH1, 0x00]);
        // a multi-byte all-zero input still collapses to PUSH1 0x00.
        let mut a = Asm::new();
        a.push(&[0, 0, 0, 0]);
        assert_eq!(a.finish(), vec![op::PUSH1, 0x00]);
        // 0xff → PUSH1 0xff.
        let mut a = Asm::new();
        a.push(&[0xff]);
        assert_eq!(a.finish(), vec![op::PUSH1, 0xff]);
        // 0x0100 → PUSH2 0x01 0x00 (256 needs 2 bytes).
        let mut a = Asm::new();
        a.push(&0x0100u16.to_be_bytes());
        assert_eq!(a.finish(), vec![op::PUSH2, 0x01, 0x00]);
        // leading-zero stripping: 0x0000_00ff → PUSH1 0xff.
        let mut a = Asm::new();
        a.push(&0x0000_00ffu32.to_be_bytes());
        assert_eq!(a.finish(), vec![op::PUSH1, 0xff]);
        // a full 32-byte value → PUSH32 (0x7f) + 32 bytes, no stripping when the
        // top byte is significant.
        let mut val = [0u8; 32];
        val[0] = 0x12;
        val[31] = 0x34;
        let mut a = Asm::new();
        a.push(&val);
        let out = a.finish();
        assert_eq!(out[0], op::PUSH1 + 31); // PUSH32 == 0x7f
        assert_eq!(out.len(), 33);
        assert_eq!(&out[1..], &val);
    }

    #[test]
    fn push32_never_strips_and_always_emits_full_width() {
        // Even a value with many leading zeros keeps its full 32-byte width.
        let mut word = [0u8; 32];
        word[31] = 0x2a; // decimal 42
        let mut a = Asm::new();
        a.push32(&word);
        let out = a.finish();
        assert_eq!(out[0], op::PUSH1 + 31); // PUSH32 == 0x7f
        assert_eq!(out.len(), 33);
        assert_eq!(&out[1..], &word);
    }

    #[test]
    fn forward_jump_resolves_to_correct_push2_operand_and_jumpdest() {
        // PUSH2 <L> JUMPI ; ... ; L: JUMPDEST
        //   offset 0: 0x61 (PUSH2)
        //   offset 1..3: operand placeholder → resolved to L
        //   offset 3: 0x57 (JUMPI)
        //   offset 4: 0x50 (POP, filler)
        //   offset 5: 0x5B (JUMPDEST = L)  ← L resolves to 5
        let mut a = Asm::new();
        let l = a.new_label();
        a.push_label(l).emit(op::JUMPI).emit(op::POP);
        a.jumpdest(l);
        let out = a.finish();
        assert_eq!(
            out,
            vec![op::PUSH2, 0x00, 0x05, op::JUMPI, op::POP, op::JUMPDEST]
        );
        // the 0x5B JUMPDEST sits at the offset the operand points to (5)
        assert_eq!(out[5], op::JUMPDEST);
        let operand = u16::from_be_bytes([out[1], out[2]]) as usize;
        assert_eq!(operand, 5);
        assert_eq!(out[operand], op::JUMPDEST);
    }

    #[test]
    fn back_jump_resolves_to_an_earlier_jumpdest() {
        // L: JUMPDEST ; PUSH2 <L> JUMP — the label is placed BEFORE the ref.
        let mut a = Asm::new();
        let l = a.new_label();
        a.jumpdest(l); // L = offset 0
        a.push_label(l).emit(op::JUMP);
        let out = a.finish();
        assert_eq!(out, vec![op::JUMPDEST, op::PUSH2, 0x00, 0x00, op::JUMP]);
        let operand = u16::from_be_bytes([out[2], out[3]]) as usize;
        assert_eq!(operand, 0);
    }

    #[test]
    fn multiple_refs_to_one_label_all_patch() {
        let mut a = Asm::new();
        let l = a.new_label();
        a.push_label(l).emit(op::POP); // ref 1 at op 0
        a.push_label(l).emit(op::POP); // ref 2 at op 4
        a.jumpdest(l); // L = offset 8
        let out = a.finish();
        assert_eq!(out.len(), 9);
        assert_eq!(u16::from_be_bytes([out[1], out[2]]), 8);
        assert_eq!(u16::from_be_bytes([out[5], out[6]]), 8);
        assert_eq!(out[8], op::JUMPDEST);
    }

    #[test]
    fn init_wrapper_has_correct_codecopy_return_prelude() {
        // A 3-byte runtime [0xAA,0xBB,0xCC]. Prelude is fixed 13 bytes; rt_off is
        // 0x000D and rt_len is 0x0003.
        let runtime = [0xAA, 0xBB, 0xCC];
        let init = init_wrapper(&runtime);
        let expected_prelude = [
            op::PUSH2, 0x00, 0x03, // PUSH2 rt_len (3)
            op::DUP1, //              DUP1
            op::PUSH2, 0x00, 0x0D, // PUSH2 rt_off (13)
            op::PUSH1, 0x00, //       PUSH1 0x00 (dest)
            op::CODECOPY, //          CODECOPY
            op::PUSH1, 0x00, //       PUSH1 0x00 (ret off)
            op::RETURN, //            RETURN
        ];
        assert_eq!(&init[..13], &expected_prelude);
        // the runtime is appended verbatim after the 13-byte prelude
        assert_eq!(&init[13..], &runtime);
        assert_eq!(init.len(), 13 + 3);
    }

    #[test]
    fn init_wrapper_encodes_a_larger_runtime_length() {
        // A 300-byte runtime → rt_len = 0x012C; rt_off stays 0x000D.
        let runtime = vec![0x5Bu8; 300];
        let init = init_wrapper(&runtime);
        assert_eq!(&init[..3], &[op::PUSH2, 0x01, 0x2C]); // PUSH2 300
        assert_eq!(&init[4..7], &[op::PUSH2, 0x00, 0x0D]); // PUSH2 13
        assert_eq!(init.len(), 13 + 300);
    }
}
