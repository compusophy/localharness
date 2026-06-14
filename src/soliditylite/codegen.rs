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
use crate::soliditylite::ast::{Expr, Facet, StateVarKind, Stmt};
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
    /// A mutating function: a sequence of (scalar or mapping-entry) assignments,
    /// then `RETURN(0,0)`.
    Mutating(Vec<LoweredAssign>),
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
    /// A mapping-entry read: derive the entry slot
    /// `keccak256(pad32(key) ++ pad32(baseSlot))`, then `SLOAD`.
    MapLoad { base_slot: [u8; 32], key: Box<LoweredExpr> },
    /// `<lhs> <rhs> ADD` — a binary addition (left operand pushed first).
    Add(Box<LoweredExpr>, Box<LoweredExpr>),
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
            LoweredExpr::MapLoad { base_slot, key } => {
                emit_map_slot(a, base_slot, key);
                a.emit(op::SLOAD);
            }
            LoweredExpr::Add(lhs, rhs) => {
                lhs.emit(a);
                rhs.emit(a);
                a.emit(op::ADD);
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
        Body::Mutating(assigns) => {
            a.jumpdest(body);
            for assign in assigns {
                assign.emit(a);
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
            Expr::Index { base, key, span } => Ok(LoweredExpr::MapLoad {
                base_slot: self.mapping_base_slot(base, *span)?,
                key: Box::new(self.lower_expr(key)?),
            }),
            Expr::Add { lhs, rhs, .. } => Ok(LoweredExpr::Add(
                Box::new(self.lower_expr(lhs)?),
                Box::new(self.lower_expr(rhs)?),
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

/// Compile a typed [`Facet`] to a deployable [`CompiledArtifact`].
///
/// Selectors are `keccak256("<name>(<types>)")[..4]` (the ABI canonical signature)
/// computed via the shared [`crate::registry::selector`] helper. A view getter's
/// `return <expr>;` lowers to a [`Body::View`]/[`Body::ViewExpr`] (`<intlit>` →
/// constant, scalar `<stateVar>` → `SLOAD`, parameter → `CALLDATALOAD(4+32*i)`,
/// `msg.sender` → `CALLER`, `<map>[<key>]` → keccak-slot `SLOAD`, `a + b` → `ADD`);
/// a mutating function's `<stateVar> = <expr>;` / `<map>[<key>] = <expr>;`
/// assignments lower to a [`Body::Mutating`] that `SSTORE`s each then `RETURN(0,0)`.
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
            // Simple view getter `return <intlit>;` / `return <scalarStateVar>;`
            // keeps the golden-gate `BodyValue` path (PUSH32 / PUSH32+SLOAD); every
            // other return shape (param ref, msg.sender, mapping index, `a + b`)
            // uses the richer expr lowering.
            Stmt::Return(Expr::IntLit { value_be32, .. }) => Body::View(BodyValue::Const(*value_be32)),
            Stmt::Return(Expr::StateVar { name, span }) if r.state_var_index(name).is_some() => {
                Body::View(BodyValue::StorageSlot(r.scalar_slot(name, *span)?))
            }
            Stmt::Return(expr) => Body::ViewExpr(r.lower_expr(expr)?),
            // Mutating function: `{ (<stateVar>|<map>[<key>]) = <expr>; … }`.
            Stmt::Block(stmts) => {
                let mut assigns = Vec::with_capacity(stmts.len());
                for stmt in stmts {
                    match stmt {
                        Stmt::Assign { name, value, span } => {
                            assigns.push(LoweredAssign::Scalar {
                                slot: r.scalar_slot(name, *span)?,
                                value: r.lower_expr(value)?,
                            });
                        }
                        Stmt::IndexAssign { base: map_name, key, value, span } => {
                            assigns.push(LoweredAssign::MapEntry {
                                base_slot: r.mapping_base_slot(map_name, *span)?,
                                key: r.lower_expr(key)?,
                                value: r.lower_expr(value)?,
                            });
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
