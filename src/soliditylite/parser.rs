//! SolidityLite parser — a [`SolTok`] stream → a [`Facet`] AST.
//!
//! Recursive-descent in the [`crate::rustlite::parser`] style, INCLUDING the
//! `MAX_RECURSION_DEPTH` guard (`enter`/`leave`): this compiler also runs in the
//! user's browser on agent/LLM-authored source, so deeply-nested adversarial
//! input must return a `CompileError` rather than overflow the wasm stack (an
//! uncatchable tab-killing abort). The floor grammar is shallow, but the guard
//! is wired now so the expression/statement layers added later inherit it for
//! free.
//!
//! Floor grammar (design §3):
//! ```text
//! facet  := ("facet"|"contract") Ident "{" state_var* function+ "}"
//! state_var := Ty Ident ";"                              (storage stretch)
//! function := "function" Ident "(" ")" "external" view? "returns" "(" Ty ")"
//!             "{" "return" expr ";" "}"
//! expr   := IntLit | Ident                               (Ident = storage stretch)
//! ```

use crate::error_codes as codes;
use crate::rustlite::{CompileError, Span};
use crate::soliditylite::ast::*;
use crate::soliditylite::lexer::{SolKind, SolTok};

/// Hard cap on recursive-descent nesting depth — same rationale as
/// [`crate::rustlite`]'s guard (browser-tab stack-overflow on adversarial input).
const MAX_RECURSION_DEPTH: usize = 96;

/// Parse a token stream into a single [`Facet`].
pub fn parse(tokens: &[SolTok]) -> Result<Facet, CompileError> {
    let mut p = Parser { tokens, pos: 0, depth: 0 };
    let facet = p.parse_facet()?;
    // Reject trailing tokens after the facet (a second top-level item, etc.).
    if !matches!(p.peek(), SolKind::Eof) {
        return Err(CompileError::at_code(
            codes::EXPECTED_ITEM,
            "only a single top-level `facet` is supported in v1".to_string(),
            p.span(),
        ));
    }
    Ok(facet)
}

struct Parser<'a> {
    tokens: &'a [SolTok],
    pos: usize,
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> &SolKind {
        // The stream always ends with Eof, so indexing is safe; clamp defensively.
        &self.tokens[self.pos.min(self.tokens.len() - 1)].kind
    }

    fn span(&self) -> Span {
        self.tokens[self.pos.min(self.tokens.len() - 1)].span
    }

    fn advance(&mut self) -> &SolTok {
        let tok = &self.tokens[self.pos.min(self.tokens.len() - 1)];
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    /// Enter one recursion level; error past the cap instead of overflowing.
    fn enter(&mut self) -> Result<(), CompileError> {
        self.depth += 1;
        if self.depth > MAX_RECURSION_DEPTH {
            return Err(CompileError::at_code(
                codes::NESTING_TOO_DEEP,
                "nesting too deep".to_string(),
                self.span(),
            ));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Consume a token of the expected kind (by discriminant) or error.
    fn expect(&mut self, want: &SolKind, what: &str) -> Result<SolTok, CompileError> {
        if std::mem::discriminant(self.peek()) == std::mem::discriminant(want) {
            Ok(self.advance().clone())
        } else {
            Err(CompileError::at_code(
                codes::UNEXPECTED_TOKEN,
                format!("expected {what}, got {:?}", self.peek()),
                self.span(),
            ))
        }
    }

    fn expect_ident(&mut self) -> Result<String, CompileError> {
        match self.peek().clone() {
            SolKind::Ident(name) => {
                self.advance();
                Ok(name)
            }
            other => Err(CompileError::at_code(
                codes::UNEXPECTED_TOKEN,
                format!("expected an identifier, got {other:?}"),
                self.span(),
            )),
        }
    }

    fn parse_facet(&mut self) -> Result<Facet, CompileError> {
        let facet_span = self.span();
        self.expect(&SolKind::Facet, "`facet`")?;
        let name = self.expect_ident()?;
        self.expect(&SolKind::LBrace, "`{`")?;

        let mut state_vars = Vec::new();
        let mut functions = Vec::new();
        // Members: state vars (TypeName-led) then functions (`function`-led), in any
        // interleaving the floor grammar+stretch admit.
        loop {
            match self.peek() {
                SolKind::Function => functions.push(self.parse_function()?),
                SolKind::TypeName(_) => state_vars.push(self.parse_state_var()?),
                SolKind::RBrace => break,
                other => {
                    return Err(CompileError::at_code(
                        codes::UNEXPECTED_TOKEN,
                        format!("expected `function`, a state-var type, or `}}`, got {other:?}"),
                        self.span(),
                    ))
                }
            }
        }
        self.expect(&SolKind::RBrace, "`}`")?;

        if functions.is_empty() {
            return Err(CompileError::at_code(
                codes::EXPECTED_ITEM,
                "a facet must declare at least one function".to_string(),
                facet_span,
            ));
        }
        Ok(Facet { name, state_vars, functions, span: facet_span })
    }

    fn parse_ty(&mut self) -> Result<Ty, CompileError> {
        match self.peek().clone() {
            SolKind::TypeName(name) => {
                let span = self.span();
                self.advance();
                match name.as_str() {
                    "uint256" => Ok(Ty::Uint256),
                    // The non-uint256 value types parse but are not yet codegen-
                    // supported in the floor grammar; surface a precise error.
                    "address" => Ok(Ty::Address),
                    "bool" => Ok(Ty::Bool),
                    "bytes32" => Ok(Ty::Bytes32),
                    _ => Err(CompileError::at_code(codes::UNKNOWN_TYPE, format!("unknown type `{name}`"), span)),
                }
            }
            other => Err(CompileError::at_code(
                codes::EXPECTED_TYPE,
                format!("expected a type, got {other:?}"),
                self.span(),
            )),
        }
    }

    fn parse_state_var(&mut self) -> Result<StateVar, CompileError> {
        let span = self.span();
        let ty = self.parse_ty()?;
        let name = self.expect_ident()?;
        self.expect(&SolKind::Semi, "`;`")?;
        Ok(StateVar { ty, name, span })
    }

    fn parse_function(&mut self) -> Result<Function, CompileError> {
        self.enter()?;
        let result = self.parse_function_inner();
        self.leave();
        result
    }

    fn parse_function_inner(&mut self) -> Result<Function, CompileError> {
        let span = self.span();
        self.expect(&SolKind::Function, "`function`")?;
        let name = self.expect_ident()?;
        // Empty parameter list `( )` — the floor grammar.
        self.expect(&SolKind::LParen, "`(`")?;
        self.expect(&SolKind::RParen, "`)` (v1 functions take no parameters)")?;
        // `external` is required by the floor grammar.
        self.expect(&SolKind::External, "`external`")?;
        // Optional mutability (`view`/`pure`).
        let mutability = match self.peek() {
            SolKind::View => {
                self.advance();
                Mutability::View
            }
            SolKind::Pure => {
                self.advance();
                Mutability::Pure
            }
            _ => Mutability::NonPayable,
        };
        // `returns ( <ty> )` — the floor grammar always returns one word.
        self.expect(&SolKind::Returns, "`returns`")?;
        self.expect(&SolKind::LParen, "`(`")?;
        let returns = self.parse_ty()?;
        self.expect(&SolKind::RParen, "`)`")?;
        // Body: `{ return <expr> ; }`.
        self.expect(&SolKind::LBrace, "`{`")?;
        let body = self.parse_return_stmt()?;
        self.expect(&SolKind::RBrace, "`}`")?;
        Ok(Function { name, mutability, returns: Some(returns), body, span })
    }

    fn parse_return_stmt(&mut self) -> Result<Stmt, CompileError> {
        self.expect(&SolKind::Return, "`return`")?;
        let expr = self.parse_expr()?;
        self.expect(&SolKind::Semi, "`;`")?;
        Ok(Stmt::Return(expr))
    }

    fn parse_expr(&mut self) -> Result<Expr, CompileError> {
        self.enter()?;
        let result = self.parse_primary();
        self.leave();
        result
    }

    fn parse_primary(&mut self) -> Result<Expr, CompileError> {
        match self.peek().clone() {
            SolKind::Int(word) => {
                let span = self.span();
                self.advance();
                Ok(Expr::IntLit { value_be32: word, span })
            }
            // A bare identifier is a state-variable read (storage stretch).
            SolKind::Ident(name) => {
                let span = self.span();
                self.advance();
                Ok(Expr::StateVar { name, span })
            }
            other => Err(CompileError::at_code(
                codes::EXPECTED_EXPRESSION,
                format!("expected an integer literal or a state variable, got {other:?}"),
                self.span(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soliditylite::lexer::lex;

    fn parse_src(src: &str) -> Result<Facet, CompileError> {
        let toks = lex(src)?;
        parse(&toks)
    }

    #[test]
    fn parses_the_floor_grammar() {
        let f = parse_src(
            "facet C { function get() external view returns (uint256) { return 42; } }",
        )
        .unwrap();
        assert_eq!(f.name, "C");
        assert_eq!(f.functions.len(), 1);
        let func = &f.functions[0];
        assert_eq!(func.name, "get");
        assert_eq!(func.mutability, Mutability::View);
        assert_eq!(func.returns, Some(Ty::Uint256));
        match &func.body {
            Stmt::Return(Expr::IntLit { value_be32, .. }) => {
                let mut w = [0u8; 32];
                w[31] = 42;
                assert_eq!(value_be32, &w);
            }
            other => panic!("unexpected body {other:?}"),
        }
    }

    #[test]
    fn parses_two_functions() {
        let f = parse_src(
            "facet Two { function a() external view returns (uint256) { return 1; } \
             function b() external view returns (uint256) { return 2; } }",
        )
        .unwrap();
        assert_eq!(f.functions.len(), 2);
        assert_eq!(f.functions[0].name, "a");
        assert_eq!(f.functions[1].name, "b");
    }

    #[test]
    fn empty_facet_is_rejected() {
        let e = parse_src("facet C { }").unwrap_err();
        assert_eq!(e.code, Some(codes::EXPECTED_ITEM));
    }

    #[test]
    fn missing_semicolon_is_a_clean_error() {
        let e = parse_src(
            "facet C { function get() external view returns (uint256) { return 42 } }",
        )
        .unwrap_err();
        assert_eq!(e.code, Some(codes::UNEXPECTED_TOKEN));
    }

    #[test]
    fn parameters_are_rejected_in_v1() {
        let e = parse_src(
            "facet C { function get(uint256 x) external view returns (uint256) { return 42; } }",
        )
        .unwrap_err();
        assert_eq!(e.code, Some(codes::UNEXPECTED_TOKEN));
    }

    #[test]
    fn trailing_item_after_facet_is_rejected() {
        let e = parse_src(
            "facet C { function get() external view returns (uint256) { return 1; } } facet D {}",
        )
        .unwrap_err();
        assert_eq!(e.code, Some(codes::EXPECTED_ITEM));
    }

    #[test]
    fn recursion_guard_errors_past_the_cap() {
        // Drive the guard directly: entering `MAX_RECURSION_DEPTH` levels is fine,
        // the next `enter` must return NESTING_TOO_DEEP instead of recursing (the
        // floor grammar's flat exprs can't naturally nest this deep, so the guard
        // is exercised at the mechanism level — same as rustlite's guard unit test).
        let toks = lex("facet C { function get() external view returns (uint256) { return 1; } }")
            .unwrap();
        let mut p = Parser { tokens: &toks, pos: 0, depth: 0 };
        for _ in 0..MAX_RECURSION_DEPTH {
            p.enter().expect("entering up to the cap is allowed");
        }
        let e = p.enter().expect_err("one past the cap must error, not overflow");
        assert_eq!(e.code, Some(codes::NESTING_TOO_DEEP));
        // And `leave` unwinds without underflowing past zero.
        for _ in 0..(MAX_RECURSION_DEPTH + 10) {
            p.leave();
        }
        assert_eq!(p.depth, 0);
    }

    #[test]
    fn parses_a_state_var_then_function() {
        let f = parse_src(
            "facet S { uint256 total; function get() external view returns (uint256) { return total; } }",
        )
        .unwrap();
        assert_eq!(f.state_vars.len(), 1);
        assert_eq!(f.state_vars[0].name, "total");
        match &f.functions[0].body {
            Stmt::Return(Expr::StateVar { name, .. }) => assert_eq!(name, "total"),
            other => panic!("unexpected body {other:?}"),
        }
    }
}
