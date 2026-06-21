//! SolidityLite AST — the parsed shape of the v1 subset.
//!
//! Intentionally tiny for Installment 1's FLOOR grammar (design §3): a `facet`
//! holds one or more `function`s; each function is `external view returns
//! (uint256)` and its body is a single `return <intlit>;`. The shape mirrors
//! [`crate::rustlite::ast`]'s `Module`/`Item`/`FnDecl` discipline (each node
//! carries a [`Span`] back into the source) so diagnostics point at real bytes,
//! but the type/expr lattice is the four EVM words rather than rustlite's numeric
//! matrix. Richer statements/expressions and the storage/mapping nodes (design
//! §3 stretch) layer on top of this without reshaping the floor.

use crate::rustlite::Span;

/// A whole compilation unit: exactly one `facet { … }`.
///
/// (Solidity allows multiple top-level contracts; v1 accepts a single facet —
/// the one the agent is compiling to cut. A second top-level item is a clean
/// `CompileError`, not a silent drop.)
///
/// Only `PartialEq` (not `Eq`): nodes carry a [`Span`], and `rustlite::Span`
/// derives `PartialEq` only.
#[derive(Debug, Clone, PartialEq)]
pub struct Facet {
    /// The facet/contract name (`facet <Ident>`). Drives the storage `BASE`
    /// (`keccak256("localharness.<lowercased name>.storage.v1")`, design §5).
    pub name: String,
    /// The facet's state variables, in declaration order. Empty for the floor
    /// grammar; one `uint256 <name>;` per entry for the storage stretch.
    pub state_vars: Vec<StateVar>,
    /// The facet's event declarations, in declaration order. Empty unless the
    /// facet declares `event <Name>(<args>);` (the events stretch). Each `emit`
    /// statement resolves its name against this list for the LOG topic0 signature.
    pub events: Vec<EventDecl>,
    /// The facet's functions, in declaration order (the dispatch order).
    pub functions: Vec<Function>,
    /// Source span of the `facet` keyword (for top-level diagnostics).
    pub span: Span,
}

/// A v1 value type — one of the four EVM-native 32-byte words (design §3).
///
/// The floor grammar only needs [`Ty::Uint256`]; the rest are declared so the
/// type position parses uniformly and a non-`uint256` use surfaces a precise
/// "unsupported in v1" error rather than an "unexpected token".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    /// `uint256` — the word as-is.
    Uint256,
    /// `address` — word, high 12 bytes zero (masked on write/decode).
    Address,
    /// `bool` — `0`/`1`.
    Bool,
    /// `bytes32` — the word as-is.
    Bytes32,
    /// `string` — a dynamic type, v1-supported ONLY as a function's RETURN type
    /// (`returns (string)`) with a constant string-literal body. Produced solely
    /// by the return-clause parser, never `parse_ty`, so `string` in a parameter,
    /// state var, or event arg stays a clean "expected type" error (NOT a silent
    /// single-word miscompile). See [`super::codegen`]'s `Body::ConstString`.
    String,
}

impl Ty {
    /// The canonical ABI type name used in the function selector signature
    /// (`keccak256("name(types)")`). For v1's value types this is just the
    /// Solidity name. `string` only ever appears as a return type (NOT in the
    /// param/event signature that feeds the selector), so its name is unused
    /// there but defined for completeness.
    pub fn abi_name(self) -> &'static str {
        match self {
            Ty::Uint256 => "uint256",
            Ty::Address => "address",
            Ty::Bool => "bool",
            Ty::Bytes32 => "bytes32",
            Ty::String => "string",
        }
    }
}

/// A facet state variable's shape: a scalar `<ty> <name>;` or a
/// `mapping(<key> => <value>) <name>;` (design §5 storage + the mapping stretch).
///
/// Both occupy ONE declaration index (the base slot). A scalar lives directly at
/// `BASE + index`; a mapping uses `BASE + index` as its keccak preimage base slot,
/// with each entry at `keccak256(pad32(key) ++ pad32(baseSlot))`.
#[derive(Debug, Clone, PartialEq)]
pub enum StateVarKind {
    /// A scalar slot: `<ty> <name>;` — the value lives directly at `BASE + index`.
    Scalar(Ty),
    /// A mapping: `mapping(<key> => <value>) <name>;` — `BASE + index` is the
    /// preimage base slot; entries hash `keccak256(pad32(key) ++ pad32(baseSlot))`.
    Mapping {
        /// The key type (`address`/`uint256`/…). v1 keys are a single 32-byte word.
        key: Ty,
        /// The stored value type (v1: a single 32-byte word).
        value: Ty,
    },
    /// A dynamic array: `<elem>[] <name>;` (the dynamic-type stretch). `BASE + index`
    /// holds the LENGTH; element `i` lives at `keccak256(pad32(BASE + index)) + i`
    /// (the canonical Solidity dynamic-array layout). Elements are a single word.
    Array {
        /// The element type (v1: a single 32-byte word, e.g. `uint256`/`address`).
        elem: Ty,
    },
}

/// A facet state variable: `<ty> <name>;` or `mapping(K => V) <name>;` (design §5).
///
/// Laid out sequentially from the keccak-namespaced `BASE`; its index in
/// [`Facet::state_vars`] is its slot offset (scalars) or preimage base slot
/// (mappings). No packing in v1.
#[derive(Debug, Clone, PartialEq)]
pub struct StateVar {
    /// Whether this is a scalar or a mapping (and its element types).
    pub kind: StateVarKind,
    /// The variable name, referenced by `return <name>;` / `<name>[<key>]`.
    pub name: String,
    /// Source span of the declaration.
    pub span: Span,
}

/// One event argument: `<ty> [indexed] <name>` inside the event parameter list.
///
/// `indexed` args become extra LOG topics (one 32-byte word each, in declaration
/// order after topic0); non-`indexed` args are ABI-encoded sequentially into the
/// log's data region (design §6 events). v1 value types are all a single word.
#[derive(Debug, Clone, PartialEq)]
pub struct EventArg {
    /// The declared value type (`uint256`/`address`/… — a single 32-byte word).
    pub ty: Ty,
    /// Whether the arg is `indexed` (a LOG topic) vs. part of the data region.
    pub indexed: bool,
    /// The argument name (cosmetic in v1 — it doesn't affect the LOG; the
    /// selector signature uses the TYPE, not the name). Kept for diagnostics.
    pub name: String,
    /// Source span of the argument.
    pub span: Span,
}

/// An event declaration: `event <Name>(<ty> [indexed] <name>, …);` at facet
/// top-level (the events stretch — the last CounterFacet primitive).
///
/// `topic0` of an emitted log is `keccak256("<Name>(<type>,<type>,…)")` (the FULL
/// 32-byte hash, NOT the 4-byte selector). Each `indexed` arg adds a topic; each
/// non-`indexed` arg is appended to the log data region. An `emit <Name>(…)`
/// statement's argument count must match this declaration's arg count.
#[derive(Debug, Clone, PartialEq)]
pub struct EventDecl {
    /// The event name (`event <Name>`), combined with the arg types into the
    /// topic0 signature `keccak256("<Name>(<types>)")`.
    pub name: String,
    /// The declared arguments, in order. The `indexed`/data split is preserved.
    pub args: Vec<EventArg>,
    /// Source span of the `event` keyword.
    pub span: Span,
}

/// One function parameter: `<ty> <name>` inside the parameter list. ABI-decoded
/// from calldata at offset `4 + 32*index` (design §5 calldata decode).
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// The declared value type (`uint256`/`address`/… — all a single 32-byte word).
    pub ty: Ty,
    /// The parameter name, referenced in the body as a bare identifier.
    pub name: String,
    /// Source span of the parameter.
    pub span: Span,
}

/// Function state-mutability (design §3): the floor grammar requires `view`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutability {
    /// `view` — reads state, never writes (the only floor-grammar mutability).
    View,
    /// `pure` — touches no state (accepted; a constant getter is effectively pure).
    Pure,
    /// No mutability keyword (a plain `external` function). A no-`view`,
    /// no-`returns` function (`function f() external { … }`) is a MUTATING
    /// function — its body may assign to state vars and falls through to an empty
    /// `RETURN(0,0)` (the storage-write stretch).
    NonPayable,
}

/// One function: `function <name>(<params>) external <mut> returns (<ty>) { <body> }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// The function name; combined with the parameter types into the selector
    /// signature `keccak256("<name>(<types>)")[..4]` (empty list → `<name>()`).
    pub name: String,
    /// The declared parameters, in order. Each is ABI-decoded from calldata at
    /// `4 + 32*index`. Empty for a no-arg function.
    pub params: Vec<Param>,
    /// State mutability (`view`/`pure`/none).
    pub mutability: Mutability,
    /// The single return type, if the function declares `returns (...)`. A view
    /// getter is `Some(ty)`; a MUTATING function (no `returns` clause) is `None`
    /// and its body emits its statements then `RETURN(0,0)`.
    pub returns: Option<Ty>,
    /// The function body. A view getter has a single [`Stmt::Return`]; a mutating
    /// function has a (possibly empty) sequence of [`Stmt::Assign`].
    pub body: Stmt,
    /// Source span of the `function` keyword.
    pub span: Span,
}

/// A comparison operator (the relational stretch). Each lowers to an EVM
/// comparison opcode (`GT`/`LT`/`EQ`), with `<=`/`>=` synthesized via `ISZERO` of
/// the strict comparison. EVM `GT`/`LT` are UNSIGNED, which is correct for the v1
/// `uint256`/`address`/`bytes32` value types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    /// `>` — `GT`.
    Gt,
    /// `<` — `LT`.
    Lt,
    /// `>=` — `ISZERO(LT(a, b))` (true iff not `a < b`).
    Ge,
    /// `<=` — `ISZERO(GT(a, b))` (true iff not `a > b`).
    Le,
    /// `==` — `EQ`.
    Eq,
    /// `!=` — `ISZERO(EQ(a, b))` (true iff not equal).
    Neq,
}

/// A statement. View getters are a single `return <expr>;`; mutating functions
/// are a `{ (<assign>|<require>)* }` block of state-var/mapping assignments and
/// `require` guards.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `return <expr>;` — evaluate the expression and return it as the 32-byte word.
    Return(Expr),
    /// `require(<cond>, "<msg>");` — evaluate `<cond>`; if it is FALSE (zero),
    /// `REVERT(0,0)`. The message is parsed but DISCARDED (an empty-data revert is
    /// enough to abort the call). `span` is the `require` keyword's span.
    Require { cond: Expr, span: Span },
    /// `<stateVar> = <expr>;` — evaluate `<expr>` and `SSTORE` it to the state
    /// var's keccak-namespaced slot (the storage-write stretch). `name` is the
    /// assignment target; `span` is the target identifier's span.
    Assign { name: String, value: Expr, span: Span },
    /// `<mapping>[<key>] = <expr>;` — `SSTORE` `<expr>` to the mapping entry slot
    /// `keccak256(pad32(key) ++ pad32(baseSlot))` (the mapping-write stretch).
    /// `base` is the mapping name; `key` is the index expression. Also covers a
    /// dynamic-array element write `<arr>[<i>] = <expr>;` (the array slot derivation
    /// `keccak256(pad32(slot)) + i` is selected at codegen by the var's kind).
    IndexAssign { base: String, key: Expr, value: Expr, span: Span },
    /// `<arr>.push(<expr>);` — append to a dynamic array (the dynamic-type stretch):
    /// store `<expr>` at `keccak256(pad32(slot)) + length`, then bump the length slot.
    /// `base` is the array name; `span` is the array identifier's span.
    Push { base: String, value: Expr, span: Span },
    /// `emit <Name>(<expr>, …);` — append an EVM log (`LOGn`). `topic0` is
    /// `keccak256("<Name>(<types>)")`; each `indexed` event arg becomes an extra
    /// topic and each non-`indexed` arg is ABI-encoded into the log data region
    /// (design §6 events). `name` is the event name (resolved against the facet's
    /// [`EventDecl`]s); `args` are the value expressions, positionally matched to
    /// the declared args. `span` is the `emit` keyword's span.
    Emit { name: String, args: Vec<Expr>, span: Span },
    /// A `{ <stmt>* }` block — a mutating function body holding zero or more
    /// statements, emitted in order. (View getters never use this; their body is a
    /// bare [`Stmt::Return`], so tick-5's pattern-matches are unaffected.)
    Block(Vec<Stmt>),
    /// `if (<cond>) { <stmt>* } [else { <stmt>* }]` — conditional control flow in a
    /// mutating body (the branch stretch). `cond` is evaluated; when FALSE (zero),
    /// execution skips `then_body` and runs `else_body` (empty when there is no
    /// `else`). `else if` chains desugar to a nested `If` as the sole `else_body`
    /// statement. Branches may nest and hold any mutating statement (assignments,
    /// `require`, `emit`, further `if`s). `span` is the `if` keyword's span.
    If { cond: Expr, then_body: Vec<Stmt>, else_body: Vec<Stmt>, span: Span },
}

/// An expression. The floor grammar has the integer literal; the storage stretch
/// adds a bare name reference and a left-associative `+`; the mapping/param/sender
/// stretch adds `msg.sender`, a `<mapping>[<key>]` index, and bare parameter refs.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// An integer literal — its big-endian 32-byte word and the literal's span.
    IntLit { value_be32: [u8; 32], span: Span },
    /// A bare identifier — a NAMED reference resolved at codegen to either a state
    /// variable (`SLOAD` of its keccak-namespaced slot) or a function parameter
    /// (`CALLDATALOAD(4 + 32*index)`). The parser cannot distinguish the two, so
    /// resolution is deferred to codegen (which knows the param/state-var tables).
    StateVar { name: String, span: Span },
    /// `msg.sender` — the caller address as a 32-byte word (`CALLER`).
    MsgSender { span: Span },
    /// `block.timestamp` — the current block's unix time as a word (`TIMESTAMP`).
    /// Enables time-based facets (deadlines, vesting, rate-limits, auctions).
    BlockTimestamp { span: Span },
    /// `block.number` — the current block height as a word (`NUMBER`).
    BlockNumber { span: Span },
    /// `<mapping>[<key>]` — a mapping-entry read: derive the entry slot
    /// `keccak256(pad32(key) ++ pad32(baseSlot))`, then `SLOAD`. `base` is the
    /// mapping name; `key` is the index expression. Also covers a dynamic-array
    /// element read `<arr>[<i>]` (slot `keccak256(pad32(slot)) + i`, selected at
    /// codegen by the var's kind).
    Index { base: String, key: Box<Expr>, span: Span },
    /// `<arr>.length` — a dynamic array's length, read directly from its base slot
    /// (`SLOAD(BASE + index)`). `base` is the array name.
    ArrayLen { base: String, span: Span },
    /// A binary `lhs + rhs` — both operands are evaluated onto the stack, then
    /// `ADD` (the arithmetic stretch; left-associative, e.g. `n = n + 1`).
    Add { lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    /// A binary `lhs - rhs` — `SUB` (same precedence/associativity as `+`). Wraps
    /// on underflow (no 0.8 revert in v1); guard with `require` where it matters,
    /// e.g. `require(bal[from] >= amt, …)` before `bal[from] = bal[from] - amt`.
    Sub { lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    /// `lhs * rhs` — `MUL`. Binds TIGHTER than `+`/`-` (the multiplicative tier),
    /// so `a + b * c` is `a + (b * c)`. Wraps on overflow (no 0.8 revert in v1).
    Mul { lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    /// `lhs / rhs` — `DIV` (integer division; EVM `DIV` yields 0 when `rhs == 0`,
    /// NOT a revert). Multiplicative precedence.
    Div { lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    /// `lhs % rhs` — `MOD` (EVM `MOD` yields 0 when `rhs == 0`). Multiplicative
    /// precedence. Useful for round-robin / wrapping / fee-remainder math.
    Mod { lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    /// A comparison `lhs <op> rhs` (the relational stretch) — both operands are
    /// evaluated, then the comparison opcode(s) for `op`, leaving a `0`/`1` word.
    /// Binds LOOSER than `+`, so `n + 1 > 0` parses as `(n + 1) > 0`.
    Cmp { op: CmpOp, lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    /// A string literal `"…"` (the dynamic-type stretch). v1 supports it ONLY as a
    /// whole `return "…";` from a `returns (string)` function (codegen lowers it to
    /// `Body::ConstString`). Any other position (assignment, comparison, emit arg)
    /// is a clean codegen error. `value` is the decoded UTF-8 bytes.
    StrLit { value: Vec<u8>, span: Span },
}

impl Expr {
    /// The source span of this expression (for diagnostics).
    pub fn span(&self) -> Span {
        match self {
            Expr::IntLit { span, .. } => *span,
            Expr::StateVar { span, .. } => *span,
            Expr::MsgSender { span, .. } => *span,
            Expr::BlockTimestamp { span, .. } => *span,
            Expr::BlockNumber { span, .. } => *span,
            Expr::Index { span, .. } => *span,
            Expr::ArrayLen { span, .. } => *span,
            Expr::Add { span, .. } => *span,
            Expr::Sub { span, .. } => *span,
            Expr::Mul { span, .. } => *span,
            Expr::Div { span, .. } => *span,
            Expr::Mod { span, .. } => *span,
            Expr::Cmp { span, .. } => *span,
            Expr::StrLit { span, .. } => *span,
        }
    }
}
