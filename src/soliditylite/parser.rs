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
//! facet  := ("facet"|"contract") Ident "{" (state_var|event_decl)* function+ "}"
//! state_var := Ty Ident ";"                              (scalar slot)
//!            | "mapping" "(" Ty "=>" Ty ")" Ident ";"    (mapping)
//! event_decl := "event" Ident "(" ( Ty "indexed"? Ident
//!                 ("," Ty "indexed"? Ident)* )? ")" ";"  (log signature)
//! params := "(" ( Ty Ident ("," Ty Ident)* )? ")"
//! function := "function" Ident params "external"
//!             ( view? "returns" "(" Ty ")" "{" "return" expr ";" "}"   (view getter)
//!             | "{" stmt* "}" )                          (mutating)
//! stmt   := "require" "(" expr "," StrLit ")" ";"        (guard / revert)
//!         | "emit" Ident "(" ( expr ("," expr)* )? ")" ";"  (LOGn)
//!         | Ident ("[" expr "]")? ("=" | "+=") expr ";"  (scalar / mapping write)
//! expr   := cmp                                          (top level)
//! cmp    := add ( ("<"|">"|"<="|">="|"==") add )?        (non-assoc comparison)
//! add    := term ("+" term)*                             (left-assoc +, binds tighter)
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

    /// Peek `offset` tokens ahead (clamped to the trailing `Eof`). Used for the
    /// one-token lookahead that distinguishes `require(` from a bare `require` ref.
    fn peek_at(&self, offset: usize) -> &SolKind {
        &self.tokens[(self.pos + offset).min(self.tokens.len() - 1)].kind
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
        let mut events = Vec::new();
        let mut functions = Vec::new();
        // Members: state vars (TypeName-led), event decls (contextual `event`),
        // then functions (`function`-led), in any interleaving the grammar admits.
        // `event` is a CONTEXTUAL keyword (a plain identifier elsewhere): it only
        // leads an event declaration when followed by `<Name> (` at facet top-level.
        loop {
            match self.peek() {
                SolKind::Function => functions.push(self.parse_function()?),
                SolKind::Mapping => state_vars.push(self.parse_mapping_var()?),
                SolKind::TypeName(_) => state_vars.push(self.parse_state_var()?),
                SolKind::Ident(name)
                    if name == "event"
                        && matches!(self.peek_at(1), SolKind::Ident(_))
                        && matches!(self.peek_at(2), SolKind::LParen) =>
                {
                    events.push(self.parse_event_decl()?)
                }
                SolKind::RBrace => break,
                other => {
                    return Err(CompileError::at_code(
                        codes::UNEXPECTED_TOKEN,
                        format!("expected `function`, a state-var type, an `event`, or `}}`, got {other:?}"),
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
        Ok(Facet { name, state_vars, events, functions, span: facet_span })
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

    /// Parse an event declaration: `event <Name> ( <ty> [indexed] <name>
    /// ("," <ty> [indexed] <name>)* ) ;`. `event` (the contextual keyword) and the
    /// leading `<Name> (` are already confirmed by the caller's lookahead.
    fn parse_event_decl(&mut self) -> Result<EventDecl, CompileError> {
        let span = self.span();
        self.advance(); // `event` (the contextual keyword)
        let name = self.expect_ident()?;
        self.expect(&SolKind::LParen, "`(`")?;
        let mut args = Vec::new();
        if !matches!(self.peek(), SolKind::RParen) {
            loop {
                let arg_span = self.span();
                let ty = self.parse_ty()?;
                // Optional `indexed` modifier (a contextual keyword) before the name.
                let indexed = matches!(self.peek(), SolKind::Ident(kw) if kw == "indexed");
                if indexed {
                    self.advance(); // `indexed`
                }
                let arg_name = self.expect_ident()?;
                args.push(EventArg { ty, indexed, name: arg_name, span: arg_span });
                if matches!(self.peek(), SolKind::Comma) {
                    self.advance(); // `,`
                    continue;
                }
                break;
            }
        }
        self.expect(&SolKind::RParen, "`)`")?;
        self.expect(&SolKind::Semi, "`;`")?;
        Ok(EventDecl { name, args, span })
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
            // `string` is recognized ONLY in the return position (it lexes as a
            // plain identifier — never `parse_ty`, so it can't appear in a param,
            // state var, or event arg). v1 supports a constant string-literal body.
            let returns = if matches!(self.peek(), SolKind::Ident(name) if name == "string") {
                self.advance();
                Ty::String
            } else {
                self.parse_ty()?
            };
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
        Ok(Stmt::Block(self.parse_brace_block()?))
    }

    /// Parse a `{ <stmt>* }` brace block into its statement list (reused by the
    /// function body and by both `if`/`else` branches). The braces are required.
    fn parse_brace_block(&mut self) -> Result<Vec<Stmt>, CompileError> {
        self.expect(&SolKind::LBrace, "`{`")?;
        let mut stmts = Vec::new();
        while !matches!(self.peek(), SolKind::RBrace) {
            stmts.push(self.parse_stmt()?);
        }
        self.expect(&SolKind::RBrace, "`}`")?;
        Ok(stmts)
    }

    /// Parse `if ( <cond> ) { <stmt>* } [ else ( { <stmt>* } | <if> ) ]`. `if` and
    /// `else` are contextual identifiers (reserved nowhere else in the v1 grammar).
    /// An `else if` chains by recursing, so the else branch is a single nested `If`.
    /// `enter`/`leave` bound nesting depth so deeply-nested `if`s error cleanly.
    fn parse_if_stmt(&mut self) -> Result<Stmt, CompileError> {
        self.enter()?;
        let span = self.span();
        self.advance(); // `if`
        self.expect(&SolKind::LParen, "`(`")?;
        let cond = self.parse_expr()?;
        self.expect(&SolKind::RParen, "`)`")?;
        let then_body = self.parse_brace_block()?;
        let else_body = if matches!(self.peek(), SolKind::Ident(name) if name == "else") {
            self.advance(); // `else`
            if matches!(self.peek(), SolKind::Ident(name) if name == "if")
                && matches!(self.peek_at(1), SolKind::LParen)
            {
                vec![self.parse_if_stmt()?] // `else if` → nested If
            } else {
                self.parse_brace_block()?
            }
        } else {
            Vec::new()
        };
        self.leave();
        Ok(Stmt::If { cond, then_body, else_body, span })
    }

    /// Parse one statement inside a mutating body: a `require(...)` guard or an
    /// assignment. `require` is a contextual keyword (a plain identifier elsewhere),
    /// recognized only when it leads a statement and is followed by `(`.
    fn parse_stmt(&mut self) -> Result<Stmt, CompileError> {
        // `if` is contextual (a plain identifier elsewhere): it leads an `if`
        // statement only when followed by `(`.
        if matches!(self.peek(), SolKind::Ident(name) if name == "if")
            && matches!(self.peek_at(1), SolKind::LParen)
        {
            return self.parse_if_stmt();
        }
        if matches!(self.peek(), SolKind::Ident(name) if name == "require")
            && matches!(self.peek_at(1), SolKind::LParen)
        {
            return self.parse_require_stmt();
        }
        // `emit` is contextual (a plain identifier elsewhere): it leads an emit
        // statement only when followed by `<Name> (`.
        if matches!(self.peek(), SolKind::Ident(name) if name == "emit")
            && matches!(self.peek_at(1), SolKind::Ident(_))
            && matches!(self.peek_at(2), SolKind::LParen)
        {
            return self.parse_emit_stmt();
        }
        self.parse_assign_stmt()
    }

    /// Parse `emit <Name> ( <expr> ("," <expr>)* ) ;`. The argument count is checked
    /// against the event declaration at codegen (the parser doesn't know the
    /// declared events). Each argument is an expression in the same lattice as a
    /// `return`/assignment value.
    fn parse_emit_stmt(&mut self) -> Result<Stmt, CompileError> {
        let span = self.span();
        self.advance(); // `emit` (the contextual keyword)
        let name = self.expect_ident()?;
        self.expect(&SolKind::LParen, "`(`")?;
        let mut args = Vec::new();
        if !matches!(self.peek(), SolKind::RParen) {
            loop {
                args.push(self.parse_expr()?);
                if matches!(self.peek(), SolKind::Comma) {
                    self.advance(); // `,`
                    continue;
                }
                break;
            }
        }
        self.expect(&SolKind::RParen, "`)`")?;
        self.expect(&SolKind::Semi, "`;`")?;
        Ok(Stmt::Emit { name, args, span })
    }

    /// Parse `require ( <cond> , <strlit> ) ;`. The condition can be any expression
    /// (typically a comparison). The message string is required by the floor grammar
    /// but DISCARDED by codegen (an empty-data revert aborts the call regardless).
    fn parse_require_stmt(&mut self) -> Result<Stmt, CompileError> {
        let span = self.span();
        self.advance(); // `require` (the contextual keyword)
        self.expect(&SolKind::LParen, "`(`")?;
        let cond = self.parse_expr()?;
        self.expect(&SolKind::Comma, "`,`")?;
        // The message: a string literal (consumed, then ignored by codegen).
        match self.peek().clone() {
            SolKind::Str(_) => {
                self.advance();
            }
            other => {
                return Err(CompileError::at_code(
                    codes::EXPECTED_EXPRESSION,
                    format!("expected a string message in `require(cond, \"…\")`, got {other:?}"),
                    self.span(),
                ))
            }
        }
        self.expect(&SolKind::RParen, "`)`")?;
        self.expect(&SolKind::Semi, "`;`")?;
        Ok(Stmt::Require { cond, span })
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
        // Either `=` (plain assign) or `+=` (compound, lexed as `+` then `=`). The
        // compound form desugars `t += e` to `t = t + e`, reusing the target as a
        // read of itself (`x += e` → `x = x + e`; `m[k] += e` → `m[k] = m[k] + e`).
        let compound = matches!(self.peek(), SolKind::Plus)
            && matches!(self.peek_at(1), SolKind::Assign);
        if compound {
            self.advance(); // `+`
            self.advance(); // `=`
        } else {
            self.expect(&SolKind::Assign, "`=` or `+=`")?;
        }
        let rhs = self.parse_expr()?;
        self.expect(&SolKind::Semi, "`;`")?;
        let value = if compound {
            // Desugar: build `<target_read> + <rhs>`.
            let target_read = match &index_key {
                Some(key) => Expr::Index { base: name.clone(), key: Box::new(key.clone()), span },
                None => Expr::StateVar { name: name.clone(), span },
            };
            Expr::Add { lhs: Box::new(target_read), rhs: Box::new(rhs), span }
        } else {
            rhs
        };
        match index_key {
            Some(key) => Ok(Stmt::IndexAssign { base: name, key, value, span }),
            None => Ok(Stmt::Assign { name, value, span }),
        }
    }

    /// Parse an expression. The top level is a comparison (binds loosest), which in
    /// turn parses additions (bind tighter), which parse terms.
    fn parse_expr(&mut self) -> Result<Expr, CompileError> {
        self.enter()?;
        let result = self.parse_cmp();
        self.leave();
        result
    }

    /// `add ( <cmp_op> add )?` — a single, NON-associative comparison (`a > b`).
    /// Comparisons bind LOOSER than `+`, so `n + 1 > 0` is `(n + 1) > 0`. Chaining
    /// (`a < b < c`) is not part of the grammar; a second comparison operator is a
    /// clean error at the enclosing `;`/`)` rather than silently mis-associating.
    fn parse_cmp(&mut self) -> Result<Expr, CompileError> {
        let lhs = self.parse_add()?;
        let op = match self.peek() {
            SolKind::Gt => CmpOp::Gt,
            SolKind::Lt => CmpOp::Lt,
            SolKind::Ge => CmpOp::Ge,
            SolKind::Le => CmpOp::Le,
            SolKind::EqEq => CmpOp::Eq,
            SolKind::BangEq => CmpOp::Neq,
            _ => return Ok(lhs),
        };
        let op_span = self.span();
        self.advance(); // the comparison operator
        let rhs = self.parse_add()?;
        Ok(Expr::Cmp { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span: op_span })
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
            // A string literal — valid only as a whole `return "…";` (enforced at
            // codegen); the decoded UTF-8 bytes ride along for `Body::ConstString`.
            SolKind::Str(s) => {
                let span = self.span();
                self.advance();
                Ok(Expr::StrLit { value: s.into_bytes(), span })
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

    #[test]
    fn comparison_parses_below_addition() {
        // `n + 1 > 0` parses as `(n + 1) > 0`: a Cmp whose lhs is an Add.
        let f = parse_src(
            "facet C { uint256 n; function f() external view returns (uint256) { return n + 1 > 0; } }",
        )
        .unwrap();
        match &f.functions[0].body {
            Stmt::Return(Expr::Cmp { op, lhs, rhs, .. }) => {
                assert_eq!(*op, CmpOp::Gt);
                assert!(matches!(**lhs, Expr::Add { .. }), "lhs must be the `n + 1` Add");
                assert!(matches!(**rhs, Expr::IntLit { .. }), "rhs is the literal 0");
            }
            other => panic!("expected a Cmp(Add, IntLit), got {other:?}"),
        }
    }

    #[test]
    fn all_comparison_operators_parse() {
        for (src_op, want) in [
            (">", CmpOp::Gt),
            ("<", CmpOp::Lt),
            (">=", CmpOp::Ge),
            ("<=", CmpOp::Le),
            ("==", CmpOp::Eq),
        ] {
            let src = format!(
                "facet C {{ function f(uint256 a) external view returns (uint256) {{ return a {src_op} 1; }} }}"
            );
            let f = parse_src(&src).unwrap();
            match &f.functions[0].body {
                Stmt::Return(Expr::Cmp { op, .. }) => assert_eq!(*op, want, "for `{src_op}`"),
                other => panic!("`{src_op}` did not parse to a Cmp: {other:?}"),
            }
        }
    }

    #[test]
    fn require_statement_parses() {
        let f = parse_src(
            "facet C { function f(uint256 n) external { require(n > 0, \"zero\"); } }",
        )
        .unwrap();
        match &f.functions[0].body {
            Stmt::Block(stmts) => {
                assert_eq!(stmts.len(), 1);
                match &stmts[0] {
                    Stmt::Require { cond: Expr::Cmp { op, .. }, .. } => assert_eq!(*op, CmpOp::Gt),
                    other => panic!("expected a Require(Cmp), got {other:?}"),
                }
            }
            other => panic!("unexpected body {other:?}"),
        }
    }

    #[test]
    fn require_then_assignment_parses_in_order() {
        // Two requires followed by a mapping write — the CounterFacet incrementBy shape.
        let f = parse_src(
            "facet C { mapping(address => uint256) count; uint256 total; \
             function incrementBy(uint256 n) external { require(n > 0, \"zero\"); \
             require(n <= 100, \"too big\"); count[msg.sender] = count[msg.sender] + n; \
             total = total + n; } }",
        )
        .unwrap();
        match &f.functions[0].body {
            Stmt::Block(stmts) => {
                assert_eq!(stmts.len(), 4);
                assert!(matches!(stmts[0], Stmt::Require { .. }));
                assert!(matches!(stmts[1], Stmt::Require { .. }));
                assert!(matches!(stmts[2], Stmt::IndexAssign { .. }));
                assert!(matches!(stmts[3], Stmt::Assign { .. }));
            }
            other => panic!("unexpected body {other:?}"),
        }
    }

    #[test]
    fn require_without_a_message_is_a_clean_error() {
        // `require(n > 0)` — missing the message operand. A clean error, not a panic.
        let e = parse_src("facet C { function f(uint256 n) external { require(n > 0); } }")
            .unwrap_err();
        assert!(e.code.is_some(), "must carry an LH code");
    }

    #[test]
    fn bad_comparison_rhs_is_a_clean_error() {
        // `n > ;` — a comparison with no right operand. Clean error, no panic.
        let e = parse_src("facet C { function f(uint256 n) external { require(n > , \"x\"); } }")
            .unwrap_err();
        assert_eq!(e.code, Some(codes::EXPECTED_EXPRESSION));
    }

    #[test]
    fn require_is_still_usable_as_a_plain_identifier() {
        // `require` is contextual: a bare `require` (not followed by `(`) is a normal
        // state-var reference, NOT the keyword. (Edge-case robustness.)
        let f = parse_src(
            "facet C { uint256 require; function f() external view returns (uint256) { return require; } }",
        )
        .unwrap();
        match &f.functions[0].body {
            Stmt::Return(Expr::StateVar { name, .. }) => assert_eq!(name, "require"),
            other => panic!("unexpected body {other:?}"),
        }
    }

    #[test]
    fn compound_plus_assign_desugars() {
        // `total += n` parses to `total = total + n`.
        let f = parse_src(
            "facet C { uint256 total; function f(uint256 n) external { total += n; } }",
        )
        .unwrap();
        match &f.functions[0].body {
            Stmt::Block(stmts) => match &stmts[0] {
                Stmt::Assign { name, value: Expr::Add { lhs, rhs, .. }, .. } => {
                    assert_eq!(name, "total");
                    // lhs is a read of `total`; rhs is `n`.
                    assert!(matches!(**lhs, Expr::StateVar { .. }));
                    assert!(matches!(**rhs, Expr::StateVar { .. }));
                }
                other => panic!("expected a desugared Assign(Add), got {other:?}"),
            },
            other => panic!("unexpected body {other:?}"),
        }
    }

    #[test]
    fn event_declaration_parses_with_indexed_and_data_args() {
        let f = parse_src(
            "facet C { event Incremented(address indexed who, uint256 newCount, uint256 newTotal); \
             function f() external { } }",
        )
        .unwrap();
        assert_eq!(f.events.len(), 1);
        let ev = &f.events[0];
        assert_eq!(ev.name, "Incremented");
        assert_eq!(ev.args.len(), 3);
        // who is indexed; the two counts are not.
        assert_eq!(ev.args[0].ty, Ty::Address);
        assert!(ev.args[0].indexed, "who is indexed");
        assert_eq!(ev.args[0].name, "who");
        assert_eq!(ev.args[1].ty, Ty::Uint256);
        assert!(!ev.args[1].indexed, "newCount is data");
        assert!(!ev.args[2].indexed, "newTotal is data");
    }

    #[test]
    fn event_with_no_args_parses() {
        let f = parse_src("facet C { event Pinged(); function f() external { } }").unwrap();
        assert_eq!(f.events.len(), 1);
        assert_eq!(f.events[0].name, "Pinged");
        assert!(f.events[0].args.is_empty());
    }

    #[test]
    fn emit_statement_parses() {
        let f = parse_src(
            "facet C { event E(address indexed a, uint256 b); \
             function f(uint256 n) external { emit E(msg.sender, n); } }",
        )
        .unwrap();
        match &f.functions[0].body {
            Stmt::Block(stmts) => match &stmts[0] {
                Stmt::Emit { name, args, .. } => {
                    assert_eq!(name, "E");
                    assert_eq!(args.len(), 2);
                    assert!(matches!(args[0], Expr::MsgSender { .. }));
                    assert!(matches!(args[1], Expr::StateVar { .. }));
                }
                other => panic!("expected an Emit, got {other:?}"),
            },
            other => panic!("unexpected body {other:?}"),
        }
    }

    #[test]
    fn emit_is_contextual_and_usable_as_an_identifier() {
        // `emit` not followed by `<Name> (` is a normal state-var reference.
        let f = parse_src(
            "facet C { uint256 emit; function f() external view returns (uint256) { return emit; } }",
        )
        .unwrap();
        match &f.functions[0].body {
            Stmt::Return(Expr::StateVar { name, .. }) => assert_eq!(name, "emit"),
            other => panic!("unexpected body {other:?}"),
        }
    }

    #[test]
    fn event_is_contextual_and_usable_as_an_identifier() {
        // `event` not followed by `<Name> (` is a normal state-var type/name pair.
        let f = parse_src(
            "facet C { uint256 event; function f() external view returns (uint256) { return event; } }",
        )
        .unwrap();
        assert_eq!(f.state_vars.len(), 1);
        assert_eq!(f.state_vars[0].name, "event");
        assert!(f.events.is_empty());
    }

    #[test]
    fn compound_plus_assign_on_a_mapping_desugars() {
        // `count[msg.sender] += n` parses to `count[msg.sender] = count[msg.sender] + n`.
        let f = parse_src(
            "facet C { mapping(address => uint256) count; \
             function f(uint256 n) external { count[msg.sender] += n; } }",
        )
        .unwrap();
        match &f.functions[0].body {
            Stmt::Block(stmts) => match &stmts[0] {
                Stmt::IndexAssign { base, value: Expr::Add { lhs, rhs, .. }, .. } => {
                    assert_eq!(base, "count");
                    assert!(matches!(**lhs, Expr::Index { .. }), "lhs is a read of count[..]");
                    assert!(matches!(**rhs, Expr::StateVar { .. }), "rhs is `n`");
                }
                other => panic!("expected a desugared IndexAssign(Add), got {other:?}"),
            },
            other => panic!("unexpected body {other:?}"),
        }
    }
}
