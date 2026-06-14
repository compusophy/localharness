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
use crate::soliditylite::ast::{Expr, Facet, Stmt};
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
/// - [`Body::Mutating`] is the write-stretch function (`{ <assign>* }`): each
///   `SSTORE(slot, eval(expr))` in order, then an empty `RETURN(0,0)` (the diamond
///   fallback returns cleanly).
#[cfg(feature = "wallet")]
enum Body {
    /// A simple view getter: push one [`BodyValue`], store + return it.
    View(BodyValue),
    /// A view getter returning a compound expression (e.g. `a + b`).
    ViewExpr(LoweredExpr),
    /// A mutating function: a sequence of `(slot, value-expr)` assignments, then
    /// `RETURN(0,0)`.
    Mutating(Vec<([u8; 32], LoweredExpr)>),
}

/// A resolved expression — names already mapped to keccak slots — ready to lower
/// to a stack-pushing instruction sequence (design §5).
#[cfg(feature = "wallet")]
enum LoweredExpr {
    /// A constant operand — pushed MINIMAL-width (`PUSH1 0x01` for `1`), the
    /// idiomatic/gas-cheap encoding for an arithmetic operand. (The TOP-LEVEL
    /// `return <intlit>;` keeps `PUSH32` via [`Body::View`] for the golden gate.)
    Const([u8; 32]),
    /// `PUSH32 <slot> SLOAD` — a state-variable read (slots are full 32-byte words).
    Load([u8; 32]),
    /// `<lhs> <rhs> ADD` — a binary addition (left operand pushed first).
    Add(Box<LoweredExpr>, Box<LoweredExpr>),
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
            LoweredExpr::Add(lhs, rhs) => {
                lhs.emit(a);
                rhs.emit(a);
                a.emit(op::ADD);
            }
        }
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
/// golden gate holds. A [`Body::Mutating`] emits each `SSTORE(slot, value)` then an
/// empty `RETURN(0,0)`: for each assignment, push the value word, push the slot,
/// `SSTORE` (operand order: `SSTORE` pops `slot` then `value`).
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
        Body::Mutating(assigns) => {
            a.jumpdest(body);
            for (slot, value) in assigns {
                // SSTORE pops slot (top) then value, so push value then slot.
                value.emit(a);
                a.push32(slot).emit(op::SSTORE);
            }
            // Empty return so the diamond fallback returns cleanly.
            a.push_u64(0x00).push_u64(0x00).emit(op::RETURN);
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

    let runtime = a.finish();
    let init_code = crate::soliditylite::asm::init_wrapper(&runtime);
    CompiledArtifact { init_code, runtime }
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

    let runtime = a.finish();
    let init_code = crate::soliditylite::asm::init_wrapper(&runtime);
    CompiledArtifact { init_code, runtime }
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

/// Resolve a state variable name to its keccak-namespaced slot (`BASE + index`).
#[cfg(feature = "wallet")]
fn resolve_slot(facet: &Facet, base: [u8; 32], name: &str, span: crate::rustlite::Span) -> Result<[u8; 32], CompileError> {
    use crate::error_codes as codes;
    let idx = facet
        .state_vars
        .iter()
        .position(|sv| sv.name == name)
        .ok_or_else(|| {
            CompileError::at_code(
                codes::UNDEFINED_VARIABLE,
                format!("unknown state variable `{name}`"),
                span,
            )
        })?;
    Ok(slot_at(base, idx as u64))
}

/// Lower an [`Expr`] to a [`LoweredExpr`], resolving any state-var name to its slot.
#[cfg(feature = "wallet")]
fn lower_expr(facet: &Facet, base: [u8; 32], expr: &Expr) -> Result<LoweredExpr, CompileError> {
    match expr {
        Expr::IntLit { value_be32, .. } => Ok(LoweredExpr::Const(*value_be32)),
        Expr::StateVar { name, span } => Ok(LoweredExpr::Load(resolve_slot(facet, base, name, *span)?)),
        Expr::Add { lhs, rhs, .. } => Ok(LoweredExpr::Add(
            Box::new(lower_expr(facet, base, lhs)?),
            Box::new(lower_expr(facet, base, rhs)?),
        )),
    }
}

/// Compile a typed [`Facet`] to a deployable [`CompiledArtifact`].
///
/// Selectors are `keccak256("<name>()")[..4]` (empty param list, v1) computed via
/// the shared [`crate::registry::selector`] helper. A view getter's `return
/// <expr>;` lowers to a [`Body::View`] (`<intlit>` → constant, `<stateVar>` →
/// `SLOAD` of the keccak-namespaced slot, `a + b` → `ADD`); a mutating function's
/// `<stateVar> = <expr>;` assignments lower to a [`Body::Mutating`] that `SSTORE`s
/// each to its slot then `RETURN(0,0)` (design §5 storage).
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
        // v1: empty parameter list, so the signature is just `name()`.
        let signature = format!("{}()", func.name);
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

        let body = match &func.body {
            // Simple view getter: `return <intlit|stateVar>;` keeps the golden-gate
            // `BodyValue` path (PUSH32). A compound return (`a + b`) uses the richer
            // expr lowering.
            Stmt::Return(Expr::IntLit { value_be32, .. }) => Body::View(BodyValue::Const(*value_be32)),
            Stmt::Return(Expr::StateVar { name, span }) => {
                Body::View(BodyValue::StorageSlot(resolve_slot(facet, base, name, *span)?))
            }
            Stmt::Return(expr) => Body::ViewExpr(lower_expr(facet, base, expr)?),
            // Mutating function: `{ <stateVar> = <expr>; … }`.
            Stmt::Block(stmts) => {
                let mut assigns = Vec::with_capacity(stmts.len());
                for stmt in stmts {
                    match stmt {
                        Stmt::Assign { name, value, span } => {
                            let slot = resolve_slot(facet, base, name, *span)?;
                            assigns.push((slot, lower_expr(facet, base, value)?));
                        }
                        // A non-assignment inside a mutating block is unreachable
                        // from the parser (it only emits assignments), but guard
                        // it as a clean error rather than panicking.
                        other => {
                            return Err(CompileError::at_code(
                                codes::UNSUPPORTED_FEATURE,
                                format!("only assignments are supported in a mutating body, got {other:?}"),
                                func.span,
                            ))
                        }
                    }
                }
                Body::Mutating(assigns)
            }
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
}
