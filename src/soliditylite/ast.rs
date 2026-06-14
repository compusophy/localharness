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

/// A facet state variable: `<ty> <name>;` (design §5 storage stretch).
///
/// Laid out sequentially from the keccak-namespaced `BASE`; its index in
/// [`Facet::state_vars`] is its slot offset (no packing in v1).
#[derive(Debug, Clone, PartialEq)]
pub struct StateVar {
    /// The declared value type (v1: `uint256` only for the storage stretch).
    pub ty: Ty,
    /// The variable name, referenced by `return <name>;`.
    pub name: String,
    /// Source span of the declaration.
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

/// One function: `function <name>() external <mut> returns (<ty>) { <body> }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// The function name; combined with the (empty, v1) parameter list into the
    /// selector signature `keccak256("<name>()")[..4]`.
    pub name: String,
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
/// are a `{ <assign>* }` block of state-var assignments.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `return <expr>;` — evaluate the expression and return it as the 32-byte word.
    Return(Expr),
    /// `<stateVar> = <expr>;` — evaluate `<expr>` and `SSTORE` it to the state
    /// var's keccak-namespaced slot (the storage-write stretch). `name` is the
    /// assignment target; `span` is the target identifier's span.
    Assign { name: String, value: Expr, span: Span },
    /// A `{ <stmt>* }` block — a mutating function body holding zero or more
    /// statements, emitted in order. (View getters never use this; their body is a
    /// bare [`Stmt::Return`], so tick-5's pattern-matches are unaffected.)
    Block(Vec<Stmt>),
}

/// An expression. The floor grammar has the integer literal; the storage stretch
/// adds a bare state-variable reference and a left-associative `+` of operands.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// An integer literal — its big-endian 32-byte word and the literal's span.
    IntLit { value_be32: [u8; 32], span: Span },
    /// A bare identifier — a state-variable read (`return <name>;`), resolved to
    /// an `SLOAD` of its keccak-namespaced slot at codegen (storage stretch).
    StateVar { name: String, span: Span },
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
            Expr::Add { span, .. } => *span,
        }
    }
}
