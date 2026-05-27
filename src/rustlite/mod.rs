pub mod token;
pub mod lexer;
pub mod ast;
pub mod parser;
pub mod typecheck;
pub mod codegen;
pub mod loader;

pub fn compile(source: &str) -> Result<Vec<u8>, CompileError> {
    let tokens = lexer::lex(source)?;
    let module = parser::parse(&tokens)?;
    let typed = typecheck::check(&module)?;
    let wasm = codegen::emit(&typed)?;
    Ok(wasm)
}

#[derive(Debug, Clone)]
pub struct CompileError {
    pub message: String,
    pub span: Option<Span>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub start: usize,
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
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), span: None }
    }
    pub fn at(message: impl Into<String>, span: Span) -> Self {
        Self { message: message.into(), span: Some(span) }
    }
}

impl From<String> for CompileError {
    fn from(s: String) -> Self { Self::new(s) }
}
