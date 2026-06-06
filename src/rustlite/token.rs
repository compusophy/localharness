use crate::rustlite::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literals
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),
    True,
    False,

    // Identifiers and paths
    Ident(String),

    // Keywords
    Fn,
    Let,
    Mut,
    If,
    Else,
    Match,
    While,
    Loop,
    Break,
    Continue,
    For,
    In,
    Return,
    Struct,
    Enum,
    Const,
    Use,

    // Type keywords
    I32,
    I64,
    F32,
    F64,
    Bool,
    StringType,

    // Punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    ColonColon,
    Semi,
    Dot,
    DotDot,     // .. (range)
    Arrow,      // ->
    FatArrow,   // =>

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    Eq,
    EqEq,
    BangEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    AmpAmp,
    PipePipe,
    // Compound assignment (`x += 1` etc.) — desugared in the parser to
    // `x = x <op> 1`.
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    // Bitwise + shift
    Amp,   // &
    Pipe,  // |
    Caret, // ^
    Shl,   // <<
    Shr,   // >>

    // Special
    Underscore,
    Eof,
}

impl TokenKind {
    pub fn keyword(s: &str) -> Option<TokenKind> {
        match s {
            "fn" => Some(TokenKind::Fn),
            "let" => Some(TokenKind::Let),
            "mut" => Some(TokenKind::Mut),
            "if" => Some(TokenKind::If),
            "else" => Some(TokenKind::Else),
            "match" => Some(TokenKind::Match),
            "while" => Some(TokenKind::While),
            "loop" => Some(TokenKind::Loop),
            "break" => Some(TokenKind::Break),
            "continue" => Some(TokenKind::Continue),
            "for" => Some(TokenKind::For),
            "in" => Some(TokenKind::In),
            "return" => Some(TokenKind::Return),
            "struct" => Some(TokenKind::Struct),
            "enum" => Some(TokenKind::Enum),
            "const" => Some(TokenKind::Const),
            "use" => Some(TokenKind::Use),
            "true" => Some(TokenKind::True),
            "false" => Some(TokenKind::False),
            "i32" => Some(TokenKind::I32),
            "i64" => Some(TokenKind::I64),
            "f32" => Some(TokenKind::F32),
            "f64" => Some(TokenKind::F64),
            "bool" => Some(TokenKind::Bool),
            "String" => Some(TokenKind::StringType),
            "_" => Some(TokenKind::Underscore),
            _ => None,
        }
    }
}
