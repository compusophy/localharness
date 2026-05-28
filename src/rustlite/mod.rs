/// Token types (keywords, operators, literals).
pub mod token;
/// Byte-level lexer with string escapes.
pub mod lexer;
/// Full AST (structs, enums, functions, match, etc.).
pub mod ast;
/// Recursive-descent parser with precedence climbing.
pub mod parser;
/// Scope-based type resolution and mutability checking.
pub mod typecheck;
/// Wasm binary emitter (sections, opcodes, LEB128).
pub mod codegen;
/// Wasm32-only cartridge instantiation via `WebAssembly`.
pub mod loader;

/// Compile a Rust-subset source string into wasm bytes.
///
/// Pipeline: lex -> parse -> typecheck -> codegen.
pub fn compile(source: &str) -> Result<Vec<u8>, CompileError> {
    let tokens = lexer::lex(source)?;
    let module = parser::parse(&tokens)?;
    let typed = typecheck::check(&module)?;
    let wasm = codegen::emit(&typed)?;
    Ok(wasm)
}

/// An error produced during compilation (lex, parse, typecheck, or codegen).
#[derive(Debug, Clone)]
pub struct CompileError {
    /// Human-readable error description.
    pub message: String,
    /// Source location, if available.
    pub span: Option<Span>,
}

/// A byte-offset range in the source text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    /// Start byte offset (inclusive).
    pub start: usize,
    /// End byte offset (exclusive).
    pub end: usize,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(span) = self.span {
            write!(f, "[{}..{}] {}", span.start, span.end, self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl std::error::Error for CompileError {}

impl CompileError {
    /// Create an error with no source span.
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), span: None }
    }
    /// Create an error pinned to a source span.
    pub fn at(message: impl Into<String>, span: Span) -> Self {
        Self { message: message.into(), span: Some(span) }
    }
}

impl From<String> for CompileError {
    fn from(s: String) -> Self { Self::new(s) }
}
