use crate::rustlite::{CompileError, Span};
use crate::rustlite::ast::*;
use crate::rustlite::token::{Token, TokenKind};

pub fn parse(tokens: &[Token]) -> Result<Module, CompileError> {
    let mut p = Parser::new(tokens);
    p.parse_module()
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn at(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(self.peek()) == std::mem::discriminant(kind)
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos];
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<&Token, CompileError> {
        if self.at(kind) {
            Ok(self.advance())
        } else {
            Err(CompileError::at(
                format!("expected {kind:?}, got {:?}", self.peek()),
                self.span(),
            ))
        }
    }

    fn expect_ident(&mut self) -> Result<String, CompileError> {
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(name)
            }
            _ => Err(CompileError::at(
                format!("expected identifier, got {:?}", self.peek()),
                self.span(),
            )),
        }
    }

    // ── Module ──────────────────────────────────────────────────────

    fn parse_module(&mut self) -> Result<Module, CompileError> {
        let mut uses = Vec::new();
        let mut items = Vec::new();

        while !self.at_eof() {
            if matches!(self.peek(), TokenKind::Use) {
                uses.push(self.parse_use()?);
            } else {
                items.push(self.parse_item()?);
            }
        }

        Ok(Module { uses, items })
    }

    fn parse_use(&mut self) -> Result<UseDecl, CompileError> {
        let start = self.span();
        self.expect(&TokenKind::Use)?;
        let path = self.parse_path()?;
        self.expect(&TokenKind::Semi)?;
        Ok(UseDecl { path, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
    }

    fn parse_path(&mut self) -> Result<Vec<String>, CompileError> {
        let mut segments = vec![self.expect_ident()?];
        while matches!(self.peek(), TokenKind::ColonColon) {
            self.advance();
            segments.push(self.expect_ident()?);
        }
        Ok(segments)
    }

    // ── Items ───────────────────────────────────────────────────────

    fn parse_item(&mut self) -> Result<Item, CompileError> {
        match self.peek() {
            TokenKind::Struct => Ok(Item::Struct(self.parse_struct()?)),
            TokenKind::Enum => Ok(Item::Enum(self.parse_enum()?)),
            TokenKind::Fn => Ok(Item::Fn(self.parse_fn()?)),
            TokenKind::Const => Ok(Item::Const(self.parse_const()?)),
            _ => Err(CompileError::at(
                format!("expected item (fn/struct/enum/const), got {:?}", self.peek()),
                self.span(),
            )),
        }
    }

    fn parse_struct(&mut self) -> Result<StructDecl, CompileError> {
        let start = self.span();
        self.expect(&TokenKind::Struct)?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let fields = self.parse_field_list()?;
        self.expect(&TokenKind::RBrace)?;
        Ok(StructDecl { name, fields, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
    }

    fn parse_enum(&mut self) -> Result<EnumDecl, CompileError> {
        let start = self.span();
        self.expect(&TokenKind::Enum)?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let variants = self.parse_variant_list()?;
        self.expect(&TokenKind::RBrace)?;
        Ok(EnumDecl { name, variants, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
    }

    fn parse_fn(&mut self) -> Result<FnDecl, CompileError> {
        let start = self.span();
        self.expect(&TokenKind::Fn)?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LParen)?;
        let params = if !matches!(self.peek(), TokenKind::RParen) {
            self.parse_param_list()?
        } else {
            Vec::new()
        };
        self.expect(&TokenKind::RParen)?;
        let ret_type = if matches!(self.peek(), TokenKind::Arrow) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };
        let body = self.parse_block()?;
        Ok(FnDecl { name, params, ret_type, body, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
    }

    fn parse_const(&mut self) -> Result<ConstDecl, CompileError> {
        let start = self.span();
        self.expect(&TokenKind::Const)?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        let ty = self.parse_type()?;
        self.expect(&TokenKind::Eq)?;
        let value = self.parse_expr()?;
        self.expect(&TokenKind::Semi)?;
        Ok(ConstDecl { name, ty, value, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
    }

    // ── Field / variant / param lists ───────────────────────────────

    fn parse_field_list(&mut self) -> Result<Vec<Field>, CompileError> {
        let mut fields = Vec::new();
        while !matches!(self.peek(), TokenKind::RBrace) {
            let start = self.span();
            let name = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let ty = self.parse_type()?;
            fields.push(Field { name, ty, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } });
            if !matches!(self.peek(), TokenKind::Comma) { break; }
            self.advance();
        }
        Ok(fields)
    }

    fn parse_variant_list(&mut self) -> Result<Vec<Variant>, CompileError> {
        let mut variants = Vec::new();
        while !matches!(self.peek(), TokenKind::RBrace) {
            let start = self.span();
            let name = self.expect_ident()?;
            let payload = match self.peek() {
                TokenKind::LParen => {
                    self.advance();
                    let mut types = Vec::new();
                    while !matches!(self.peek(), TokenKind::RParen) {
                        types.push(self.parse_type()?);
                        if !matches!(self.peek(), TokenKind::Comma) { break; }
                        self.advance();
                    }
                    self.expect(&TokenKind::RParen)?;
                    VariantPayload::Tuple(types)
                }
                TokenKind::LBrace => {
                    self.advance();
                    let fields = self.parse_field_list()?;
                    self.expect(&TokenKind::RBrace)?;
                    VariantPayload::Struct(fields)
                }
                _ => VariantPayload::Unit,
            };
            variants.push(Variant { name, payload, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } });
            if !matches!(self.peek(), TokenKind::Comma) { break; }
            self.advance();
        }
        Ok(variants)
    }

    fn parse_param_list(&mut self) -> Result<Vec<Param>, CompileError> {
        let mut params = Vec::new();
        loop {
            let start = self.span();
            let name = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let ty = self.parse_type()?;
            params.push(Param { name, ty, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } });
            if !matches!(self.peek(), TokenKind::Comma) { break; }
            self.advance();
        }
        Ok(params)
    }

    // ── Types ───────────────────────────────────────────────────────

    fn parse_type(&mut self) -> Result<Ty, CompileError> {
        match self.peek() {
            TokenKind::I32 => { self.advance(); Ok(Ty::I32) }
            TokenKind::I64 => { self.advance(); Ok(Ty::I64) }
            TokenKind::F32 => { self.advance(); Ok(Ty::F32) }
            TokenKind::F64 => { self.advance(); Ok(Ty::F64) }
            TokenKind::Bool => { self.advance(); Ok(Ty::Bool) }
            TokenKind::StringType => { self.advance(); Ok(Ty::String) }
            TokenKind::Ident(_) => {
                let name = self.expect_ident()?;
                Ok(Ty::Named(name))
            }
            TokenKind::LParen => {
                self.advance();
                let first = self.parse_type()?;
                if matches!(self.peek(), TokenKind::Comma) {
                    let mut types = vec![first];
                    while matches!(self.peek(), TokenKind::Comma) {
                        self.advance();
                        types.push(self.parse_type()?);
                    }
                    self.expect(&TokenKind::RParen)?;
                    Ok(Ty::Tuple(types))
                } else {
                    self.expect(&TokenKind::RParen)?;
                    Ok(first)
                }
            }
            _ => Err(CompileError::at(
                format!("expected type, got {:?}", self.peek()),
                self.span(),
            )),
        }
    }

    // ── Blocks ──────────────────────────────────────────────────────

    fn parse_block(&mut self) -> Result<Block, CompileError> {
        let start = self.span();
        self.expect(&TokenKind::LBrace)?;

        let mut stmts = Vec::new();
        let mut tail: Option<Box<Expr>> = None;

        while !matches!(self.peek(), TokenKind::RBrace) {
            // Try parsing as a statement
            match self.peek() {
                TokenKind::Let => {
                    stmts.push(self.parse_let_stmt()?);
                }
                TokenKind::Return => {
                    stmts.push(self.parse_return_stmt()?);
                }
                _ => {
                    let expr = self.parse_expr()?;

                    // Assignment: expr followed by `=` (and the expr must be a place)
                    if matches!(self.peek(), TokenKind::Eq) && !matches!(self.peek(), TokenKind::EqEq) {
                        // Check it's actually `=` not `==`
                        if matches!(self.tokens[self.pos].kind, TokenKind::Eq) {
                            self.advance(); // consume =
                            let value = self.parse_expr()?;
                            self.expect(&TokenKind::Semi)?;
                            let place = expr_to_place(&expr)?;
                            stmts.push(Stmt::Assign {
                                place,
                                value,
                                span: expr.span,
                            });
                            continue;
                        }
                    }

                    if is_block_expr(&expr) {
                        // while/loop are void — always statements, no `;` needed
                        let span = expr.span;
                        stmts.push(Stmt::Expr { expr, span });
                    } else if matches!(self.peek(), TokenKind::Semi) {
                        let span = expr.span;
                        self.advance();
                        stmts.push(Stmt::Expr { expr, span });
                    } else if matches!(self.peek(), TokenKind::RBrace) {
                        tail = Some(Box::new(expr));
                    } else {
                        return Err(CompileError::at(
                            format!("expected ';' or '}}' after expression, got {:?}", self.peek()),
                            self.span(),
                        ));
                    }
                }
            }
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(Block { stmts, tail, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
    }

    fn parse_let_stmt(&mut self) -> Result<Stmt, CompileError> {
        let start = self.span();
        self.expect(&TokenKind::Let)?;
        let mutable = if matches!(self.peek(), TokenKind::Mut) {
            self.advance();
            true
        } else {
            false
        };
        let name = self.expect_ident()?;
        let ty = if matches!(self.peek(), TokenKind::Colon) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(&TokenKind::Eq)?;
        let init = self.parse_expr()?;
        self.expect(&TokenKind::Semi)?;
        Ok(Stmt::Let { name, mutable, ty, init, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
    }

    fn parse_return_stmt(&mut self) -> Result<Stmt, CompileError> {
        let start = self.span();
        self.expect(&TokenKind::Return)?;
        let value = if matches!(self.peek(), TokenKind::Semi) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(&TokenKind::Semi)?;
        Ok(Stmt::Return { value, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
    }

    // ── Expressions (precedence climbing) ───────────────────────────

    fn parse_expr(&mut self) -> Result<Expr, CompileError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, CompileError> {
        let mut lhs = self.parse_and()?;
        while matches!(self.peek(), TokenKind::PipePipe) {
            self.advance();
            let rhs = self.parse_and()?;
            let span = Span { start: lhs.span.start, end: rhs.span.end };
            lhs = Expr { kind: ExprKind::BinOp { op: BinOp::Or, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span };
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, CompileError> {
        let mut lhs = self.parse_cmp()?;
        while matches!(self.peek(), TokenKind::AmpAmp) {
            self.advance();
            let rhs = self.parse_cmp()?;
            let span = Span { start: lhs.span.start, end: rhs.span.end };
            lhs = Expr { kind: ExprKind::BinOp { op: BinOp::And, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span };
        }
        Ok(lhs)
    }

    fn parse_cmp(&mut self) -> Result<Expr, CompileError> {
        let lhs = self.parse_sum()?;
        let op = match self.peek() {
            TokenKind::EqEq => BinOp::Eq,
            TokenKind::BangEq => BinOp::Ne,
            TokenKind::Lt => BinOp::Lt,
            TokenKind::Gt => BinOp::Gt,
            TokenKind::LtEq => BinOp::Le,
            TokenKind::GtEq => BinOp::Ge,
            _ => return Ok(lhs),
        };
        self.advance();
        let rhs = self.parse_sum()?;
        let span = Span { start: lhs.span.start, end: rhs.span.end };
        Ok(Expr { kind: ExprKind::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span })
    }

    fn parse_sum(&mut self) -> Result<Expr, CompileError> {
        let mut lhs = self.parse_term()?;
        while matches!(self.peek(), TokenKind::Plus | TokenKind::Minus) {
            let op = if matches!(self.peek(), TokenKind::Plus) { BinOp::Add } else { BinOp::Sub };
            self.advance();
            let rhs = self.parse_term()?;
            let span = Span { start: lhs.span.start, end: rhs.span.end };
            lhs = Expr { kind: ExprKind::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span };
        }
        Ok(lhs)
    }

    fn parse_term(&mut self) -> Result<Expr, CompileError> {
        let mut lhs = self.parse_unary()?;
        while matches!(self.peek(), TokenKind::Star | TokenKind::Slash | TokenKind::Percent) {
            let op = match self.peek() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                _ => BinOp::Mod,
            };
            self.advance();
            let rhs = self.parse_unary()?;
            let span = Span { start: lhs.span.start, end: rhs.span.end };
            lhs = Expr { kind: ExprKind::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, CompileError> {
        match self.peek() {
            TokenKind::Minus => {
                let start = self.span();
                self.advance();
                let operand = self.parse_unary()?;
                let span = Span { start: start.start, end: operand.span.end };
                Ok(Expr { kind: ExprKind::UnaryOp { op: UnaryOp::Neg, operand: Box::new(operand) }, span })
            }
            TokenKind::Bang => {
                let start = self.span();
                self.advance();
                let operand = self.parse_unary()?;
                let span = Span { start: start.start, end: operand.span.end };
                Ok(Expr { kind: ExprKind::UnaryOp { op: UnaryOp::Not, operand: Box::new(operand) }, span })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, CompileError> {
        let mut expr = self.parse_atom()?;
        loop {
            match self.peek() {
                TokenKind::Dot => {
                    self.advance();
                    let field = self.expect_ident()?;
                    if matches!(self.peek(), TokenKind::LParen) {
                        // Method call
                        self.advance();
                        let args = self.parse_arg_list()?;
                        self.expect(&TokenKind::RParen)?;
                        let span = Span { start: expr.span.start, end: self.tokens[self.pos - 1].span.end };
                        expr = Expr { kind: ExprKind::MethodCall { object: Box::new(expr), method: field, args }, span };
                    } else {
                        // Field access
                        let span = Span { start: expr.span.start, end: self.tokens[self.pos - 1].span.end };
                        expr = Expr { kind: ExprKind::FieldAccess { object: Box::new(expr), field }, span };
                    }
                }
                TokenKind::LParen => {
                    self.advance();
                    let args = self.parse_arg_list()?;
                    self.expect(&TokenKind::RParen)?;
                    let span = Span { start: expr.span.start, end: self.tokens[self.pos - 1].span.end };
                    expr = Expr { kind: ExprKind::Call { func: Box::new(expr), args }, span };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_atom(&mut self) -> Result<Expr, CompileError> {
        let start = self.span();
        match self.peek().clone() {
            TokenKind::IntLit(n) => {
                let n = n;
                self.advance();
                Ok(Expr { kind: ExprKind::IntLit(n), span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
            }
            TokenKind::FloatLit(n) => {
                let n = n;
                self.advance();
                Ok(Expr { kind: ExprKind::FloatLit(n), span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
            }
            TokenKind::StringLit(s) => {
                let s = s.clone();
                self.advance();
                Ok(Expr { kind: ExprKind::StringLit(s), span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
            }
            TokenKind::True => {
                self.advance();
                Ok(Expr { kind: ExprKind::BoolLit(true), span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
            }
            TokenKind::False => {
                self.advance();
                Ok(Expr { kind: ExprKind::BoolLit(false), span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
            }

            TokenKind::If => self.parse_if_expr(),
            TokenKind::Match => self.parse_match_expr(),
            TokenKind::While => self.parse_while_expr(),
            TokenKind::Loop => self.parse_loop_expr(),

            TokenKind::Break => {
                self.advance();
                let value = if !matches!(self.peek(), TokenKind::Semi | TokenKind::RBrace | TokenKind::Comma) {
                    Some(Box::new(self.parse_expr()?))
                } else {
                    None
                };
                Ok(Expr { kind: ExprKind::Break { value }, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
            }

            TokenKind::Continue => {
                self.advance();
                Ok(Expr { kind: ExprKind::Continue, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
            }

            TokenKind::LParen => {
                self.advance();
                let first = self.parse_expr()?;
                if matches!(self.peek(), TokenKind::Comma) {
                    // Tuple literal
                    let mut exprs = vec![first];
                    while matches!(self.peek(), TokenKind::Comma) {
                        self.advance();
                        if matches!(self.peek(), TokenKind::RParen) { break; }
                        exprs.push(self.parse_expr()?);
                    }
                    self.expect(&TokenKind::RParen)?;
                    Ok(Expr { kind: ExprKind::TupleLit(exprs), span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
                } else {
                    // Parenthesized expr
                    self.expect(&TokenKind::RParen)?;
                    Ok(first)
                }
            }

            TokenKind::Ident(_) => {
                // Could be: variable, path, struct literal, or path call
                let path = self.parse_path()?;

                if matches!(self.peek(), TokenKind::LBrace) && self.looks_like_struct_lit() {
                    // Struct literal: Path { field: value, ... }
                    self.advance();
                    let fields = self.parse_field_init_list()?;
                    self.expect(&TokenKind::RBrace)?;
                    Ok(Expr {
                        kind: ExprKind::StructLit { path, fields },
                        span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end },
                    })
                } else if path.len() == 1 {
                    Ok(Expr {
                        kind: ExprKind::Var(path.into_iter().next().unwrap()),
                        span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end },
                    })
                } else {
                    Ok(Expr {
                        kind: ExprKind::Path(path),
                        span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end },
                    })
                }
            }

            _ => Err(CompileError::at(
                format!("expected expression, got {:?}", self.peek()),
                self.span(),
            )),
        }
    }

    fn looks_like_struct_lit(&self) -> bool {
        // Peek ahead: `{` followed by `ident :` or `ident ,` or `ident }`
        // means struct literal. `{` followed by something else means block.
        if self.pos + 2 >= self.tokens.len() { return false; }
        let after_brace = &self.tokens[self.pos + 1].kind;
        let after_that = &self.tokens[self.pos + 2].kind;
        match after_brace {
            TokenKind::Ident(_) => matches!(after_that, TokenKind::Colon | TokenKind::Comma | TokenKind::RBrace),
            TokenKind::RBrace => true, // empty struct lit
            _ => false,
        }
    }

    fn parse_if_expr(&mut self) -> Result<Expr, CompileError> {
        let start = self.span();
        self.expect(&TokenKind::If)?;
        let cond = self.parse_expr()?;
        let then_block = self.parse_block()?;
        let else_block = if matches!(self.peek(), TokenKind::Else) {
            self.advance();
            if matches!(self.peek(), TokenKind::If) {
                Some(ElseBranch::If(Box::new(self.parse_if_expr()?)))
            } else {
                Some(ElseBranch::Block(self.parse_block()?))
            }
        } else {
            None
        };
        Ok(Expr {
            kind: ExprKind::If { cond: Box::new(cond), then_block, else_block },
            span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end },
        })
    }

    fn parse_match_expr(&mut self) -> Result<Expr, CompileError> {
        let start = self.span();
        self.expect(&TokenKind::Match)?;
        let scrutinee = self.parse_expr()?;
        self.expect(&TokenKind::LBrace)?;
        let mut arms = Vec::new();
        while !matches!(self.peek(), TokenKind::RBrace) {
            arms.push(self.parse_match_arm()?);
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Expr {
            kind: ExprKind::Match { scrutinee: Box::new(scrutinee), arms },
            span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end },
        })
    }

    fn parse_match_arm(&mut self) -> Result<MatchArm, CompileError> {
        let start = self.span();
        let pattern = self.parse_pattern()?;
        self.expect(&TokenKind::FatArrow)?;

        // Arm body: either a block, or an expression followed by a comma
        let body = if matches!(self.peek(), TokenKind::LBrace) {
            let block = self.parse_block()?;
            // Optional trailing comma after block arm
            if matches!(self.peek(), TokenKind::Comma) { self.advance(); }
            Expr { span: block.span, kind: ExprKind::Block(block) }
        } else {
            let expr = self.parse_expr()?;
            // Comma required after non-block arm (unless it's the last arm before `}`)
            if matches!(self.peek(), TokenKind::Comma) { self.advance(); }
            expr
        };

        Ok(MatchArm { pattern, body, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, CompileError> {
        let start = self.span();
        match self.peek().clone() {
            TokenKind::Underscore => {
                self.advance();
                Ok(Pattern { kind: PatternKind::Wildcard, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
            }
            TokenKind::IntLit(n) => {
                self.advance();
                Ok(Pattern { kind: PatternKind::Literal(LitPattern::Int(n)), span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
            }
            TokenKind::FloatLit(n) => {
                self.advance();
                Ok(Pattern { kind: PatternKind::Literal(LitPattern::Float(n)), span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
            }
            TokenKind::StringLit(s) => {
                let s = s.clone();
                self.advance();
                Ok(Pattern { kind: PatternKind::Literal(LitPattern::String(s)), span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
            }
            TokenKind::True => {
                self.advance();
                Ok(Pattern { kind: PatternKind::Literal(LitPattern::Bool(true)), span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
            }
            TokenKind::False => {
                self.advance();
                Ok(Pattern { kind: PatternKind::Literal(LitPattern::Bool(false)), span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
            }
            TokenKind::Ident(_) => {
                let path = self.parse_path()?;
                match self.peek() {
                    TokenKind::LParen => {
                        // Tuple variant pattern
                        self.advance();
                        let mut fields = Vec::new();
                        while !matches!(self.peek(), TokenKind::RParen) {
                            fields.push(self.parse_pattern()?);
                            if !matches!(self.peek(), TokenKind::Comma) { break; }
                            self.advance();
                        }
                        self.expect(&TokenKind::RParen)?;
                        Ok(Pattern { kind: PatternKind::TupleVariant { path, fields }, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
                    }
                    TokenKind::LBrace => {
                        // Struct variant pattern
                        self.advance();
                        let mut fields = Vec::new();
                        while !matches!(self.peek(), TokenKind::RBrace) {
                            let fstart = self.span();
                            let name = self.expect_ident()?;
                            let pattern = if matches!(self.peek(), TokenKind::Colon) {
                                self.advance();
                                Some(self.parse_pattern()?)
                            } else {
                                None
                            };
                            fields.push(FieldPattern { name, pattern, span: Span { start: fstart.start, end: self.tokens[self.pos - 1].span.end } });
                            if !matches!(self.peek(), TokenKind::Comma) { break; }
                            self.advance();
                        }
                        self.expect(&TokenKind::RBrace)?;
                        Ok(Pattern { kind: PatternKind::StructVariant { path, fields }, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
                    }
                    _ => {
                        if path.len() == 1 {
                            Ok(Pattern { kind: PatternKind::Binding(path.into_iter().next().unwrap()), span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
                        } else {
                            Ok(Pattern { kind: PatternKind::Path(path), span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
                        }
                    }
                }
            }
            _ => Err(CompileError::at(
                format!("expected pattern, got {:?}", self.peek()),
                self.span(),
            )),
        }
    }

    fn parse_while_expr(&mut self) -> Result<Expr, CompileError> {
        let start = self.span();
        self.expect(&TokenKind::While)?;
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(Expr {
            kind: ExprKind::While { cond: Box::new(cond), body },
            span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end },
        })
    }

    fn parse_loop_expr(&mut self) -> Result<Expr, CompileError> {
        let start = self.span();
        self.expect(&TokenKind::Loop)?;
        let body = self.parse_block()?;
        Ok(Expr {
            kind: ExprKind::Loop { body },
            span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end },
        })
    }

    fn parse_arg_list(&mut self) -> Result<Vec<Expr>, CompileError> {
        let mut args = Vec::new();
        if matches!(self.peek(), TokenKind::RParen) { return Ok(args); }
        loop {
            args.push(self.parse_expr()?);
            if !matches!(self.peek(), TokenKind::Comma) { break; }
            self.advance();
        }
        Ok(args)
    }

    fn parse_field_init_list(&mut self) -> Result<Vec<FieldInit>, CompileError> {
        let mut fields = Vec::new();
        while !matches!(self.peek(), TokenKind::RBrace) {
            let start = self.span();
            let name = self.expect_ident()?;
            let value = if matches!(self.peek(), TokenKind::Colon) {
                self.advance();
                Some(self.parse_expr()?)
            } else {
                None // shorthand: `name` == `name: name`
            };
            fields.push(FieldInit { name, value, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } });
            if !matches!(self.peek(), TokenKind::Comma) { break; }
            self.advance();
        }
        Ok(fields)
    }
}

fn is_block_expr(expr: &Expr) -> bool {
    matches!(expr.kind, ExprKind::While { .. } | ExprKind::Loop { .. })
}

fn expr_to_place(expr: &Expr) -> Result<Place, CompileError> {
    match &expr.kind {
        ExprKind::Var(name) => Ok(Place { root: name.clone(), fields: Vec::new(), span: expr.span }),
        ExprKind::FieldAccess { object, field } => {
            let mut place = expr_to_place(object)?;
            place.fields.push(field.clone());
            place.span = expr.span;
            Ok(place)
        }
        _ => Err(CompileError::at("invalid assignment target", expr.span)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rustlite::lexer;

    fn parse_str(s: &str) -> Module {
        let tokens = lexer::lex(s).unwrap();
        parse(&tokens).unwrap()
    }

    #[test]
    fn parse_empty_fn() {
        let m = parse_str("fn main() {}");
        assert_eq!(m.items.len(), 1);
        match &m.items[0] {
            Item::Fn(f) => {
                assert_eq!(f.name, "main");
                assert!(f.params.is_empty());
                assert!(f.ret_type.is_none());
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn parse_fn_with_return() {
        let m = parse_str("fn add(a: i32, b: i32) -> i32 { a + b }");
        match &m.items[0] {
            Item::Fn(f) => {
                assert_eq!(f.name, "add");
                assert_eq!(f.params.len(), 2);
                assert!(matches!(f.ret_type, Some(Ty::I32)));
                assert!(f.body.tail.is_some());
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn parse_struct() {
        let m = parse_str("struct Point { x: i32, y: i32 }");
        match &m.items[0] {
            Item::Struct(s) => {
                assert_eq!(s.name, "Point");
                assert_eq!(s.fields.len(), 2);
            }
            _ => panic!("expected struct"),
        }
    }

    #[test]
    fn parse_enum() {
        let m = parse_str("enum Option { None, Some(i32) }");
        match &m.items[0] {
            Item::Enum(e) => {
                assert_eq!(e.name, "Option");
                assert_eq!(e.variants.len(), 2);
                assert!(matches!(e.variants[0].payload, VariantPayload::Unit));
                assert!(matches!(e.variants[1].payload, VariantPayload::Tuple(_)));
            }
            _ => panic!("expected enum"),
        }
    }

    #[test]
    fn parse_match() {
        let m = parse_str(r#"
            fn check(x: i32) -> i32 {
                match x {
                    0 => 1,
                    _ => x + 1,
                }
            }
        "#);
        match &m.items[0] {
            Item::Fn(f) => {
                assert!(f.body.tail.is_some());
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn parse_if_else() {
        let m = parse_str("fn f(x: i32) -> i32 { if x > 0 { x } else { 0 - x } }");
        match &m.items[0] {
            Item::Fn(f) => assert!(f.body.tail.is_some()),
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn parse_struct_literal() {
        let m = parse_str("fn f() -> Point { Point { x: 1, y: 2 } }");
        match &m.items[0] {
            Item::Fn(f) => {
                let tail = f.body.tail.as_ref().unwrap();
                assert!(matches!(tail.kind, ExprKind::StructLit { .. }));
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn parse_let_mut_assign() {
        let m = parse_str("fn f() { let mut x: i32 = 0; x = 42; }");
        match &m.items[0] {
            Item::Fn(f) => {
                assert_eq!(f.body.stmts.len(), 2);
                assert!(matches!(f.body.stmts[0], Stmt::Let { mutable: true, .. }));
                assert!(matches!(f.body.stmts[1], Stmt::Assign { .. }));
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn parse_use_decl() {
        let m = parse_str("use host::log; fn f() {}");
        assert_eq!(m.uses.len(), 1);
        assert_eq!(m.uses[0].path, vec!["host", "log"]);
    }

    #[test]
    fn parse_while_loop() {
        let m = parse_str("fn f() { let mut i: i32 = 0; while i < 10 { i = i + 1; } }");
        match &m.items[0] {
            Item::Fn(f) => assert_eq!(f.body.stmts.len(), 2),
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn parse_method_call() {
        let m = parse_str("fn f(s: String) -> i32 { s.len() }");
        match &m.items[0] {
            Item::Fn(f) => {
                let tail = f.body.tail.as_ref().unwrap();
                assert!(matches!(tail.kind, ExprKind::MethodCall { .. }));
            }
            _ => panic!("expected fn"),
        }
    }
}
