use crate::rustlite::Span;

#[derive(Debug, Clone)]
pub struct Module {
    pub uses: Vec<UseDecl>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub struct UseDecl {
    pub path: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Item {
    Struct(StructDecl),
    Enum(EnumDecl),
    Fn(FnDecl),
    Const(ConstDecl),
}

#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
    pub fields: Vec<Field>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    pub variants: Vec<Variant>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub ret_type: Option<Ty>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: String,
    pub ty: Ty,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Variant {
    pub name: String,
    pub payload: VariantPayload,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum VariantPayload {
    Unit,
    Tuple(Vec<Ty>),
    Struct(Vec<Field>),
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Ty {
    I32,
    I64,
    F32,
    F64,
    Bool,
    String,
    Named(String),
    Tuple(Vec<Ty>),
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        name: String,
        mutable: bool,
        ty: Option<Ty>,
        init: Expr,
        span: Span,
    },
    Assign {
        place: Place,
        value: Expr,
        span: Span,
    },
    Return {
        value: Option<Expr>,
        span: Span,
    },
    Expr {
        expr: Expr,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct Place {
    pub root: String,
    pub fields: Vec<String>,
    /// `Some(expr)` for an INDEXED assignment target `base[index] = value`,
    /// where `base` is `root[.fields…]` (an array value) and `index` is an
    /// `i32`. `None` for a plain variable / struct-field place. Mirrors the
    /// read side's `ExprKind::Index { base, index }` (address = base + idx*4).
    pub index: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),
    BoolLit(bool),

    Var(String),

    Path(Vec<String>),

    FieldAccess {
        object: Box<Expr>,
        field: String,
    },

    Call {
        func: Box<Expr>,
        args: Vec<Expr>,
    },

    MethodCall {
        object: Box<Expr>,
        method: String,
        args: Vec<Expr>,
    },

    StructLit {
        path: Vec<String>,
        fields: Vec<FieldInit>,
    },

    TupleLit(Vec<Expr>),

    /// `[e0, e1, …]` — a fixed-size array literal (stored in linear memory).
    ArrayLit(Vec<Expr>),

    /// `base[index]` — array element access (read).
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },

    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },

    UnaryOp {
        op: UnaryOp,
        operand: Box<Expr>,
    },

    /// `expr as Ty` — a numeric conversion (i32/i64/f32/f64).
    Cast {
        expr: Box<Expr>,
        ty: Ty,
    },

    If {
        cond: Box<Expr>,
        then_block: Block,
        else_block: Option<ElseBranch>,
    },

    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },

    While {
        cond: Box<Expr>,
        body: Block,
    },

    Loop {
        body: Block,
    },

    Break {
        value: Option<Box<Expr>>,
    },

    Continue,

    Block(Block),
}

#[derive(Debug, Clone)]
pub enum ElseBranch {
    Block(Block),
    If(Box<Expr>),
}

#[derive(Debug, Clone)]
pub struct FieldInit {
    pub name: String,
    pub value: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum PatternKind {
    Wildcard,
    Literal(LitPattern),
    /// An integer range pattern: `lo..=hi` (inclusive) or `lo..hi` (exclusive).
    IntRange {
        lo: i64,
        hi: i64,
        inclusive: bool,
    },
    Binding(String),
    Path(Vec<String>),
    TupleVariant {
        path: Vec<String>,
        fields: Vec<Pattern>,
    },
    StructVariant {
        path: Vec<String>,
        fields: Vec<FieldPattern>,
    },
}

#[derive(Debug, Clone)]
pub enum LitPattern {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
}

#[derive(Debug, Clone)]
pub struct FieldPattern {
    pub name: String,
    pub pattern: Option<Pattern>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    Eq, Ne, Lt, Gt, Le, Ge,
    And, Or,
    Shl, Shr, BitAnd, BitOr, BitXor,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
}
