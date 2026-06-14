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
//! Grammar (design §3 floor + the storage / mapping / param / msg.sender stretch):
//! ```text
//! facet  := ("facet"|"contract") Ident "{" state_var* function+ "}"
//! state_var := Ty Ident ";"                              (scalar slot)
//!            | "mapping" "(" Ty "=>" Ty ")" Ident ";"    (mapping)
//! params := "(" ( Ty Ident ("," Ty Ident)* )? ")"
//! function := "function" Ident params "external"
//!             ( view? "returns" "(" Ty ")" "{" "return" expr ";" "}"   (view getter)
//!             | "{" assign* "}" )                        (mutating)
//! assign := Ident ("[" expr "]")? "=" expr ";"           (scalar / mapping write)
//! expr   := term ("+" term)*                             (left-assoc +)
//! term   := IntLit | "msg" "." "sender" | Ident ("[" expr "]")?
//!           (Ident = scalar/param read; Ident[expr] = mapping read)
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
                SolKind::Mapping => state_vars.push(self.parse_mapping_var()?),
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
        let kind = StateVarKind::Scalar(self.parse_ty()?);
        let name = self.expect_ident()?;
        self.expect(&SolKind::Semi, "`;`")?;
        Ok(StateVar { kind, name, span })
    }

    /// Parse a mapping state var: `mapping ( <key> => <value> ) <name> ;`.
    fn parse_mapping_var(&mut self) -> Result<StateVar, CompileError> {
        let span = self.span();
        self.expect(&SolKind::Mapping, "`mapping`")?;
        self.expect(&SolKind::LParen, "`(`")?;
        let key = self.parse_ty()?;
        self.expect(&SolKind::FatArrow, "`=>`")?;
        let value = self.parse_ty()?;
        self.expect(&SolKind::RParen, "`)`")?;
        let name = self.expect_ident()?;
        self.expect(&SolKind::Semi, "`;`")?;
        Ok(StateVar { kind: StateVarKind::Mapping { key, value }, name, span })
    }

    /// Parse a function parameter list `( )` or `( <ty> <name> ("," <ty> <name>)* )`.
    /// Each parameter is a value type plus a name; trailing commas are rejected.
    fn parse_param_list(&mut self) -> Result<Vec<Param>, CompileError> {
        self.expect(&SolKind::LParen, "`(`")?;
        let mut params = Vec::new();
        if !matches!(self.peek(), SolKind::RParen) {
            loop {
                let span = self.span();
                let ty = self.parse_ty()?;
                let name = self.expect_ident()?;
                params.push(Param { ty, name, span });
                if matches!(self.peek(), SolKind::Comma) {
                    self.advance(); // `,`
                    continue;
                }
                break;
            }
        }
        self.expect(&SolKind::RParen, "`)`")?;
        Ok(params)
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
        // Parameter list `( <ty> <name> ("," <ty> <name>)* )` (possibly empty).
        let params = self.parse_param_list()?;
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
        // Two shapes: a view getter (`returns (Ty) { return e; }`) or a mutating
        // function (no `returns`, body is `{ assign* }` — the write stretch).
        if matches!(self.peek(), SolKind::Returns) {
            // `returns ( <ty> )` — the getter always returns one word.
            self.advance(); // `returns`
            self.expect(&SolKind::LParen, "`(`")?;
            let returns = self.parse_ty()?;
            self.expect(&SolKind::RParen, "`)`")?;
            // Body: `{ return <expr> ; }`.
            self.expect(&SolKind::LBrace, "`{`")?;
            let body = self.parse_return_stmt()?;
            self.expect(&SolKind::RBrace, "`}`")?;
            Ok(Function { name, params, mutability, returns: Some(returns), body, span })
        } else {
            // Mutating function: no `returns`, body is a block of assignments.
            let body = self.parse_mutating_block()?;
            Ok(Function { name, params, mutability, returns: None, body, span })
        }
    }

    fn parse_return_stmt(&mut self) -> Result<Stmt, CompileError> {
        self.expect(&SolKind::Return, "`return`")?;
        let expr = self.parse_expr()?;
        self.expect(&SolKind::Semi, "`;`")?;
        Ok(Stmt::Return(expr))
    }

    /// Parse a mutating function body `{ <assign>* }` into a [`Stmt::Block`]. Each
    /// statement is a state-var assignment (`<ident> = <expr> ;`); zero statements
    /// (an empty body) is allowed.
    fn parse_mutating_block(&mut self) -> Result<Stmt, CompileError> {
        self.enter()?;
        let result = self.parse_mutating_block_inner();
        self.leave();
        result
    }

    fn parse_mutating_block_inner(&mut self) -> Result<Stmt, CompileError> {
        self.expect(&SolKind::LBrace, "`{`")?;
        let mut stmts = Vec::new();
        while !matches!(self.peek(), SolKind::RBrace) {
            stmts.push(self.parse_assign_stmt()?);
        }
        self.expect(&SolKind::RBrace, "`}`")?;
        Ok(Stmt::Block(stmts))
    }

    /// Parse one assignment: `<stateVar> = <expr> ;` or `<mapping>[<key>] = <expr> ;`.
    /// The target must be a bare identifier (optionally followed by an `[<key>]`
    /// index) — a literal/expression on the left is an invalid assign target.
    fn parse_assign_stmt(&mut self) -> Result<Stmt, CompileError> {
        let span = self.span();
        let name = match self.peek().clone() {
            SolKind::Ident(name) => {
                self.advance();
                name
            }
            other => {
                return Err(CompileError::at_code(
                    codes::INVALID_ASSIGN_TARGET,
                    format!("assignment target must be a state variable, got {other:?}"),
                    span,
                ))
            }
        };
        // Optional index `[<key>]` for a mapping-entry assignment.
        let index_key = if matches!(self.peek(), SolKind::LBracket) {
            self.advance(); // `[`
            let key = self.parse_expr()?;
            self.expect(&SolKind::RBracket, "`]`")?;
            Some(key)
        } else {
            None
        };
        self.expect(&SolKind::Assign, "`=`")?;
        let value = self.parse_expr()?;
        self.expect(&SolKind::Semi, "`;`")?;
        match index_key {
            Some(key) => Ok(Stmt::IndexAssign { base: name, key, value, span }),
            None => Ok(Stmt::Assign { name, value, span }),
        }
    }

    /// Parse an expression: `term ("+" term)*`, left-associative (e.g. `n + 1`).
    fn parse_expr(&mut self) -> Result<Expr, CompileError> {
        self.enter()?;
        let result = self.parse_add();
        self.leave();
        result
    }

    /// `term ("+" term)*` — folds left so `a + b + c` parses as `(a + b) + c`.
    fn parse_add(&mut self) -> Result<Expr, CompileError> {
        let mut lhs = self.parse_primary()?;
        while matches!(self.peek(), SolKind::Plus) {
            let op_span = self.span();
            self.advance(); // `+`
            let rhs = self.parse_primary()?;
            lhs = Expr::Add { lhs: Box::new(lhs), rhs: Box::new(rhs), span: op_span };
        }
        Ok(lhs)
    }

    fn parse_primary(&mut self) -> Result<Expr, CompileError> {
        match self.peek().clone() {
            SolKind::Int(word) => {
                let span = self.span();
                self.advance();
                Ok(Expr::IntLit { value_be32: word, span })
            }
            // An identifier is `msg.sender`, a `<mapping>[<key>]` index, a bare
            // state-variable read, or a bare parameter reference (the last two are
            // both `Expr::StateVar`, disambiguated at codegen).
            SolKind::Ident(name) => {
                let span = self.span();
                self.advance();
                // `msg.sender`: `msg` is a plain identifier, then `.` `sender`.
                if name == "msg" && matches!(self.peek(), SolKind::Dot) {
                    self.advance(); // `.`
                    let member = self.expect_ident()?;
                    if member != "sender" {
                        return Err(CompileError::at_code(
                            codes::UNSUPPORTED_FEATURE,
                            format!("only `msg.sender` is supported, got `msg.{member}`"),
                            span,
                        ));
                    }
                    return Ok(Expr::MsgSender { span });
                }
                // `<mapping>[<key>]` index read.
                if matches!(self.peek(), SolKind::LBracket) {
                    self.advance(); // `[`
                    let key = self.parse_expr()?;
                    self.expect(&SolKind::RBracket, "`]`")?;
                    return Ok(Expr::Index { base: name, key: Box::new(key), span });
                }
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
    fn function_parameters_parse() {
        // Parameters are now supported: one `uint256 x` arg, referenced in the body.
        let f = parse_src(
            "facet C { function get(uint256 x) external view returns (uint256) { return x; } }",
        )
        .unwrap();
        let func = &f.functions[0];
        assert_eq!(func.params.len(), 1);
        assert_eq!(func.params[0].name, "x");
        assert_eq!(func.params[0].ty, Ty::Uint256);
        // The bare `x` in the body is a (param) StateVar reference.
        match &func.body {
            Stmt::Return(Expr::StateVar { name, .. }) => assert_eq!(name, "x"),
            other => panic!("unexpected body {other:?}"),
        }
    }

    #[test]
    fn multiple_parameters_parse_comma_separated() {
        let f = parse_src(
            "facet C { function f(uint256 a, address b) external { } }",
        )
        .unwrap();
        let params = &f.functions[0].params;
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "a");
        assert_eq!(params[0].ty, Ty::Uint256);
        assert_eq!(params[1].name, "b");
        assert_eq!(params[1].ty, Ty::Address);
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

    #[test]
    fn parses_the_tally_facet() {
        // A mutating `bump()` (assignment + `n + 1`) plus a view `get()`.
        let f = parse_src(
            "facet Tally { uint256 n; \
             function bump() external { n = n + 1; } \
             function get() external view returns (uint256) { return n; } }",
        )
        .unwrap();
        assert_eq!(f.state_vars.len(), 1);
        assert_eq!(f.state_vars[0].name, "n");
        assert_eq!(f.functions.len(), 2);

        // bump(): mutating (no `returns`), body is a single `n = n + 1;` assignment.
        let bump = &f.functions[0];
        assert_eq!(bump.name, "bump");
        assert_eq!(bump.mutability, Mutability::NonPayable);
        assert_eq!(bump.returns, None);
        match &bump.body {
            Stmt::Block(stmts) => {
                assert_eq!(stmts.len(), 1);
                match &stmts[0] {
                    Stmt::Assign { name, value, .. } => {
                        assert_eq!(name, "n");
                        // value is `n + 1`: Add(StateVar n, IntLit 1).
                        match value {
                            Expr::Add { lhs, rhs, .. } => {
                                assert!(matches!(**lhs, Expr::StateVar { .. }));
                                assert!(matches!(**rhs, Expr::IntLit { .. }));
                            }
                            other => panic!("expected an Add, got {other:?}"),
                        }
                    }
                    other => panic!("expected an assignment, got {other:?}"),
                }
            }
            other => panic!("expected a block body, got {other:?}"),
        }

        // get(): a view getter returning the state var.
        let get = &f.functions[1];
        assert_eq!(get.name, "get");
        assert_eq!(get.mutability, Mutability::View);
        assert_eq!(get.returns, Some(Ty::Uint256));
        assert!(matches!(get.body, Stmt::Return(Expr::StateVar { .. })));
    }

    #[test]
    fn empty_mutating_body_is_allowed() {
        let f = parse_src("facet E { function noop() external { } }").unwrap();
        assert_eq!(f.functions[0].returns, None);
        match &f.functions[0].body {
            Stmt::Block(stmts) => assert!(stmts.is_empty()),
            other => panic!("expected an empty block, got {other:?}"),
        }
    }

    #[test]
    fn left_associative_addition_parses() {
        // `a + b + c` folds left: Add(Add(a, b), c).
        let f = parse_src(
            "facet A { uint256 a; uint256 b; uint256 c; \
             function set() external { a = a + b + c; } }",
        )
        .unwrap();
        match &f.functions[0].body {
            Stmt::Block(stmts) => match &stmts[0] {
                Stmt::Assign { value: Expr::Add { lhs, rhs, .. }, .. } => {
                    // top-level rhs is `c`, lhs is `a + b`.
                    assert!(matches!(**rhs, Expr::StateVar { .. }));
                    assert!(matches!(**lhs, Expr::Add { .. }));
                }
                other => panic!("unexpected stmt {other:?}"),
            },
            other => panic!("unexpected body {other:?}"),
        }
    }

    #[test]
    fn assignment_to_a_literal_is_a_clean_error() {
        let e = parse_src("facet C { function f() external { 1 = 2; } }").unwrap_err();
        assert_eq!(e.code, Some(codes::INVALID_ASSIGN_TARGET));
    }

    #[test]
    fn missing_semicolon_in_assignment_is_a_clean_error() {
        let e = parse_src("facet C { function f() external { n = 1 } }").unwrap_err();
        assert_eq!(e.code, Some(codes::UNEXPECTED_TOKEN));
    }

    #[test]
    fn mapping_state_var_parses() {
        let f = parse_src(
            "facet L { mapping(address => uint256) bal; \
             function get() external view returns (uint256) { return 0; } }",
        )
        .unwrap();
        assert_eq!(f.state_vars.len(), 1);
        assert_eq!(f.state_vars[0].name, "bal");
        match &f.state_vars[0].kind {
            StateVarKind::Mapping { key, value } => {
                assert_eq!(*key, Ty::Address);
                assert_eq!(*value, Ty::Uint256);
            }
            other => panic!("expected a mapping, got {other:?}"),
        }
    }

    #[test]
    fn index_expr_and_msg_sender_parse() {
        // `bal[msg.sender]` as a return value: an Index whose key is MsgSender.
        let f = parse_src(
            "facet L { mapping(address => uint256) bal; \
             function mine() external view returns (uint256) { return bal[msg.sender]; } }",
        )
        .unwrap();
        match &f.functions[0].body {
            Stmt::Return(Expr::Index { base, key, .. }) => {
                assert_eq!(base, "bal");
                assert!(matches!(**key, Expr::MsgSender { .. }));
            }
            other => panic!("unexpected body {other:?}"),
        }
    }

    #[test]
    fn index_assignment_parses() {
        // `bal[msg.sender] = bal[msg.sender] + amt;` is an IndexAssign.
        let f = parse_src(
            "facet L { mapping(address => uint256) bal; \
             function add(uint256 amt) external { bal[msg.sender] = bal[msg.sender] + amt; } }",
        )
        .unwrap();
        match &f.functions[0].body {
            Stmt::Block(stmts) => match &stmts[0] {
                Stmt::IndexAssign { base, key, value, .. } => {
                    assert_eq!(base, "bal");
                    assert!(matches!(*key, Expr::MsgSender { .. }));
                    // value is `bal[msg.sender] + amt`: Add(Index, StateVar amt).
                    match value {
                        Expr::Add { lhs, rhs, .. } => {
                            assert!(matches!(**lhs, Expr::Index { .. }));
                            assert!(matches!(**rhs, Expr::StateVar { .. }));
                        }
                        other => panic!("expected an Add, got {other:?}"),
                    }
                }
                other => panic!("expected an IndexAssign, got {other:?}"),
            },
            other => panic!("unexpected body {other:?}"),
        }
    }

    #[test]
    fn bare_msg_member_other_than_sender_is_rejected() {
        let e = parse_src(
            "facet C { function f() external view returns (uint256) { return msg.value; } }",
        )
        .unwrap_err();
        assert_eq!(e.code, Some(codes::UNSUPPORTED_FEATURE));
    }

    #[test]
    fn parameter_without_a_name_is_a_clean_error() {
        // `function f(uint256) external` — a type with no parameter name. The param
        // parser expects an identifier after the type; this is a clean error, not a
        // panic (covers malformed/wrong-arity parameter lists).
        let e = parse_src("facet C { function f(uint256) external { } }").unwrap_err();
        assert_eq!(e.code, Some(codes::UNEXPECTED_TOKEN));
    }

    #[test]
    fn trailing_comma_in_param_list_is_a_clean_error() {
        let e = parse_src("facet C { function f(uint256 a,) external { } }").unwrap_err();
        // After the comma the parser expects a type, but sees `)`.
        assert_eq!(e.code, Some(codes::EXPECTED_TYPE));
    }
}
