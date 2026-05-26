use std::collections::HashMap;
use crate::rustlite::{CompileError, Span};
use crate::rustlite::ast::*;

pub fn check(module: &Module) -> Result<TypedModule, CompileError> {
    let mut ctx = TypeContext::new();
    ctx.register_module(module)?;
    ctx.check_module(module)
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedType {
    I32,
    I64,
    F32,
    F64,
    Bool,
    String,
    Void,
    Never,
    Struct { name: String, fields: Vec<(String, ResolvedType)> },
    Enum { name: String, variants: Vec<(String, VariantShape)> },
    Tuple(Vec<ResolvedType>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum VariantShape {
    Unit,
    Tuple(Vec<ResolvedType>),
    Struct(Vec<(String, ResolvedType)>),
}

#[derive(Debug, Clone)]
pub struct TypedModule {
    pub uses: Vec<UseDecl>,
    pub structs: Vec<TypedStruct>,
    pub enums: Vec<TypedEnum>,
    pub functions: Vec<TypedFn>,
    pub consts: Vec<TypedConst>,
}

#[derive(Debug, Clone)]
pub struct TypedStruct {
    pub name: String,
    pub fields: Vec<(String, ResolvedType)>,
}

#[derive(Debug, Clone)]
pub struct TypedEnum {
    pub name: String,
    pub variants: Vec<(String, VariantShape)>,
}

#[derive(Debug, Clone)]
pub struct TypedFn {
    pub name: String,
    pub params: Vec<(String, ResolvedType)>,
    pub ret_type: ResolvedType,
    pub body: TypedBlock,
}

#[derive(Debug, Clone)]
pub struct TypedConst {
    pub name: String,
    pub ty: ResolvedType,
    pub value: TypedExpr,
}

#[derive(Debug, Clone)]
pub struct TypedBlock {
    pub stmts: Vec<TypedStmt>,
    pub tail: Option<Box<TypedExpr>>,
    pub ty: ResolvedType,
}

#[derive(Debug, Clone)]
pub enum TypedStmt {
    Let { name: String, mutable: bool, ty: ResolvedType, init: TypedExpr },
    Assign { place: Place, value: TypedExpr },
    Return { value: Option<TypedExpr> },
    Expr { expr: TypedExpr },
}

#[derive(Debug, Clone)]
pub struct TypedExpr {
    pub kind: TypedExprKind,
    pub ty: ResolvedType,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TypedExprKind {
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),
    BoolLit(bool),
    Var(String),
    Path(Vec<String>),

    FieldAccess { object: Box<TypedExpr>, field: String, field_index: usize },

    Call { func: Box<TypedExpr>, args: Vec<TypedExpr> },
    MethodCall { object: Box<TypedExpr>, method: String, args: Vec<TypedExpr> },

    StructLit { name: String, fields: Vec<(String, TypedExpr)> },

    TupleLit(Vec<TypedExpr>),

    BinOp { op: BinOp, lhs: Box<TypedExpr>, rhs: Box<TypedExpr> },
    UnaryOp { op: UnaryOp, operand: Box<TypedExpr> },

    If { cond: Box<TypedExpr>, then_block: TypedBlock, else_block: Option<TypedElse> },
    Match { scrutinee: Box<TypedExpr>, arms: Vec<TypedMatchArm>, result_ty: ResolvedType },
    While { cond: Box<TypedExpr>, body: TypedBlock },
    Loop { body: TypedBlock },
    Break { value: Option<Box<TypedExpr>> },
    Continue,
    Block(TypedBlock),
}

#[derive(Debug, Clone)]
pub enum TypedElse {
    Block(TypedBlock),
    If(Box<TypedExpr>),
}

#[derive(Debug, Clone)]
pub struct TypedMatchArm {
    pub pattern: Pattern,
    pub body: TypedExpr,
}

#[derive(Debug, Clone)]
struct FnSig {
    params: Vec<ResolvedType>,
    ret: ResolvedType,
}

struct TypeContext {
    types: HashMap<String, ResolvedType>,
    functions: HashMap<String, FnSig>,
    locals: Vec<HashMap<String, (ResolvedType, bool)>>,
    current_return: ResolvedType,
}

impl TypeContext {
    fn new() -> Self {
        Self {
            types: HashMap::new(),
            functions: HashMap::new(),
            locals: Vec::new(),
            current_return: ResolvedType::Void,
        }
    }

    fn push_scope(&mut self) {
        self.locals.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.locals.pop();
    }

    fn define_local(&mut self, name: &str, ty: ResolvedType, mutable: bool) {
        if let Some(scope) = self.locals.last_mut() {
            scope.insert(name.to_string(), (ty, mutable));
        }
    }

    fn lookup_local(&self, name: &str) -> Option<&(ResolvedType, bool)> {
        for scope in self.locals.iter().rev() {
            if let Some(entry) = scope.get(name) {
                return Some(entry);
            }
        }
        None
    }

    fn resolve_ty(&self, ty: &Ty) -> Result<ResolvedType, CompileError> {
        match ty {
            Ty::I32 => Ok(ResolvedType::I32),
            Ty::I64 => Ok(ResolvedType::I64),
            Ty::F32 => Ok(ResolvedType::F32),
            Ty::F64 => Ok(ResolvedType::F64),
            Ty::Bool => Ok(ResolvedType::Bool),
            Ty::String => Ok(ResolvedType::String),
            Ty::Named(name) => {
                self.types.get(name).cloned()
                    .ok_or_else(|| CompileError::new(format!("unknown type '{name}'")))
            }
            Ty::Tuple(tys) => {
                let resolved: Result<Vec<_>, _> = tys.iter().map(|t| self.resolve_ty(t)).collect();
                Ok(ResolvedType::Tuple(resolved?))
            }
        }
    }

    fn register_module(&mut self, module: &Module) -> Result<(), CompileError> {
        // First pass: register all type and fn signatures
        for item in &module.items {
            match item {
                Item::Struct(s) => {
                    let fields: Result<Vec<(String, ResolvedType)>, CompileError> = s.fields.iter()
                        .map(|f| Ok((f.name.clone(), self.resolve_ty(&f.ty)?)))
                        .collect();
                    let ty = ResolvedType::Struct { name: s.name.clone(), fields: fields? };
                    self.types.insert(s.name.clone(), ty);
                }
                Item::Enum(e) => {
                    let variants: Result<Vec<(String, VariantShape)>, CompileError> = e.variants.iter()
                        .map(|v| {
                            let shape = match &v.payload {
                                VariantPayload::Unit => VariantShape::Unit,
                                VariantPayload::Tuple(tys) => {
                                    let resolved: Result<Vec<ResolvedType>, CompileError> = tys.iter().map(|t| self.resolve_ty(t)).collect();
                                    VariantShape::Tuple(resolved?)
                                }
                                VariantPayload::Struct(fields) => {
                                    let resolved: Result<Vec<(String, ResolvedType)>, CompileError> = fields.iter()
                                        .map(|f| Ok((f.name.clone(), self.resolve_ty(&f.ty)?)))
                                        .collect();
                                    VariantShape::Struct(resolved?)
                                }
                            };
                            Ok((v.name.clone(), shape))
                        })
                        .collect();
                    let ty = ResolvedType::Enum { name: e.name.clone(), variants: variants? };
                    self.types.insert(e.name.clone(), ty);
                }
                Item::Fn(f) => {
                    let params: Result<Vec<_>, _> = f.params.iter()
                        .map(|p| self.resolve_ty(&p.ty))
                        .collect();
                    let ret = f.ret_type.as_ref()
                        .map(|t| self.resolve_ty(t))
                        .transpose()?
                        .unwrap_or(ResolvedType::Void);
                    self.functions.insert(f.name.clone(), FnSig { params: params?, ret });
                }
                Item::Const(_) => {} // handled in check pass
            }
        }
        Ok(())
    }

    fn check_module(&mut self, module: &Module) -> Result<TypedModule, CompileError> {
        let mut structs = Vec::new();
        let mut enums = Vec::new();
        let mut functions = Vec::new();
        let mut consts = Vec::new();

        for item in &module.items {
            match item {
                Item::Struct(s) => {
                    if let ResolvedType::Struct { name, fields } = self.types.get(&s.name).unwrap().clone() {
                        structs.push(TypedStruct { name, fields });
                    }
                }
                Item::Enum(e) => {
                    if let ResolvedType::Enum { name, variants } = self.types.get(&e.name).unwrap().clone() {
                        enums.push(TypedEnum { name, variants });
                    }
                }
                Item::Fn(f) => {
                    functions.push(self.check_fn(f)?);
                }
                Item::Const(c) => {
                    let ty = self.resolve_ty(&c.ty)?;
                    self.push_scope();
                    let value = self.check_expr(&c.value)?;
                    self.pop_scope();
                    if value.ty != ty {
                        return Err(CompileError::at(
                            format!("const type mismatch: expected {ty:?}, got {:?}", value.ty),
                            c.span,
                        ));
                    }
                    consts.push(TypedConst { name: c.name.clone(), ty, value });
                }
            }
        }

        Ok(TypedModule { uses: module.uses.clone(), structs, enums, functions, consts })
    }

    fn check_fn(&mut self, f: &FnDecl) -> Result<TypedFn, CompileError> {
        let sig = self.functions.get(&f.name).unwrap().clone();
        self.current_return = sig.ret.clone();

        self.push_scope();
        let mut params = Vec::new();
        for (param, ty) in f.params.iter().zip(sig.params.iter()) {
            self.define_local(&param.name, ty.clone(), false);
            params.push((param.name.clone(), ty.clone()));
        }

        let body = self.check_block(&f.body)?;
        self.pop_scope();

        if sig.ret != ResolvedType::Void && body.ty != sig.ret && body.ty != ResolvedType::Never {
            return Err(CompileError::at(
                format!("fn '{}': body returns {:?}, expected {:?}", f.name, body.ty, sig.ret),
                f.span,
            ));
        }

        Ok(TypedFn { name: f.name.clone(), params, ret_type: sig.ret, body })
    }

    fn check_block(&mut self, block: &Block) -> Result<TypedBlock, CompileError> {
        self.push_scope();
        let mut stmts = Vec::new();

        for stmt in &block.stmts {
            stmts.push(self.check_stmt(stmt)?);
        }

        let (tail, ty) = if let Some(tail_expr) = &block.tail {
            let typed = self.check_expr(tail_expr)?;
            let ty = typed.ty.clone();
            (Some(Box::new(typed)), ty)
        } else {
            (None, ResolvedType::Void)
        };

        self.pop_scope();
        Ok(TypedBlock { stmts, tail, ty })
    }

    fn check_stmt(&mut self, stmt: &Stmt) -> Result<TypedStmt, CompileError> {
        match stmt {
            Stmt::Let { name, mutable, ty, init, span } => {
                let init_typed = self.check_expr(init)?;
                let resolved_ty = if let Some(declared) = ty {
                    let declared = self.resolve_ty(declared)?;
                    if init_typed.ty != declared {
                        return Err(CompileError::at(
                            format!("let type mismatch: declared {:?}, got {:?}", declared, init_typed.ty),
                            *span,
                        ));
                    }
                    declared
                } else {
                    init_typed.ty.clone()
                };
                self.define_local(name, resolved_ty.clone(), *mutable);
                Ok(TypedStmt::Let { name: name.clone(), mutable: *mutable, ty: resolved_ty, init: init_typed })
            }
            Stmt::Assign { place, value, span } => {
                let (local_ty, is_mut) = self.lookup_local(&place.root)
                    .ok_or_else(|| CompileError::at(format!("undefined variable '{}'", place.root), *span))?
                    .clone();
                if !is_mut {
                    return Err(CompileError::at(format!("'{}' is not mutable", place.root), *span));
                }
                let mut target_ty = local_ty;
                for field in &place.fields {
                    target_ty = self.field_type(&target_ty, field, *span)?;
                }
                let val = self.check_expr(value)?;
                if val.ty != target_ty {
                    return Err(CompileError::at(
                        format!("assignment type mismatch: expected {:?}, got {:?}", target_ty, val.ty),
                        *span,
                    ));
                }
                Ok(TypedStmt::Assign { place: place.clone(), value: val })
            }
            Stmt::Return { value, span } => {
                let val = value.as_ref().map(|v| self.check_expr(v)).transpose()?;
                let ret_ty = val.as_ref().map(|v| v.ty.clone()).unwrap_or(ResolvedType::Void);
                if ret_ty != self.current_return {
                    return Err(CompileError::at(
                        format!("return type mismatch: expected {:?}, got {:?}", self.current_return, ret_ty),
                        *span,
                    ));
                }
                Ok(TypedStmt::Return { value: val })
            }
            Stmt::Expr { expr, .. } => {
                let typed = self.check_expr(expr)?;
                Ok(TypedStmt::Expr { expr: typed })
            }
        }
    }

    fn field_type(&self, ty: &ResolvedType, field: &str, span: Span) -> Result<ResolvedType, CompileError> {
        match ty {
            ResolvedType::Struct { fields, .. } => {
                fields.iter()
                    .find(|(name, _)| name == field)
                    .map(|(_, ty)| ty.clone())
                    .ok_or_else(|| CompileError::at(format!("no field '{field}' on struct"), span))
            }
            _ => Err(CompileError::at(format!("field access on non-struct type {:?}", ty), span)),
        }
    }

    fn check_expr(&mut self, expr: &Expr) -> Result<TypedExpr, CompileError> {
        let span = expr.span;
        match &expr.kind {
            ExprKind::IntLit(n) => Ok(TypedExpr { kind: TypedExprKind::IntLit(*n), ty: ResolvedType::I32, span }),
            ExprKind::FloatLit(n) => Ok(TypedExpr { kind: TypedExprKind::FloatLit(*n), ty: ResolvedType::F64, span }),
            ExprKind::StringLit(s) => Ok(TypedExpr { kind: TypedExprKind::StringLit(s.clone()), ty: ResolvedType::String, span }),
            ExprKind::BoolLit(b) => Ok(TypedExpr { kind: TypedExprKind::BoolLit(*b), ty: ResolvedType::Bool, span }),

            ExprKind::Var(name) => {
                if let Some((ty, _)) = self.lookup_local(name) {
                    Ok(TypedExpr { kind: TypedExprKind::Var(name.clone()), ty: ty.clone(), span })
                } else {
                    // Could be a function name
                    if self.functions.contains_key(name) {
                        Ok(TypedExpr { kind: TypedExprKind::Var(name.clone()), ty: ResolvedType::Void, span })
                    } else {
                        Err(CompileError::at(format!("undefined variable '{name}'"), span))
                    }
                }
            }

            ExprKind::Path(segments) => {
                // Could be an enum variant constructor
                if segments.len() == 2 {
                    if let Some(ResolvedType::Enum { name, variants }) = self.types.get(&segments[0]).cloned() {
                        if let Some((_, shape)) = variants.iter().find(|(vn, _)| *vn == segments[1]) {
                            if matches!(shape, VariantShape::Unit) {
                                return Ok(TypedExpr {
                                    kind: TypedExprKind::Path(segments.clone()),
                                    ty: ResolvedType::Enum { name, variants },
                                    span,
                                });
                            }
                        }
                    }
                }
                Ok(TypedExpr { kind: TypedExprKind::Path(segments.clone()), ty: ResolvedType::Void, span })
            }

            ExprKind::FieldAccess { object, field } => {
                let obj = self.check_expr(object)?;
                let field_ty = self.field_type(&obj.ty, field, span)?;
                let field_index = match &obj.ty {
                    ResolvedType::Struct { fields, .. } => {
                        fields.iter().position(|(n, _)| n == field).unwrap_or(0)
                    }
                    _ => 0,
                };
                Ok(TypedExpr {
                    ty: field_ty,
                    kind: TypedExprKind::FieldAccess { object: Box::new(obj), field: field.clone(), field_index },
                    span,
                })
            }

            ExprKind::Call { func, args } => {
                let checked_args: Result<Vec<_>, _> = args.iter().map(|a| self.check_expr(a)).collect();
                let checked_args = checked_args?;

                // Resolve function name
                let fn_name = match &func.kind {
                    ExprKind::Var(name) => name.clone(),
                    ExprKind::Path(segments) => segments.join("::"),
                    _ => return Err(CompileError::at("cannot call non-function", span)),
                };

                if let Some(sig) = self.functions.get(&fn_name).cloned() {
                    if checked_args.len() != sig.params.len() {
                        return Err(CompileError::at(
                            format!("fn '{fn_name}' expects {} args, got {}", sig.params.len(), checked_args.len()),
                            span,
                        ));
                    }
                    let func_typed = self.check_expr(func)?;
                    Ok(TypedExpr {
                        ty: sig.ret.clone(),
                        kind: TypedExprKind::Call { func: Box::new(func_typed), args: checked_args },
                        span,
                    })
                } else {
                    // Enum variant constructor call (tuple variant)
                    let func_typed = self.check_expr(func)?;
                    Ok(TypedExpr {
                        ty: ResolvedType::Void,
                        kind: TypedExprKind::Call { func: Box::new(func_typed), args: checked_args },
                        span,
                    })
                }
            }

            ExprKind::MethodCall { object, method, args } => {
                let obj = self.check_expr(object)?;
                let checked_args: Result<Vec<_>, _> = args.iter().map(|a| self.check_expr(a)).collect();
                Ok(TypedExpr {
                    ty: ResolvedType::Void, // host resolves method types
                    kind: TypedExprKind::MethodCall { object: Box::new(obj), method: method.clone(), args: checked_args? },
                    span,
                })
            }

            ExprKind::StructLit { path, fields } => {
                let type_name = path.last().unwrap().clone();
                let struct_ty = self.types.get(&type_name)
                    .ok_or_else(|| CompileError::at(format!("unknown struct '{type_name}'"), span))?
                    .clone();

                let mut typed_fields = Vec::new();
                for fi in fields {
                    let value = if let Some(v) = &fi.value {
                        self.check_expr(v)?
                    } else {
                        // Shorthand: field name = variable name
                        self.check_expr(&Expr { kind: ExprKind::Var(fi.name.clone()), span: fi.span })?
                    };
                    typed_fields.push((fi.name.clone(), value));
                }

                Ok(TypedExpr {
                    ty: struct_ty,
                    kind: TypedExprKind::StructLit { name: type_name, fields: typed_fields },
                    span,
                })
            }

            ExprKind::TupleLit(exprs) => {
                let typed: Result<Vec<_>, _> = exprs.iter().map(|e| self.check_expr(e)).collect();
                let typed = typed?;
                let tys: Vec<_> = typed.iter().map(|e| e.ty.clone()).collect();
                Ok(TypedExpr {
                    ty: ResolvedType::Tuple(tys),
                    kind: TypedExprKind::TupleLit(typed),
                    span,
                })
            }

            ExprKind::BinOp { op, lhs, rhs } => {
                let l = self.check_expr(lhs)?;
                let r = self.check_expr(rhs)?;

                let result_ty = match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                        if l.ty != r.ty {
                            return Err(CompileError::at(
                                format!("binary op type mismatch: {:?} vs {:?}", l.ty, r.ty),
                                span,
                            ));
                        }
                        l.ty.clone()
                    }
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                        ResolvedType::Bool
                    }
                    BinOp::And | BinOp::Or => ResolvedType::Bool,
                };

                Ok(TypedExpr {
                    ty: result_ty,
                    kind: TypedExprKind::BinOp { op: *op, lhs: Box::new(l), rhs: Box::new(r) },
                    span,
                })
            }

            ExprKind::UnaryOp { op, operand } => {
                let operand = self.check_expr(operand)?;
                let ty = match op {
                    UnaryOp::Neg => operand.ty.clone(),
                    UnaryOp::Not => ResolvedType::Bool,
                };
                Ok(TypedExpr {
                    ty,
                    kind: TypedExprKind::UnaryOp { op: *op, operand: Box::new(operand) },
                    span,
                })
            }

            ExprKind::If { cond, then_block, else_block } => {
                let cond = self.check_expr(cond)?;
                let then_typed = self.check_block(then_block)?;
                let else_typed = match else_block {
                    Some(ElseBranch::Block(b)) => Some(TypedElse::Block(self.check_block(b)?)),
                    Some(ElseBranch::If(e)) => Some(TypedElse::If(Box::new(self.check_expr(e)?))),
                    None => None,
                };
                let ty = then_typed.ty.clone();
                Ok(TypedExpr {
                    ty,
                    kind: TypedExprKind::If { cond: Box::new(cond), then_block: then_typed, else_block: else_typed },
                    span,
                })
            }

            ExprKind::Match { scrutinee, arms } => {
                let scrutinee = self.check_expr(scrutinee)?;
                let mut typed_arms = Vec::new();
                let mut result_ty = ResolvedType::Void;

                for (i, arm) in arms.iter().enumerate() {
                    self.push_scope();
                    self.bind_pattern(&arm.pattern, &scrutinee.ty)?;
                    let body = self.check_expr(&arm.body)?;
                    self.pop_scope();

                    if i == 0 {
                        result_ty = body.ty.clone();
                    }

                    typed_arms.push(TypedMatchArm { pattern: arm.pattern.clone(), body });
                }

                Ok(TypedExpr {
                    ty: result_ty.clone(),
                    kind: TypedExprKind::Match { scrutinee: Box::new(scrutinee), arms: typed_arms, result_ty },
                    span,
                })
            }

            ExprKind::While { cond, body } => {
                let cond = self.check_expr(cond)?;
                let body = self.check_block(body)?;
                Ok(TypedExpr {
                    ty: ResolvedType::Void,
                    kind: TypedExprKind::While { cond: Box::new(cond), body },
                    span,
                })
            }

            ExprKind::Loop { body } => {
                let body = self.check_block(body)?;
                Ok(TypedExpr {
                    ty: ResolvedType::Void,
                    kind: TypedExprKind::Loop { body },
                    span,
                })
            }

            ExprKind::Break { value } => {
                let val = value.as_ref().map(|v| self.check_expr(v)).transpose()?;
                Ok(TypedExpr {
                    ty: ResolvedType::Never,
                    kind: TypedExprKind::Break { value: val.map(Box::new) },
                    span,
                })
            }

            ExprKind::Continue => {
                Ok(TypedExpr { kind: TypedExprKind::Continue, ty: ResolvedType::Never, span })
            }

            ExprKind::Block(block) => {
                let typed = self.check_block(block)?;
                let ty = typed.ty.clone();
                Ok(TypedExpr { kind: TypedExprKind::Block(typed), ty, span })
            }
        }
    }

    fn bind_pattern(&mut self, pattern: &Pattern, scrutinee_ty: &ResolvedType) -> Result<(), CompileError> {
        match &pattern.kind {
            PatternKind::Wildcard => Ok(()),
            PatternKind::Literal(_) => Ok(()),
            PatternKind::Binding(name) => {
                self.define_local(name, scrutinee_ty.clone(), false);
                Ok(())
            }
            PatternKind::Path(_) => Ok(()),
            PatternKind::TupleVariant { path, fields } => {
                if let ResolvedType::Enum { variants, .. } = scrutinee_ty {
                    let variant_name = path.last().unwrap();
                    if let Some((_, VariantShape::Tuple(tys))) = variants.iter().find(|(n, _)| n == variant_name) {
                        for (pat, ty) in fields.iter().zip(tys.iter()) {
                            self.bind_pattern(pat, ty)?;
                        }
                    }
                }
                Ok(())
            }
            PatternKind::StructVariant { path, fields } => {
                if let ResolvedType::Enum { variants, .. } = scrutinee_ty {
                    let variant_name = path.last().unwrap();
                    if let Some((_, VariantShape::Struct(field_tys))) = variants.iter().find(|(n, _)| n == variant_name) {
                        for fp in fields {
                            if let Some((_, ty)) = field_tys.iter().find(|(n, _)| n == &fp.name) {
                                if let Some(inner_pat) = &fp.pattern {
                                    self.bind_pattern(inner_pat, ty)?;
                                } else {
                                    self.define_local(&fp.name, ty.clone(), false);
                                }
                            }
                        }
                    }
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rustlite::{lexer, parser};

    fn check_str(s: &str) -> TypedModule {
        let tokens = lexer::lex(s).unwrap();
        let module = parser::parse(&tokens).unwrap();
        check(&module).unwrap()
    }

    #[test]
    fn check_simple_fn() {
        let m = check_str("fn add(a: i32, b: i32) -> i32 { a + b }");
        assert_eq!(m.functions.len(), 1);
        assert_eq!(m.functions[0].ret_type, ResolvedType::I32);
    }

    #[test]
    fn check_struct_and_field_access() {
        let m = check_str(r#"
            struct Point { x: i32, y: i32 }
            fn get_x(p: Point) -> i32 { p.x }
        "#);
        assert_eq!(m.structs.len(), 1);
        assert_eq!(m.functions[0].ret_type, ResolvedType::I32);
    }

    #[test]
    fn check_let_and_assign() {
        let m = check_str("fn f() { let mut x: i32 = 0; x = 42; }");
        assert_eq!(m.functions.len(), 1);
    }

    #[test]
    fn check_type_mismatch() {
        let tokens = lexer::lex("fn f() -> i32 { true }").unwrap();
        let module = parser::parse(&tokens).unwrap();
        assert!(check(&module).is_err());
    }

    #[test]
    fn check_immutable_assign() {
        let tokens = lexer::lex("fn f() { let x: i32 = 0; x = 1; }").unwrap();
        let module = parser::parse(&tokens).unwrap();
        assert!(check(&module).is_err());
    }
}
