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

use crate::rustlite::CompileError;
use crate::soliditylite::asm::{op, Asm, Label};
use crate::soliditylite::ast::{Expr, Facet, Stmt};
use crate::soliditylite::CompiledArtifact;

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

/// Compile a typed [`Facet`] to a deployable [`CompiledArtifact`].
///
/// Selectors are `keccak256("<name>()")[..4]` (empty param list, v1) computed via
/// the shared [`crate::registry::selector`] helper. A `return <intlit>;` lowers to
/// a [`BodyValue::Const`]; a `return <stateVar>;` resolves the variable to its
/// keccak-namespaced slot and lowers to a [`BodyValue::StorageSlot`] (design §5).
///
/// Gated on `wallet` because selector keccak + storage-slot keccak both live
/// behind that feature (sha3/registry); without it, the frontend still
/// lexes/parses but cannot emit selectors.
#[cfg(feature = "wallet")]
pub fn compile(facet: &Facet) -> Result<CompiledArtifact, CompileError> {
    use crate::error_codes as codes;

    let base = storage_base(&facet.name);
    let mut lowered: Vec<([u8; 4], BodyValue)> = Vec::with_capacity(facet.functions.len());
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

        let value = match &func.body {
            Stmt::Return(Expr::IntLit { value_be32, .. }) => BodyValue::Const(*value_be32),
            Stmt::Return(Expr::StateVar { name, span }) => {
                // Resolve the state var to its declaration-order slot index.
                let idx = facet
                    .state_vars
                    .iter()
                    .position(|sv| &sv.name == name)
                    .ok_or_else(|| {
                        CompileError::at_code(
                            codes::UNDEFINED_VARIABLE,
                            format!("unknown state variable `{name}`"),
                            *span,
                        )
                    })?;
                BodyValue::StorageSlot(slot_at(base, idx as u64))
            }
        };
        lowered.push((selector, value));
    }

    Ok(assemble(&lowered))
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
}
