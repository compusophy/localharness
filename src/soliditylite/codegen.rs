//! SolidityLite codegen — a typed [`Facet`] → EVM runtime bytecode, wrapped into
//! deployable init code (a [`CompiledArtifact`]).
//!
//! This is the EVM emitter (design §5). It owns the SHARED dispatch + body
//! emission that the worked [`super::emit_constant_getter`] also drives, so the
//! source-compiled path and the hand-built emitter produce byte-IDENTICAL
//! bytecode for the single-constant-function case (the golden gate). Discipline
//! mirrors [`crate::rustlite::codegen`]: a single [`Asm`] accumulates bytes,
//! opcodes are named consts, labels resolve in [`Asm::finish`]'s second pass.
//!
//! Runtime layout (design §5):
//! ```text
//! [calldatasize guard][selector extract][dispatch arms][fallback REVERT][fn bodies]
//! ```

use crate::soliditylite::asm::{op, Asm, Label};
use crate::soliditylite::CompiledArtifact;
// `Expr`/`Facet`/`Stmt` are only used by the source-compile path, which is
// wallet-gated (selector + slot keccak live there). `CompileError`/`Span` are also
// used by the (non-gated) `assemble_with` oversize guard, so they stay un-gated.
use crate::rustlite::{CompileError, Span};
#[cfg(feature = "wallet")]
use crate::soliditylite::ast::{CmpOp, Expr, Facet, StateVarKind, Stmt};

/// What a function body returns — the value-producing prefix that precedes the
/// shared `MSTORE(0,·) RETURN(0,0x20)` tail.
///
/// Both shapes push exactly one 32-byte word onto the stack; the tail stores it at
/// `mem[0..32]` and returns it. This is the single point that decides "constant vs
/// storage read", keeping the rest of the body byte-identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyValue {
    /// Push a constant 32-byte word (`PUSH32 <value>`) — `return <intlit>;`.
    Const([u8; 32]),
    /// Load from a keccak-namespaced storage slot (`PUSH32 <slot> SLOAD`) —
    /// `return <stateVar>;` (design §5 storage).
    StorageSlot([u8; 32]),
}

/// A function body lowered to the shape codegen emits, after name/slot resolution.
///
/// - [`Body::View`] is the SIMPLE floor-grammar getter (`return <intlit|stateVar>;`):
///   a single [`BodyValue`] pushed onto the stack, then the shared
///   `MSTORE/RETURN(0,0x20)` tail. Byte-IDENTICAL to the worked emitter (the golden
///   gate) — the simple return keeps `PUSH32`.
/// - [`Body::ViewExpr`] is a getter whose return is a COMPOUND expression
///   (`return a + b;`): the lowered expr leaves one word, then the same tail.
/// - [`Body::Mutating`] is the write-stretch function (`{ (<require>|<assign>)* }`):
///   each `require` guard (`cond ISZERO PUSH2 <revert> JUMPI`) and each
///   `SSTORE(slot, eval(expr))` in order, then an empty `RETURN(0,0)` (the diamond
///   fallback returns cleanly); if any `require` is present, one shared
///   `<revert>: REVERT(0,0)` stub is emitted after the return.
#[cfg(feature = "wallet")]
enum Body {
    /// A simple view getter: push one [`BodyValue`], store + return it.
    View(BodyValue),
    /// A view getter returning a compound expression (e.g. `a + b`).
    ViewExpr(LoweredExpr),
    /// A mutating function: a sequence of statements (`require` guards + scalar /
    /// mapping-entry assignments), then `RETURN(0,0)`.
    Mutating(Vec<LoweredStmt>),
    /// A `returns (string)` getter returning a CONSTANT string literal (the
    /// dynamic-type stretch, slice 1). Emits the ABI string encoding —
    /// `head offset 0x20 ‖ length ‖ right-padded data` — and `RETURN`s the exact
    /// dynamic size. The bytes are the decoded UTF-8 literal.
    ConstString(Vec<u8>),
    /// A `returns (string|bytes)` getter returning a dynamic `string`/`bytes` STATE
    /// VARIABLE (#37 slice 1, the READ side). Reads the canonical Solidity header at
    /// `slot`, branches on its low bit (even = short/inline, odd = long/spilled),
    /// decodes the length, copies the data into the ABI return region
    /// (`offset 0x20 ‖ length ‖ data`), and `RETURN`s it. Uses a runtime copy loop
    /// for the LONG case.
    DynamicStorageReturn { slot: [u8; 32] },
    /// A `returns (string|bytes)` getter that ECHOES a dynamic PARAMETER (#37
    /// slices 2+3): `return s;` where `s` is a `string`/`bytes` param. ABI-decodes
    /// the dynamic arg from calldata and ABI-re-encodes it as the return — a verbatim
    /// `[length ‖ data]` copy via `CALLDATACOPY`, prefixed with the `0x20` offset word.
    EchoParam { param_index: u64 },
}

/// One statement inside a mutating body: a `require` guard, an assignment, or an
/// `emit`.
#[cfg(feature = "wallet")]
enum LoweredStmt {
    /// `require(cond, …)` — branch to the shared revert stub when `cond` is FALSE.
    Require(LoweredExpr),
    /// A scalar or mapping-entry store.
    Assign(LoweredAssign),
    /// `emit <Event>(…)` — append an EVM `LOGn`.
    Emit(LoweredEmit),
    /// `if (<cond>) { <then> } else { <else> }` — branch on `cond` (the branch
    /// stretch). `else_body` is empty when there is no `else`.
    If {
        cond: LoweredExpr,
        then_body: Vec<LoweredStmt>,
        else_body: Vec<LoweredStmt>,
    },
}

/// A resolved `emit` statement: the event's topic0 (the FULL 32-byte keccak of the
/// canonical signature, NOT the 4-byte selector), the indexed-arg expressions (each
/// becomes an extra LOG topic, in declaration order), and the non-indexed-arg
/// expressions (each is ABI-encoded into the log data region). See [`emit_log`].
#[cfg(feature = "wallet")]
struct LoweredEmit {
    /// `topic0` = `keccak256("<Event>(<types>)")` — the full 32-byte hash.
    topic0: [u8; 32],
    /// The `indexed` args, in declaration order → topics 1..=N.
    indexed: Vec<LoweredExpr>,
    /// The non-`indexed` args, in declaration order → the data region words.
    data: Vec<LoweredExpr>,
}

/// A resolved expression — names already mapped to slots / param indices — ready
/// to lower to a stack-pushing instruction sequence (design §5).
#[cfg(feature = "wallet")]
enum LoweredExpr {
    /// A constant operand — pushed MINIMAL-width (`PUSH1 0x01` for `1`), the
    /// idiomatic/gas-cheap encoding for an arithmetic operand. (The TOP-LEVEL
    /// `return <intlit>;` keeps `PUSH32` via [`Body::View`] for the golden gate.)
    Const([u8; 32]),
    /// `PUSH32 <slot> SLOAD` — a scalar state-variable read (full 32-byte slots).
    Load([u8; 32]),
    /// `CALLDATALOAD(4 + 32*index)` — a function-parameter read. The ABI lays arg
    /// `i` at calldata offset `4 + 32*i`; both `uint256` and `address` load as a
    /// full word (addresses are already left-padded by the ABI).
    Param(u64),
    /// `CALLER` — `msg.sender`, the caller address as a 32-byte word.
    Caller,
    /// `TIMESTAMP` — `block.timestamp`, the block's unix time as a word.
    Timestamp,
    /// `NUMBER` — `block.number`, the block height as a word.
    Number,
    /// A mapping-entry read: derive the entry slot
    /// `keccak256(pad32(key) ++ pad32(baseSlot))`, then `SLOAD`.
    MapLoad { base_slot: [u8; 32], key: Box<LoweredExpr> },
    /// A dynamic-array length read: `PUSH32 <slot> SLOAD` (the length lives at the
    /// array's base slot).
    ArrayLen { slot: [u8; 32] },
    /// A dynamic-array element read: derive the element slot
    /// `keccak256(pad32(slot)) + index`, then `SLOAD`.
    ArrayLoad { slot: [u8; 32], index: Box<LoweredExpr> },
    /// `<lhs> <rhs> ADD` — a binary addition (left operand pushed first).
    Add(Box<LoweredExpr>, Box<LoweredExpr>),
    /// `lhs - rhs` — `SUB` (`lhs` pushed on TOP so `SUB` = top − next = `lhs − rhs`).
    Sub(Box<LoweredExpr>, Box<LoweredExpr>),
    /// `lhs * rhs` — `MUL` (commutative; operand order irrelevant).
    Mul(Box<LoweredExpr>, Box<LoweredExpr>),
    /// `lhs / rhs` — `DIV` (`lhs` on TOP so `DIV` = top / next = `lhs / rhs`).
    Div(Box<LoweredExpr>, Box<LoweredExpr>),
    /// `lhs % rhs` — `MOD` (`lhs` on TOP so `MOD` = top % next = `lhs % rhs`).
    Mod(Box<LoweredExpr>, Box<LoweredExpr>),
    /// `<lhs> <rhs> <cmp>` — a comparison leaving a `0`/`1` word (the relational
    /// stretch). The operand order matters: both EVM `GT`/`LT` pop `a` (top) then
    /// `b` and compute `a <cmp> b`, so we push `lhs` first then `rhs` and the strict
    /// ops map directly; `<=`/`>=` append an `ISZERO` to invert the strict result.
    Cmp { op: CmpOp, lhs: Box<LoweredExpr>, rhs: Box<LoweredExpr> },
}

/// Emit the mapping-entry SLOT-DERIVATION for `keccak256(pad32(key) ++ pad32(base))`,
/// leaving the 32-byte slot on top of the stack (design §5 mapping rule). The `key`
/// sub-expression is evaluated and consumed by `MSTORE`, so anything already on the
/// stack BELOW is preserved (this is what lets a write push its value first).
///
/// ```text
/// <key>  PUSH1 0x00 MSTORE          ; mem[0x00..0x20] = key   (first preimage word)
/// PUSH32 <base> PUSH1 0x20 MSTORE   ; mem[0x20..0x40] = base  (second preimage word)
/// PUSH1 0x40 PUSH1 0x00 KECCAK256   ; slot = keccak256(mem[0x00..0x40])
/// ```
#[cfg(feature = "wallet")]
fn emit_map_slot(a: &mut Asm, base_slot: &[u8; 32], key: &LoweredExpr) {
    // mem[0x00] = key (the FIRST preimage word).
    key.emit(a);
    a.push_u64(0x00).emit(op::MSTORE);
    // mem[0x20] = base slot (the SECOND preimage word).
    a.push32(base_slot).push_u64(0x20).emit(op::MSTORE);
    // slot = keccak256(mem[0x00..0x40]).
    a.push_u64(0x40).push_u64(0x00).emit(op::KECCAK256);
}

/// Emit the dynamic-array ELEMENT-SLOT derivation `keccak256(pad32(slot)) + index`,
/// leaving the 32-byte element slot on top of the stack (the canonical Solidity
/// dynamic-array layout: the length lives at `slot`, element `i` at
/// `keccak256(slot) + i`). The `index` sub-expression is evaluated and ADDed last,
/// so anything already on the stack BELOW is preserved (lets a write push its value
/// first). `mem[0x00..0x20]` is reused as keccak scratch (below [`LOG_DATA_BASE`]).
///
/// ```text
/// PUSH32 <slot> PUSH1 0x00 MSTORE   ; mem[0x00..0x20] = slot   (the preimage word)
/// PUSH1 0x20 PUSH1 0x00 KECCAK256   ; base = keccak256(mem[0x00..0x20])
/// <index>  ADD                      ; elem slot = base + index
/// ```
#[cfg(feature = "wallet")]
fn emit_array_slot(a: &mut Asm, slot: &[u8; 32], index: &LoweredExpr) {
    // mem[0x00] = slot (the single keccak preimage word).
    a.push32(slot).push_u64(0x00).emit(op::MSTORE);
    // base = keccak256(mem[0x00..0x20]).
    a.push_u64(0x20).push_u64(0x00).emit(op::KECCAK256);
    // elem slot = base + index.
    index.emit(a);
    a.emit(op::ADD);
}

#[cfg(feature = "wallet")]
impl LoweredExpr {
    /// Emit this expression so it leaves exactly one 32-byte word on the stack.
    fn emit(&self, a: &mut Asm) {
        match self {
            LoweredExpr::Const(word) => {
                a.push(word); // minimal-width push of the operand
            }
            LoweredExpr::Load(slot) => {
                a.push32(slot).emit(op::SLOAD);
            }
            LoweredExpr::Param(index) => {
                // CALLDATALOAD(4 + 32*index) — ABI arg slot.
                a.push_u64(4 + 32 * index).emit(op::CALLDATALOAD);
            }
            LoweredExpr::Caller => {
                a.emit(op::CALLER);
            }
            LoweredExpr::Timestamp => {
                a.emit(op::TIMESTAMP);
            }
            LoweredExpr::Number => {
                a.emit(op::NUMBER);
            }
            LoweredExpr::MapLoad { base_slot, key } => {
                emit_map_slot(a, base_slot, key);
                a.emit(op::SLOAD);
            }
            LoweredExpr::ArrayLen { slot } => {
                a.push32(slot).emit(op::SLOAD); // length lives at the base slot
            }
            LoweredExpr::ArrayLoad { slot, index } => {
                emit_array_slot(a, slot, index);
                a.emit(op::SLOAD);
            }
            LoweredExpr::Add(lhs, rhs) => {
                lhs.emit(a);
                rhs.emit(a);
                a.emit(op::ADD);
            }
            LoweredExpr::Sub(lhs, rhs) => {
                // Push rhs (deeper) then lhs (top) so `SUB` = μs[0] - μs[1] = lhs - rhs.
                rhs.emit(a);
                lhs.emit(a);
                a.emit(op::SUB);
            }
            LoweredExpr::Mul(lhs, rhs) => {
                lhs.emit(a);
                rhs.emit(a);
                a.emit(op::MUL); // commutative — order irrelevant
            }
            LoweredExpr::Div(lhs, rhs) => {
                // lhs on top so `DIV` = μs[0] / μs[1] = lhs / rhs.
                rhs.emit(a);
                lhs.emit(a);
                a.emit(op::DIV);
            }
            LoweredExpr::Mod(lhs, rhs) => {
                // lhs on top so `MOD` = μs[0] % μs[1] = lhs % rhs.
                rhs.emit(a);
                lhs.emit(a);
                a.emit(op::MOD);
            }
            LoweredExpr::Cmp { op: cmp, lhs, rhs } => {
                // Push `lhs` (deeper) then `rhs` (top). EVM `GT`/`LT`/`EQ` pop the
                // top operand as `a` and the next as `b`, computing `a <op> b`. With
                // `rhs` on top, `GT` yields `rhs > lhs`, which is NOT what we want;
                // we want `lhs <op> rhs`. So push `rhs` FIRST then `lhs` so `lhs` is
                // on top → `GT` = `lhs > rhs`, `LT` = `lhs < rhs`. `EQ` is symmetric.
                rhs.emit(a);
                lhs.emit(a);
                match cmp {
                    CmpOp::Gt => {
                        a.emit(op::GT);
                    }
                    CmpOp::Lt => {
                        a.emit(op::LT);
                    }
                    CmpOp::Eq => {
                        a.emit(op::EQ);
                    }
                    // `a != b` ⇔ NOT (a == b).
                    CmpOp::Neq => {
                        a.emit(op::EQ).emit(op::ISZERO);
                    }
                    // `a <= b` ⇔ NOT (a > b); `a >= b` ⇔ NOT (a < b).
                    CmpOp::Le => {
                        a.emit(op::GT).emit(op::ISZERO);
                    }
                    CmpOp::Ge => {
                        a.emit(op::LT).emit(op::ISZERO);
                    }
                }
            }
        }
    }
}

/// A resolved assignment target inside a mutating body: a scalar slot or a
/// mapping entry (derived from a key expression at the base slot).
#[cfg(feature = "wallet")]
enum LoweredAssign {
    /// `SSTORE(slot, value)` — a scalar state-var write.
    Scalar { slot: [u8; 32], value: LoweredExpr },
    /// `SSTORE(keccak256(pad32(key) ++ pad32(base)), value)` — a mapping-entry write.
    MapEntry { base_slot: [u8; 32], key: LoweredExpr, value: LoweredExpr },
    /// `SSTORE(keccak256(pad32(slot)) + index, value)` — a dynamic-array element write.
    ArrayElem { slot: [u8; 32], index: LoweredExpr, value: LoweredExpr },
    /// `<arr>.push(value)` — append: store `value` at `keccak256(pad32(slot)) + len`,
    /// then bump the length slot to `len + 1` (the length is re-`SLOAD`ed rather than
    /// duplicated, avoiding a `DUP2`/`SWAP` the v1 assembler doesn't expose).
    ArrayPush { slot: [u8; 32], value: LoweredExpr },
    /// `<arr>.pop()` — remove the last element: zero the element at
    /// `keccak256(pad32(slot)) + (len - 1)`, then store the decremented length
    /// `len - 1` back at the base slot. The length is re-`SLOAD`ed for each use
    /// (no `DUP2` in v1), and `len - 1` wraps on an empty array (no 0.8-style
    /// revert in v1 — guard with `require(<arr>.length > 0, …)` where it matters).
    ArrayPop { slot: [u8; 32] },
    /// `delete <arr>[index]` — zero the dynamic-array element at
    /// `keccak256(pad32(slot)) + index`. The length is UNCHANGED (matching
    /// Solidity's `delete arr[i]`); this is exactly an element write of `0`.
    ArrayDelete { slot: [u8; 32], index: LoweredExpr },
    /// `<dynBytesVar> = "<literal>";` (#37 slice 1, the WRITE side). Stores a
    /// COMPILE-TIME-KNOWN `string`/`bytes` literal at `base_slot` in canonical
    /// Solidity layout — fully unrolled (SHORT = one packed-word `SSTORE`; LONG =
    /// a header `SSTORE` plus one `SSTORE` per 32-byte data chunk at precomputed
    /// `keccak256(pad32(slot)) + i` slots). No runtime loop (the bytes are known).
    ConstDynamicBytes { base_slot: [u8; 32], bytes: Vec<u8> },
}

#[cfg(feature = "wallet")]
impl LoweredAssign {
    /// Emit the store. `SSTORE` pops `slot` (top) then `value`, so we push `value`
    /// FIRST, then leave the slot on top.
    fn emit(&self, a: &mut Asm) {
        match self {
            LoweredAssign::Scalar { slot, value } => {
                value.emit(a);
                a.push32(slot).emit(op::SSTORE);
            }
            LoweredAssign::MapEntry { base_slot, key, value } => {
                // value first (stays below), then derive the slot on top, then store.
                value.emit(a);
                emit_map_slot(a, base_slot, key);
                a.emit(op::SSTORE);
            }
            LoweredAssign::ArrayElem { slot, index, value } => {
                // value first (stays below), then derive keccak256(slot)+index, store.
                value.emit(a);
                emit_array_slot(a, slot, index);
                a.emit(op::SSTORE);
            }
            LoweredAssign::ArrayPush { slot, value } => {
                // 1. element write: SSTORE(keccak256(slot) + len, value). The index
                //    is the CURRENT length (SLOAD of the base slot).
                value.emit(a); // value first (stays below)
                emit_array_slot(a, slot, &LoweredExpr::ArrayLen { slot: *slot });
                a.emit(op::SSTORE);
                // 2. length bump: SSTORE(slot, len + 1). Re-SLOAD the length (warm)
                //    rather than DUP it past the consumed value (no DUP2 in v1).
                let mut one = [0u8; 32];
                one[31] = 1;
                a.push32(slot).emit(op::SLOAD).push(&one).emit(op::ADD);
                a.push32(slot).emit(op::SSTORE);
            }
            LoweredAssign::ArrayPop { slot } => {
                // The new length = len - 1 (re-derived from the live length slot).
                let new_len =
                    LoweredExpr::Sub(Box::new(LoweredExpr::ArrayLen { slot: *slot }), Box::new(one_word()));
                // 1. element clear: SSTORE(keccak256(slot) + (len - 1), 0). Push the
                //    zero value first (stays below), then derive the element slot.
                LoweredExpr::Const([0u8; 32]).emit(a); // value 0
                emit_array_slot(a, slot, &new_len);
                a.emit(op::SSTORE);
                // 2. length decrement: SSTORE(slot, len - 1). Re-`SLOAD` the length
                //    (no DUP2 in v1) and subtract one.
                new_len.emit(a);
                a.push32(slot).emit(op::SSTORE);
            }
            LoweredAssign::ArrayDelete { slot, index } => {
                // `delete arr[i]` is an element write of 0 at keccak256(slot)+i; the
                // length is left untouched. Push 0 first (stays below), then derive.
                LoweredExpr::Const([0u8; 32]).emit(a); // value 0
                emit_array_slot(a, slot, index);
                a.emit(op::SSTORE);
            }
            LoweredAssign::ConstDynamicBytes { base_slot, bytes } => {
                emit_const_dynamic_bytes_store(a, base_slot, bytes);
            }
        }
    }
}

/// Emit the canonical Solidity `string`/`bytes` storage WRITE for a COMPILE-TIME
/// literal at `base_slot` (#37 slice 1). Fully unrolled — no runtime loop, because
/// every byte (and therefore the short/long choice and each data slot) is known:
///
/// - **SHORT** (len ≤ 31): one `SSTORE(base_slot, word)` where `word` packs the data
///   left-aligned in the high bytes with `len*2` in the lowest byte.
/// - **LONG** (len ≥ 32): `SSTORE(base_slot, len*2 + 1)` then, for each 32-byte data
///   chunk `i`, `SSTORE(keccak256(pad32(base_slot)) + i, chunk_i)`. The data start
///   slot is computed in Rust (it is constant for a fixed slot), so no on-chain
///   `KECCAK256` is needed.
#[cfg(feature = "wallet")]
fn emit_const_dynamic_bytes_store(a: &mut Asm, base_slot: &[u8; 32], bytes: &[u8]) {
    let len = bytes.len();
    if len <= 31 {
        // SHORT: data left-aligned, low byte = len*2 (even ⇒ short).
        let mut word = [0u8; 32];
        word[..len].copy_from_slice(bytes);
        word[31] = (len as u8) * 2;
        a.push32(&word).push32(base_slot).emit(op::SSTORE);
    } else {
        // LONG: header = len*2 + 1 (odd ⇒ long).
        let mut header = [0u8; 32];
        // len fits in u64 for any realistic literal; encode big-endian into the word.
        let marker = (len as u128) * 2 + 1;
        header[16..].copy_from_slice(&marker.to_be_bytes());
        a.push32(&header).push32(base_slot).emit(op::SSTORE);
        // Data slots from keccak256(pad32(base_slot)), one SSTORE per 32-byte chunk.
        let data_slot0 = dynamic_data_slot0(base_slot);
        for (i, chunk) in bytes.chunks(32).enumerate() {
            let mut word = [0u8; 32];
            word[..chunk.len()].copy_from_slice(chunk); // left-aligned, right-padded
            let slot = slot_at(data_slot0, i as u64);
            a.push32(&word).push32(&slot).emit(op::SSTORE);
        }
    }
}

/// The first data slot of a dynamic `string`/`bytes` (or array) at `slot`:
/// `keccak256(pad32(slot))`. Computed in Rust (the slot is constant), matching the
/// on-chain `MSTORE(0,slot); KECCAK256(0,0x20)` derivation [`emit_array_slot`] runs.
#[cfg(feature = "wallet")]
fn dynamic_data_slot0(slot: &[u8; 32]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    Keccak256::digest(slot).into()
}

/// The 32-byte word for the constant `1` (used by `len - 1` in array pop).
#[cfg(feature = "wallet")]
fn one_word() -> LoweredExpr {
    let mut one = [0u8; 32];
    one[31] = 1;
    LoweredExpr::Const(one)
}

/// The memory offset where a log's DATA region is staged, ABOVE the keccak scratch
/// region (`mem[0x00..0x40]`, used by [`emit_map_slot`]). Staging the data at
/// `0x40` means evaluating a mapping-read arg (which clobbers `mem[0x00..0x40]`
/// during its keccak) never corrupts a data/topic word — order-independent and
/// robust for the general case. The resulting log is identical regardless of which
/// scratch offset stages it (the EVM reads `mem[offset..offset+len]`).
#[cfg(feature = "wallet")]
const LOG_DATA_BASE: u64 = 0x40;

#[cfg(feature = "wallet")]
impl LoweredEmit {
    /// Emit the `LOGn` for this `emit` statement. The EVM `LOGn` pops (top → bottom)
    /// `offset, length, topic0, topic1, …, topic(n-1)`, so we leave the stack in
    /// exactly that arrangement before the opcode:
    ///
    /// ```text
    /// ; 1. stage the data region at mem[0x40 + 0x20*i] (above the keccak scratch)
    /// for i: <data[i]>  PUSH (0x40 + 0x20*i)  MSTORE
    /// ; 2. push the topics DEEPEST-first so topic0 ends up shallowest:
    /// <indexed[last]> … <indexed[0]>  PUSH32 <topic0>
    /// ; 3. push length then offset (offset ends on top), then LOGn:
    /// PUSH <0x20*numData>  PUSH 0x40  LOGn          (n = 1 + numIndexed)
    /// ```
    ///
    /// `n` (the LOG topic count) is `1 + indexed.len()` (topic0 is always present);
    /// `LOGn` = `LOG0 + n`. The data region is `0x20 * data.len()` bytes; with no
    /// data args the length is 0 (and the offset is harmless).
    fn emit(&self, a: &mut Asm) {
        // 1. Stage each non-indexed arg into the data region (above keccak scratch),
        //    in declaration order. Evaluating a mapping-read arg uses mem[0..0x40]
        //    scratch, which never overlaps the 0x40+ data region.
        for (i, word) in self.data.iter().enumerate() {
            word.emit(a); // value on top
            a.push_u64(LOG_DATA_BASE + 0x20 * i as u64).emit(op::MSTORE);
        }
        // 2. Push the topics deepest-first: the LAST indexed arg goes deepest, the
        //    FIRST shallowest, and topic0 (the signature hash) shallowest of all so
        //    it pops first as topic0.
        for indexed in self.indexed.iter().rev() {
            indexed.emit(a);
        }
        a.push32(&self.topic0);
        // 3. length = 0x20 * numData, then offset = LOG_DATA_BASE (offset on top),
        //    then LOGn.
        let length = 0x20u64 * self.data.len() as u64;
        a.push_u64(length).push_u64(LOG_DATA_BASE);
        let n = 1 + self.indexed.len(); // topic0 + one topic per indexed arg
        a.emit(log_op(n));
    }
}

/// The `LOG<n>` opcode for `n` topics (`n` in `0..=4`, the EVM maximum). v1 supports
/// at most one indexed arg beyond topic0 in practice, but the full `LOG0..LOG4`
/// range is mapped so more-indexed events lower without a special case.
#[cfg(feature = "wallet")]
fn log_op(n: usize) -> u8 {
    match n {
        0 => op::LOG0,
        1 => op::LOG1,
        2 => op::LOG2,
        3 => op::LOG3,
        4 => op::LOG4,
        // Unreachable: the emit lowering caps indexed topics at 3 (+ topic0 = 4)
        // via a clean CompileError before this is reached.
        _ => op::LOG4,
    }
}

/// Emit the SHARED dispatcher prelude into `a`: the calldatasize guard + the
/// selector extract. Byte-for-byte the head of [`super::emit_constant_getter`]
/// (design §5). `fb` is the fallback label (referenced here, placed later).
///
/// ```text
/// PUSH1 0x04 CALLDATASIZE LT PUSH2 <FB> JUMPI   ; calldata < 4 → fallback
/// PUSH1 0x00 CALLDATALOAD PUSH1 0xE0 SHR        ; selector = calldata[0:32] >> 224
/// ```
pub fn emit_dispatch_prelude(a: &mut Asm, fb: Label) {
    a.push_u64(0x04)
        .emit(op::CALLDATASIZE)
        .emit(op::LT)
        .push_label(fb)
        .emit(op::JUMPI);
    a.push_u64(0x00)
        .emit(op::CALLDATALOAD)
        .push_u64(0xE0)
        .emit(op::SHR);
}

/// Emit ONE dispatch arm into `a` (selector already on the stack):
/// `DUP1 PUSH4 <sel> EQ PUSH2 <body> JUMPI`. Byte-for-byte the dispatch step of
/// [`super::emit_constant_getter`].
pub fn emit_dispatch_arm(a: &mut Asm, selector: [u8; 4], body: Label) {
    a.emit(op::DUP1)
        .push(&selector) // PUSH4 — selectors are 4 significant bytes (non-zero high byte)
        .emit(op::EQ)
        .push_label(body)
        .emit(op::JUMPI);
}

/// Emit the SHARED fallback stub at `fb`: `JUMPDEST PUSH1 0x00 PUSH1 0x00 REVERT`.
/// Byte-for-byte the fallback of [`super::emit_constant_getter`].
pub fn emit_fallback(a: &mut Asm, fb: Label) {
    a.jumpdest(fb).push_u64(0x00).push_u64(0x00).emit(op::REVERT);
}

/// Emit ONE function body at `body`: a JUMPDEST, the value-producing prefix
/// ([`BodyValue`]), then the SHARED `MSTORE(0,·) RETURN(0,0x20)` tail.
///
/// For [`BodyValue::Const`] this is byte-for-byte the body of
/// [`super::emit_constant_getter`].
pub fn emit_body(a: &mut Asm, body: Label, value: BodyValue) {
    a.jumpdest(body);
    match value {
        BodyValue::Const(word) => {
            a.push32(&word); // PUSH32 <value>
        }
        BodyValue::StorageSlot(slot) => {
            a.push32(&slot).emit(op::SLOAD); // PUSH32 <slot> SLOAD
        }
    }
    // Shared tail: mem[0..32] = word ; return mem[0..32].
    a.push_u64(0x00)
        .emit(op::MSTORE)
        .push_u64(0x20)
        .push_u64(0x00)
        .emit(op::RETURN);
}

/// Emit one function body at `body` from a fully-lowered [`Body`].
///
/// A [`Body::View`] is byte-IDENTICAL to [`emit_body`] for the `Const`/`StorageSlot`
/// cases (same JUMPDEST + value prefix + `MSTORE/RETURN(0,0x20)` tail), so the
/// golden gate holds. A [`Body::Mutating`] emits each [`LoweredAssign`] (a scalar
/// `SSTORE(slot, value)` or a mapping-entry `SSTORE(keccak-slot, value)`) in order,
/// then an empty `RETURN(0,0)`.
#[cfg(feature = "wallet")]
fn emit_full_body(a: &mut Asm, body: Label, b: &Body) {
    match b {
        // The simple getter delegates to the SHARED `emit_body` so its bytes are
        // byte-identical to the worked emitter (the golden gate).
        Body::View(value) => emit_body(a, body, *value),
        Body::ViewExpr(expr) => {
            a.jumpdest(body);
            expr.emit(a);
            // Shared tail: mem[0..32] = word ; return mem[0..32].
            a.push_u64(0x00)
                .emit(op::MSTORE)
                .push_u64(0x20)
                .push_u64(0x00)
                .emit(op::RETURN);
        }
        Body::ConstString(bytes) => {
            a.jumpdest(body);
            // ABI `string` return: a single head word (offset 0x20 to the data),
            // then the length, then the UTF-8 bytes LEFT-aligned and right-padded
            // to a 32-byte multiple — all known at compile time for a literal.
            //   mem[0x00] = 0x20            ; offset to (length, data)
            //   mem[0x20] = len
            //   mem[0x40 + 32*i] = data word i
            //   RETURN(0x00, 0x40 + ceil(len/32)*32)
            a.push_u64(0x20).push_u64(0x00).emit(op::MSTORE);
            a.push_u64(bytes.len() as u64).push_u64(0x20).emit(op::MSTORE);
            let mut off = 0x40u64;
            for chunk in bytes.chunks(32) {
                let mut word = [0u8; 32];
                word[..chunk.len()].copy_from_slice(chunk); // left-aligned, right-padded
                a.push32(&word).push_u64(off).emit(op::MSTORE);
                off += 32;
            }
            let padded = bytes.len().div_ceil(32) * 32;
            a.push_u64(0x40 + padded as u64).push_u64(0x00).emit(op::RETURN);
        }
        Body::DynamicStorageReturn { slot } => {
            a.jumpdest(body);
            emit_dynamic_storage_return(a, slot);
        }
        Body::EchoParam { param_index } => {
            a.jumpdest(body);
            emit_echo_param(a, *param_index);
        }
        Body::Mutating(stmts) => {
            a.jumpdest(body);
            // Allocate ONE shared revert label for this function's `require`s (incl.
            // any nested inside `if` branches); only emit the stub if at least one
            // require references it (a require-free body keeps byte-identical to the
            // pre-require Mutating shape).
            let has_require = stmts.iter().any(stmt_has_require);
            let revert = if has_require { Some(a.new_label()) } else { None };
            emit_stmts(a, stmts, revert);
            // Empty return so the diamond fallback returns cleanly.
            a.push_u64(0x00).push_u64(0x00).emit(op::RETURN);
            // The shared revert stub (only when a require exists): a failed guard
            // lands here. REVERT with empty data aborts the call (no message — the
            // tx revert is the behavior we need).
            if let Some(revert) = revert {
                a.jumpdest(revert).push_u64(0x00).push_u64(0x00).emit(op::REVERT);
            }
        }
    }
}

/// Push the absolute calldata offset of a dynamic arg's `[length ‖ data]` TAIL
/// onto the stack: `tailAbs = CALLDATALOAD(4 + 32*param_index) + 4`. The head word
/// at `4 + 32*i` is the arg's offset RELATIVE to the start of the args (byte 4), so
/// adding 4 gives the absolute calldata offset of the length word.
#[cfg(feature = "wallet")]
fn emit_param_tail_abs(a: &mut Asm, param_index: u64) {
    a.push_u64(4 + 32 * param_index)
        .emit(op::CALLDATALOAD) // head (offset relative to byte 4)
        .push_u64(0x04)
        .emit(op::ADD); // tailAbs = head + 4
}

/// Given a dynamic value's byte length on top of the stack, replace it with the
/// number of bytes to copy for an ABI `[length ‖ data]` tail:
/// `copyLen = 32 + ceil(len/32)*32`. (`ceil(len/32)` via `(len + 31) >> 5`;
/// `* 32` via `MUL 0x20` so no `SHL` is needed.)
#[cfg(feature = "wallet")]
fn emit_len_to_tail_copy_len(a: &mut Asm) {
    // stack: [len]
    a.push_u64(0x1f).emit(op::ADD); // len + 31
    a.push_u64(0x05).emit(op::SHR); // (len + 31) >> 5  = ceil(len/32) words
    a.push_u64(0x20).emit(op::MUL); // words * 32        = padded data bytes
    a.push_u64(0x20).emit(op::ADD); // + 32 (the length word) = copyLen
}

/// Emit the ECHO of a dynamic `string`/`bytes` PARAMETER (#37 slices 2+3):
/// ABI-decode the dynamic arg from calldata and ABI-re-encode it as the return.
///
/// The input tail `[length ‖ data‖pad]` and the output tail have IDENTICAL layout,
/// so the whole tail is copied verbatim (`CALLDATACOPY`) into `mem[0x20..]` and the
/// `0x20` ABI offset word is prepended at `mem[0x00]`:
/// ```text
/// mem[0x00] = 0x20                              ; ABI offset to the tail
/// CALLDATACOPY(0x20, tailAbs, copyLen)         ; mem[0x20..] = [length ‖ data]
/// RETURN(0x00, 0x20 + copyLen)
/// ```
/// `tailAbs`/`copyLen` derive purely from calldata, so they are recomputed where
/// needed rather than juggled on the stack (no `DUP`/`SWAP`).
#[cfg(feature = "wallet")]
fn emit_echo_param(a: &mut Asm, param_index: u64) {
    // mem[0x00] = 0x20 (the ABI offset to the dynamic tail).
    a.push_u64(0x20).push_u64(0x00).emit(op::MSTORE);
    // CALLDATACOPY(dest=0x20, src=tailAbs, len=copyLen). Operand order: dest popped
    // first, so push len (deepest), then src, then dest.
    emit_param_tail_abs(a, param_index); // [tailAbs]
    a.emit(op::CALLDATALOAD); // [len] (length word lives at tailAbs)
    emit_len_to_tail_copy_len(a); // [copyLen]
    emit_param_tail_abs(a, param_index); // [copyLen, tailAbs]  (src)
    a.push_u64(0x20); // [copyLen, tailAbs, 0x20]  (dest)
    a.emit(op::CALLDATACOPY); // pops 0x20, tailAbs, copyLen → []
    // RETURN(0x00, 0x20 + copyLen). Recompute copyLen, add the 0x20 offset word.
    emit_param_tail_abs(a, param_index);
    a.emit(op::CALLDATALOAD); // [len]
    emit_len_to_tail_copy_len(a); // [copyLen]
    a.push_u64(0x20).emit(op::ADD); // retLen = 0x20 + copyLen
    a.push_u64(0x00).emit(op::RETURN); // RETURN(0, retLen)
}

/// Emit the dynamic `string`/`bytes` STORAGE READ getter (#37 slice 1, READ side):
/// load the canonical header at `slot`, branch on its low bit, decode the length,
/// copy the data into the ABI return region, and `RETURN` it.
///
/// ABI return memory layout (as [`Body::ConstString`]):
/// `mem[0x00]=0x20` (offset) · `mem[0x20]=len` · `mem[0x40+32*i]=data word i`.
///
/// ```text
/// h = SLOAD(slot)
/// if (h & 1) == 0 -> SHORT: data is inline in h's high bytes; len = (h & 0xff) / 2
///                          mem[0x40] = h ; RETURN(0, 0x60)
/// else            -> LONG:  len = (h - 1) / 2 ; data at keccak256(slot)+i
///                          copy ceil(len/32) words into mem[0x40+32*i]
///                          RETURN(0, 0x40 + count*32)
/// ```
/// The LONG case runs a runtime copy loop keeping a `[dataSlot0, count, i]` frame
/// (read via `DUP2`/`DUP3`).
#[cfg(feature = "wallet")]
fn emit_dynamic_storage_return(a: &mut Asm, slot: &[u8; 32]) {
    // NOTE: the LONG branch uses mem[0x00..0x20] as keccak scratch, which collides
    // with the ABI offset word, so `mem[0x00] = 0x20` is written PER BRANCH (after
    // the keccak in the LONG case) rather than once up front.

    // h = SLOAD(slot); test the low bit (h & 1).
    let long_lbl = a.new_label();
    a.push32(slot).emit(op::SLOAD); // [h]
    a.emit(op::DUP1).push_u64(0x01).emit(op::AND); // [h, h&1]
    a.push_label(long_lbl).emit(op::JUMPI); // if odd → LONG ; [h]

    // ── SHORT branch ──  stack: [h]
    // mem[0x00] = 0x20 (ABI offset; SHORT never uses the keccak scratch).
    a.push_u64(0x20).push_u64(0x00).emit(op::MSTORE); // [h]
    // len = (h & 0xff) >> 1
    a.emit(op::DUP1).push_u64(0xff).emit(op::AND); // [h, lenByte]
    a.push_u64(0x01).emit(op::SHR); // [h, len]
    // mem[0x20] = len.
    a.push_u64(0x20).emit(op::MSTORE); // [h]
    // mem[0x40] = h (the data word — its high bytes ARE the inline data; the low
    // marker byte sits beyond `len` and is never read by a correct consumer).
    a.push_u64(0x40).emit(op::MSTORE); // []
    // RETURN(0x00, 0x60) — short is always exactly one data word.
    a.push_u64(0x60).push_u64(0x00).emit(op::RETURN);

    // ── LONG branch ──  stack: [h]
    a.jumpdest(long_lbl);
    // len = (h - 1) >> 1.  SUB = top - next; push 1 then SWAP1 so h is on top.
    a.push_u64(0x01).emit(op::SWAP1).emit(op::SUB); // [h-1]
    a.push_u64(0x01).emit(op::SHR); // [len]
    // mem[0x20] = len  (keep one copy on the stack to derive count).
    a.emit(op::DUP1).push_u64(0x20).emit(op::MSTORE); // [len]
    // count = ceil(len/32) = (len + 31) >> 5.
    a.push_u64(0x1f).emit(op::ADD).push_u64(0x05).emit(op::SHR); // [count]
    // dataSlot0 = keccak256(pad32(slot)). This CLOBBERS mem[0x00..0x20].
    a.push32(slot).push_u64(0x00).emit(op::MSTORE); // mem[0..0x20] = slot
    a.push_u64(0x20).push_u64(0x00).emit(op::KECCAK256); // [count, dataSlot0]
    // Restore the ABI offset word now that the keccak scratch is done with mem[0x00].
    a.push_u64(0x20).push_u64(0x00).emit(op::MSTORE); // [count, dataSlot0]
    // Arrange the loop frame [dataSlot0, count, i].
    a.emit(op::SWAP1); // [dataSlot0, count]
    a.push_u64(0x00); // [dataSlot0, count, i=0]

    let loop_head = a.new_label();
    let loop_end = a.new_label();
    // loop_head: stack = [dataSlot0, count, i]
    a.jumpdest(loop_head);
    // Test `i < count`. The interpreter's GT pops a (top) then b (next) and computes
    // `a > b`, so with `count` on top and `i` beneath, `GT` = `count > i` = `i < count`.
    a.emit(op::DUP1); // [dataSlot0, count, i, i]
    a.emit(op::DUP3); // [..., i, count]   (count on top, i beneath)
    a.emit(op::GT).emit(op::ISZERO); // [..., (i >= count)]   ; GT = count > i = i < count
    a.push_label(loop_end).emit(op::JUMPI); // exit when i >= count ; [dataSlot0, count, i]
    // body: word = SLOAD(dataSlot0 + i); mem[0x40 + 32*i] = word.
    a.emit(op::DUP3); // [..., i, dataSlot0]
    a.emit(op::DUP2); // [..., i, dataSlot0, i]
    a.emit(op::ADD).emit(op::SLOAD); // [..., i, word]   word = SLOAD(dataSlot0 + i)
    a.emit(op::DUP2); // [..., i, word, i]
    a.push_u64(0x20).emit(op::MUL); // [..., i, word, 32*i]
    a.push_u64(0x40).emit(op::ADD); // [..., i, word, dest]
    a.emit(op::MSTORE); // mem[dest] = word ; [dataSlot0, count, i]
    // i = i + 1.
    a.push_u64(0x01).emit(op::ADD); // [dataSlot0, count, i+1]
    a.push_label(loop_head).emit(op::JUMP);

    // loop_end: stack = [dataSlot0, count, i]
    a.jumpdest(loop_end);
    a.emit(op::POP); // [dataSlot0, count]
    // retLen = 0x40 + count*32.
    a.push_u64(0x20).emit(op::MUL); // [dataSlot0, count*32]
    a.push_u64(0x40).emit(op::ADD); // [dataSlot0, retLen]
    a.push_u64(0x00); // [dataSlot0, retLen, 0]
    a.emit(op::RETURN); // RETURN(0, retLen) — leftover dataSlot0 is harmless (halt)
}

/// `true` if `s` (or any statement nested inside its `if` branches) is a
/// `require` — so a function with a guarded branch still allocates the shared
/// revert stub.
#[cfg(feature = "wallet")]
fn stmt_has_require(s: &LoweredStmt) -> bool {
    match s {
        LoweredStmt::Require(_) => true,
        LoweredStmt::If { then_body, else_body, .. } => {
            then_body.iter().any(stmt_has_require) || else_body.iter().any(stmt_has_require)
        }
        _ => false,
    }
}

/// Emit a sequence of [`LoweredStmt`] in order, branching to the shared `revert`
/// stub on a failed `require`. Recurses for `if`/`else`. Emitting `Require`,
/// `Assign`, and `Emit` is byte-identical to the pre-branch inline loop, so the
/// golden gates for those shapes are unaffected.
#[cfg(feature = "wallet")]
fn emit_stmts(a: &mut Asm, stmts: &[LoweredStmt], revert: Option<Label>) {
    for stmt in stmts {
        match stmt {
            LoweredStmt::Require(cond) => {
                // Emit cond; if FALSE (zero) jump to the revert stub.
                // `cond ISZERO PUSH2 <revert> JUMPI`.
                cond.emit(a);
                a.emit(op::ISZERO)
                    .push_label(revert.expect("revert label allocated when a require is present"))
                    .emit(op::JUMPI);
            }
            LoweredStmt::Assign(assign) => assign.emit(a),
            LoweredStmt::Emit(ev) => ev.emit(a),
            LoweredStmt::If { cond, then_body, else_body } => {
                // `cond ISZERO` → if cond is FALSE, skip the then-branch.
                cond.emit(a);
                a.emit(op::ISZERO);
                if else_body.is_empty() {
                    //   <cond> ISZERO PUSH2 <end> JUMPI ; <then> ; <end>:
                    let end = a.new_label();
                    a.push_label(end).emit(op::JUMPI);
                    emit_stmts(a, then_body, revert);
                    a.jumpdest(end);
                } else {
                    //   <cond> ISZERO PUSH2 <else> JUMPI ; <then> ; PUSH2 <end> JUMP ;
                    //   <else>: <else-body> ; <end>:
                    let else_lbl = a.new_label();
                    let end = a.new_label();
                    a.push_label(else_lbl).emit(op::JUMPI);
                    emit_stmts(a, then_body, revert);
                    a.push_label(end).emit(op::JUMP);
                    a.jumpdest(else_lbl);
                    emit_stmts(a, else_body, revert);
                    a.jumpdest(end);
                }
            }
        }
    }
}

/// EIP-170's deployed-bytecode size limit (bytes). A facet whose runtime exceeds
/// this can never be deployed on-chain anyway, and a length / jump target past
/// `u16::MAX` would `expect`-panic in the assembler ([`Asm::finish`] /
/// [`asm::init_wrapper`]) — an uncatchable, tab-killing abort in the browser. We
/// reject an oversized facet with a clean [`CompileError`] BEFORE finishing.
const MAX_RUNTIME_BYTES: usize = 24576;

/// The ONE place dispatch + bodies are laid out — used by BOTH the source-compile
/// path ([`assemble_full`] / [`compile`], `Body`) and the worked
/// [`super::emit_constant_getter`] ([`assemble`], `BodyValue`), so the two layout
/// routines can't drift. `emit_one_body(a, i, body_label)` emits function `i`'s
/// body at its allocated label (the caller owns the per-fn body data).
///
/// Layout: prelude → one dispatch arm per fn (in order) → fallback REVERT → one
/// body per fn (in order) → finish + init-wrap.
///
/// Returns a clean `CompileError` (NEVER panics) when the assembled runtime would
/// exceed [`MAX_RUNTIME_BYTES`] — the size at which the assembler's u16 jump/length
/// operands would otherwise `expect`-abort. `span` pins that error to the source.
fn assemble_with(
    selectors: &[[u8; 4]],
    span: Span,
    emit_one_body: impl Fn(&mut Asm, usize, Label),
) -> Result<CompiledArtifact, CompileError> {
    let mut a = Asm::new();
    let fb = a.new_label();
    // Allocate every body label up front so the dispatch arms (emitted first) can
    // forward-reference them.
    let body_labels: Vec<Label> = selectors.iter().map(|_| a.new_label()).collect();

    emit_dispatch_prelude(&mut a, fb);
    for (sel, &body) in selectors.iter().zip(&body_labels) {
        emit_dispatch_arm(&mut a, *sel, body);
    }
    emit_fallback(&mut a, fb);
    for (i, &body) in body_labels.iter().enumerate() {
        emit_one_body(&mut a, i, body);
    }

    // Reject an oversized facet BEFORE `finish()`/`init_wrapper` would u16-overflow
    // and panic. `a.here()` IS the runtime length here (finish only back-patches
    // placeholder bytes, never grows the code).
    let rt_len = a.here();
    if rt_len > MAX_RUNTIME_BYTES {
        return Err(CompileError::at_code(
            crate::error_codes::UNSUPPORTED_FEATURE,
            format!(
                "facet too large: runtime {rt_len} bytes exceeds the {MAX_RUNTIME_BYTES}-byte EIP-170 contract-size limit"
            ),
            span,
        ));
    }

    let runtime = a.finish();
    let init_code = crate::soliditylite::asm::init_wrapper(&runtime);
    Ok(CompiledArtifact { init_code, runtime, selectors: selectors.to_vec() })
}

/// Assemble a full runtime from a list of `(selector, full body)` pairs — the
/// source-compiled path. Shares the dispatch prelude/arms/fallback with
/// [`assemble`] via [`assemble_with`] (so a single const getter stays
/// byte-identical), emitting each body via [`emit_full_body`] to support storage
/// writes and `+` expressions. `facet_span` pins an oversize error to the source.
#[cfg(feature = "wallet")]
fn assemble_full(
    functions: Vec<([u8; 4], Body)>,
    facet_span: Span,
) -> Result<CompiledArtifact, CompileError> {
    let selectors: Vec<[u8; 4]> = functions.iter().map(|(s, _)| *s).collect();
    assemble_with(&selectors, facet_span, |a, i, body| {
        emit_full_body(a, body, &functions[i].1)
    })
}

/// Assemble a full runtime from a list of `(selector, body value)` pairs and wrap
/// it as a [`CompiledArtifact`], via the shared [`assemble_with`]. Drives the
/// worked [`super::emit_constant_getter`] — so a single function yields bytes
/// identical to [`compile`]'ing the same `return <intlit>;` facet.
///
/// The const-getter family this serves is always far under [`MAX_RUNTIME_BYTES`],
/// so the size guard cannot fire here (asserted via `expect`); the source-compile
/// path ([`assemble_full`]) is where an oversized facet is caught and surfaced.
pub fn assemble(functions: &[([u8; 4], BodyValue)]) -> CompiledArtifact {
    let selectors: Vec<[u8; 4]> = functions.iter().map(|(s, _)| *s).collect();
    assemble_with(&selectors, Span { start: 0, end: 0 }, |a, i, body| {
        emit_body(a, body, functions[i].1)
    })
    .expect("constant-getter runtime is always far below the EIP-170 size cap")
}

/// The storage `BASE` slot for a facet: `keccak256("localharness.<name>.storage.v1")`
/// with `<name>` LOWERCASED (design §5). Each scalar state var lives at `BASE + i`
/// (its declaration index), no packing in v1.
#[cfg(feature = "wallet")]
fn storage_base(facet_name: &str) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let preimage = format!("localharness.{}.storage.v1", facet_name.to_ascii_lowercase());
    let mut h = Keccak256::new();
    h.update(preimage.as_bytes());
    let digest = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Add `index` to a 32-byte big-endian slot word (`BASE + i`). v1 indices are tiny
/// so this is a simple low-byte add-with-carry.
#[cfg(feature = "wallet")]
fn slot_at(base: [u8; 32], index: u64) -> [u8; 32] {
    let mut out = base;
    let mut carry = index as u128;
    for byte in out.iter_mut().rev() {
        if carry == 0 {
            break;
        }
        let v = *byte as u128 + (carry & 0xFF);
        *byte = (v & 0xFF) as u8;
        carry = (carry >> 8) + (v >> 8);
    }
    out
}

/// A per-function name-resolution context: the facet's state vars + storage base,
/// plus the function being compiled (for its parameter list). A bare identifier in
/// an expression resolves to a SCALAR state var, then a parameter, in that order.
#[cfg(feature = "wallet")]
struct Resolver<'a> {
    facet: &'a Facet,
    base: [u8; 32],
    func: &'a crate::soliditylite::ast::Function,
}

#[cfg(feature = "wallet")]
impl Resolver<'_> {
    /// The declaration index of a state var by name (its slot offset / mapping base
    /// slot offset), or `None` if no state var has that name.
    fn state_var_index(&self, name: &str) -> Option<usize> {
        self.facet.state_vars.iter().position(|sv| sv.name == name)
    }

    /// The parameter index of a name, or `None` if it isn't a parameter.
    fn param_index(&self, name: &str) -> Option<usize> {
        self.func.params.iter().position(|p| p.name == name)
    }

    /// Resolve a SCALAR state variable name to its keccak-namespaced slot
    /// (`BASE + index`). Errors if the name is unknown OR names a mapping (a mapping
    /// has no scalar slot — it must be indexed).
    fn scalar_slot(&self, name: &str, span: crate::rustlite::Span) -> Result<[u8; 32], CompileError> {
        use crate::error_codes as codes;
        let idx = self.state_var_index(name).ok_or_else(|| {
            CompileError::at_code(
                codes::UNDEFINED_VARIABLE,
                format!("unknown state variable `{name}`"),
                span,
            )
        })?;
        match self.facet.state_vars[idx].kind {
            StateVarKind::Mapping { .. } => {
                return Err(CompileError::at_code(
                    codes::TYPE_MISMATCH,
                    format!("`{name}` is a mapping; it must be indexed (`{name}[key]`)"),
                    span,
                ))
            }
            StateVarKind::Array { .. } => {
                return Err(CompileError::at_code(
                    codes::TYPE_MISMATCH,
                    format!("`{name}` is an array; index it (`{name}[i]`) or read `{name}.length`"),
                    span,
                ))
            }
            StateVarKind::DynamicBytes { .. } => {
                return Err(CompileError::at_code(
                    codes::TYPE_MISMATCH,
                    format!("`{name}` is a dynamic `string`/`bytes`; it is not a single-word scalar"),
                    span,
                ))
            }
            StateVarKind::Scalar(_) => {}
        }
        Ok(slot_at(self.base, idx as u64))
    }

    /// Resolve a MAPPING name to its preimage base slot (`BASE + index`). Errors if
    /// the name is unknown OR names a scalar (a scalar can't be indexed).
    fn mapping_base_slot(&self, name: &str, span: crate::rustlite::Span) -> Result<[u8; 32], CompileError> {
        use crate::error_codes as codes;
        let idx = self.state_var_index(name).ok_or_else(|| {
            CompileError::at_code(
                codes::UNDEFINED_VARIABLE,
                format!("unknown state variable `{name}`"),
                span,
            )
        })?;
        match self.facet.state_vars[idx].kind {
            StateVarKind::Mapping { .. } => Ok(slot_at(self.base, idx as u64)),
            StateVarKind::Scalar(_) => Err(CompileError::at_code(
                codes::TYPE_MISMATCH,
                format!("`{name}` is not a mapping; it cannot be indexed"),
                span,
            )),
            StateVarKind::Array { .. } => Err(CompileError::at_code(
                codes::TYPE_MISMATCH,
                format!("`{name}` is an array, not a mapping (indexing is array-shaped, handled separately)"),
                span,
            )),
            StateVarKind::DynamicBytes { .. } => Err(CompileError::at_code(
                codes::TYPE_MISMATCH,
                format!("`{name}` is a dynamic `string`/`bytes`, not a mapping"),
                span,
            )),
        }
    }

    /// Resolve a DYNAMIC-ARRAY name to its base slot (`BASE + index`, where the length
    /// lives; elements at `keccak256(slot) + i`). Errors if the name is unknown OR
    /// names a non-array (a scalar/mapping isn't a dynamic array).
    fn array_base_slot(&self, name: &str, span: crate::rustlite::Span) -> Result<[u8; 32], CompileError> {
        use crate::error_codes as codes;
        let idx = self.state_var_index(name).ok_or_else(|| {
            CompileError::at_code(
                codes::UNDEFINED_VARIABLE,
                format!("unknown state variable `{name}`"),
                span,
            )
        })?;
        match self.facet.state_vars[idx].kind {
            StateVarKind::Array { .. } => Ok(slot_at(self.base, idx as u64)),
            _ => Err(CompileError::at_code(
                codes::TYPE_MISMATCH,
                format!("`{name}` is not a dynamic array"),
                span,
            )),
        }
    }

    /// `true` if `name` is a declared dynamic-array state var (used to route an
    /// `<name>[<i>]` index to the array layout vs. the mapping layout).
    fn is_array(&self, name: &str) -> bool {
        self.state_var_index(name)
            .map(|idx| matches!(self.facet.state_vars[idx].kind, StateVarKind::Array { .. }))
            .unwrap_or(false)
    }

    /// `true` if `name` is a declared dynamic `string`/`bytes` state var (#37).
    fn is_dynamic_bytes_var(&self, name: &str) -> bool {
        self.state_var_index(name)
            .map(|idx| matches!(self.facet.state_vars[idx].kind, StateVarKind::DynamicBytes { .. }))
            .unwrap_or(false)
    }

    /// Resolve a dynamic `string`/`bytes` state var to its base slot (`BASE + index`,
    /// the canonical-layout header slot). Errors if the name is unknown OR not a
    /// dynamic-bytes var.
    fn dynamic_bytes_slot(&self, name: &str, span: crate::rustlite::Span) -> Result<[u8; 32], CompileError> {
        use crate::error_codes as codes;
        let idx = self.state_var_index(name).ok_or_else(|| {
            CompileError::at_code(codes::UNDEFINED_VARIABLE, format!("unknown state variable `{name}`"), span)
        })?;
        match self.facet.state_vars[idx].kind {
            StateVarKind::DynamicBytes { .. } => Ok(slot_at(self.base, idx as u64)),
            _ => Err(CompileError::at_code(
                codes::TYPE_MISMATCH,
                format!("`{name}` is not a `string`/`bytes` state variable"),
                span,
            )),
        }
    }

    /// If `name` is a declared dynamic `string`/`bytes` PARAMETER, return its index;
    /// else `None` (used to route `return <param>;` to the echo lowering).
    fn dynamic_param_index(&self, name: &str) -> Option<u64> {
        self.func
            .params
            .iter()
            .position(|p| p.name == name && p.ty.is_dynamic())
            .map(|i| i as u64)
    }

    /// Lower an [`Expr`] to a [`LoweredExpr`], resolving names to slots / params.
    fn lower_expr(&self, expr: &Expr) -> Result<LoweredExpr, CompileError> {
        use crate::error_codes as codes;
        match expr {
            Expr::IntLit { value_be32, .. } => Ok(LoweredExpr::Const(*value_be32)),
            // A bare identifier: a scalar state var (SLOAD) or a parameter
            // (CALLDATALOAD), preferring a state var on a name clash.
            Expr::StateVar { name, span } => {
                if self.state_var_index(name).is_some() {
                    // `scalar_slot` rejects mapping/array/dynamic-bytes vars cleanly.
                    Ok(LoweredExpr::Load(self.scalar_slot(name, *span)?))
                } else if let Some(p) = self.param_index(name) {
                    // A dynamic `string`/`bytes` param is NOT a single word — it is
                    // only valid as a whole `return <param>;` (the echo path, handled
                    // in the body match), never inside an expression.
                    if self.func.params[p].ty.is_dynamic() {
                        return Err(CompileError::at_code(
                            codes::TYPE_MISMATCH,
                            format!(
                                "`{name}` is a dynamic `string`/`bytes` parameter; it is only \
                                 supported as a whole `return {name};`, not inside an expression"
                            ),
                            *span,
                        ));
                    }
                    Ok(LoweredExpr::Param(p as u64))
                } else {
                    Err(CompileError::at_code(
                        codes::UNDEFINED_VARIABLE,
                        format!("unknown variable `{name}`"),
                        *span,
                    ))
                }
            }
            Expr::MsgSender { .. } => Ok(LoweredExpr::Caller),
            Expr::BlockTimestamp { .. } => Ok(LoweredExpr::Timestamp),
            Expr::BlockNumber { .. } => Ok(LoweredExpr::Number),
            // `<name>[<i>]`: a dynamic-array element (keccak256(slot)+i) when `name`
            // is an array, else a mapping entry (keccak256(key ++ base)).
            Expr::Index { base, key, span } if self.is_array(base) => Ok(LoweredExpr::ArrayLoad {
                slot: self.array_base_slot(base, *span)?,
                index: Box::new(self.lower_expr(key)?),
            }),
            Expr::Index { base, key, span } => Ok(LoweredExpr::MapLoad {
                base_slot: self.mapping_base_slot(base, *span)?,
                key: Box::new(self.lower_expr(key)?),
            }),
            Expr::ArrayLen { base, span } => Ok(LoweredExpr::ArrayLen {
                slot: self.array_base_slot(base, *span)?,
            }),
            Expr::Add { lhs, rhs, .. } => Ok(LoweredExpr::Add(
                Box::new(self.lower_expr(lhs)?),
                Box::new(self.lower_expr(rhs)?),
            )),
            Expr::Sub { lhs, rhs, .. } => Ok(LoweredExpr::Sub(
                Box::new(self.lower_expr(lhs)?),
                Box::new(self.lower_expr(rhs)?),
            )),
            Expr::Mul { lhs, rhs, .. } => Ok(LoweredExpr::Mul(
                Box::new(self.lower_expr(lhs)?),
                Box::new(self.lower_expr(rhs)?),
            )),
            Expr::Div { lhs, rhs, .. } => Ok(LoweredExpr::Div(
                Box::new(self.lower_expr(lhs)?),
                Box::new(self.lower_expr(rhs)?),
            )),
            Expr::Mod { lhs, rhs, .. } => Ok(LoweredExpr::Mod(
                Box::new(self.lower_expr(lhs)?),
                Box::new(self.lower_expr(rhs)?),
            )),
            Expr::Cmp { op, lhs, rhs, .. } => Ok(LoweredExpr::Cmp {
                op: *op,
                lhs: Box::new(self.lower_expr(lhs)?),
                rhs: Box::new(self.lower_expr(rhs)?),
            }),
            // A string literal is a dynamic value, not a single word — it is only
            // valid as a whole `return "…";` (handled in the body match, not here).
            Expr::StrLit { span, .. } => Err(CompileError::at_code(
                crate::error_codes::UNSUPPORTED_FEATURE,
                "a string literal is only supported as a whole `return` value in v1 (not in an \
                 assignment, comparison, arithmetic, or event argument)"
                    .to_string(),
                *span,
            )),
        }
    }
}

/// The selector signature `<name>(<type>,<type>,…)` for a function — the ABI
/// canonical form whose `keccak256[..4]` is the 4-byte selector.
#[cfg(feature = "wallet")]
fn function_signature(func: &crate::soliditylite::ast::Function) -> String {
    let types: Vec<&str> = func.params.iter().map(|p| p.ty.abi_name()).collect();
    format!("{}({})", func.name, types.join(","))
}

/// The canonical event signature `<Name>(<type>,<type>,…)` — the ABI form whose
/// FULL `keccak256` (all 32 bytes, NOT the 4-byte selector) is the log's `topic0`.
/// `indexed`/non-`indexed` does NOT change the signature (only the type list does),
/// matching Solidity's `eventID` rule.
#[cfg(feature = "wallet")]
fn event_signature(ev: &crate::soliditylite::ast::EventDecl) -> String {
    let types: Vec<&str> = ev.args.iter().map(|arg| arg.ty.abi_name()).collect();
    format!("{}({})", ev.name, types.join(","))
}

/// `topic0` for an event = the FULL 32-byte `keccak256` of its canonical signature
/// (`keccak256("Incremented(address,uint256,uint256)")`). This is the whole hash,
/// NOT the 4-byte function-selector truncation — events index on the full word.
#[cfg(feature = "wallet")]
pub fn event_topic0(signature: &str) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let mut h = Keccak256::new();
    h.update(signature.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&h.finalize());
    out
}

/// Lower an `emit <Event>(<args>)` statement: resolve the event by name, check the
/// argument count matches the declaration, compute `topic0` (the full keccak of the
/// canonical signature), and split the args into indexed (topics) and non-indexed
/// (data) by the DECLARED `indexed` flags — each lowered against the function's
/// resolver.
///
/// Errors (all clean [`CompileError`]s, never a panic):
/// - `UNKNOWN_FUNCTION` — no `event <name>` declared in the facet;
/// - `ARITY_MISMATCH` — the emit arg count ≠ the declared arg count;
/// - `UNSUPPORTED_FEATURE` — more than 3 `indexed` args (EVM `LOG`s cap at 4 topics,
///   one of which is always `topic0`).
#[cfg(feature = "wallet")]
fn lower_emit(
    facet: &Facet,
    r: &Resolver,
    ev_name: &str,
    args: &[Expr],
    span: crate::rustlite::Span,
) -> Result<LoweredEmit, CompileError> {
    use crate::error_codes as codes;
    let decl = facet.events.iter().find(|e| e.name == ev_name).ok_or_else(|| {
        CompileError::at_code(
            codes::UNKNOWN_FUNCTION,
            format!("unknown event `{ev_name}` (no matching `event` declaration)"),
            span,
        )
    })?;
    if args.len() != decl.args.len() {
        return Err(CompileError::at_code(
            codes::ARITY_MISMATCH,
            format!(
                "event `{ev_name}` expects {} argument(s), got {}",
                decl.args.len(),
                args.len()
            ),
            span,
        ));
    }
    let num_indexed = decl.args.iter().filter(|a| a.indexed).count();
    // EVM LOGs carry at most 4 topics; topic0 is the signature hash, so at most 3
    // indexed args. (Solidity has the same 3-indexed limit for non-anonymous events.)
    if num_indexed > 3 {
        return Err(CompileError::at_code(
            codes::UNSUPPORTED_FEATURE,
            format!("event `{ev_name}` has {num_indexed} indexed args; at most 3 are allowed (LOG topic cap)"),
            span,
        ));
    }
    let topic0 = event_topic0(&event_signature(decl));
    let mut indexed = Vec::with_capacity(num_indexed);
    let mut data = Vec::with_capacity(decl.args.len() - num_indexed);
    for (arg_decl, arg_expr) in decl.args.iter().zip(args) {
        let lowered = r.lower_expr(arg_expr)?;
        if arg_decl.indexed {
            indexed.push(lowered);
        } else {
            data.push(lowered);
        }
    }
    Ok(LoweredEmit { topic0, indexed, data })
}

/// Lower a list of mutating-body statements (a function body or an `if`/`else`
/// branch). Recurses for `if`.
#[cfg(feature = "wallet")]
fn lower_stmts(facet: &Facet, r: &Resolver, stmts: &[Stmt]) -> Result<Vec<LoweredStmt>, CompileError> {
    stmts.iter().map(|s| lower_stmt(facet, r, s)).collect()
}

/// Lower one mutating-body statement: a `require` guard, a scalar/mapping
/// assignment, an `emit`, or an `if`/`else` branch (its bodies lowered
/// recursively). Anything else (a bare `return`, a nested raw block) is
/// parser-unreachable in a mutating body and surfaces a clean error.
#[cfg(feature = "wallet")]
fn lower_stmt(facet: &Facet, r: &Resolver, stmt: &Stmt) -> Result<LoweredStmt, CompileError> {
    use crate::error_codes as codes;
    Ok(match stmt {
        Stmt::Require { cond, .. } => LoweredStmt::Require(r.lower_expr(cond)?),
        // `<dynBytesVar> = "<literal>";` (#37 slice 1, WRITE): a dynamic `string`/
        // `bytes` state var is written from a COMPILE-TIME literal only in v1 (a
        // runtime dynamic value would need a copy loop). A non-literal RHS to a
        // dynamic var, or a string literal to a non-dynamic var, is a clean error.
        Stmt::Assign { name, value: Expr::StrLit { value: bytes, .. }, span }
            if r.is_dynamic_bytes_var(name) =>
        {
            LoweredStmt::Assign(LoweredAssign::ConstDynamicBytes {
                base_slot: r.dynamic_bytes_slot(name, *span)?,
                bytes: bytes.clone(),
            })
        }
        Stmt::Assign { name, span, .. } if r.is_dynamic_bytes_var(name) => {
            return Err(CompileError::at_code(
                codes::UNSUPPORTED_FEATURE,
                format!("`{name}` is a dynamic `string`/`bytes`; v1 only supports assigning a string literal to it"),
                *span,
            ))
        }
        Stmt::Assign { name, value, span } => LoweredStmt::Assign(LoweredAssign::Scalar {
            slot: r.scalar_slot(name, *span)?,
            value: r.lower_expr(value)?,
        }),
        // `<name>[<i>] = <e>;` — a dynamic-array element write when `name` is an
        // array, else a mapping-entry write.
        Stmt::IndexAssign { base: idx_name, key, value, span } if r.is_array(idx_name) => {
            LoweredStmt::Assign(LoweredAssign::ArrayElem {
                slot: r.array_base_slot(idx_name, *span)?,
                index: r.lower_expr(key)?,
                value: r.lower_expr(value)?,
            })
        }
        Stmt::IndexAssign { base: map_name, key, value, span } => LoweredStmt::Assign(LoweredAssign::MapEntry {
            base_slot: r.mapping_base_slot(map_name, *span)?,
            key: r.lower_expr(key)?,
            value: r.lower_expr(value)?,
        }),
        Stmt::Push { base, value, span } => LoweredStmt::Assign(LoweredAssign::ArrayPush {
            slot: r.array_base_slot(base, *span)?,
            value: r.lower_expr(value)?,
        }),
        Stmt::Pop { base, span } => LoweredStmt::Assign(LoweredAssign::ArrayPop {
            slot: r.array_base_slot(base, *span)?,
        }),
        // `delete <arr>[<i>];` — only a dynamic-array element delete is supported in
        // v1 (`array_base_slot` rejects a non-array, e.g. a mapping/scalar, cleanly).
        Stmt::DeleteIndex { base, key, span } => LoweredStmt::Assign(LoweredAssign::ArrayDelete {
            slot: r.array_base_slot(base, *span)?,
            index: r.lower_expr(key)?,
        }),
        Stmt::Emit { name: ev_name, args, span } => LoweredStmt::Emit(lower_emit(facet, r, ev_name, args, *span)?),
        Stmt::If { cond, then_body, else_body, .. } => LoweredStmt::If {
            cond: r.lower_expr(cond)?,
            then_body: lower_stmts(facet, r, then_body)?,
            else_body: lower_stmts(facet, r, else_body)?,
        },
        other => {
            return Err(CompileError::at_code(
                codes::UNSUPPORTED_FEATURE,
                format!("only `require`, `if`, `emit`, and assignments are supported in a mutating body, got {other:?}"),
                r.func.span,
            ))
        }
    })
}

/// Compile a typed [`Facet`] to a deployable [`CompiledArtifact`].
///
/// Selectors are `keccak256("<name>(<types>)")[..4]` (the ABI canonical signature)
/// computed via the shared [`crate::registry::selector`] helper. A view getter's
/// `return <expr>;` lowers to a [`Body::View`]/[`Body::ViewExpr`] (`<intlit>` →
/// constant, scalar `<stateVar>` → `SLOAD`, parameter → `CALLDATALOAD(4+32*i)`,
/// `msg.sender` → `CALLER`, `<map>[<key>]` → keccak-slot `SLOAD`, `a + b` → `ADD`,
/// `a <op> b` → `GT`/`LT`/`EQ` (`<=`/`>=` via `ISZERO`)); a mutating function's
/// `require(cond, "…")` guards and `<stateVar> = <expr>;` / `<map>[<key>] = <expr>;`
/// assignments lower to a [`Body::Mutating`] that branches a failed `require` to a
/// shared `REVERT(0,0)` stub and `SSTORE`s each assignment, then `RETURN(0,0)`.
///
/// Gated on `wallet` because selector keccak + storage-slot keccak both live
/// behind that feature (sha3/registry); without it, the frontend still
/// lexes/parses but cannot emit selectors.
#[cfg(feature = "wallet")]
pub fn compile(facet: &Facet) -> Result<CompiledArtifact, CompileError> {
    use crate::error_codes as codes;

    let base = storage_base(&facet.name);
    let mut lowered: Vec<([u8; 4], Body)> = Vec::with_capacity(facet.functions.len());
    let mut seen_selectors: Vec<[u8; 4]> = Vec::new();

    for func in &facet.functions {
        let signature = function_signature(func);
        let selector = crate::registry::selector(&signature);
        // Intra-facet keccak4 collision check (design §7 layer-1 (a)).
        if seen_selectors.contains(&selector) {
            return Err(CompileError::at_code(
                codes::UNSUPPORTED_FEATURE,
                format!("selector collision: two functions hash to {selector:02x?}"),
                func.span,
            ));
        }
        seen_selectors.push(selector);

        let r = Resolver { facet, base, func };
        let returns_dynamic = func.returns.map(|t| t.is_dynamic()).unwrap_or(false);
        let body = match &func.body {
            // `return "<lit>";` from a `returns (string|bytes)` function → a constant
            // dynamic ABI return (#37 slice 1: a literal, no storage/calldata).
            // A string literal returned WITHOUT a dynamic return type is a type error.
            Stmt::Return(Expr::StrLit { value, span }) => {
                if !returns_dynamic {
                    return Err(CompileError::at_code(
                        codes::TYPE_MISMATCH,
                        "a string literal can only be returned from a `returns (string)`/`returns (bytes)` function"
                            .to_string(),
                        *span,
                    ));
                }
                Body::ConstString(value.clone())
            }
            // `return <dynBytesStateVar>;` from a dynamic-return function → ABI-encode
            // the stored value (#37 slice 1, READ side).
            Stmt::Return(Expr::StateVar { name, span })
                if returns_dynamic && r.is_dynamic_bytes_var(name) =>
            {
                Body::DynamicStorageReturn { slot: r.dynamic_bytes_slot(name, *span)? }
            }
            // `return <dynParam>;` from a dynamic-return function → echo the dynamic
            // parameter (#37 slices 2+3).
            Stmt::Return(Expr::StateVar { name, .. })
                if returns_dynamic && r.dynamic_param_index(name).is_some() =>
            {
                Body::EchoParam { param_index: r.dynamic_param_index(name).unwrap() }
            }
            // A `returns (string|bytes)` function MUST be one of the dynamic shapes
            // above (literal / state-var read / param echo) in v1 — catch any other
            // body before it falls into the single-word return paths below.
            _ if returns_dynamic => {
                return Err(CompileError::at_code(
                    codes::TYPE_MISMATCH,
                    "a `returns (string)`/`returns (bytes)` function must return a string literal, a \
                     `string`/`bytes` state variable, or a `string`/`bytes` parameter in v1"
                        .to_string(),
                    func.span,
                ))
            }
            // Simple view getter `return <intlit>;` / `return <scalarStateVar>;`
            // keeps the golden-gate `BodyValue` path (PUSH32 / PUSH32+SLOAD); every
            // other return shape (param ref, msg.sender, mapping index, `a + b`)
            // uses the richer expr lowering.
            Stmt::Return(Expr::IntLit { value_be32, .. }) => Body::View(BodyValue::Const(*value_be32)),
            Stmt::Return(Expr::StateVar { name, span }) if r.state_var_index(name).is_some() => {
                Body::View(BodyValue::StorageSlot(r.scalar_slot(name, *span)?))
            }
            Stmt::Return(expr) => Body::ViewExpr(r.lower_expr(expr)?),
            // Mutating function: `{ (require|if|emit|<stateVar> = e|<map>[k] = e); … }`.
            Stmt::Block(stmts) => Body::Mutating(lower_stmts(facet, &r, stmts)?),
            // A bare assignment as a whole function body is parser-unreachable; treat
            // it as an unsupported shape rather than panicking.
            other => {
                return Err(CompileError::at_code(
                    codes::UNSUPPORTED_FEATURE,
                    format!("unsupported function body {other:?}"),
                    func.span,
                ))
            }
        };
        lowered.push((selector, body));
    }

    assemble_full(lowered, facet.span)
}

#[cfg(test)]
mod tests {
    #[test]
    fn assemble_one_const_fn_round_trips() {
        let mut w = [0u8; 32];
        w[31] = 7;
        let art = super::assemble(&[([0xaa, 0xbb, 0xcc, 0xdd], super::BodyValue::Const(w))]);
        // init_code == init_wrapper(runtime)
        assert_eq!(art.init_code, crate::soliditylite::asm::init_wrapper(&art.runtime));
    }

    #[cfg(feature = "wallet")]
    #[test]
    fn slot_at_adds_index_to_base() {
        let base = [0u8; 32]; // a base ending in 0 for an easy check
        assert_eq!(super::slot_at(base, 0), base);
        let mut one = [0u8; 32];
        one[31] = 1;
        assert_eq!(super::slot_at(base, 1), one);
    }

    #[cfg(feature = "wallet")]
    #[test]
    fn slot_at_carries_across_a_byte_boundary() {
        // base ...00ff + 1 = ...0100
        let mut base = [0u8; 32];
        base[31] = 0xff;
        let got = super::slot_at(base, 1);
        let mut want = [0u8; 32];
        want[30] = 0x01;
        assert_eq!(got, want);
    }

    /// L40 regression: a facet whose runtime would blow past the EIP-170 size limit
    /// (and overflow the assembler's u16 jump/length operands) returns a clean
    /// `CompileError`, NOT an `expect`-panic — which in the browser is an
    /// uncatchable, tab-killing abort. ~1400 trivial getters push the runtime well
    /// past 64KB; pre-fix this aborted in `Asm::finish`. The test reaching its
    /// assertion at all proves the panic is gone. (The breadth guard at 300 fns —
    /// `mod.rs`'s `breadth_does_not_trip_the_depth_guard` — still compiles fine.)
    #[cfg(feature = "wallet")]
    #[test]
    fn oversized_facet_errors_cleanly_instead_of_panicking() {
        let mut src = String::from("facet Huge {");
        for i in 0..1400 {
            src.push_str(&format!(
                " function f{i}() external view returns (uint256) {{ return {i}; }}"
            ));
        }
        src.push('}');
        let err = super::super::compile(&src)
            .expect_err("a facet past the EIP-170 size limit must error, not compile or panic");
        assert_eq!(
            err.code,
            Some(crate::error_codes::UNSUPPORTED_FEATURE),
            "oversize must surface as a clean UNSUPPORTED_FEATURE: {err}"
        );
    }

    /// THE TARGET: `facet Tally { uint256 n; function bump() external { n = n + 1; }
    /// function get() external view returns (uint256) { return n; } }` compiles, and
    /// bump()'s body is exactly `SLOAD(slot) PUSH1 0x01 ADD SSTORE(slot) RETURN(0,0)`.
    #[cfg(feature = "wallet")]
    #[test]
    fn tally_bump_emits_sload_add_sstore() {
        use super::super::asm::op;
        const SRC: &str = "facet Tally { uint256 n; \
             function bump() external { n = n + 1; } \
             function get() external view returns (uint256) { return n; } }";
        let art = super::super::compile(SRC).expect("Tally must compile");
        let rt = &art.runtime;

        // Slot of `n` = BASE + 0 = keccak256("localharness.tally.storage.v1").
        let base = super::storage_base("Tally");
        let slot_n = super::slot_at(base, 0);

        // Both selectors must appear in the dispatch region.
        let sel_bump = crate::registry::selector("bump()");
        let sel_get = crate::registry::selector("get()");
        let push4 = |sel: [u8; 4]| -> Vec<u8> { std::iter::once(op::PUSH1 + 3).chain(sel).collect() };
        assert!(
            rt.windows(5).any(|w| w == push4(sel_bump)),
            "bump() selector PUSH4 must be present"
        );
        assert!(
            rt.windows(5).any(|w| w == push4(sel_get)),
            "get() selector PUSH4 must be present"
        );

        // The bump() body, byte-for-byte:
        //   PUSH32 <slot_n> SLOAD   ; lhs = n
        //   PUSH1  0x01             ; rhs = 1 (minimal-width operand)
        //   ADD                     ; n + 1
        //   PUSH32 <slot_n> SSTORE  ; n = n + 1
        //   PUSH1 0x00 PUSH1 0x00 RETURN  ; empty return
        let mut expected = Vec::new();
        expected.push(op::PUSH1 + 31); // PUSH32
        expected.extend_from_slice(&slot_n);
        expected.push(op::SLOAD);
        expected.extend_from_slice(&[op::PUSH1, 0x01]);
        expected.push(op::ADD);
        expected.push(op::PUSH1 + 31); // PUSH32
        expected.extend_from_slice(&slot_n);
        expected.push(op::SSTORE);
        expected.extend_from_slice(&[op::PUSH1, 0x00, op::PUSH1, 0x00, op::RETURN]);

        assert!(
            rt.windows(expected.len()).any(|w| w == expected.as_slice()),
            "bump() must emit SLOAD/PUSH1 0x01/ADD/SSTORE/RETURN(0,0) at slot n.\n\
             expected window not found; runtime = {}",
            to_hex(rt)
        );

        // The bump() body must land at bump()'s JUMPDEST: find the dispatch arm's
        // PUSH2 <body> operand, follow it to a JUMPDEST, and the very next bytes are
        // the SLOAD/ADD/SSTORE sequence (after that one JUMPDEST byte).
        let arm = push4(sel_bump);
        let arm_pos = rt.windows(arm.len()).position(|w| w == arm.as_slice()).unwrap();
        // arm layout: DUP1(prev) PUSH4 sel EQ PUSH2 body JUMPI — body operand sits at
        // arm_pos + 5 (after PUSH4+sel) + 1 (EQ) + 1 (PUSH2) = arm_pos + 7.
        let body_op = arm_pos + 5 + 1 + 1;
        let body_off = u16::from_be_bytes([rt[body_op], rt[body_op + 1]]) as usize;
        assert_eq!(rt[body_off], op::JUMPDEST, "bump() body must start with JUMPDEST");
        // Right after the JUMPDEST: PUSH32 slot_n SLOAD …
        assert_eq!(rt[body_off + 1], op::PUSH1 + 31, "first op after JUMPDEST is PUSH32");
        assert_eq!(&rt[body_off + 2..body_off + 34], &slot_n, "PUSH32 pushes slot n");
        assert_eq!(rt[body_off + 34], op::SLOAD, "then SLOAD");
        assert_eq!(&rt[body_off + 35..body_off + 37], &[op::PUSH1, 0x01], "then PUSH1 1");
        assert_eq!(rt[body_off + 37], op::ADD, "then ADD");
        assert_eq!(rt[body_off + 38], op::PUSH1 + 31, "then PUSH32 (slot)");
        assert_eq!(&rt[body_off + 39..body_off + 71], &slot_n, "the SSTORE slot");
        assert_eq!(rt[body_off + 71], op::SSTORE, "then SSTORE");

        // init_code wraps the runtime.
        assert_eq!(art.init_code, super::super::asm::init_wrapper(rt));
    }

    /// `n = n + 1`'s value emits exactly `SLOAD PUSH1 0x01 ADD` (the expression
    /// lowering, independent of dispatch placement) — a focused opcode-order check.
    #[cfg(feature = "wallet")]
    #[test]
    fn add_expression_lowers_to_sload_push_add() {
        use super::super::asm::op;
        let art = super::super::compile(
            "facet C { uint256 n; function bump() external { n = n + 1; } }",
        )
        .unwrap();
        let rt = &art.runtime;
        let base = super::storage_base("C");
        let slot = super::slot_at(base, 0);
        // SLOAD must be immediately followed by PUSH1 0x01 then ADD.
        let pos = rt.iter().position(|&b| b == op::SLOAD).expect("an SLOAD must be present");
        assert_eq!(&rt[pos + 1..pos + 3], &[op::PUSH1, 0x01], "SLOAD then PUSH1 1");
        assert_eq!(rt[pos + 3], op::ADD, "then ADD");
        // and an SSTORE to the same slot follows.
        let mut push32_slot = vec![op::PUSH1 + 31];
        push32_slot.extend_from_slice(&slot);
        assert!(
            rt.windows(33).any(|w| w == push32_slot.as_slice()),
            "the slot is PUSH32'd for the SSTORE"
        );
        assert!(rt.contains(&op::SSTORE), "an SSTORE must be present");
    }

    /// An assignment to an undeclared state var is a clean `CompileError`, not a panic.
    #[cfg(feature = "wallet")]
    #[test]
    fn assign_to_unknown_var_is_a_clean_error() {
        let err = super::super::compile(
            "facet C { function f() external { ghost = 1; } }",
        )
        .expect_err("assigning an undeclared var must fail cleanly");
        assert_eq!(err.code, Some(crate::error_codes::UNDEFINED_VARIABLE));
        assert!(err.to_string().starts_with("LH0"));
    }

    /// Reading an undeclared state var inside an expression is also a clean error.
    #[cfg(feature = "wallet")]
    #[test]
    fn read_unknown_var_in_add_is_a_clean_error() {
        let err = super::super::compile(
            "facet C { uint256 n; function f() external { n = n + missing; } }",
        )
        .expect_err("reading an undeclared var must fail cleanly");
        assert_eq!(err.code, Some(crate::error_codes::UNDEFINED_VARIABLE));
    }

    // ── PARAMS / msg.sender / MAPPINGS (Installment 1 MVP) ───────────────────

    /// The keccak mapping-entry slot for a key word at a base slot, mirroring the
    /// EVM `keccak256(pad32(key) ++ pad32(base))` derivation — the off-chain truth
    /// the emitted MSTORE/MSTORE/KECCAK256 must reproduce on-chain.
    #[cfg(feature = "wallet")]
    fn map_entry_slot(key: &[u8; 32], base: &[u8; 32]) -> [u8; 32] {
        use sha3::{Digest, Keccak256};
        let mut h = Keccak256::new();
        h.update(key); // FIRST preimage word
        h.update(base); // SECOND preimage word
        let mut out = [0u8; 32];
        out.copy_from_slice(&h.finalize());
        out
    }

    /// `add(uint256 amt)`'s body loads `amt` via `CALLDATALOAD(0x04)` (ABI arg 0)
    /// and stores into the `bal[msg.sender]` mapping entry — exercising params,
    /// msg.sender (CALLER), and the mapping slot derivation in one body.
    #[cfg(feature = "wallet")]
    #[test]
    fn add_loads_param_via_calldataload_and_writes_map_entry() {
        use super::super::asm::op;
        const SRC: &str = "facet Ledger { mapping(address => uint256) bal; \
             function add(uint256 amt) external { bal[msg.sender] = bal[msg.sender] + amt; } }";
        let art = super::super::compile(SRC).expect("Ledger add() must compile");
        let rt = &art.runtime;

        // The mapping base slot = BASE + 0 = keccak256("localharness.ledger.storage.v1").
        let base = super::storage_base("Ledger");
        let map_base = super::slot_at(base, 0);

        // `amt` (param index 0) → CALLDATALOAD(4): PUSH1 0x04 CALLDATALOAD.
        assert!(
            rt.windows(3).any(|w| w == [op::PUSH1, 0x04, op::CALLDATALOAD]),
            "amt must load via CALLDATALOAD(0x04); runtime = {}",
            to_hex(rt)
        );
        // `msg.sender` → CALLER.
        assert!(rt.contains(&op::CALLER), "msg.sender must emit CALLER");

        // The mapping slot derivation appears: CALLER (key) PUSH1 0x00 MSTORE
        // PUSH32 <map_base> PUSH1 0x20 MSTORE PUSH1 0x40 PUSH1 0x00 KECCAK256.
        let mut derive = vec![op::CALLER, op::PUSH1, 0x00, op::MSTORE, op::PUSH1 + 31];
        derive.extend_from_slice(&map_base);
        derive.extend_from_slice(&[
            op::PUSH1, 0x20, op::MSTORE, op::PUSH1, 0x40, op::PUSH1, 0x00, op::KECCAK256,
        ]);
        assert!(
            rt.windows(derive.len()).any(|w| w == derive.as_slice()),
            "the bal[msg.sender] slot derivation (MSTORE key / MSTORE base / KECCAK256) \
             must be present; runtime = {}",
            to_hex(rt)
        );
        // The write ends in an SSTORE to the derived slot; the read side SLOADs.
        assert!(rt.contains(&op::SSTORE), "the map write must SSTORE");
        assert!(rt.contains(&op::SLOAD), "the map read must SLOAD");
        assert!(rt.contains(&op::ADD), "bal[..] + amt must ADD");
    }

    /// `balanceOf(address who)` derives the entry slot from the CALLDATA `who`
    /// (param 0, CALLDATALOAD(0x04)) and SLOADs it — the read-only mapping getter.
    #[cfg(feature = "wallet")]
    #[test]
    fn balance_of_derives_slot_from_calldata_key_then_sloads() {
        use super::super::asm::op;
        const SRC: &str = "facet Ledger { mapping(address => uint256) bal; \
             function balanceOf(address who) external view returns (uint256) { return bal[who]; } }";
        let art = super::super::compile(SRC).expect("balanceOf must compile");
        let rt = &art.runtime;

        let base = super::storage_base("Ledger");
        let map_base = super::slot_at(base, 0);

        // The full read body: derive slot from CALLDATALOAD(0x04) (the key `who`),
        // then SLOAD, then the MSTORE/RETURN(0,0x20) tail.
        //   PUSH1 0x04 CALLDATALOAD          ; key = who
        //   PUSH1 0x00 MSTORE                ; mem[0x00] = key
        //   PUSH32 <map_base> PUSH1 0x20 MSTORE  ; mem[0x20] = base
        //   PUSH1 0x40 PUSH1 0x00 KECCAK256  ; slot
        //   SLOAD                            ; bal[who]
        let mut expected = vec![
            op::PUSH1, 0x04, op::CALLDATALOAD, op::PUSH1, 0x00, op::MSTORE, op::PUSH1 + 31,
        ];
        expected.extend_from_slice(&map_base);
        expected.extend_from_slice(&[
            op::PUSH1, 0x20, op::MSTORE, op::PUSH1, 0x40, op::PUSH1, 0x00, op::KECCAK256, op::SLOAD,
        ]);
        assert!(
            rt.windows(expected.len()).any(|w| w == expected.as_slice()),
            "balanceOf must derive bal[who]'s slot from calldata then SLOAD; runtime = {}",
            to_hex(rt)
        );

        // The selector is keccak256("balanceOf(address)")[..4] — the ABI signature
        // INCLUDES the param type, proving function_signature picks it up.
        let sel = crate::registry::selector("balanceOf(address)");
        let push4: Vec<u8> = std::iter::once(op::PUSH1 + 3).chain(sel).collect();
        assert!(
            rt.windows(5).any(|w| w == push4.as_slice()),
            "balanceOf(address) selector must be dispatched"
        );
    }

    /// THE TARGET facet (`Ledger`) compiles end-to-end and the off-chain keccak
    /// truth for `bal[msg.sender]` matches the PUSH32'd base + derivation order.
    #[cfg(feature = "wallet")]
    #[test]
    fn ledger_target_compiles_and_slot_matches_offchain_keccak() {
        const SRC: &str = "facet Ledger { mapping(address => uint256) bal; \
             function add(uint256 amt) external { bal[msg.sender] = bal[msg.sender] + amt; } \
             function balanceOf(address who) external view returns (uint256) { return bal[who]; } }";
        let art = super::super::compile(SRC).expect("the Ledger TARGET must compile");
        assert_eq!(art.init_code, super::super::asm::init_wrapper(&art.runtime));

        // Sanity-check the off-chain slot helper against a hand-rolled keccak for a
        // sample address key — this is the value the on-chain KECCAK256 reproduces.
        let base = super::storage_base("Ledger");
        let map_base = super::slot_at(base, 0);
        let mut key = [0u8; 32];
        key[12..].copy_from_slice(&[0x11; 20]); // a left-padded 20-byte address
        let slot = map_entry_slot(&key, &map_base);
        // It must equal keccak256(key ++ base) computed independently.
        use sha3::{Digest, Keccak256};
        let mut h = Keccak256::new();
        h.update(key);
        h.update(map_base);
        let mut want = [0u8; 32];
        want.copy_from_slice(&h.finalize());
        assert_eq!(slot, want, "map entry slot = keccak256(key ++ base)");
    }

    /// Indexing a non-mapping scalar is a clean `TYPE_MISMATCH`, not a panic.
    #[cfg(feature = "wallet")]
    #[test]
    fn indexing_a_scalar_is_a_clean_error() {
        let err = super::super::compile(
            "facet C { uint256 n; function f() external view returns (uint256) { return n[0]; } }",
        )
        .expect_err("indexing a scalar must fail cleanly");
        assert_eq!(err.code, Some(crate::error_codes::TYPE_MISMATCH));
    }

    /// Using a mapping name bare (without an index) is a clean `TYPE_MISMATCH`.
    #[cfg(feature = "wallet")]
    #[test]
    fn bare_mapping_reference_is_a_clean_error() {
        let err = super::super::compile(
            "facet C { mapping(address => uint256) m; \
             function f() external view returns (uint256) { return m; } }",
        )
        .expect_err("a bare mapping reference must fail cleanly");
        assert_eq!(err.code, Some(crate::error_codes::TYPE_MISMATCH));
    }

    /// Referencing an unknown name (neither a state var nor a parameter) is a clean
    /// `UNDEFINED_VARIABLE` error.
    #[cfg(feature = "wallet")]
    #[test]
    fn unknown_param_reference_is_a_clean_error() {
        let err = super::super::compile(
            "facet C { function f(uint256 a) external view returns (uint256) { return b; } }",
        )
        .expect_err("an unknown name must fail cleanly");
        assert_eq!(err.code, Some(crate::error_codes::UNDEFINED_VARIABLE));
    }

    // ── DYNAMIC ARRAYS (uint256[] storage: length / [i] / push) ──────────────

    /// The off-chain truth for a dynamic-array element slot:
    /// `keccak256(pad32(slot)) + index` — the canonical Solidity layout the emitted
    /// MSTORE/KECCAK256/ADD must reproduce on-chain. Returns the 32-byte BE slot.
    #[cfg(feature = "wallet")]
    fn array_elem_slot(slot: &[u8; 32], index: u64) -> [u8; 32] {
        use sha3::{Digest, Keccak256};
        let base: [u8; 32] = Keccak256::digest(slot).into();
        super::slot_at(base, index)
    }

    /// `xs.length` reads the array's BASE slot directly: `PUSH32 <slot> SLOAD`. The
    /// length is NOT keccak-derived (only the elements are).
    #[cfg(feature = "wallet")]
    #[test]
    fn array_length_reads_the_base_slot() {
        use super::super::asm::op;
        const SRC: &str = "facet C { uint256[] xs; \
             function len() external view returns (uint256) { return xs.length; } }";
        let rt = &super::super::compile(SRC).unwrap().runtime;
        let slot = super::slot_at(super::storage_base("C"), 0);
        let mut want = vec![op::PUSH1 + 31];
        want.extend_from_slice(&slot);
        want.push(op::SLOAD);
        assert!(
            rt.windows(want.len()).any(|w| w == want.as_slice()),
            "xs.length must be PUSH32 <baseSlot> SLOAD; runtime = {}",
            to_hex(rt)
        );
    }

    /// `xs[i]` (read) derives the element slot `keccak256(pad32(slot)) + i` then
    /// SLOADs — the exact MSTORE/KECCAK256/<index>/ADD sequence, with the keccak
    /// preimage being the BASE slot (not key++base like a mapping).
    #[cfg(feature = "wallet")]
    #[test]
    fn array_index_read_derives_keccak_slot_plus_index() {
        use super::super::asm::op;
        const SRC: &str = "facet C { uint256[] xs; \
             function at(uint256 i) external view returns (uint256) { return xs[i]; } }";
        let rt = &super::super::compile(SRC).unwrap().runtime;
        let slot = super::slot_at(super::storage_base("C"), 0);

        // PUSH32 <slot> PUSH1 0x00 MSTORE  ; mem[0..0x20] = slot
        // PUSH1 0x20 PUSH1 0x00 KECCAK256  ; base = keccak256(slot)
        // PUSH1 0x04 CALLDATALOAD          ; i (param 0)
        // ADD                              ; elem slot = base + i
        // SLOAD
        let mut want = vec![op::PUSH1 + 31];
        want.extend_from_slice(&slot);
        want.extend_from_slice(&[
            op::PUSH1, 0x00, op::MSTORE, op::PUSH1, 0x20, op::PUSH1, 0x00, op::KECCAK256,
            op::PUSH1, 0x04, op::CALLDATALOAD, op::ADD, op::SLOAD,
        ]);
        assert!(
            rt.windows(want.len()).any(|w| w == want.as_slice()),
            "xs[i] must derive keccak256(slot)+i then SLOAD; runtime = {}",
            to_hex(rt)
        );
        // The selector includes the param type.
        let sel = crate::registry::selector("at(uint256)");
        let push4: Vec<u8> = std::iter::once(op::PUSH1 + 3).chain(sel).collect();
        assert!(rt.windows(5).any(|w| w == push4.as_slice()), "at(uint256) dispatched");
    }

    /// `xs[i] = v` (write) stores `v` at the keccak-derived element slot. The store
    /// pushes the VALUE first (stays below), derives the slot on top, then SSTOREs.
    #[cfg(feature = "wallet")]
    #[test]
    fn array_index_write_sstores_to_keccak_slot() {
        use super::super::asm::op;
        const SRC: &str = "facet C { uint256[] xs; \
             function set(uint256 i, uint256 v) external { xs[i] = v; } }";
        let rt = &super::super::compile(SRC).unwrap().runtime;
        let slot = super::slot_at(super::storage_base("C"), 0);

        // value v (param 1 → CALLDATALOAD(0x24)) pushed first, then the slot
        // derivation from i (param 0 → CALLDATALOAD(0x04)), then SSTORE.
        let mut want = vec![op::PUSH1, 0x24, op::CALLDATALOAD, op::PUSH1 + 31];
        want.extend_from_slice(&slot);
        want.extend_from_slice(&[
            op::PUSH1, 0x00, op::MSTORE, op::PUSH1, 0x20, op::PUSH1, 0x00, op::KECCAK256,
            op::PUSH1, 0x04, op::CALLDATALOAD, op::ADD, op::SSTORE,
        ]);
        assert!(
            rt.windows(want.len()).any(|w| w == want.as_slice()),
            "xs[i] = v must push v, derive keccak256(slot)+i, SSTORE; runtime = {}",
            to_hex(rt)
        );
    }

    /// `xs.push(v)` (1) stores `v` at `keccak256(slot) + length` and (2) bumps the
    /// length slot to `length + 1`. The element index is the CURRENT length (an
    /// `SLOAD` of the base slot), and the length is re-`SLOAD`ed for the bump (no
    /// `DUP2` in the v1 assembler).
    #[cfg(feature = "wallet")]
    #[test]
    fn array_push_stores_element_then_bumps_length() {
        use super::super::asm::op;
        const SRC: &str = "facet C { uint256[] xs; \
             function add(uint256 v) external { xs.push(v); } }";
        let rt = &super::super::compile(SRC).unwrap().runtime;
        let slot = super::slot_at(super::storage_base("C"), 0);

        // 1. element write: v (param 0 → CALLDATALOAD(0x04)), then
        //    keccak256(slot) + length (length = PUSH32 <slot> SLOAD), SSTORE.
        let mut elem = vec![op::PUSH1, 0x04, op::CALLDATALOAD, op::PUSH1 + 31];
        elem.extend_from_slice(&slot);
        elem.extend_from_slice(&[op::PUSH1, 0x00, op::MSTORE, op::PUSH1, 0x20, op::PUSH1, 0x00, op::KECCAK256]);
        // index = length = PUSH32 <slot> SLOAD ; ADD ; SSTORE
        elem.push(op::PUSH1 + 31);
        elem.extend_from_slice(&slot);
        elem.extend_from_slice(&[op::SLOAD, op::ADD, op::SSTORE]);
        assert!(
            rt.windows(elem.len()).any(|w| w == elem.as_slice()),
            "push must store v at keccak256(slot)+length; runtime = {}",
            to_hex(rt)
        );

        // 2. length bump: PUSH32 <slot> SLOAD PUSH1 0x01 ADD PUSH32 <slot> SSTORE.
        let mut bump = vec![op::PUSH1 + 31];
        bump.extend_from_slice(&slot);
        bump.extend_from_slice(&[op::SLOAD, op::PUSH1, 0x01, op::ADD, op::PUSH1 + 31]);
        bump.extend_from_slice(&slot);
        bump.push(op::SSTORE);
        assert!(
            rt.windows(bump.len()).any(|w| w == bump.as_slice()),
            "push must bump the length slot to length + 1; runtime = {}",
            to_hex(rt)
        );
    }

    /// `xs.pop()` (#37): (1) zeroes the element at `keccak256(slot) + (len - 1)` and
    /// (2) stores the decremented length `len - 1` back at the base slot. The shapes:
    /// a `PUSH1 0x00` value, the keccak element-slot derivation with index `len - 1`,
    /// `SSTORE`, then `PUSH32 <slot> SLOAD` (len) `... SUB` and `SSTORE` of the new len.
    #[cfg(feature = "wallet")]
    #[test]
    fn array_pop_zeroes_last_element_and_decrements_length() {
        use super::super::asm::op;
        const SRC: &str = "facet C { uint256[] xs; \
             function pop() external { xs.pop(); } }";
        let rt = &super::super::compile(SRC).unwrap().runtime;
        let slot = super::slot_at(super::storage_base("C"), 0);

        // The element clear: value 0 first, then keccak256(slot) derivation.
        //   PUSH1 0x00                       ; value 0 (stays below)
        //   PUSH32 <slot> PUSH1 0x00 MSTORE  ; mem[0..0x20] = slot
        //   PUSH1 0x20 PUSH1 0x00 KECCAK256  ; base = keccak256(slot)
        //   <len - 1> ADD SSTORE             ; clear elem at base + (len - 1)
        let mut clear = vec![op::PUSH1, 0x00, op::PUSH1 + 31];
        clear.extend_from_slice(&slot);
        clear.extend_from_slice(&[op::PUSH1, 0x00, op::MSTORE, op::PUSH1, 0x20, op::PUSH1, 0x00, op::KECCAK256]);
        assert!(
            rt.windows(clear.len()).any(|w| w == clear.as_slice()),
            "pop must clear the element with value 0 at keccak256(slot)+(len-1); runtime = {}",
            to_hex(rt)
        );
        // `len - 1` lowers to `PUSH32 <slot> SLOAD` (len) with `1` pushed on top, SUB.
        // (SUB pushes rhs deeper then lhs on top: rhs=1, lhs=len → SUB = len - 1.)
        let mut len_minus_one = vec![op::PUSH1, 0x01, op::PUSH1 + 31];
        len_minus_one.extend_from_slice(&slot);
        len_minus_one.extend_from_slice(&[op::SLOAD, op::SUB]);
        assert!(
            rt.windows(len_minus_one.len()).any(|w| w == len_minus_one.as_slice()),
            "pop must compute len - 1 (PUSH1 1, PUSH32 slot SLOAD, SUB); runtime = {}",
            to_hex(rt)
        );
        // The new length is SSTORE'd back to the base slot (PUSH32 <slot> SSTORE).
        let mut store_len = vec![op::PUSH1 + 31];
        store_len.extend_from_slice(&slot);
        store_len.push(op::SSTORE);
        assert!(
            rt.windows(store_len.len()).any(|w| w == store_len.as_slice()),
            "pop must SSTORE the decremented length to the base slot"
        );
    }

    /// `delete xs[i]` (#37) is exactly an element write of `0` at `keccak256(slot)+i`,
    /// leaving the length slot untouched: `PUSH1 0x00` (value), keccak slot derivation
    /// from `i`, `ADD`, `SSTORE` — and NO write to the base/length slot.
    #[cfg(feature = "wallet")]
    #[test]
    fn delete_index_zeroes_element_and_leaves_length() {
        use super::super::asm::op;
        const SRC: &str = "facet C { uint256[] xs; \
             function clear(uint256 i) external { delete xs[i]; } }";
        let rt = &super::super::compile(SRC).unwrap().runtime;
        let slot = super::slot_at(super::storage_base("C"), 0);

        // value 0, then keccak256(slot) + i (i = param 0 → CALLDATALOAD(0x04)), SSTORE.
        let mut want = vec![op::PUSH1, 0x00, op::PUSH1 + 31];
        want.extend_from_slice(&slot);
        want.extend_from_slice(&[
            op::PUSH1, 0x00, op::MSTORE, op::PUSH1, 0x20, op::PUSH1, 0x00, op::KECCAK256,
            op::PUSH1, 0x04, op::CALLDATALOAD, op::ADD, op::SSTORE,
        ]);
        assert!(
            rt.windows(want.len()).any(|w| w == want.as_slice()),
            "delete xs[i] must push 0, derive keccak256(slot)+i, SSTORE; runtime = {}",
            to_hex(rt)
        );
        // Exactly ONE SSTORE (the element clear) — delete never touches the length.
        let sstores = count_op(rt, op::SSTORE);
        assert_eq!(sstores, 1, "delete xs[i] performs a single SSTORE (no length write)");
    }

    /// `delete` / `.pop()` on a NON-array are clean `TYPE_MISMATCH` errors.
    #[cfg(feature = "wallet")]
    #[test]
    fn pop_and_delete_on_non_arrays_are_clean_errors() {
        // `.pop()` on a scalar.
        let err = super::super::compile(
            "facet C { uint256 n; function f() external { n.pop(); } }",
        )
        .expect_err("n.pop() on a scalar must fail cleanly");
        assert_eq!(err.code, Some(crate::error_codes::TYPE_MISMATCH));
        // `delete` on a mapping index (delete is array-element-only in v1).
        let err = super::super::compile(
            "facet C { mapping(address => uint256) m; function f() external { delete m[msg.sender]; } }",
        )
        .expect_err("delete on a mapping must fail cleanly");
        assert_eq!(err.code, Some(crate::error_codes::TYPE_MISMATCH));
    }

    /// The off-chain element-slot helper equals an independent `keccak256(slot) + i`
    /// — the value the on-chain MSTORE/KECCAK256/ADD reproduces (the load-bearing
    /// layout invariant for cross-checking reads against deployed state).
    #[cfg(feature = "wallet")]
    #[test]
    fn array_elem_slot_matches_independent_keccak() {
        use sha3::{Digest, Keccak256};
        let slot = super::slot_at(super::storage_base("C"), 0);
        let base: [u8; 32] = Keccak256::digest(slot).into();
        // element 0 is keccak256(slot) itself; element 3 is +3.
        assert_eq!(array_elem_slot(&slot, 0), base);
        assert_eq!(array_elem_slot(&slot, 3), super::slot_at(base, 3));
    }

    /// THE ARRAY TARGET: a `uint256[]` Stack facet (push / pop-via-length / indexed
    /// read / length) compiles end-to-end with the canonical selectors, and the
    /// array slot is laid out AFTER any preceding scalar (declaration-index slots).
    #[cfg(feature = "wallet")]
    #[test]
    fn array_target_facet_compiles_with_canonical_layout() {
        use super::super::asm::op;
        // `total` is slot 0, `xs` is slot 1 — the array length lives at slot 1, its
        // elements at keccak256(slot 1) + i. Proves arrays index AFTER scalars.
        const SRC: &str = "facet Stack { uint256 total; uint256[] xs; \
             function push(uint256 v) external { xs.push(v); total = total + 1; } \
             function set(uint256 i, uint256 v) external { xs[i] = v; } \
             function get(uint256 i) external view returns (uint256) { return xs[i]; } \
             function size() external view returns (uint256) { return xs.length; } }";
        let art = super::super::compile(SRC).expect("the array Stack TARGET must compile");
        let rt = &art.runtime;

        // xs is the SECOND state var → base slot = BASE + 1.
        let xs_slot = super::slot_at(super::storage_base("Stack"), 1);

        // size() returns the base slot directly (the length).
        let mut len_read = vec![op::PUSH1 + 31];
        len_read.extend_from_slice(&xs_slot);
        len_read.push(op::SLOAD);
        assert!(rt.windows(len_read.len()).any(|w| w == len_read.as_slice()), "size() reads slot 1");

        // All four selectors dispatch.
        for sig in ["push(uint256)", "set(uint256,uint256)", "get(uint256)", "size()"] {
            let sel = crate::registry::selector(sig);
            let push4: Vec<u8> = std::iter::once(op::PUSH1 + 3).chain(sel).collect();
            assert!(rt.windows(5).any(|w| w == push4.as_slice()), "{sig} dispatched");
        }
        assert_eq!(art.init_code, super::super::asm::init_wrapper(rt));
    }

    /// `<scalar>.length` / `<mapping>.length` / `.push` on a non-array are clean
    /// `TYPE_MISMATCH` errors, never a panic or a silent miscompile.
    #[cfg(feature = "wallet")]
    #[test]
    fn array_ops_on_non_arrays_are_clean_errors() {
        // `.length` on a scalar.
        let err = super::super::compile(
            "facet C { uint256 n; function f() external view returns (uint256) { return n.length; } }",
        )
        .expect_err("n.length on a scalar must fail cleanly");
        assert_eq!(err.code, Some(crate::error_codes::TYPE_MISMATCH));
        // `.push` on a scalar.
        let err = super::super::compile(
            "facet C { uint256 n; function f() external { n.push(1); } }",
        )
        .expect_err("n.push on a scalar must fail cleanly");
        assert_eq!(err.code, Some(crate::error_codes::TYPE_MISMATCH));
        // A bare array reference (not indexed / no `.length`) is a clean error.
        let err = super::super::compile(
            "facet C { uint256[] xs; function f() external view returns (uint256) { return xs; } }",
        )
        .expect_err("a bare array reference must fail cleanly");
        assert_eq!(err.code, Some(crate::error_codes::TYPE_MISMATCH));
    }

    // ── #37 DYNAMIC string/bytes (codegen-shape; behavior proven in interp) ───

    /// A SHORT `string` literal store emits exactly `PUSH32 <packedWord> PUSH32 <slot>
    /// SSTORE`, where the packed word holds the data left-aligned and `len*2` in its
    /// lowest byte (canonical short layout). ONE SSTORE (no spill).
    #[cfg(feature = "wallet")]
    #[test]
    fn const_short_string_store_emits_one_packed_sstore() {
        use super::super::asm::op;
        const SRC: &str = "facet Note { string s; function set() external { s = \"hi\"; } }";
        let rt = &super::super::compile(SRC).unwrap().runtime;
        let slot = super::slot_at(super::storage_base("Note"), 0);
        // Packed short word: "hi" left-aligned, low byte = 2*2 = 4.
        let mut packed = [0u8; 32];
        packed[..2].copy_from_slice(b"hi");
        packed[31] = 4;
        let mut want = vec![op::PUSH1 + 31];
        want.extend_from_slice(&packed);
        want.push(op::PUSH1 + 31);
        want.extend_from_slice(&slot);
        want.push(op::SSTORE);
        assert!(
            rt.windows(want.len()).any(|w| w == want.as_slice()),
            "short store must be PUSH32 packed / PUSH32 slot / SSTORE; runtime = {}",
            to_hex(rt)
        );
        // Exactly ONE SSTORE in this single-statement function (no length spill).
        assert_eq!(count_op(rt, op::SSTORE), 1, "short store is a single SSTORE");
    }

    /// A LONG `string` literal store emits the header `SSTORE(slot, len*2+1)` plus one
    /// `SSTORE` per 32-byte data chunk at the precomputed `keccak256(slot) + i` slots
    /// (no runtime KECCAK256 — the bytes/slots are compile-time constants).
    #[cfg(feature = "wallet")]
    #[test]
    fn const_long_string_store_unrolls_header_plus_data_sstores() {
        use super::super::asm::op;
        // 40 bytes → header + 2 data chunks (32 + 8) = 3 SSTOREs, no KECCAK256.
        const S: &str = "this string is forty bytes long, yes sir";
        let src = format!("facet Note {{ string s; function set() external {{ s = \"{S}\"; }} }}");
        let rt = &super::super::compile(&src).unwrap().runtime;
        let slot = super::slot_at(super::storage_base("Note"), 0);

        // Header word = len*2 + 1 = 81, SSTORE'd to the base slot.
        let mut header = [0u8; 32];
        header[31] = 81;
        let mut want_header = vec![op::PUSH1 + 31];
        want_header.extend_from_slice(&header);
        want_header.push(op::PUSH1 + 31);
        want_header.extend_from_slice(&slot);
        want_header.push(op::SSTORE);
        assert!(
            rt.windows(want_header.len()).any(|w| w == want_header.as_slice()),
            "long store must SSTORE the len*2+1 header to the base slot; runtime = {}",
            to_hex(rt)
        );
        // Three SSTOREs total (header + two data chunks); NO on-chain KECCAK256.
        assert_eq!(count_op(rt, op::SSTORE), 3, "header + 2 data chunks = 3 SSTOREs");
        assert_eq!(count_op(rt, op::KECCAK256), 0, "data slots are precomputed (no runtime KECCAK256)");

        // The data slots are keccak256(slot) + i, computed off-chain to cross-check.
        use sha3::{Digest, Keccak256};
        let data0: [u8; 32] = Keccak256::digest(slot).into();
        for slot_i in [data0, super::slot_at(data0, 1)] {
            let mut push_slot = vec![op::PUSH1 + 31];
            push_slot.extend_from_slice(&slot_i);
            assert!(
                rt.windows(33).any(|w| w == push_slot.as_slice()),
                "each data chunk SSTOREs to its precomputed keccak256(slot)+i slot"
            );
        }
    }

    /// The dynamic STORAGE-READ getter branches on the low bit (`AND 1`) and, for the
    /// LONG case, runs a copy loop using the new `DUP2`/`DUP3`/`SWAP1`/`AND` opcodes
    /// and a runtime `KECCAK256` for the data-slot base.
    #[cfg(feature = "wallet")]
    #[test]
    fn dynamic_storage_getter_emits_branch_and_copy_loop_opcodes() {
        use super::super::asm::op;
        const SRC: &str = "facet Note { string s; \
             function get() external view returns (string) { return s; } }";
        let rt = &super::super::compile(SRC).unwrap().runtime;
        // The short/long discriminator masks the low bit: a `PUSH1 0x01 AND`.
        assert!(
            rt.windows(3).any(|w| w == [op::PUSH1, 0x01, op::AND]),
            "the getter must test the slot's low bit via AND 1; runtime = {}",
            to_hex(rt)
        );
        // The LONG copy loop uses the new stack opcodes + a runtime KECCAK256 (data
        // base) + a per-iteration SLOAD/MSTORE.
        for o in [op::DUP2, op::DUP3, op::SWAP1, op::AND, op::KECCAK256] {
            assert!(rt.contains(&o), "the getter must emit {o:#x}");
        }
    }

    /// The dynamic PARAM echo uses `CALLDATACOPY` (the bulk tail copy) and prepends
    /// the `0x20` ABI offset — and emits NO copy loop (it is loop-free).
    #[cfg(feature = "wallet")]
    #[test]
    fn dynamic_param_echo_emits_calldatacopy() {
        use super::super::asm::op;
        const SRC: &str =
            "facet E { function echo(string s) external pure returns (string) { return s; } }";
        let rt = &super::super::compile(SRC).unwrap().runtime;
        assert!(rt.contains(&op::CALLDATACOPY), "the echo must bulk-copy via CALLDATACOPY");
        // The ABI offset word 0x20 is MSTORE'd at mem[0x00]: `PUSH1 0x20 PUSH1 0x00 MSTORE`.
        assert!(
            rt.windows(5).any(|w| w == [op::PUSH1, 0x20, op::PUSH1, 0x00, op::MSTORE]),
            "the echo must write the 0x20 ABI offset at mem[0]; runtime = {}",
            to_hex(rt)
        );
    }

    /// `bytes` (not `string`) flows through the same dynamic lowering; its SELECTOR
    /// uses the `bytes` ABI type name (proving `abi_name` distinguishes them).
    #[cfg(feature = "wallet")]
    #[test]
    fn bytes_param_uses_the_bytes_abi_selector() {
        use super::super::asm::op;
        const SRC: &str =
            "facet E { function echo(bytes b) external pure returns (bytes) { return b; } }";
        let rt = &super::super::compile(SRC).unwrap().runtime;
        let sel = crate::registry::selector("echo(bytes)");
        let push4: Vec<u8> = std::iter::once(op::PUSH1 + 3).chain(sel).collect();
        assert!(rt.windows(5).any(|w| w == push4.as_slice()), "echo(bytes) selector dispatched");
        assert!(rt.contains(&op::CALLDATACOPY), "bytes echo uses the same CALLDATACOPY path");
    }

    /// A dynamic `string`/`bytes` used where a single word is required is a clean
    /// `TYPE_MISMATCH`, never a silent single-word miscompile.
    #[cfg(feature = "wallet")]
    #[test]
    fn dynamic_value_in_word_context_is_a_clean_error() {
        // a string state var read as a scalar.
        let err = super::super::compile(
            "facet C { string s; function f() external view returns (uint256) { return s; } }",
        )
        .expect_err("a string state var is not a single word");
        assert_eq!(err.code, Some(crate::error_codes::TYPE_MISMATCH));
        // a dynamic param in arithmetic.
        let err = super::super::compile(
            "facet C { function f(bytes b) external pure returns (uint256) { return b + 1; } }",
        )
        .expect_err("a bytes param is not a single word");
        assert_eq!(err.code, Some(crate::error_codes::TYPE_MISMATCH));
        // a non-literal assigned to a dynamic state var (only a literal is supported).
        let err = super::super::compile(
            "facet C { string s; uint256 n; function f() external { s = n; } }",
        )
        .expect_err("a dynamic state var only accepts a string literal in v1");
        assert_eq!(err.code, Some(crate::error_codes::UNSUPPORTED_FEATURE));
    }

    // ── COMPARISONS + require/revert (Installment 1 CounterFacet) ─────────────

    /// `n > 0` lowers to `…GT` and `n <= 100` lowers to `…GT…ISZERO` — the
    /// comparison opcode shapes the task pins.
    #[cfg(feature = "wallet")]
    #[test]
    fn comparison_lowers_to_gt_and_gt_iszero() {
        use super::super::asm::op;
        // `n > 0` → GT present; `n <= 100` → GT then ISZERO present.
        let art = super::super::compile(
            "facet C { function f(uint256 n) external { require(n > 0, \"a\"); require(n <= 100, \"b\"); } }",
        )
        .unwrap();
        let rt = &art.runtime;
        assert!(rt.contains(&op::GT), "a `>` (and the `<=` inversion) must emit GT");
        // The `<=` is `ISZERO(GT(..))`: a GT immediately followed by ISZERO.
        assert!(
            rt.windows(2).any(|w| w == [op::GT, op::ISZERO]),
            "`n <= 100` must emit GT then ISZERO; runtime = {}",
            to_hex(rt)
        );
    }

    /// Each comparison operator emits its mandated opcode sequence.
    #[cfg(feature = "wallet")]
    #[test]
    fn each_comparison_emits_its_opcodes() {
        use super::super::asm::op;
        let cases: &[(&str, &[u8])] = &[
            (">", &[op::GT]),
            ("<", &[op::LT]),
            ("==", &[op::EQ]),
            ("<=", &[op::GT, op::ISZERO]), // a <= b ⇔ !(a > b)
            (">=", &[op::LT, op::ISZERO]), // a >= b ⇔ !(a < b)
        ];
        for (src_op, want) in cases {
            let src = format!(
                "facet C {{ function f(uint256 n) external {{ require(n {src_op} 1, \"x\"); }} }}"
            );
            let rt = super::super::compile(&src).unwrap().runtime;
            assert!(
                rt.windows(want.len()).any(|w| w == *want),
                "`{src_op}` must emit {want:02x?}; runtime = {}",
                to_hex(&rt)
            );
        }
    }

    /// A `require` emits `<cond> ISZERO PUSH2 <revert> JUMPI`, and the referenced
    /// label lands on a `JUMPDEST` that begins a `REVERT(0,0)` stub.
    #[cfg(feature = "wallet")]
    #[test]
    fn require_emits_iszero_jumpi_to_a_revert_stub() {
        use super::super::asm::op;
        let art = super::super::compile(
            "facet C { function f(uint256 n) external { require(n > 0, \"zero\"); } }",
        )
        .unwrap();
        let rt = &art.runtime;

        // Find `ISZERO PUSH2 <hi> <lo> JUMPI` — the require branch.
        let pos = rt
            .windows(5)
            .position(|w| w[0] == op::ISZERO && w[1] == op::PUSH2 && w[4] == op::JUMPI)
            .expect("require must emit ISZERO PUSH2 <revert> JUMPI");
        let target = u16::from_be_bytes([rt[pos + 2], rt[pos + 3]]) as usize;
        // The target is a JUMPDEST beginning a REVERT(0,0) stub.
        assert_eq!(rt[target], op::JUMPDEST, "the require target must be a JUMPDEST");
        assert_eq!(
            &rt[target..target + 6],
            &[op::JUMPDEST, op::PUSH1, 0x00, op::PUSH1, 0x00, op::REVERT],
            "the revert stub must be JUMPDEST PUSH1 0 PUSH1 0 REVERT"
        );
    }

    /// Two `require`s in one function SHARE a single revert stub (only one extra
    /// JUMPDEST+REVERT(0,0) is emitted beyond the dispatcher fallback).
    #[cfg(feature = "wallet")]
    #[test]
    fn multiple_requires_share_one_revert_stub() {
        use super::super::asm::op;
        let art = super::super::compile(
            "facet C { function f(uint256 n) external { require(n > 0, \"a\"); require(n <= 100, \"b\"); } }",
        )
        .unwrap();
        let rt = &art.runtime;
        // Two require branches (two `ISZERO … JUMPI`).
        let jumpis = rt
            .windows(5)
            .filter(|w| w[0] == op::ISZERO && w[1] == op::PUSH2 && w[4] == op::JUMPI)
            .count();
        assert_eq!(jumpis, 2, "two requires → two ISZERO/JUMPI branches");
        // Both branches resolve to the SAME revert target.
        let targets: Vec<usize> = rt
            .windows(5)
            .filter(|w| w[0] == op::ISZERO && w[1] == op::PUSH2 && w[4] == op::JUMPI)
            .map(|w| u16::from_be_bytes([w[2], w[3]]) as usize)
            .collect();
        assert_eq!(targets[0], targets[1], "both requires share ONE revert stub");
        // Exactly TWO REVERT(0,0) stubs total in the runtime: the dispatcher
        // fallback + the one shared require stub (no per-require duplication).
        let revert_stubs = rt
            .windows(6)
            .filter(|w| w == &[op::JUMPDEST, op::PUSH1, 0x00, op::PUSH1, 0x00, op::REVERT])
            .count();
        assert_eq!(revert_stubs, 2, "only the fallback + one shared require stub");
    }

    /// A require-FREE mutating body emits NO revert stub (no JUMPDEST/REVERT beyond
    /// the dispatcher fallback) — requires don't bloat functions that lack them.
    #[cfg(feature = "wallet")]
    #[test]
    fn require_free_body_has_no_extra_revert_stub() {
        use super::super::asm::op;
        let art = super::super::compile(
            "facet C { uint256 n; function bump() external { n = n + 1; } }",
        )
        .unwrap();
        let rt = &art.runtime;
        let revert_stubs = rt
            .windows(6)
            .filter(|w| w == &[op::JUMPDEST, op::PUSH1, 0x00, op::PUSH1, 0x00, op::REVERT])
            .count();
        assert_eq!(revert_stubs, 1, "only the dispatcher fallback REVERT(0,0)");
    }

    /// A `require(true, …)` style guard with a constant condition still compiles to
    /// a well-formed branch (codegen-shape check — a truthy constant never reverts
    /// at runtime because ISZERO(1) = 0, so the JUMPI is not taken).
    #[cfg(feature = "wallet")]
    #[test]
    fn require_with_true_constant_compiles_and_does_not_take_the_branch() {
        use super::super::asm::op;
        // `1 == 1` is always true; the require branch exists but ISZERO(EQ(1,1))=0.
        let art = super::super::compile(
            "facet C { function f() external { require(1 == 1, \"never\"); } }",
        )
        .unwrap();
        let rt = &art.runtime;
        // The shape is present: an EQ feeding an ISZERO feeding a JUMPI.
        assert!(rt.contains(&op::EQ), "1 == 1 emits EQ");
        assert!(
            rt.windows(5).any(|w| w[0] == op::ISZERO && w[1] == op::PUSH2 && w[4] == op::JUMPI),
            "the require branch is well-formed"
        );
        assert_eq!(art.init_code, super::super::asm::init_wrapper(rt));
    }

    /// A malformed `require` (bad comparison operand) surfaces a clean `CompileError`
    /// (from the parser), never a panic.
    #[cfg(feature = "wallet")]
    #[test]
    fn malformed_require_is_a_clean_compile_error() {
        // `n > ` has no right operand.
        let err = super::super::compile(
            "facet C { function f(uint256 n) external { require(n > , \"x\"); } }",
        )
        .expect_err("a malformed comparison must fail cleanly");
        assert!(err.code.is_some(), "carries an LH code");
        assert!(err.to_string().starts_with("LH0"));
        // An unknown variable inside a require condition is also a clean error.
        let err = super::super::compile(
            "facet C { function f() external { require(ghost > 0, \"x\"); } }",
        )
        .expect_err("an unknown var in a require must fail cleanly");
        assert_eq!(err.code, Some(crate::error_codes::UNDEFINED_VARIABLE));
    }

    /// THE INSTALLMENT-1 TARGET: the full `CounterFacet` (minus the event) compiles
    /// and all four selectors are dispatched with the canonical hashes.
    #[cfg(feature = "wallet")]
    #[test]
    fn counter_target_facet_compiles_with_canonical_selectors() {
        use super::super::asm::op;
        const SRC: &str = "facet Counter { mapping(address => uint256) count; uint256 total; \
             function increment() external { count[msg.sender] = count[msg.sender] + 1; total = total + 1; } \
             function incrementBy(uint256 n) external { require(n > 0, \"zero\"); require(n <= 100, \"too big\"); \
             count[msg.sender] = count[msg.sender] + n; total = total + n; } \
             function countOf(address who) external view returns (uint256) { return count[who]; } \
             function totalCount() external view returns (uint256) { return total; } }";
        let art = super::super::compile(SRC).expect("the CounterFacet TARGET must compile");
        let rt = &art.runtime;

        // The four canonical selectors (pinned in the task).
        let sels: [(&str, [u8; 4]); 4] = [
            ("increment()", [0xd0, 0x9d, 0xe0, 0x8a]),
            ("incrementBy(uint256)", [0x03, 0xdf, 0x17, 0x9c]),
            ("countOf(address)", [0xf8, 0x97, 0x7e, 0x96]),
            ("totalCount()", [0x34, 0xea, 0xfb, 0x11]),
        ];
        for (sig, want) in sels {
            assert_eq!(crate::registry::selector(sig), want, "selector pin for {sig}");
            let push4: Vec<u8> = std::iter::once(op::PUSH1 + 3).chain(want).collect();
            assert!(
                rt.windows(5).any(|w| w == push4.as_slice()),
                "{sig} selector {want:02x?} must be dispatched"
            );
        }
        // The relational + require primitives are exercised.
        assert!(rt.contains(&op::GT), "incrementBy uses `>` / `<=` → GT");
        assert!(rt.windows(2).any(|w| w == [op::GT, op::ISZERO]), "`<=` → GT ISZERO");
        assert!(
            rt.windows(5).any(|w| w[0] == op::ISZERO && w[1] == op::PUSH2 && w[4] == op::JUMPI),
            "require → ISZERO/JUMPI"
        );
        // init_code wraps the runtime.
        assert_eq!(art.init_code, super::super::asm::init_wrapper(rt));
    }

    // ── EVENTS + emit → LOGn (Installment 1 capstone) ────────────────────────

    /// `topic0` for `Incremented(address,uint256,uint256)` is the FULL 32-byte
    /// keccak256 of the signature, NOT the 4-byte selector — cross-checked against
    /// an independent keccak of the same string.
    #[cfg(feature = "wallet")]
    #[test]
    fn event_topic0_is_full_keccak_of_the_signature() {
        use sha3::{Digest, Keccak256};
        const SIG: &str = "Incremented(address,uint256,uint256)";
        let topic0 = super::event_topic0(SIG);
        // Independent keccak of the same string.
        let mut want = [0u8; 32];
        want.copy_from_slice(&Keccak256::digest(SIG.as_bytes()));
        assert_eq!(topic0, want, "topic0 must be the full keccak of the event sig");
        // It must NOT be the 4-byte selector zero-extended — the full word is used.
        let sel = crate::registry::selector(SIG);
        assert_eq!(&topic0[..4], &sel, "the first 4 bytes coincide with the selector");
        assert!(topic0[4..].iter().any(|&b| b != 0), "topic0 is more than 4 bytes");
        // Pin the exact hash (printed for the report).
        assert_eq!(to_hex(&topic0), TOPIC0_INCREMENTED, "Incremented topic0 drifted");
    }

    /// The pinned topic0 for `Incremented(address,uint256,uint256)` — the FULL
    /// 32-byte keccak256 of the canonical event signature.
    #[cfg(feature = "wallet")]
    const TOPIC0_INCREMENTED: &str =
        "0xcd5ad702c30bb253c9e421ea7f3e00faee62ce859708bfdaf949788e5ba0fdb5";

    /// `event_signature` builds the canonical ABI form, IGNORING `indexed`/names.
    #[cfg(feature = "wallet")]
    #[test]
    fn event_signature_uses_types_only() {
        use super::super::ast::*;
        use crate::rustlite::Span;
        let sp = Span { start: 0, end: 0 };
        let ev = EventDecl {
            name: "Incremented".into(),
            args: vec![
                EventArg { ty: Ty::Address, indexed: true, name: "who".into(), span: sp },
                EventArg { ty: Ty::Uint256, indexed: false, name: "newCount".into(), span: sp },
                EventArg { ty: Ty::Uint256, indexed: false, name: "newTotal".into(), span: sp },
            ],
            span: sp,
        };
        assert_eq!(super::event_signature(&ev), "Incremented(address,uint256,uint256)");
    }

    /// `emit Incremented(msg.sender, count[msg.sender], total)` lowers to a `LOG2`
    /// (topic0 + the one indexed `who`), with `topic0` PUSH32'd and the two
    /// non-indexed words MSTORE'd into the data region before the LOG.
    #[cfg(feature = "wallet")]
    #[test]
    fn emit_lowers_to_log2_with_topic0_push32_and_data_mstores() {
        use super::super::asm::op;
        const SRC: &str = "facet C { mapping(address => uint256) count; uint256 total; \
             event Incremented(address indexed who, uint256 newCount, uint256 newTotal); \
             function increment() external { count[msg.sender] = count[msg.sender] + 1; \
             total = total + 1; emit Incremented(msg.sender, count[msg.sender], total); } }";
        let art = super::super::compile(SRC).expect("emit facet must compile");
        let rt = &art.runtime;

        // A LOG2 (1 indexed + topic0) is emitted exactly once; no other LOGn opcode
        // (decoding push widths so data bytes equal to a LOG opcode don't false-hit).
        assert_eq!(count_op(rt, op::LOG2), 1, "exactly one LOG2");
        for other in [op::LOG0, op::LOG1, op::LOG3, op::LOG4] {
            assert_eq!(count_op(rt, other), 0, "no other LOGn opcode, found {other:#x}");
        }

        // topic0 is PUSH32'd as the full event-sig hash.
        let topic0 = super::event_topic0("Incremented(address,uint256,uint256)");
        let mut push32_topic0 = vec![op::PUSH1 + 31];
        push32_topic0.extend_from_slice(&topic0);
        assert!(
            rt.windows(33).any(|w| w == push32_topic0.as_slice()),
            "topic0 must be PUSH32'd; runtime = {}",
            to_hex(rt)
        );

        // The data region is staged with MSTOREs before the LOG.
        assert!(count_op(rt, op::MSTORE) >= 2, "the two data words are MSTORE'd into memory");

        // The LOG2's immediate stack setup ends with PUSH length then PUSH offset
        // then LOG2. Locate the REAL LOG2 opcode and check the two preceding pushes.
        let log_pos = real_opcodes(rt)
            .iter()
            .find(|(_, o)| *o == op::LOG2)
            .map(|(off, _)| *off)
            .unwrap();
        // offset (on top) = PUSH1 0x40.
        assert_eq!(
            &rt[log_pos - 2..log_pos],
            &[op::PUSH1, 0x40],
            "LOG2 is preceded by PUSH1 0x40 (the data offset on top of the stack)"
        );
        // length (below offset) = 0x40 = 0x20 * 2 data words.
        assert_eq!(
            &rt[log_pos - 4..log_pos - 2],
            &[op::PUSH1, 0x40],
            "the length (0x40 = two 32-byte data words) is pushed before the offset"
        );

        assert_eq!(art.init_code, super::super::asm::init_wrapper(rt));
    }

    /// THE LOG STACK ORDER (the load-bearing invariant): the bytes immediately
    /// before a 1-indexed `LOG2` are, in order, `CALLER` (topic1, deepest), `PUSH32
    /// topic0` (shallowest topic), `PUSH length`, `PUSH offset` (on top), `LOG2`.
    /// A swapped topic/data or topic1/topic0 would be a silent wrong-log bug.
    #[cfg(feature = "wallet")]
    #[test]
    fn emit_stack_order_is_topic1_topic0_len_offset_logn() {
        use super::super::asm::op;
        // ONE indexed (who → topic1 = CALLER), ONE data word (n → mem). LOG2.
        const SRC: &str = "facet C { event E(address indexed who, uint256 amt); \
             function f(uint256 n) external { emit E(msg.sender, n); } }";
        let rt = &super::super::compile(SRC).unwrap().runtime;
        let topic0 = super::event_topic0("E(address,uint256)");

        // The exact emit tail: CALLER, PUSH32 topic0, PUSH1 0x20 (len=1 data word),
        // PUSH1 0x40 (offset), LOG2.
        let mut tail = vec![op::CALLER, op::PUSH1 + 31];
        tail.extend_from_slice(&topic0);
        tail.extend_from_slice(&[op::PUSH1, 0x20, op::PUSH1, 0x40, op::LOG2]);
        assert!(
            rt.windows(tail.len()).any(|w| w == tail.as_slice()),
            "emit tail must be CALLER, PUSH32 topic0, PUSH len, PUSH offset, LOG2 (in that \
             order so the EVM pops offset, length, topic0, topic1); runtime = {}",
            to_hex(rt)
        );
        // The single data word `n` (CALLDATALOAD(0x04)) is MSTORE'd at the data base
        // (mem[0x40]) BEFORE the topic pushes — i.e. a `PUSH1 0x40 MSTORE` exists.
        assert!(
            rt.windows(3).any(|w| w == [op::PUSH1, 0x40, op::MSTORE]),
            "the data word is MSTORE'd at the data base mem[0x40]"
        );
    }

    /// An indexed arg becomes a LOG TOPIC, not a data word: an event with ONE
    /// indexed and ZERO data args lowers to a LOG2 (topic0 + the indexed) with a
    /// zero-length data region.
    #[cfg(feature = "wallet")]
    #[test]
    fn indexed_only_event_has_zero_length_data() {
        use super::super::asm::op;
        const SRC: &str = "facet C { event Hit(address indexed who); \
             function f() external { emit Hit(msg.sender); } }";
        let rt = &super::super::compile(SRC).unwrap().runtime;
        // LOG2: topic0 + who.
        assert_eq!(count_op(rt, op::LOG2), 1, "one LOG2 (topic0 + who)");
        let log_pos = real_opcodes(rt)
            .iter()
            .find(|(_, o)| *o == op::LOG2)
            .map(|(off, _)| *off)
            .expect("a LOG2");
        // length = 0 (no data args): PUSH1 0x00 then PUSH1 0x40 (offset) then LOG2.
        assert_eq!(&rt[log_pos - 2..log_pos], &[op::PUSH1, 0x40], "offset on top");
        assert_eq!(&rt[log_pos - 4..log_pos - 2], &[op::PUSH1, 0x00], "zero length");
        // CALLER (the indexed who) is pushed as a topic.
        assert!(count_op(rt, op::CALLER) >= 1, "the indexed who emits CALLER");
    }

    /// A no-arg event lowers to a LOG1 (just topic0) with a zero-length data region.
    #[cfg(feature = "wallet")]
    #[test]
    fn no_arg_event_lowers_to_log1() {
        use super::super::asm::op;
        const SRC: &str = "facet C { event Ping(); function f() external { emit Ping(); } }";
        let rt = &super::super::compile(SRC).unwrap().runtime;
        assert_eq!(count_op(rt, op::LOG1), 1, "one LOG1");
        let topic0 = super::event_topic0("Ping()");
        let mut push32 = vec![op::PUSH1 + 31];
        push32.extend_from_slice(&topic0);
        assert!(rt.windows(33).any(|w| w == push32.as_slice()), "topic0 PUSH32");
    }

    /// `emit` of an undeclared event is a clean `UNKNOWN_FUNCTION` (no panic).
    #[cfg(feature = "wallet")]
    #[test]
    fn emit_unknown_event_is_a_clean_error() {
        let err = super::super::compile(
            "facet C { function f() external { emit Ghost(1); } }",
        )
        .expect_err("emitting an undeclared event must fail cleanly");
        assert_eq!(err.code, Some(crate::error_codes::UNKNOWN_FUNCTION));
        assert!(err.to_string().starts_with("LH0"));
    }

    /// `emit` with the wrong argument count is a clean `ARITY_MISMATCH`.
    #[cfg(feature = "wallet")]
    #[test]
    fn emit_arg_count_mismatch_is_a_clean_error() {
        // Declared 2 args, emitted 1.
        let err = super::super::compile(
            "facet C { event E(address indexed a, uint256 b); \
             function f() external { emit E(msg.sender); } }",
        )
        .expect_err("an arg-count mismatch must fail cleanly");
        assert_eq!(err.code, Some(crate::error_codes::ARITY_MISMATCH));
        // Too many args is also caught.
        let err = super::super::compile(
            "facet C { event E(uint256 a); function f(uint256 n) external { emit E(n, n); } }",
        )
        .expect_err("too many args must fail cleanly");
        assert_eq!(err.code, Some(crate::error_codes::ARITY_MISMATCH));
    }

    /// THE INSTALLMENT-1 CAPSTONE: the FULL CounterFacet (with the event + emits)
    /// compiles end-to-end, all four canonical selectors are dispatched, and the
    /// `Incremented` LOG2 fires (its topic0 is the full event-sig keccak).
    #[cfg(feature = "wallet")]
    #[test]
    fn full_counter_facet_with_events_compiles() {
        use super::super::asm::op;
        const SRC: &str = "facet CounterFacet { mapping(address => uint256) count; uint256 total; \
             event Incremented(address indexed who, uint256 newCount, uint256 newTotal); \
             function increment() external { count[msg.sender] = count[msg.sender] + 1; total = total + 1; \
             emit Incremented(msg.sender, count[msg.sender], total); } \
             function incrementBy(uint256 n) external { require(n > 0, \"zero\"); require(n <= 100, \"too big\"); \
             count[msg.sender] = count[msg.sender] + n; total = total + n; \
             emit Incremented(msg.sender, count[msg.sender], total); } \
             function countOf(address who) external view returns (uint256) { return count[who]; } \
             function totalCount() external view returns (uint256) { return total; } }";
        let art = super::super::compile(SRC).expect("the FULL CounterFacet must compile");
        let rt = &art.runtime;

        // All four canonical selectors are dispatched.
        let sels: [(&str, [u8; 4]); 4] = [
            ("increment()", [0xd0, 0x9d, 0xe0, 0x8a]),
            ("incrementBy(uint256)", [0x03, 0xdf, 0x17, 0x9c]),
            ("countOf(address)", [0xf8, 0x97, 0x7e, 0x96]),
            ("totalCount()", [0x34, 0xea, 0xfb, 0x11]),
        ];
        for (sig, want) in sels {
            assert_eq!(crate::registry::selector(sig), want, "selector pin for {sig}");
            let push4: Vec<u8> = std::iter::once(op::PUSH1 + 3).chain(want).collect();
            assert!(rt.windows(5).any(|w| w == push4.as_slice()), "{sig} dispatched");
        }
        // BOTH increment() and incrementBy() emit the event → exactly two LOG2s
        // (opcode-decoded so PUSH operand bytes equal to 0xA2 don't false-count).
        assert_eq!(count_op(rt, op::LOG2), 2, "two Incremented LOG2s");
        // The shared topic0 is PUSH32'd (appears at least twice, once per emit).
        let topic0 = super::event_topic0("Incremented(address,uint256,uint256)");
        let mut push32_topic0 = vec![op::PUSH1 + 31];
        push32_topic0.extend_from_slice(&topic0);
        let occurrences = rt.windows(33).filter(|w| *w == push32_topic0.as_slice()).count();
        assert_eq!(occurrences, 2, "topic0 PUSH32'd once per emit");
        assert_eq!(art.init_code, super::super::asm::init_wrapper(rt));
    }

    /// 0x-prefixed lowercase hex (test helper). `allow(dead_code)`: the call
    /// sites are gated, so it reads idle under the default feature set.
    #[allow(dead_code)]
    fn to_hex(bytes: &[u8]) -> String {
        use core::fmt::Write;
        let mut s = String::with_capacity(2 + bytes.len() * 2);
        s.push_str("0x");
        for b in bytes {
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    /// Walk EVM bytecode opcode-by-opcode, SKIPPING `PUSH<n>` operand bytes, and
    /// return the list of real instruction `(offset, opcode)` pairs. Naively
    /// scanning the byte stream for an opcode value is WRONG — an opcode byte can
    /// appear inside a PUSH immediate (a hash/slot word) — so opcode-presence /
    /// counting must decode the push widths. (Test helper.)
    #[cfg(feature = "wallet")]
    fn real_opcodes(code: &[u8]) -> Vec<(usize, u8)> {
        use super::super::asm::op;
        let mut out = Vec::new();
        let mut i = 0;
        while i < code.len() {
            let opc = code[i];
            out.push((i, opc));
            // PUSH1..PUSH32 carry 1..=32 operand bytes after the opcode.
            if (op::PUSH1..=op::PUSH1 + 31).contains(&opc) {
                let n = (opc - op::PUSH1) as usize + 1;
                i += 1 + n;
            } else {
                i += 1;
            }
        }
        out
    }

    /// Count REAL occurrences of an opcode (not operand bytes).
    #[cfg(feature = "wallet")]
    fn count_op(code: &[u8], opcode: u8) -> usize {
        real_opcodes(code).iter().filter(|(_, o)| *o == opcode).count()
    }
}
