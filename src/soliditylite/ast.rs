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
}

impl Ty {
    /// The canonical ABI type name used in the function selector signature
    /// (`keccak256("name(types)")`). For v1's value types this is just the
    /// Solidity name.
    pub fn abi_name(self) -> &'static str {
        match self {
            Ty::Uint256 => "uint256",
            Ty::Address => "address",
            Ty::Bool => "bool",
            Ty::Bytes32 => "bytes32",
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

/// A statement. View getters are a single `return <expr>;`; mutating functions
/// are a `{ <assign>* }` block of state-var (or mapping-entry) assignments.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `return <expr>;` — evaluate the expression and return it as the 32-byte word.
    Return(Expr),
    /// `<stateVar> = <expr>;` — evaluate `<expr>` and `SSTORE` it to the state
    /// var's keccak-namespaced slot (the storage-write stretch). `name` is the
    /// assignment target; `span` is the target identifier's span.
    Assign { name: String, value: Expr, span: Span },
    /// `<mapping>[<key>] = <expr>;` — `SSTORE` `<expr>` to the mapping entry slot
    /// `keccak256(pad32(key) ++ pad32(baseSlot))` (the mapping-write stretch).
    /// `base` is the mapping name; `key` is the index expression.
    IndexAssign { base: String, key: Expr, value: Expr, span: Span },
    /// A `{ <stmt>* }` block — a mutating function body holding zero or more
    /// statements, emitted in order. (View getters never use this; their body is a
    /// bare [`Stmt::Return`], so tick-5's pattern-matches are unaffected.)
    Block(Vec<Stmt>),
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
    /// `<mapping>[<key>]` — a mapping-entry read: derive the entry slot
    /// `keccak256(pad32(key) ++ pad32(baseSlot))`, then `SLOAD`. `base` is the
    /// mapping name; `key` is the index expression.
    Index { base: String, key: Box<Expr>, span: Span },
    /// A binary `lhs + rhs` — both operands are evaluated onto the stack, then
    /// `ADD` (the arithmetic stretch; left-associative, e.g. `n = n + 1`).
    Add { lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
}

impl Expr {
    /// The source span of this expression (for diagnostics).
    pub fn span(&self) -> Span {
        match self {
            Expr::IntLit { span, .. } => *span,
            Expr::StateVar { span, .. } => *span,
            Expr::MsgSender { span, .. } => *span,
            Expr::Index { span, .. } => *span,
            Expr::Add { span, .. } => *span,
        }
    }
}
