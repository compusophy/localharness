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
// `Expr`/`Facet`/`Stmt` + the `CompileError` diagnostics are only used by the
// source-compile path, which is wallet-gated (selector + slot keccak live there).
#[cfg(feature = "wallet")]
use crate::soliditylite::ast::{CmpOp, Expr, Facet, StateVarKind, Stmt, Ty};
#[cfg(feature = "wallet")]
use crate::rustlite::CompileError;

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
        }
    }
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

/// One dispatchable function lowered to its selector + body value. The body
/// `Label` is allocated by the caller (so the dispatch arm can reference it before
/// the body is placed — a forward jump).
struct LoweredFn {
    selector: [u8; 4],
    value: BodyValue,
    body_label: Label,
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

/// One dispatchable function lowered to its selector + full [`Body`]. The body
/// `Label` is allocated by the caller so dispatch arms can forward-reference it.
#[cfg(feature = "wallet")]
struct LoweredFnFull {
    selector: [u8; 4],
    body: Body,
    body_label: Label,
}

/// Assemble a full runtime from a list of `(selector, full body)` pairs — the
/// source-compiled path. Shares the dispatch prelude/arms/fallback with
/// [`assemble`] (so a single const getter stays byte-identical), but emits each
/// body via [`emit_full_body`] to support storage writes and `+` expressions.
#[cfg(feature = "wallet")]
fn assemble_full(functions: Vec<([u8; 4], Body)>) -> CompiledArtifact {
    let mut a = Asm::new();
    let fb = a.new_label();
    let lowered: Vec<LoweredFnFull> = functions
        .into_iter()
        .map(|(selector, body)| LoweredFnFull { selector, body, body_label: a.new_label() })
        .collect();

    emit_dispatch_prelude(&mut a, fb);
    for lf in &lowered {
        emit_dispatch_arm(&mut a, lf.selector, lf.body_label);
    }
    emit_fallback(&mut a, fb);
    for lf in &lowered {
        emit_full_body(&mut a, lf.body_label, &lf.body);
    }

    let selectors = lowered.iter().map(|lf| lf.selector).collect();
    let runtime = a.finish();
    let init_code = crate::soliditylite::asm::init_wrapper(&runtime);
    CompiledArtifact { init_code, runtime, selectors }
}

/// Assemble a full runtime from a list of `(selector, body value)` pairs and wrap
/// it as a [`CompiledArtifact`]. This is the ONE place dispatch + bodies are laid
/// out, used by BOTH the compiler ([`compile`]) and the worked
/// [`super::emit_constant_getter`] — so a single function yields identical bytes
/// on either path.
///
/// Layout: prelude → one dispatch arm per fn (in order) → fallback REVERT → one
/// body per fn (in order).
pub fn assemble(functions: &[([u8; 4], BodyValue)]) -> CompiledArtifact {
    let mut a = Asm::new();
    let fb = a.new_label();
    // Allocate every body label up front so the dispatch arms (emitted first) can
    // forward-reference them.
    let lowered: Vec<LoweredFn> = functions
        .iter()
        .map(|(selector, value)| LoweredFn { selector: *selector, value: *value, body_label: a.new_label() })
        .collect();

    emit_dispatch_prelude(&mut a, fb);
    for lf in &lowered {
        emit_dispatch_arm(&mut a, lf.selector, lf.body_label);
    }
    emit_fallback(&mut a, fb);
    for lf in &lowered {
        emit_body(&mut a, lf.body_label, lf.value);
    }

    let selectors = lowered.iter().map(|lf| lf.selector).collect();
    let runtime = a.finish();
    let init_code = crate::soliditylite::asm::init_wrapper(&runtime);
    CompiledArtifact { init_code, runtime, selectors }
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
        if let StateVarKind::Mapping { .. } = self.facet.state_vars[idx].kind {
            return Err(CompileError::at_code(
                codes::TYPE_MISMATCH,
                format!("`{name}` is a mapping; it must be indexed (`{name}[key]`)"),
                span,
            ));
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
        }
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
                    Ok(LoweredExpr::Load(self.scalar_slot(name, *span)?))
                } else if let Some(p) = self.param_index(name) {
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
            Expr::Index { base, key, span } => Ok(LoweredExpr::MapLoad {
                base_slot: self.mapping_base_slot(base, *span)?,
                key: Box::new(self.lower_expr(key)?),
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
        Stmt::Assign { name, value, span } => LoweredStmt::Assign(LoweredAssign::Scalar {
            slot: r.scalar_slot(name, *span)?,
            value: r.lower_expr(value)?,
        }),
        Stmt::IndexAssign { base: map_name, key, value, span } => LoweredStmt::Assign(LoweredAssign::MapEntry {
            base_slot: r.mapping_base_slot(map_name, *span)?,
            key: r.lower_expr(key)?,
            value: r.lower_expr(value)?,
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
        let body = match &func.body {
            // `return "<lit>";` from a `returns (string)` function → a constant
            // string ABI return (the dynamic-type stretch, slice 1: no storage).
            // A string literal returned WITHOUT `returns (string)` is a type error.
            Stmt::Return(Expr::StrLit { value, span }) => {
                if func.returns != Some(Ty::String) {
                    return Err(CompileError::at_code(
                        codes::TYPE_MISMATCH,
                        "a string literal can only be returned from a `returns (string)` function".to_string(),
                        *span,
                    ));
                }
                Body::ConstString(value.clone())
            }
            // Conversely a `returns (string)` function MUST return a string literal
            // in v1 (no dynamic storage/calldata strings yet) — catch a non-literal
            // body before it falls into the single-word return paths below.
            _ if func.returns == Some(Ty::String) => {
                return Err(CompileError::at_code(
                    codes::TYPE_MISMATCH,
                    "a `returns (string)` function must return a string literal in v1".to_string(),
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

    Ok(assemble_full(lowered))
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

    /// 0x-prefixed lowercase hex (test helper).
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
