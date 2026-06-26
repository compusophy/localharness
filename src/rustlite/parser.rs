use crate::error_codes as codes;
use crate::rustlite::{CompileError, Span};
use crate::rustlite::ast::*;
use crate::rustlite::token::{Token, TokenKind};

pub fn parse(tokens: &[Token]) -> Result<Module, CompileError> {
    let mut p = Parser::new(tokens);
    p.parse_module()
}

/// Hard cap on recursive-descent nesting depth. The compiler runs in the
/// user's browser on agent/LLM-authored source; without this, deeply
/// nested input (`((((…))))`, `-!-!-!…`, `{{{…}}}`) recurses one stack
/// frame per token and overflows the wasm stack — an UNCATCHABLE abort
/// that kills the whole tab rather than returning a `CompileError`.
///
/// The cap is in "guard entries", not source nesting levels: one paren
/// level costs ~2 entries (parse_expr + parse_unary) and ~10 actual stack
/// frames (the precedence ladder), so the cap must stay well under
/// stack_size / frames_per_entry. 96 entries ≈ 48 paren levels ≈ a few
/// hundred frames — comfortably inside the browser's wasm stack while far
/// beyond anything a real rustlite cartridge nests.
const MAX_RECURSION_DEPTH: usize = 96;

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    depth: usize,
    /// Rust's "no struct literal" restriction. While true, a bare `ident {`
    /// is parsed as just the identifier (the `{` opens a block) rather than a
    /// struct literal — so `if a < b { … }`, `while i < len { … }`,
    /// `for k in 0..n { … }`, and `match x { … }` parse `b`/`len`/`n`/`x` as
    /// expressions, not struct-literal heads. Set true around the if/while
    /// CONDITION, the for ITERABLE, and the match SCRUTINEE; reset to false
    /// inside any parenthesised sub-expr, array/tuple/index/arg position, and
    /// (always) the block body. A struct literal in a restricted position must
    /// then be parenthesised: `if x == (Foo { a: 1 }) { … }`.
    no_struct_literal: bool,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, pos: 0, depth: 0, no_struct_literal: false }
    }

    /// Parse `f` with the no-struct-literal restriction set to `flag`, restoring
    /// the previous setting afterward. Used to TURN ON the restriction for a
    /// condition/scrutinee/iterable and to TURN IT OFF again for the bracketed
    /// sub-contexts (parens, `[ ]`, args) where a struct literal is unambiguous.
    fn with_no_struct_literal<T>(
        &mut self,
        flag: bool,
        f: impl FnOnce(&mut Self) -> Result<T, CompileError>,
    ) -> Result<T, CompileError> {
        let prev = self.no_struct_literal;
        self.no_struct_literal = flag;
        let r = f(self);
        self.no_struct_literal = prev;
        r
    }

    /// Enter one recursion level; error (don't overflow) past the cap.
    /// Paired with [`leave`](Self::leave) on every return path of the
    /// guarded function.
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
            Err(CompileError::at_code(
                codes::UNEXPECTED_TOKEN,
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
            _ => Err(CompileError::at_code(
                codes::UNEXPECTED_TOKEN,
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

    /// Consume and discard an optional `pub` / `pub(crate)` / `pub(...)`
    /// visibility modifier. rustlite is a single flat module so visibility
    /// is meaningless here, but agent/LLM-authored source routinely writes
    /// `pub fn` / `pub struct` / `pub x: i32`; accept it rather than erroring.
    fn skip_visibility(&mut self) {
        if matches!(self.peek(), TokenKind::Ident(n) if n.as_str() == "pub") {
            self.advance();
            // optional restriction group: `(crate)`, `(super)`, `(in path)`
            if self.at(&TokenKind::LParen) {
                let mut depth = 0usize;
                loop {
                    match self.peek() {
                        TokenKind::LParen => depth += 1,
                        TokenKind::RParen => depth -= 1,
                        TokenKind::Eof => break,
                        _ => {}
                    }
                    self.advance();
                    if depth == 0 {
                        break;
                    }
                }
            }
        }
    }

    fn parse_item(&mut self) -> Result<Item, CompileError> {
        self.skip_visibility();
        match self.peek() {
            TokenKind::Struct => Ok(Item::Struct(self.parse_struct()?)),
            TokenKind::Enum => Ok(Item::Enum(self.parse_enum()?)),
            TokenKind::Fn => Ok(Item::Fn(self.parse_fn()?)),
            TokenKind::Const => Ok(Item::Const(self.parse_const()?)),
            _ => Err(CompileError::at_code(
                codes::EXPECTED_ITEM,
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
            self.skip_visibility();
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
        while !matches!(self.peek(), TokenKind::RParen) {
            let start = self.span();
            let name = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let ty = self.parse_type()?;
            params.push(Param { name, ty, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } });
            if !matches!(self.peek(), TokenKind::Comma) { break; }
            self.advance();
            // Trailing comma: `fn f(a: i32,)` — the comma may be the last token.
            if matches!(self.peek(), TokenKind::RParen) { break; }
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
            // `[T; N]` — fixed-size array type. Enables array fn parameters /
            // returns (`fn f(cur: [i32; 64], …)`), passed as an i32 base pointer.
            TokenKind::LBracket => {
                self.advance();
                let elem = self.parse_type()?;
                self.expect(&TokenKind::Semi)?;
                let n = match self.peek().clone() {
                    TokenKind::IntLit(n) if n >= 0 => {
                        self.advance();
                        n as usize
                    }
                    other => {
                        return Err(CompileError::at_code(
                            codes::EXPECTED_TYPE,
                            format!("array length must be a non-negative integer literal, got {other:?}"),
                            self.span(),
                        ))
                    }
                };
                self.expect(&TokenKind::RBracket)?;
                Ok(Ty::Array(Box::new(elem), n))
            }
            _ => Err(CompileError::at_code(
                codes::EXPECTED_TYPE,
                format!("expected type, got {:?}", self.peek()),
                self.span(),
            )),
        }
    }

    // ── Blocks ──────────────────────────────────────────────────────

    fn parse_block(&mut self) -> Result<Block, CompileError> {
        self.enter()?;
        // A block body is always unrestricted: struct literals are fine inside
        // `if cond { let p = Foo { .. }; }`. Reset the no-struct-literal flag
        // for the whole body regardless of the enclosing condition context.
        let r = self.with_no_struct_literal(false, |p| p.parse_block_inner());
        self.leave();
        r
    }

    fn parse_block_inner(&mut self) -> Result<Block, CompileError> {
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

                    // Compound assignment: `place OP= value` desugars to
                    // `place = place OP value` (e.g. `x += 1` → `x = x + 1`).
                    let compound = match self.peek() {
                        TokenKind::PlusEq => Some(BinOp::Add),
                        TokenKind::MinusEq => Some(BinOp::Sub),
                        TokenKind::StarEq => Some(BinOp::Mul),
                        TokenKind::SlashEq => Some(BinOp::Div),
                        TokenKind::PercentEq => Some(BinOp::Mod),
                        _ => None,
                    };
                    if let Some(op) = compound {
                        self.advance(); // consume the `OP=` token
                        let rhs = self.parse_expr()?;
                        self.expect(&TokenKind::Semi)?;
                        let span = expr.span;
                        let place = expr_to_place(&expr)?;
                        let value = Expr {
                            kind: ExprKind::BinOp {
                                op,
                                lhs: Box::new(expr),
                                rhs: Box::new(rhs),
                            },
                            span,
                        };
                        stmts.push(Stmt::Assign { place, value, span });
                        continue;
                    }

                    // Assignment: expr followed by `=` (and the expr must be a
                    // place). The lexer emits distinct `Eq`/`EqEq` tokens, so a
                    // single `Eq` check already excludes `==`.
                    if matches!(self.peek(), TokenKind::Eq) {
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

                    let is_void_loop =
                        matches!(expr.kind, ExprKind::While { .. } | ExprKind::Loop { .. });
                    if is_void_loop {
                        // Loops have no value — always a statement, with an
                        // optional trailing `;`. Never the block tail.
                        if matches!(self.peek(), TokenKind::Semi) {
                            self.advance();
                        }
                        let span = expr.span;
                        stmts.push(Stmt::Expr { expr, span });
                    } else if matches!(self.peek(), TokenKind::Semi) {
                        let span = expr.span;
                        self.advance();
                        stmts.push(Stmt::Expr { expr, span });
                    } else if matches!(self.peek(), TokenKind::RBrace) {
                        // Last expr in the block with no `;` — the tail
                        // (block value). `if`/`match` can return a value
                        // here, e.g. `{ if c { a } else { b } }`.
                        tail = Some(Box::new(expr));
                    } else if is_block_expr(&expr) {
                        // A block-like expression (if/match/block) not at
                        // the end and not followed by `;` is a statement;
                        // its value (if any) is discarded. This lets
                        // `if c { ... }` sit between other statements
                        // without a trailing semicolon, like in Rust.
                        let span = expr.span;
                        stmts.push(Stmt::Expr { expr, span });
                    } else {
                        return Err(CompileError::at_code(
                            codes::MISSING_SEMICOLON,
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
        self.enter()?;
        let r = self.parse_or();
        self.leave();
        r
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
        let lhs = self.parse_bitor()?;
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
        let rhs = self.parse_bitor()?;
        let span = Span { start: lhs.span.start, end: rhs.span.end };
        Ok(Expr { kind: ExprKind::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span })
    }

    // Bitwise + shift precedence (loosest → tightest, between comparison and
    // additive, matching Rust): `|` < `^` < `&` < `<< >>` < `+ -`.
    fn parse_bitor(&mut self) -> Result<Expr, CompileError> {
        let mut lhs = self.parse_bitxor()?;
        while matches!(self.peek(), TokenKind::Pipe) {
            self.advance();
            let rhs = self.parse_bitxor()?;
            let span = Span { start: lhs.span.start, end: rhs.span.end };
            lhs = Expr { kind: ExprKind::BinOp { op: BinOp::BitOr, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span };
        }
        Ok(lhs)
    }

    fn parse_bitxor(&mut self) -> Result<Expr, CompileError> {
        let mut lhs = self.parse_bitand()?;
        while matches!(self.peek(), TokenKind::Caret) {
            self.advance();
            let rhs = self.parse_bitand()?;
            let span = Span { start: lhs.span.start, end: rhs.span.end };
            lhs = Expr { kind: ExprKind::BinOp { op: BinOp::BitXor, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span };
        }
        Ok(lhs)
    }

    fn parse_bitand(&mut self) -> Result<Expr, CompileError> {
        let mut lhs = self.parse_shift()?;
        while matches!(self.peek(), TokenKind::Amp) {
            self.advance();
            let rhs = self.parse_shift()?;
            let span = Span { start: lhs.span.start, end: rhs.span.end };
            lhs = Expr { kind: ExprKind::BinOp { op: BinOp::BitAnd, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span };
        }
        Ok(lhs)
    }

    fn parse_shift(&mut self) -> Result<Expr, CompileError> {
        let mut lhs = self.parse_sum()?;
        while matches!(self.peek(), TokenKind::Shl | TokenKind::Shr) {
            let op = if matches!(self.peek(), TokenKind::Shl) { BinOp::Shl } else { BinOp::Shr };
            self.advance();
            let rhs = self.parse_sum()?;
            let span = Span { start: lhs.span.start, end: rhs.span.end };
            lhs = Expr { kind: ExprKind::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span };
        }
        Ok(lhs)
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
        let mut lhs = self.parse_cast()?;
        while matches!(self.peek(), TokenKind::Star | TokenKind::Slash | TokenKind::Percent) {
            let op = match self.peek() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                _ => BinOp::Mod,
            };
            self.advance();
            let rhs = self.parse_cast()?;
            let span = Span { start: lhs.span.start, end: rhs.span.end };
            lhs = Expr { kind: ExprKind::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span };
        }
        Ok(lhs)
    }

    /// `expr as Type` — binds tighter than `* / %`, looser than unary (Rust).
    /// Left-assoc so `x as i64 as f64` chains.
    fn parse_cast(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.parse_unary()?;
        while matches!(self.peek(), TokenKind::As) {
            self.advance();
            let ty = self.parse_type()?;
            let span = Span { start: e.span.start, end: self.tokens[self.pos - 1].span.end };
            e = Expr { kind: ExprKind::Cast { expr: Box::new(e), ty }, span };
        }
        Ok(e)
    }

    fn parse_unary(&mut self) -> Result<Expr, CompileError> {
        self.enter()?;
        let r = self.parse_unary_inner();
        self.leave();
        r
    }

    fn parse_unary_inner(&mut self) -> Result<Expr, CompileError> {
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
                    // `host::display::draw_string(x, y, "LIT", color, scale)` is a
                    // COMPILE-TIME MACRO, not a host import: desugar it to a block
                    // of per-glyph `draw_char` calls so the host ABI stays
                    // integer-only (no string-passing import). Any other call is
                    // a normal `Call`.
                    expr = if is_draw_string_path(&expr) {
                        self.desugar_draw_string(args, span)?
                    } else {
                        Expr { kind: ExprKind::Call { func: Box::new(expr), args }, span }
                    };
                }
                TokenKind::LBracket => {
                    self.advance();
                    // `[ … ]` is unambiguous: a struct literal index is fine.
                    let index = self.with_no_struct_literal(false, |p| p.parse_expr())?;
                    self.expect(&TokenKind::RBracket)?;
                    let span = Span { start: expr.span.start, end: self.tokens[self.pos - 1].span.end };
                    expr = Expr { kind: ExprKind::Index { base: Box::new(expr), index: Box::new(index) }, span };
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
                self.advance();
                Ok(Expr { kind: ExprKind::IntLit(n), span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
            }
            TokenKind::FloatLit(n) => {
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

            TokenKind::LBracket => {
                self.advance();
                // Inside `[ … ]` a struct literal is unambiguous — lift the
                // no-struct-literal restriction for the elements.
                //
                // Two forms share the `[`: the comma LITERAL `[a, b, c]` and the
                // sized REPEAT init `[value; N]`. Parse the first element, then
                // disambiguate on the next token: `;` → repeat, else → literal.
                let array = self.with_no_struct_literal(false, |p| {
                    // Empty literal `[]` (rejected later in typecheck, but parse it).
                    if matches!(p.peek(), TokenKind::RBracket) {
                        return Ok(ExprKind::ArrayLit(Vec::new()));
                    }
                    let first = p.parse_expr()?;
                    if matches!(p.peek(), TokenKind::Semi) {
                        // `[value; N]` — repeat init. N is a constant int literal.
                        p.advance();
                        let count = match p.peek().clone() {
                            TokenKind::IntLit(n) if n >= 0 => {
                                p.advance();
                                n as usize
                            }
                            other => {
                                return Err(CompileError::at_code(
                                    codes::EXPECTED_EXPRESSION,
                                    format!("array repeat count must be a non-negative integer literal, got {other:?}"),
                                    p.span(),
                                ))
                            }
                        };
                        return Ok(ExprKind::ArrayRepeat { value: Box::new(first), count });
                    }
                    // `[a, b, c]` — comma literal.
                    let mut elems = vec![first];
                    while matches!(p.peek(), TokenKind::Comma) {
                        p.advance();
                        if matches!(p.peek(), TokenKind::RBracket) {
                            break; // trailing comma
                        }
                        elems.push(p.parse_expr()?);
                    }
                    Ok(ExprKind::ArrayLit(elems))
                })?;
                self.expect(&TokenKind::RBracket)?;
                Ok(Expr { kind: array, span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
            }

            TokenKind::If => self.parse_if_expr(),
            TokenKind::Match => self.parse_match_expr(),
            TokenKind::While => self.parse_while_expr(),
            TokenKind::Loop => self.parse_loop_expr(),
            TokenKind::For => self.parse_for_expr(),

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
                // Inside parens the grammar is unambiguous, so a struct literal
                // is allowed here even when the enclosing context (an if/while
                // condition etc.) forbids a BARE one: `if x == (Foo { a: 1 })`.
                let (first, exprs) = self.with_no_struct_literal(false, |p| {
                    let first = p.parse_expr()?;
                    if matches!(p.peek(), TokenKind::Comma) {
                        // Tuple literal
                        let mut exprs = vec![first];
                        while matches!(p.peek(), TokenKind::Comma) {
                            p.advance();
                            if matches!(p.peek(), TokenKind::RParen) { break; }
                            exprs.push(p.parse_expr()?);
                        }
                        Ok((None, Some(exprs)))
                    } else {
                        Ok((Some(first), None))
                    }
                })?;
                if let Some(exprs) = exprs {
                    self.expect(&TokenKind::RParen)?;
                    Ok(Expr { kind: ExprKind::TupleLit(exprs), span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
                } else {
                    // Parenthesized expr
                    self.expect(&TokenKind::RParen)?;
                    Ok(first.unwrap())
                }
            }

            TokenKind::Ident(_) => {
                // Could be: variable, path, struct literal, or path call
                let path = self.parse_path()?;

                if !self.no_struct_literal
                    && matches!(self.peek(), TokenKind::LBrace)
                    && self.looks_like_struct_lit()
                {
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

            _ => Err(CompileError::at_code(
                codes::EXPECTED_EXPRESSION,
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
        // The condition forbids a BARE struct literal so `if a < b { … }` reads
        // `b` as a variable and `{` as the block. A struct literal here needs
        // parens. `parse_block` resets the flag, so the body is unrestricted.
        let cond = self.with_no_struct_literal(true, |p| p.parse_expr())?;
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
        // Scrutinee forbids a bare struct literal so `match x { … }` reads `x`
        // as a variable and `{` as the arm block (parens for a struct literal).
        let scrutinee = self.with_no_struct_literal(true, |p| p.parse_expr())?;
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
            // A leading `-` negates the integer literal (or range lower bound):
            // `-1 =>` and `-5..=5 =>` are both idiomatic match arms.
            TokenKind::Minus => {
                self.advance();
                let n = match self.peek().clone() {
                    TokenKind::IntLit(n) => {
                        self.advance();
                        -n
                    }
                    _ => {
                        return Err(CompileError::at_code(
                            codes::EXPECTED_PATTERN,
                            "`-` in a pattern must be followed by an integer literal",
                            self.span(),
                        ))
                    }
                };
                self.parse_int_pattern_tail(n, start)
            }
            TokenKind::IntLit(n) => {
                self.advance();
                self.parse_int_pattern_tail(n, start)
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
            _ => Err(CompileError::at_code(
                codes::EXPECTED_PATTERN,
                format!("expected pattern, got {:?}", self.peek()),
                self.span(),
            )),
        }
    }

    /// Finish an integer pattern after its (possibly negated) lower bound `lo`
    /// has been consumed: either a bare literal or `lo..=hi` / `lo..hi` where
    /// `hi` may itself be negated (`-5..=-1`).
    fn parse_int_pattern_tail(&mut self, lo: i64, start: Span) -> Result<Pattern, CompileError> {
        if matches!(self.peek(), TokenKind::DotDot | TokenKind::DotDotEq) {
            let inclusive = matches!(self.peek(), TokenKind::DotDotEq);
            self.advance();
            let neg = matches!(self.peek(), TokenKind::Minus);
            if neg { self.advance(); }
            let hi = match self.peek().clone() {
                TokenKind::IntLit(h) => {
                    self.advance();
                    if neg { -h } else { h }
                }
                _ => {
                    return Err(CompileError::at_code(
                        codes::EXPECTED_PATTERN,
                        "range pattern needs an integer upper bound",
                        self.span(),
                    ))
                }
            };
            return Ok(Pattern {
                kind: PatternKind::IntRange { lo, hi, inclusive },
                span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end },
            });
        }
        Ok(Pattern { kind: PatternKind::Literal(LitPattern::Int(lo)), span: Span { start: start.start, end: self.tokens[self.pos - 1].span.end } })
    }

    fn parse_while_expr(&mut self) -> Result<Expr, CompileError> {
        let start = self.span();
        self.expect(&TokenKind::While)?;
        // Condition forbids a bare struct literal (`while i < n { … }`).
        let cond = self.with_no_struct_literal(true, |p| p.parse_expr())?;
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

    /// `for v in a..b { body }` (exclusive) or `for v in a..=b { body }`
    /// (inclusive) — desugared to a `Block`:
    ///   { let __end = b; let mut v = a - 1;
    ///     loop { v = v + 1; if v >= __end { break; } body } }
    /// (exclusive uses `>= __end`, inclusive uses `> __end`, so `..=b` runs the
    /// body for `v == b`). The increment sits at the TOP of the loop (with `v`
    /// pre-decremented), so `continue` — which compiles to the loop back-edge
    /// (`br 0`) — correctly re-runs the increment + bound check instead of
    /// skipping them. Bounds are evaluated once. Pure AST rewrite over
    /// loop/if/break — no codegen support.
    ///
    /// The iterable bounds parse under the no-struct-literal restriction so
    /// `for k in 0..n { … }` reads `n` as a variable and `{` as the body, not a
    /// struct literal `n { … }`. `parse_block` resets the flag for the body.
    fn parse_for_expr(&mut self) -> Result<Expr, CompileError> {
        let start = self.span();
        self.expect(&TokenKind::For)?;
        let var = self.expect_ident()?;
        self.expect(&TokenKind::In)?;
        let start_expr = self.with_no_struct_literal(true, |p| p.parse_expr())?;
        // `..` exclusive (default) or `..=` inclusive upper bound.
        let inclusive = matches!(self.peek(), TokenKind::DotDotEq);
        if inclusive {
            self.expect(&TokenKind::DotDotEq)?;
        } else {
            self.expect(&TokenKind::DotDot)?;
        }
        let end_expr = self.with_no_struct_literal(true, |p| p.parse_expr())?;
        let body = self.parse_block()?;
        let span = Span { start: start.start, end: self.tokens[self.pos - 1].span.end };
        let end_name = format!("__for_end_{}", start.start);

        let var_read = |v: &str| Expr { kind: ExprKind::Var(v.to_string()), span };
        let int = |n| Expr { kind: ExprKind::IntLit(n), span };
        let bin = |op, l, r| Expr {
            kind: ExprKind::BinOp { op, lhs: Box::new(l), rhs: Box::new(r) },
            span,
        };

        // loop body: [ v = v + 1;  if v >= __end { break; }  <user body> ]
        // (inclusive `..=` uses `>` so the body runs for `v == __end`.)
        let exit_op = if inclusive { BinOp::Gt } else { BinOp::Ge };
        let mut loop_stmts = vec![
            Stmt::Assign {
                place: Place { root: var.clone(), fields: Vec::new(), index: None, span },
                value: bin(BinOp::Add, var_read(&var), int(1)),
                span,
            },
            Stmt::Expr {
                expr: Expr {
                    kind: ExprKind::If {
                        cond: Box::new(bin(exit_op, var_read(&var), var_read(&end_name))),
                        then_block: Block {
                            stmts: vec![Stmt::Expr {
                                expr: Expr { kind: ExprKind::Break { value: None }, span },
                                span,
                            }],
                            tail: None,
                            span,
                        },
                        else_block: None,
                    },
                    span,
                },
                span,
            },
        ];
        loop_stmts.extend(body.stmts);
        if let Some(tail) = body.tail {
            loop_stmts.push(Stmt::Expr { expr: *tail, span });
        }
        let the_loop = Expr {
            kind: ExprKind::Loop {
                body: Block { stmts: loop_stmts, tail: None, span },
            },
            span,
        };

        // outer block: [ let __end = b;  let mut v = a - 1;  <loop> ]
        let outer = Block {
            stmts: vec![
                Stmt::Let { name: end_name, mutable: false, ty: None, init: end_expr, span },
                Stmt::Let {
                    name: var,
                    mutable: true,
                    ty: None,
                    init: bin(BinOp::Sub, start_expr, int(1)),
                    span,
                },
                Stmt::Expr { expr: the_loop, span },
            ],
            tail: None,
            span,
        };
        Ok(Expr { kind: ExprKind::Block(outer), span })
    }

    fn parse_arg_list(&mut self) -> Result<Vec<Expr>, CompileError> {
        // Call/method args sit inside `( … )` — unambiguous, so lift the
        // no-struct-literal restriction (`f(Foo { a: 1 })` is a struct literal
        // arg even inside an if/while condition).
        self.with_no_struct_literal(false, |p| {
            let mut args = Vec::new();
            if matches!(p.peek(), TokenKind::RParen) { return Ok(args); }
            loop {
                args.push(p.parse_expr()?);
                if !matches!(p.peek(), TokenKind::Comma) { break; }
                p.advance();
                // Trailing comma: `g(1, 2,)` — the comma may close the list.
                if matches!(p.peek(), TokenKind::RParen) { break; }
            }
            Ok(args)
        })
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

    /// Desugar `draw_string(x, y, "LIT", color, scale)` to a block of one
    /// `draw_char` per glyph. The literal is validated at compile time (exactly
    /// 5 args, 3rd a printable-ASCII string literal, bounded length); each char
    /// advances `x` by `6 * scale` (the glyph stride `raster::draw_number` uses).
    /// `x`/`y`/`color`/`scale` are bound to temp locals so a side-effecting arg
    /// runs once; the host boundary still sees only `draw_char` (integer ABI).
    fn desugar_draw_string(&self, args: Vec<Expr>, span: Span) -> Result<Expr, CompileError> {
        if args.len() != 5 {
            return Err(CompileError::at_code(
                codes::ARITY_MISMATCH,
                format!("draw_string expects 5 args (x, y, \"literal\", color, scale), got {}", args.len()),
                span,
            ));
        }
        let mut it = args.into_iter();
        let x = it.next().unwrap();
        let y = it.next().unwrap();
        let text_expr = it.next().unwrap();
        let color = it.next().unwrap();
        let scale = it.next().unwrap();
        let text = match &text_expr.kind {
            ExprKind::StringLit(s) => s.clone(),
            _ => return Err(CompileError::at_code(
                codes::EXPECTED_EXPRESSION,
                "draw_string's 3rd arg must be a string literal (it is lowered to draw_char calls at compile time)",
                text_expr.span,
            )),
        };
        // Bound the length: 256 glyphs is far beyond any framebuffer line and
        // keeps the unrolled block small.
        if text.len() > 256 {
            return Err(CompileError::at_code(
                codes::OVERSIZE,
                format!("draw_string literal is {} bytes; cap is 256", text.len()),
                text_expr.span,
            ));
        }
        if let Some(b) = text.bytes().find(|b| !(0x20..=0x7E).contains(b)) {
            return Err(CompileError::at_code(
                codes::UNEXPECTED_BYTE,
                format!("draw_string literal must be printable ASCII (0x20..0x7E); found byte 0x{b:02X}"),
                text_expr.span,
            ));
        }

        let int = |n| Expr { kind: ExprKind::IntLit(n), span };
        let var = |n: &str| Expr { kind: ExprKind::Var(n.to_string()), span };
        let bin = |op, l, r| Expr {
            kind: ExprKind::BinOp { op, lhs: Box::new(l), rhs: Box::new(r) },
            span,
        };
        // Unique temp names (span-keyed) so nested draw_string calls don't clash.
        let xn = format!("__ds_x_{}", span.start);
        let yn = format!("__ds_y_{}", span.start);
        let cn = format!("__ds_c_{}", span.start);
        let sn = format!("__ds_s_{}", span.start);
        let mklet = |name: String, init: Expr| Stmt::Let { name, mutable: false, ty: None, init, span };

        let mut stmts = vec![
            mklet(xn.clone(), x),
            mklet(yn.clone(), y),
            mklet(cn.clone(), color),
            mklet(sn.clone(), scale),
        ];
        // draw_char(__ds_x + (i*6) * __ds_s, __ds_y, code, __ds_c, __ds_s)
        let func = Expr {
            kind: ExprKind::Path(vec!["host".into(), "display".into(), "draw_char".into()]),
            span,
        };
        for (i, b) in text.bytes().enumerate() {
            let dx = bin(BinOp::Mul, int((i as i64) * 6), var(&sn));
            let call = Expr {
                kind: ExprKind::Call {
                    func: Box::new(func.clone()),
                    args: vec![
                        bin(BinOp::Add, var(&xn), dx),
                        var(&yn),
                        int(b as i64),
                        var(&cn),
                        var(&sn),
                    ],
                },
                span,
            };
            stmts.push(Stmt::Expr { expr: call, span });
        }
        Ok(Expr {
            kind: ExprKind::Block(Block { stmts, tail: None, span }),
            span,
        })
    }
}

/// Whether `expr` names the `draw_string` compile-time macro in any spelling:
/// bare `draw_string`, `display::draw_string`, or `host::display::draw_string`.
fn is_draw_string_path(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Var(name) => name == "draw_string",
        ExprKind::Path(segs) => segs.last().map(|s| s == "draw_string").unwrap_or(false),
        _ => false,
    }
}

fn is_block_expr(expr: &Expr) -> bool {
    matches!(
        expr.kind,
        ExprKind::While { .. }
            | ExprKind::Loop { .. }
            | ExprKind::If { .. }
            | ExprKind::Match { .. }
            | ExprKind::Block(_)
    )
}

fn expr_to_place(expr: &Expr) -> Result<Place, CompileError> {
    match &expr.kind {
        ExprKind::Var(name) => Ok(Place { root: name.clone(), fields: Vec::new(), index: None, span: expr.span }),
        ExprKind::FieldAccess { object, field } => {
            let mut place = expr_to_place(object)?;
            // A field access AFTER an index (`a[i].x`) is not a supported
            // place — `index` is the OUTERMOST component of a place.
            if place.index.is_some() {
                return Err(CompileError::at_code(codes::INVALID_ASSIGN_TARGET, "invalid assignment target", expr.span));
            }
            place.fields.push(field.clone());
            place.span = expr.span;
            Ok(place)
        }
        ExprKind::Index { base, index } => {
            // Indexed assignment target `base[index] = value`. The `base` must
            // itself be a place (var / struct-field chain); the index is the
            // single trailing component. Nested indices (`a[i][j] = v`) and
            // post-index field access are rejected — same MVP scope as reads
            // that mutate one array element.
            let mut place = expr_to_place(base)?;
            if place.index.is_some() {
                return Err(CompileError::at_code(codes::INVALID_ASSIGN_TARGET, "invalid assignment target", expr.span));
            }
            place.index = Some(Box::new((**index).clone()));
            place.span = expr.span;
            Ok(place)
        }
        _ => Err(CompileError::at_code(codes::INVALID_ASSIGN_TARGET, "invalid assignment target", expr.span)),
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
    fn parse_if_statement_then_more() {
        // An `if` used as a statement (no trailing `;`, more code after)
        // must parse — agents write control flow this way constantly.
        let m = parse_str(
            "fn f(x: i32) -> i32 { let mut a: i32 = 0; if x > 0 { a = 1; } a }",
        );
        match &m.items[0] {
            Item::Fn(f) => {
                // let, if-as-statement, and the tail `a`
                assert_eq!(f.body.stmts.len(), 2);
                assert!(f.body.tail.is_some());
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn parse_if_as_tail_value() {
        // `if`/`else` as the block tail still returns a value.
        let m = parse_str("fn abs(x: i32) -> i32 { if x > 0 { x } else { 0 - x } }");
        match &m.items[0] {
            Item::Fn(f) => {
                assert!(f.body.stmts.is_empty());
                assert!(matches!(
                    f.body.tail.as_deref().map(|e| &e.kind),
                    Some(ExprKind::If { .. })
                ));
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn parse_compound_assign_desugars() {
        // `x += 5` → `x = x + 5`; `x -= 2` → `x = x - 2`. Operand order matters
        // for the non-commutative ops (`-= /= %=`): the place is the LHS.
        let m = parse_str("fn f() { let mut x: i32 = 0; x += 5; x -= 2; }");
        let Item::Fn(f) = &m.items[0] else {
            panic!("expected fn")
        };
        let check = |s: &Stmt, add: bool, want_rhs: i64| {
            let Stmt::Assign { place, value, .. } = s else {
                panic!("expected Assign")
            };
            assert_eq!(place.root, "x");
            let ExprKind::BinOp { op, lhs, rhs } = &value.kind else {
                panic!("expected BinOp value")
            };
            assert!(if add { matches!(op, BinOp::Add) } else { matches!(op, BinOp::Sub) });
            assert!(matches!(&lhs.kind, ExprKind::Var(n) if n == "x"));
            assert!(matches!(&rhs.kind, ExprKind::IntLit(n) if *n == want_rhs));
        };
        check(&f.body.stmts[1], true, 5);
        check(&f.body.stmts[2], false, 2);
    }

    fn parse_err(s: &str) -> CompileError {
        let tokens = lexer::lex(s).unwrap();
        parse(&tokens).expect_err("expected a parse error")
    }

    #[test]
    fn draw_string_desugars_to_a_draw_char_per_glyph() {
        // `draw_string(x, y, "ab", c, s)` lowers to a block of 4 temp `let`s +
        // one `draw_char` Call per character (here 2). No `draw_string` survives.
        let m = parse_str(r#"fn frame(t: i32) { host::display::draw_string(0, 0, "ab", 255, 1); }"#);
        let Item::Fn(f) = &m.items[0] else { panic!("expected fn") };
        let Stmt::Expr { expr, .. } = &f.body.stmts[0] else { panic!("expected stmt") };
        let ExprKind::Block(b) = &expr.kind else { panic!("draw_string should desugar to a block") };
        // 4 temp lets (x, y, color, scale) + 2 draw_char calls.
        assert_eq!(b.stmts.len(), 6);
        let calls = b.stmts.iter().filter(|s| matches!(
            s, Stmt::Expr { expr, .. } if matches!(&expr.kind, ExprKind::Call { func, .. }
                if matches!(&func.kind, ExprKind::Path(p) if p.last().unwrap() == "draw_char"))
        )).count();
        assert_eq!(calls, 2, "one draw_char per glyph");
    }

    #[test]
    fn draw_string_rejects_non_literal_third_arg() {
        // The 3rd arg MUST be a string literal (it is lowered at compile time).
        let err = parse_err("fn frame(t: i32) { host::display::draw_string(0, 0, t, 255, 1); }");
        assert_eq!(err.code, Some(codes::EXPECTED_EXPRESSION));
    }

    #[test]
    fn draw_string_rejects_non_ascii_and_bad_arity() {
        // Non-printable bytes in the literal are rejected (here a `\t` escape →
        // byte 0x09, below the printable-ASCII floor 0x20).
        assert_eq!(
            parse_err(r#"fn frame(t: i32) { host::display::draw_string(0, 0, "\t", 255, 1); }"#).code,
            Some(codes::UNEXPECTED_BYTE),
        );
        // Wrong arg count is a clean arity error, not a panic.
        assert_eq!(
            parse_err("fn frame(t: i32) { host::display::draw_string(0, 0, \"x\"); }").code,
            Some(codes::ARITY_MISMATCH),
        );
    }

    #[test]
    fn parse_for_desugars_to_loop_with_top_increment() {
        // `for i in 0..3 { … }` desugars to:
        //   { let __end = 3; let mut i = 0 - 1;
        //     loop { i = i + 1; if i >= __end { break; } … } }
        // The increment-at-TOP is the whole point — it keeps `continue` correct.
        let m = parse_str("fn f() { for i in 0..3 { let x: i32 = i; } }");
        let Item::Fn(f) = &m.items[0] else {
            panic!("expected fn")
        };
        let for_expr = f
            .body
            .tail
            .as_deref()
            .or_else(|| match f.body.stmts.last() {
                Some(Stmt::Expr { expr, .. }) => Some(expr),
                _ => None,
            })
            .expect("for expr");
        let ExprKind::Block(outer) = &for_expr.kind else {
            panic!("for should desugar to a block")
        };
        assert_eq!(outer.stmts.len(), 3, "let __end, let mut i, loop");
        // `let mut i = start - 1`
        match &outer.stmts[1] {
            Stmt::Let { name, mutable, init, .. } => {
                assert_eq!(name, "i");
                assert!(*mutable);
                assert!(matches!(&init.kind, ExprKind::BinOp { op: BinOp::Sub, .. }));
            }
            _ => panic!("expected `let mut i`"),
        }
        // the loop: body starts with `i = i + 1` then the `if i >= __end { break }`
        let Stmt::Expr { expr, .. } = &outer.stmts[2] else {
            panic!("expected loop statement")
        };
        let ExprKind::Loop { body } = &expr.kind else {
            panic!("expected a loop")
        };
        match &body.stmts[0] {
            Stmt::Assign { value, .. } => {
                assert!(matches!(&value.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
            }
            _ => panic!("loop body must start with the increment"),
        }
        assert!(matches!(
            &body.stmts[1],
            Stmt::Expr { expr, .. } if matches!(&expr.kind, ExprKind::If { .. })
        ));
    }

    #[test]
    fn parse_bitwise_precedence() {
        // Shift is LOOSER than additive (Rust): `1 + 2 << 3` → `(1 + 2) << 3`.
        let m = parse_str("fn f() -> i32 { 1 + 2 << 3 }");
        let Item::Fn(f) = &m.items[0] else {
            panic!("expected fn")
        };
        let tail = f.body.tail.as_deref().expect("tail expr");
        let ExprKind::BinOp { op: BinOp::Shl, lhs, .. } = &tail.kind else {
            panic!("top operator should be Shl")
        };
        assert!(matches!(&lhs.kind, ExprKind::BinOp { op: BinOp::Add, .. }));

        // `&` binds tighter than `|`: `1 | 2 & 3` → `1 | (2 & 3)`.
        let m = parse_str("fn f() -> i32 { 1 | 2 & 3 }");
        let Item::Fn(f) = &m.items[0] else {
            panic!("expected fn")
        };
        let tail = f.body.tail.as_deref().expect("tail expr");
        let ExprKind::BinOp { op: BinOp::BitOr, rhs, .. } = &tail.kind else {
            panic!("top operator should be BitOr")
        };
        assert!(matches!(&rhs.kind, ExprKind::BinOp { op: BinOp::BitAnd, .. }));
    }

    #[test]
    fn parse_match_range_pattern() {
        // `0..=5 =>` parses to an inclusive IntRange pattern.
        let m = parse_str("fn f(x: i32) -> i32 { match x { 0..=5 => 1, _ => 2 } }");
        let Item::Fn(f) = &m.items[0] else {
            panic!("expected fn")
        };
        let tail = f.body.tail.as_deref().expect("match tail");
        let ExprKind::Match { arms, .. } = &tail.kind else {
            panic!("expected match")
        };
        assert!(matches!(
            &arms[0].pattern.kind,
            PatternKind::IntRange { lo: 0, hi: 5, inclusive: true }
        ));
    }

    #[test]
    fn parse_trailing_comma_in_fn_params() {
        // `fn f(a: i32,)` — idiomatic Rust; the trailing comma must not error.
        let m = parse_str("fn f(a: i32,) -> i32 { a }");
        let Item::Fn(f) = &m.items[0] else {
            panic!("expected fn")
        };
        assert_eq!(f.params.len(), 1);
        assert_eq!(f.params[0].name, "a");
        // Multiple params with a trailing comma too.
        let m = parse_str("fn g(a: i32, b: i32,) -> i32 { a + b }");
        let Item::Fn(g) = &m.items[0] else {
            panic!("expected fn")
        };
        assert_eq!(g.params.len(), 2);
    }

    #[test]
    fn parse_trailing_comma_in_call_args() {
        // `g(1, 2,)` and `g(1,)` — a trailing arg comma must not error.
        let m = parse_str("fn f() -> i32 { g(1, 2,) } fn g(a: i32, b: i32) -> i32 { a + b }");
        let Item::Fn(f) = &m.items[0] else {
            panic!("expected fn")
        };
        let tail = f.body.tail.as_deref().expect("call tail");
        let ExprKind::Call { args, .. } = &tail.kind else {
            panic!("expected call")
        };
        assert_eq!(args.len(), 2);
        // Single arg with a trailing comma.
        let m = parse_str("fn f() -> i32 { g(1,) } fn g(a: i32) -> i32 { a }");
        let Item::Fn(f) = &m.items[0] else {
            panic!("expected fn")
        };
        let tail = f.body.tail.as_deref().expect("call tail");
        let ExprKind::Call { args, .. } = &tail.kind else {
            panic!("expected call")
        };
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn parse_negative_int_and_range_patterns() {
        // A negative literal arm: `-1 => …`.
        let m = parse_str("fn f(x: i32) -> i32 { match x { -1 => 1, _ => 2 } }");
        let Item::Fn(f) = &m.items[0] else {
            panic!("expected fn")
        };
        let tail = f.body.tail.as_deref().expect("match tail");
        let ExprKind::Match { arms, .. } = &tail.kind else {
            panic!("expected match")
        };
        assert!(matches!(
            &arms[0].pattern.kind,
            PatternKind::Literal(LitPattern::Int(-1))
        ));

        // A negative-bounded range arm: `-5..=5 => …`.
        let m = parse_str("fn f(x: i32) -> i32 { match x { -5..=5 => 1, _ => 2 } }");
        let Item::Fn(f) = &m.items[0] else {
            panic!("expected fn")
        };
        let tail = f.body.tail.as_deref().expect("match tail");
        let ExprKind::Match { arms, .. } = &tail.kind else {
            panic!("expected match")
        };
        assert!(matches!(
            &arms[0].pattern.kind,
            PatternKind::IntRange { lo: -5, hi: 5, inclusive: true }
        ));

        // Both bounds negative: `-5..=-1 => …`.
        let m = parse_str("fn f(x: i32) -> i32 { match x { -5..=-1 => 1, _ => 2 } }");
        let Item::Fn(f) = &m.items[0] else {
            panic!("expected fn")
        };
        let tail = f.body.tail.as_deref().expect("match tail");
        let ExprKind::Match { arms, .. } = &tail.kind else {
            panic!("expected match")
        };
        assert!(matches!(
            &arms[0].pattern.kind,
            PatternKind::IntRange { lo: -5, hi: -1, inclusive: true }
        ));
    }

    #[test]
    fn parse_pub_visibility_is_ignored() {
        // `pub` / `pub(crate)` on items and struct fields is accepted and
        // discarded — rustlite has one flat module, but agents write `pub`.
        let m = parse_str(
            "pub struct P { pub x: i32, y: i32 } pub(crate) fn f() -> i32 { 0 } pub const K: i32 = 1;",
        );
        assert_eq!(m.items.len(), 3);
        match &m.items[0] {
            Item::Struct(s) => {
                assert_eq!(s.name, "P");
                assert_eq!(s.fields.len(), 2);
                assert_eq!(s.fields[0].name, "x");
            }
            _ => panic!("expected struct"),
        }
        assert!(matches!(&m.items[1], Item::Fn(f) if f.name == "f"));
        assert!(matches!(&m.items[2], Item::Const(c) if c.name == "K"));
    }

    #[test]
    fn parse_attributes_are_skipped() {
        // `#[...]` outer and `#![...]` inner attributes are lexer trivia.
        let m = parse_str(
            "#![allow(dead_code)]\n#[no_mangle]\nfn frame(t: i32) -> i32 { #[allow(unused)] let mut a: i32 = t; a }",
        );
        assert_eq!(m.items.len(), 1);
        assert!(matches!(&m.items[0], Item::Fn(f) if f.name == "frame"));
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

    // ── no-struct-literal restriction (the `if a < b {` parser surprise) ──
    //
    // Rust forbids a BARE struct literal in the condition of if/while, the
    // iterable of for, and the scrutinee of match — so the `{` after an
    // identifier opens the BLOCK, not a struct literal. These guard that
    // `if a < b { … }` / `while i < n { … }` / `for k in 0..n { … }` (identifier
    // upper bounds) compile, while struct literals keep working everywhere they
    // are unambiguous.

    #[test]
    fn if_cond_ident_lt_ident_is_not_struct_lit() {
        // `if a < b { … }` — `b` is a VARIABLE, `{` opens the block. This was
        // misparsed as `b { … }` (struct literal) before the restriction.
        let m = parse_str("fn f(a: i32, b: i32) -> i32 { if a < b { 1 } else { 0 } }");
        let Item::Fn(f) = &m.items[0] else { panic!("expected fn") };
        let tail = f.body.tail.as_deref().expect("if tail");
        let ExprKind::If { cond, .. } = &tail.kind else { panic!("expected if") };
        let ExprKind::BinOp { op: BinOp::Lt, lhs, rhs } = &cond.kind else {
            panic!("condition should be `a < b`")
        };
        assert!(matches!(&lhs.kind, ExprKind::Var(n) if n == "a"));
        assert!(matches!(&rhs.kind, ExprKind::Var(n) if n == "b"), "rhs must be the variable `b`, not a struct literal");
    }

    #[test]
    fn while_cond_ident_bound_is_not_struct_lit() {
        // `while i < n { … }` — `n` is the loop bound, not a struct head.
        let m = parse_str("fn f(n: i32) { let mut i: i32 = 0; while i < n { i = i + 1; } }");
        let Item::Fn(f) = &m.items[0] else { panic!("expected fn") };
        let Stmt::Expr { expr, .. } = &f.body.stmts[1] else { panic!("expected while stmt") };
        let ExprKind::While { cond, .. } = &expr.kind else { panic!("expected while") };
        let ExprKind::BinOp { op: BinOp::Lt, rhs, .. } = &cond.kind else {
            panic!("condition should be `i < n`")
        };
        assert!(matches!(&rhs.kind, ExprKind::Var(n) if n == "n"));
    }

    #[test]
    fn for_iterable_ident_bound_compiles() {
        // `for k in 0..n { … }` — `n` is the upper bound, not a struct head.
        // (for desugars to a block; just assert it parses without erroring.)
        assert!(
            try_parse("fn f(n: i32) { let mut s: i32 = 0; for k in 0..n { s = s + k; } }").is_ok(),
            "`for k in 0..n {{ … }}` (identifier bound) must compile"
        );
    }

    #[test]
    fn match_scrutinee_ident_is_not_struct_lit() {
        // `match x { … }` — `x` is the scrutinee variable, `{` opens the arms.
        let m = parse_str("fn f(x: i32) -> i32 { match x { 0 => 1, _ => 2 } }");
        let Item::Fn(f) = &m.items[0] else { panic!("expected fn") };
        let tail = f.body.tail.as_deref().expect("match tail");
        let ExprKind::Match { scrutinee, .. } = &tail.kind else { panic!("expected match") };
        assert!(matches!(&scrutinee.kind, ExprKind::Var(n) if n == "x"));
    }

    #[test]
    fn struct_literal_still_parses_in_normal_positions() {
        // The restriction is ONLY for if/while/for/match heads. Struct literals
        // in let-init, return, and call-arg positions must still parse.
        let m = parse_str(
            "fn g(p: Point) -> Point { let q: Point = Point { x: 1, y: 2 }; g(Point { x: 3, y: 4 }); return Point { x: 5, y: 6 }; }",
        );
        let Item::Fn(f) = &m.items[0] else { panic!("expected fn") };
        // let q = Point { .. }
        let Stmt::Let { init, .. } = &f.body.stmts[0] else { panic!("expected let") };
        assert!(matches!(init.kind, ExprKind::StructLit { .. }), "let-init struct literal");
        // g(Point { .. })
        let Stmt::Expr { expr, .. } = &f.body.stmts[1] else { panic!("expected call stmt") };
        let ExprKind::Call { args, .. } = &expr.kind else { panic!("expected call") };
        assert!(matches!(args[0].kind, ExprKind::StructLit { .. }), "call-arg struct literal");
        // return Point { .. }
        let Stmt::Return { value: Some(v), .. } = &f.body.stmts[2] else { panic!("expected return") };
        assert!(matches!(v.kind, ExprKind::StructLit { .. }), "return struct literal");
    }

    #[test]
    fn struct_literal_in_condition_works_with_parens() {
        // A struct literal IS allowed in a condition when parenthesised:
        // `if x == (Foo { a: 1 }) { … }`.
        let m = parse_str("fn f(x: Foo) -> i32 { if x == (Foo { a: 1 }) { 1 } else { 0 } }");
        let Item::Fn(f) = &m.items[0] else { panic!("expected fn") };
        let tail = f.body.tail.as_deref().expect("if tail");
        let ExprKind::If { cond, .. } = &tail.kind else { panic!("expected if") };
        let ExprKind::BinOp { op: BinOp::Eq, rhs, .. } = &cond.kind else {
            panic!("condition should be `x == (Foo {{ … }})`")
        };
        assert!(matches!(rhs.kind, ExprKind::StructLit { .. }), "parenthesised struct literal in condition");
    }

    #[test]
    fn condition_body_can_use_struct_literals() {
        // The block BODY of an if/while is unrestricted — a bare struct literal
        // there still parses (the flag is reset for the body).
        let m = parse_str("fn f(c: bool) -> Point { if c { Point { x: 1, y: 2 } } else { Point { x: 0, y: 0 } } }");
        let Item::Fn(f) = &m.items[0] else { panic!("expected fn") };
        let tail = f.body.tail.as_deref().expect("if tail");
        let ExprKind::If { then_block, .. } = &tail.kind else { panic!("expected if") };
        assert!(
            matches!(then_block.tail.as_deref().map(|e| &e.kind), Some(ExprKind::StructLit { .. })),
            "struct literal in the then-block body"
        );
    }

    #[test]
    fn for_inclusive_range_runs_through_upper_bound() {
        // `for i in 0..=n` desugars with a `>` exit test (vs `>=` for `..`), so
        // the body runs for `i == n`. Assert the inclusive form picks `Gt`.
        let m = parse_str("fn f(n: i32) { for i in 0..=n { let x: i32 = i; } }");
        let Item::Fn(f) = &m.items[0] else { panic!("expected fn") };
        let for_expr = f.body.tail.as_deref().or_else(|| match f.body.stmts.last() {
            Some(Stmt::Expr { expr, .. }) => Some(expr),
            _ => None,
        }).expect("for expr");
        let ExprKind::Block(outer) = &for_expr.kind else { panic!("for should desugar to a block") };
        let Stmt::Expr { expr, .. } = &outer.stmts[2] else { panic!("expected loop stmt") };
        let ExprKind::Loop { body } = &expr.kind else { panic!("expected loop") };
        // body[1] is `if i > __end { break; }` for inclusive ranges.
        let Stmt::Expr { expr, .. } = &body.stmts[1] else { panic!("expected exit-check stmt") };
        let ExprKind::If { cond, .. } = &expr.kind else { panic!("expected if") };
        assert!(
            matches!(&cond.kind, ExprKind::BinOp { op: BinOp::Gt, .. }),
            "inclusive `..=` must use `>` for the exit test (so the body runs at i == n)"
        );
        // And the exclusive form still uses `>=`.
        let m = parse_str("fn f(n: i32) { for i in 0..n { let x: i32 = i; } }");
        let Item::Fn(f) = &m.items[0] else { panic!("expected fn") };
        let for_expr = f.body.tail.as_deref().or_else(|| match f.body.stmts.last() {
            Some(Stmt::Expr { expr, .. }) => Some(expr),
            _ => None,
        }).expect("for expr");
        let ExprKind::Block(outer) = &for_expr.kind else { panic!("for should desugar to a block") };
        let Stmt::Expr { expr, .. } = &outer.stmts[2] else { panic!("expected loop stmt") };
        let ExprKind::Loop { body } = &expr.kind else { panic!("expected loop") };
        let Stmt::Expr { expr, .. } = &body.stmts[1] else { panic!("expected exit-check stmt") };
        let ExprKind::If { cond, .. } = &expr.kind else { panic!("expected if") };
        assert!(
            matches!(&cond.kind, ExprKind::BinOp { op: BinOp::Ge, .. }),
            "exclusive `..` must use `>=` for the exit test"
        );
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

    fn try_parse(s: &str) -> Result<Module, CompileError> {
        let tokens = lexer::lex(s).unwrap();
        parse(&tokens)
    }

    // ── Recursion-depth guard (DoS) ─────────────────────────────────
    // These must return a CompileError, NOT overflow the stack. A
    // regression here aborts the test process (uncatchable) instead of
    // failing — which is itself the signal that the guard is gone.

    #[test]
    fn deeply_nested_parens_error_not_overflow() {
        let n = MAX_RECURSION_DEPTH + 5_000;
        let src = format!(
            "fn f() -> i32 {{ {}1{} }}",
            "(".repeat(n),
            ")".repeat(n)
        );
        assert!(try_parse(&src).is_err(), "deep paren nesting must error, not overflow");
    }

    #[test]
    fn deeply_nested_unary_error_not_overflow() {
        let n = MAX_RECURSION_DEPTH + 5_000;
        let src = format!("fn f() -> i32 {{ {}1 }}", "-".repeat(n));
        assert!(try_parse(&src).is_err(), "deep unary chain must error, not overflow");
    }

    #[test]
    fn modest_nesting_still_parses() {
        // Well under the cap — must still compile fine.
        let src = "fn f() -> i32 { (((((1)))))  }";
        assert!(try_parse(src).is_ok(), "modest nesting must still parse");
    }
}
