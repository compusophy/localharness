use std::collections::HashMap;
use crate::error_codes as codes;
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
    /// `[T; N]` — fixed-size array. The runtime value is a base pointer (i32)
    /// into linear memory.
    Array(Box<ResolvedType>, usize),
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

// Variants carry whole `TypedExpr`s of differing arity (`Assign` holds two,
// `Return` an `Option`), so their sizes differ. Boxing to equalize would force
// a Box at every construction + match site across the codegen pass for no real
// gain — rustlite programs are tiny, so the per-stmt slack is irrelevant.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum TypedStmt {
    Let { name: String, mutable: bool, ty: ResolvedType, init: TypedExpr },
    /// `place = value`. `index` is `Some(typed_i32_expr)` for an indexed array
    /// write `place[index] = value` (mirrors the read's `Index` address math),
    /// `None` for a plain variable / struct-field assignment.
    Assign { place: Place, index: Option<TypedExpr>, value: TypedExpr },
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
    /// A call to a `host::<module>::<func>` builtin (e.g.
    /// `display::fill_rect(...)`). Resolved against the host-function
    /// table, not the module's own functions — codegen emits it as a
    /// wasm import call. `module`/`func` are the resolved names
    /// (leading `host::` stripped); `ret_ty` is the host signature's
    /// return so codegen can declare the import type without its own
    /// table.
    HostCall { module: String, func: String, args: Vec<TypedExpr>, ret_ty: ResolvedType },
    MethodCall { object: Box<TypedExpr>, method: String, args: Vec<TypedExpr> },

    StructLit { name: String, fields: Vec<(String, TypedExpr)> },

    TupleLit(Vec<TypedExpr>),
    /// `[e0, e1, …]` — stored to a static linear-memory region at codegen time;
    /// the expression's value is the region's base pointer (i32).
    ArrayLit(Vec<TypedExpr>),
    /// `[value; count]` — reserve `count` i32 slots in a static region, fill each
    /// with `value`; the expression's value is the region's base pointer (i32).
    ArrayRepeat { value: Box<TypedExpr>, count: usize },
    /// `base[index]` — reads `i32.load(base + index*4)`.
    Index { base: Box<TypedExpr>, index: Box<TypedExpr> },

    BinOp { op: BinOp, lhs: Box<TypedExpr>, rhs: Box<TypedExpr> },
    UnaryOp { op: UnaryOp, operand: Box<TypedExpr> },
    /// `expr as <node.ty>` — numeric conversion. Source = inner expr's `.ty`.
    Cast { expr: Box<TypedExpr> },

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
    /// Top-level `const`s, INLINED at each reference (name → typed value). A
    /// const is a compile-time value, so a `Var` naming one returns a clone of
    /// its value expr — no runtime global, no codegen change.
    consts: HashMap<String, TypedExpr>,
}

impl TypeContext {
    fn new() -> Self {
        Self {
            types: HashMap::new(),
            functions: HashMap::new(),
            locals: Vec::new(),
            current_return: ResolvedType::Void,
            consts: HashMap::new(),
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
                    .ok_or_else(|| CompileError::new_code(codes::UNKNOWN_TYPE, format!("unknown type '{name}'")))
            }
            Ty::Tuple(tys) => {
                let resolved: Result<Vec<_>, _> = tys.iter().map(|t| self.resolve_ty(t)).collect();
                Ok(ResolvedType::Tuple(resolved?))
            }
            Ty::Array(elem, n) => {
                // v1: i32 elements only (matches the array-literal restriction);
                // the runtime value is an i32 base pointer.
                let elem_ty = self.resolve_ty(elem)?;
                if elem_ty != ResolvedType::I32 {
                    return Err(CompileError::new_code(
                        codes::BAD_INDEX,
                        format!("arrays support i32 elements for now, got {elem_ty:?}"),
                    ));
                }
                Ok(ResolvedType::Array(Box::new(elem_ty), *n))
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
                    // RETURNING an array is unsound under the static-region model:
                    // an array value is just the base pointer of a per-AST-node
                    // region that is REUSED on every call, so two live results of
                    // the same array-returning fn alias and the second call
                    // silently clobbers the first (`let a = mk(1); let b = mk(2);`
                    // makes `a` read as 2). Reject it at the signature rather than
                    // emit code that corrupts. The intended pattern is an array
                    // PARAM the callee mutates in place (C-style, shared backing).
                    if let ResolvedType::Array(_, _) = ret {
                        return Err(CompileError::at_code(
                            codes::UNSUPPORTED_FEATURE,
                            format!(
                                "fn '{}': returning an array is unsupported (the static \
                                 region a returned array points into is reused every \
                                 call, so two results would alias and corrupt). Pass a \
                                 mutable array param for the callee to fill instead.",
                                f.name
                            ),
                            f.span,
                        ));
                    }
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

        // Consts FIRST, so a function body resolves them no matter the source
        // order; each is registered in `self.consts` and inlined at its uses.
        for item in &module.items {
            if let Item::Const(c) = item {
                let ty = self.resolve_ty(&c.ty)?;
                self.push_scope();
                let value = self.check_expr(&c.value)?;
                self.pop_scope();
                if value.ty != ty {
                    return Err(CompileError::at_code(
                        codes::TYPE_MISMATCH,
                        format!("const type mismatch: expected {ty:?}, got {:?}", value.ty),
                        c.span,
                    ));
                }
                self.consts.insert(c.name.clone(), value.clone());
                consts.push(TypedConst { name: c.name.clone(), ty, value });
            }
        }

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
                Item::Const(_) => {} // processed in the consts-first pass above
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
            // An array param is an i32 base pointer aliasing the caller's
            // backing region (rustlite has no `&`/`&mut` distinction), so an
            // indexed write THROUGH it mutates shared memory — like C. Mark the
            // binding mutable so `a[i] = v` in the callee is allowed and visible
            // to the caller. Scalar params stay immutable (no reassignment).
            let mutable = matches!(ty, ResolvedType::Array(_, _));
            self.define_local(&param.name, ty.clone(), mutable);
            params.push((param.name.clone(), ty.clone()));
        }

        let body = self.check_block(&f.body)?;
        self.pop_scope();

        if sig.ret != ResolvedType::Void && body.ty != sig.ret && body.ty != ResolvedType::Never {
            return Err(CompileError::at_code(
                codes::TYPE_MISMATCH,
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
                        return Err(CompileError::at_code(
                            codes::TYPE_MISMATCH,
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
                    .ok_or_else(|| CompileError::at_code(codes::UNDEFINED_VARIABLE, format!("undefined variable '{}'", place.root), *span))?
                    .clone();
                if !is_mut {
                    return Err(CompileError::at_code(codes::NOT_MUTABLE, format!("'{}' is not mutable", place.root), *span));
                }
                // Resolve the base type at `root[.fields…]`.
                let mut target_ty = local_ty;
                for field in &place.fields {
                    target_ty = self.field_type(&target_ty, field, *span)?;
                }
                // Indexed write: the base must be an array, the index an i32, and
                // the value the element type. Mirrors `ExprKind::Index` reads.
                let typed_index = if let Some(index_expr) = &place.index {
                    // v1: index a NAMED array local directly (`arr[i] = v`).
                    // Writing through a struct field (`s.grid[i] = v`) needs
                    // working struct-in-memory codegen (not yet present), so
                    // reject it cleanly rather than emit wrong stores.
                    if !place.fields.is_empty() {
                        return Err(CompileError::at_code(
                            codes::INVALID_ASSIGN_TARGET,
                            "indexed write through a struct field is not yet supported",
                            *span,
                        ));
                    }
                    let elem_ty = match &target_ty {
                        ResolvedType::Array(elem, _) => (**elem).clone(),
                        other => {
                            return Err(CompileError::at_code(
                                codes::BAD_INDEX,
                                format!("cannot index into {other:?} (only arrays are indexable)"),
                                *span,
                            ))
                        }
                    };
                    let index_t = self.check_expr(index_expr)?;
                    if index_t.ty != ResolvedType::I32 {
                        return Err(CompileError::at_code(
                            codes::BAD_INDEX,
                            format!("array index must be i32, got {:?}", index_t.ty),
                            *span,
                        ));
                    }
                    // From here the value must match the ELEMENT type, not the array.
                    target_ty = elem_ty;
                    Some(index_t)
                } else {
                    None
                };
                let val = self.check_expr(value)?;
                if val.ty != target_ty {
                    return Err(CompileError::at_code(
                        codes::TYPE_MISMATCH,
                        format!("assignment type mismatch: expected {:?}, got {:?}", target_ty, val.ty),
                        *span,
                    ));
                }
                Ok(TypedStmt::Assign { place: place.clone(), index: typed_index, value: val })
            }
            Stmt::Return { value, span } => {
                let val = value.as_ref().map(|v| self.check_expr(v)).transpose()?;
                let ret_ty = val.as_ref().map(|v| v.ty.clone()).unwrap_or(ResolvedType::Void);
                if ret_ty != self.current_return {
                    return Err(CompileError::at_code(
                        codes::TYPE_MISMATCH,
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
                    .ok_or_else(|| CompileError::at_code(codes::BAD_FIELD_ACCESS, format!("no field '{field}' on struct"), span))
            }
            _ => Err(CompileError::at_code(codes::BAD_FIELD_ACCESS, format!("field access on non-struct type {:?}", ty), span)),
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
                } else if let Some(value) = self.consts.get(name) {
                    // A top-level const → inline a clone of its typed value.
                    Ok(value.clone())
                } else if self.functions.contains_key(name) {
                    // Could be a function name
                    Ok(TypedExpr { kind: TypedExprKind::Var(name.clone()), ty: ResolvedType::Void, span })
                } else {
                    Err(CompileError::at_code(codes::UNDEFINED_VARIABLE, format!("undefined variable '{name}'"), span))
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
                    _ => return Err(CompileError::at_code(codes::UNKNOWN_FUNCTION, "cannot call non-function", span)),
                };

                if let Some(sig) = self.functions.get(&fn_name).cloned() {
                    if checked_args.len() != sig.params.len() {
                        return Err(CompileError::at_code(
                            codes::ARITY_MISMATCH,
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
                } else if let Some((module, func_name, params, ret)) = resolve_host_fn(&fn_name) {
                    if checked_args.len() != params.len() {
                        return Err(CompileError::at_code(
                            codes::ARITY_MISMATCH,
                            format!("host fn '{fn_name}' expects {} args, got {}", params.len(), checked_args.len()),
                            span,
                        ));
                    }
                    for (i, (arg, expected)) in checked_args.iter().zip(params.iter()).enumerate() {
                        if arg.ty != *expected {
                            return Err(CompileError::at_code(
                                codes::TYPE_MISMATCH,
                                format!("host fn '{fn_name}' arg {i}: expected {expected:?}, got {:?}", arg.ty),
                                span,
                            ));
                        }
                    }
                    Ok(TypedExpr {
                        ty: ret.clone(),
                        kind: TypedExprKind::HostCall { module, func: func_name, args: checked_args, ret_ty: ret },
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
                    .ok_or_else(|| CompileError::at_code(codes::UNKNOWN_STRUCT, format!("unknown struct '{type_name}'"), span))?
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

            ExprKind::ArrayLit(elems) => {
                if elems.is_empty() {
                    return Err(CompileError::at_code(codes::BAD_INDEX, "empty array literal is unsupported", span));
                }
                let typed: Result<Vec<_>, _> = elems.iter().map(|e| self.check_expr(e)).collect();
                let typed = typed?;
                let elem_ty = typed[0].ty.clone();
                // v1: i32 elements only (colours, coords, tile/lookup tables).
                if elem_ty != ResolvedType::I32 {
                    return Err(CompileError::at_code(
                        codes::BAD_INDEX,
                        format!("arrays support i32 elements for now, got {elem_ty:?}"),
                        span,
                    ));
                }
                if typed.iter().any(|e| e.ty != elem_ty) {
                    return Err(CompileError::at_code(codes::BAD_INDEX, "array elements must all be the same type", span));
                }
                let n = typed.len();
                Ok(TypedExpr {
                    ty: ResolvedType::Array(Box::new(elem_ty), n),
                    kind: TypedExprKind::ArrayLit(typed),
                    span,
                })
            }

            ExprKind::ArrayRepeat { value, count } => {
                let val = self.check_expr(value)?;
                // v1: i32 elements only, mirroring the array-literal restriction.
                if val.ty != ResolvedType::I32 {
                    return Err(CompileError::at_code(
                        codes::BAD_INDEX,
                        format!("arrays support i32 elements for now, got {:?}", val.ty),
                        span,
                    ));
                }
                if *count == 0 {
                    return Err(CompileError::at_code(codes::BAD_INDEX, "empty array (`[v; 0]`) is unsupported", span));
                }
                Ok(TypedExpr {
                    ty: ResolvedType::Array(Box::new(val.ty.clone()), *count),
                    kind: TypedExprKind::ArrayRepeat { value: Box::new(val), count: *count },
                    span,
                })
            }

            ExprKind::Index { base, index } => {
                let base_t = self.check_expr(base)?;
                let index_t = self.check_expr(index)?;
                let elem_ty = match &base_t.ty {
                    ResolvedType::Array(elem, _) => (**elem).clone(),
                    other => {
                        return Err(CompileError::at_code(
                            codes::BAD_INDEX,
                            format!("cannot index into {other:?} (only arrays are indexable)"),
                            span,
                        ))
                    }
                };
                if index_t.ty != ResolvedType::I32 {
                    return Err(CompileError::at_code(
                        codes::BAD_INDEX,
                        format!("array index must be i32, got {:?}", index_t.ty),
                        span,
                    ));
                }
                Ok(TypedExpr {
                    ty: elem_ty,
                    kind: TypedExprKind::Index { base: Box::new(base_t), index: Box::new(index_t) },
                    span,
                })
            }

            ExprKind::BinOp { op, lhs, rhs } => {
                let l = self.check_expr(lhs)?;
                let r = self.check_expr(rhs)?;

                let result_ty = match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                        if l.ty != r.ty {
                            return Err(CompileError::at_code(
                                codes::TYPE_MISMATCH,
                                format!("binary op type mismatch: {:?} vs {:?}", l.ty, r.ty),
                                span,
                            ));
                        }
                        l.ty.clone()
                    }
                    BinOp::Shl | BinOp::Shr | BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                        if l.ty != r.ty {
                            return Err(CompileError::at_code(
                                codes::TYPE_MISMATCH,
                                format!("bitwise op type mismatch: {:?} vs {:?}", l.ty, r.ty),
                                span,
                            ));
                        }
                        if l.ty != ResolvedType::I32 && l.ty != ResolvedType::I64 {
                            return Err(CompileError::at_code(
                                codes::TYPE_MISMATCH,
                                format!("bitwise/shift ops require integers, got {:?}", l.ty),
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

            ExprKind::Cast { expr, ty } => {
                let inner = self.check_expr(expr)?;
                let target = self.resolve_ty(ty)?;
                let numeric = |t: &ResolvedType| {
                    matches!(
                        t,
                        ResolvedType::I32 | ResolvedType::I64 | ResolvedType::F32 | ResolvedType::F64
                    )
                };
                if !numeric(&inner.ty) || !numeric(&target) {
                    return Err(CompileError::at_code(
                        codes::BAD_CAST,
                        format!("`as` converts between numbers, not {:?} -> {:?}", inner.ty, target),
                        span,
                    ));
                }
                Ok(TypedExpr {
                    ty: target,
                    kind: TypedExprKind::Cast { expr: Box::new(inner) },
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
                // An `if` WITHOUT an `else` is a statement, never a value (Rust
                // semantics): the missing branch can't produce a value, so the
                // whole `if` is `Void`. A non-void tail in the `then` block would
                // therefore be dropped on the floor on the false path — codegen
                // would have to emit an `(if (result T))` frame with no else,
                // which is stack-imbalanced (invalid) wasm. Reject it here so the
                // emitter always stays on the void path (BLOCK_VOID).
                let ty = if else_typed.is_some() {
                    then_typed.ty.clone()
                } else {
                    if then_typed.ty != ResolvedType::Void && then_typed.ty != ResolvedType::Never {
                        return Err(CompileError::at_code(
                            codes::TYPE_MISMATCH,
                            format!(
                                "`if` without `else` evaluates to {:?}, but an else-less `if` is a statement (add an `else` branch to use its value)",
                                then_typed.ty
                            ),
                            span,
                        ));
                    }
                    ResolvedType::Void
                };
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
                    // An irrefutable arm (`_` or a bare binding) matches
                    // everything, so any arm after it is dead — and codegen lowers
                    // a non-last match to a chain of `if/else` frames that assumes
                    // the irrefutable arm is the terminal `else`. A non-last
                    // wildcard/binding would emit a stack-imbalanced (invalid)
                    // module, so reject it here (the arm must be moved last).
                    if is_irrefutable_pattern(&arm.pattern) && i != arms.len() - 1 {
                        return Err(CompileError::at_code(
                            codes::TYPE_MISMATCH,
                            "a `_`/binding match arm matches everything; move it last (arms after it are unreachable)",
                            arm.span,
                        ));
                    }

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
            PatternKind::IntRange { .. } => Ok(()),
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

/// Whether a match pattern matches EVERY value (so any later arm is dead code).
/// Mirrors codegen's `is_wildcard`: a bare `_` or a binding (`x =>`) catches all;
/// literals, ranges, and variant patterns are refutable. Codegen lowers the
/// terminal catch-all to a plain `else`, so a non-last irrefutable arm is
/// rejected in the typechecker (it would otherwise emit invalid wasm).
fn is_irrefutable_pattern(pattern: &Pattern) -> bool {
    matches!(pattern.kind, PatternKind::Wildcard | PatternKind::Binding(_))
}

/// Resolve a `module::func` path against the host-function table.
///
/// `fn_name` is the call path joined with `::` (e.g. `display::clear`
/// or `host::display::clear`); a leading `host::` is stripped. Returns
/// `(module, func, param_types, return_type)` for known host builtins.
///
/// These are the **Orbclient-style** drawing primitives a cartridge
/// uses to draw onto the host-owned framebuffer (see `src/app/display.rs`
/// for the matching imports). Colours are `0xRRGGBB` (opaque).
fn resolve_host_fn(fn_name: &str) -> Option<(String, String, Vec<ResolvedType>, ResolvedType)> {
    use ResolvedType::*;
    let stripped = fn_name.strip_prefix("host::").unwrap_or(fn_name);
    // Every host builtin lives in the `display` module today. Accept the
    // module-elided spellings — `state_get`, `host::state_get`, or (after
    // `use host::display;`) a bare `state_get` — by defaulting the module
    // to `display`. Without this, `host::state_get` resolved to nothing,
    // fell through to the enum-variant branch, and got typed `Void` — the
    // "declared I32, got Void" bug that blocked all stateful cartridges.
    // NB: `use ResolvedType::*` above shadows the std `String` type with
    // the `String` variant, so don't annotate this `let` with `String`.
    let key = if stripped.contains("::") {
        stripped.to_string()
    } else {
        format!("display::{stripped}")
    };
    let (params, ret): (Vec<ResolvedType>, ResolvedType) = match key.as_str() {
        "display::clear" => (vec![I32], Void),
        "display::set_pixel" => (vec![I32, I32, I32], Void),
        "display::fill_rect" => (vec![I32, I32, I32, I32, I32], Void),
        "display::draw_char" => (vec![I32, I32, I32, I32, I32], Void),
        "display::draw_number" => (vec![I32, I32, I32, I32, I32], Void),
        // --- software 3D (FB#12b): framebuffer primitives, integer ABI, same
        // pixel/viewport model as the other display fns (no WebGL/iframe).
        // `draw_line(x0,y0,x1,y1,rgb)`; `fill_triangle(x0,y0,x1,y1,x2,y2,rgb)`
        // flat-fill (painter's order — depth/overlap is the cartridge's job; a
        // per-pixel z-buffered fill needs a packed ABI to fit the host closure
        // arity limit, so it's deferred to v2). See `web/cartridge-worker.js`
        // host_display + `src/raster.rs`.
        "display::draw_line" => (vec![I32, I32, I32, I32, I32], Void),
        "display::fill_triangle" => (vec![I32, I32, I32, I32, I32, I32, I32], Void),
        "display::present" => (vec![], Void),
        "display::width" => (vec![], I32),
        "display::height" => (vec![], I32),
        "display::pointer_x" => (vec![], I32),
        "display::pointer_y" => (vec![], I32),
        "display::pointer_down" => (vec![], I32),
        "display::state_get" => (vec![I32], I32),
        "display::state_set" => (vec![I32, I32], Void),
        // --- networking (host_net): WebSocket-backed multiplayer/sync I/O.
        // Strings (URL, message bodies) use the same length-prefixed memory
        // layout as the loader's `read_string`: 4 bytes LE length, then UTF-8
        // payload, at the given cartridge-memory pointer.
        //
        // `open(url_ptr) -> handle`   open a WebSocket to the url at `url_ptr`;
        //                             returns a handle >= 0, or -1 on error.
        // `send(handle, ptr) -> ok`   send the length-prefixed message at `ptr`;
        //                             returns 1 if queued, 0 if not (closed/bad).
        // `poll(handle, out_ptr, max) -> len`  copy the next inbound message
        //                             (length-prefixed) into memory at `out_ptr`,
        //                             writing at most `max` payload bytes; returns
        //                             the payload byte length, 0 if the inbox is
        //                             empty, or -1 on a bad handle.
        // `status(handle) -> i32`     0 connecting, 1 open, 2 closing, 3 closed,
        //                             -1 bad handle.
        // `close(handle)`             close the socket and drop its inbox.
        "net::open" => (vec![I32], I32),
        "net::send" => (vec![I32, I32], I32),
        "net::poll" => (vec![I32, I32, I32], I32),
        "net::status" => (vec![I32], I32),
        "net::close" => (vec![I32], Void),
        // --- multiplayer (host_mp): browser-to-browser P2P over a WebRTC data
        // channel, off-chain-signaled (design/multiplayer-cartridges.md). DISTINCT
        // from `net` (that's a WebSocket-to-server client). Integer-only: a SHARED
        // STATE vector per peer (continuous "where is everyone") + an EVENT queue
        // (discrete "something happened"). 2-peer v1.
        //
        // `open() -> code`           host a room; returns a numeric CODE to show the
        //                            other player (they `join(code)`).
        // `join(code)`               join the room with that code.
        // `connected() -> i32`       1 once the data channel is open, else 0.
        // `self_index() -> i32`      this peer's index (host 0 / joiner 1; -1 none).
        // `peer_count() -> i32`      participants connected (self + peers).
        // `set(slot, value)`         write MY shared-state slot (broadcast, coalesced).
        // `get(peer, slot) -> i32`   read peer `peer`'s slot (last seen; 0 if unknown).
        // `send(value)`              broadcast a discrete EVENT to peers.
        // `event_count() -> i32`     received events queued.
        // `event_next() -> i32`      pop + return the oldest received event (0 if none).
        "mp::open" => (vec![], I32),
        "mp::join" => (vec![I32], Void),
        "mp::connected" => (vec![], I32),
        "mp::self_index" => (vec![], I32),
        "mp::peer_count" => (vec![], I32),
        "mp::set" => (vec![I32, I32], Void),
        "mp::get" => (vec![I32, I32], I32),
        "mp::send" => (vec![I32], Void),
        "mp::event_count" => (vec![], I32),
        "mp::event_next" => (vec![], I32),
        // --- open chatroom (host_chat): a per-ROOM (= this subdomain) append-only
        // text log over the proxy's off-chain /api/chat relay. rustlite has no
        // String/Vec + arrays are read-only, so ALL text lives HOST-side (the
        // worker holds the received-line ring + the outgoing compose buffer); the
        // cartridge reads/writes it purely as INTEGERS (codepoints), keeping the
        // host ABI integer-only. The host auto-polls the relay on first `poll()`.
        //
        //   poll() -> n           start polling (idempotent) + return # lines held.
        //   line_count() -> n     received lines currently buffered (oldest first).
        //   line_len(i) -> len    byte length of received line i (0 if out of range).
        //   line_char(i,p) -> cp  codepoint at p of line i (-1 out of range).
        //   key(cp)               append a char to the outgoing compose buffer.
        //   backspace()           delete the last compose char.
        //   compose_len() -> len  current compose-buffer length.
        //   compose_char(p) -> cp codepoint at p of the compose buffer (-1 oob).
        //   send() -> 1/0         flush the compose buffer as a message (1 if it
        //                         was non-empty), then clear it.
        "chat::poll" => (vec![], I32),
        "chat::line_count" => (vec![], I32),
        "chat::line_len" => (vec![I32], I32),
        "chat::line_char" => (vec![I32, I32], I32),
        "chat::key" => (vec![I32], Void),
        "chat::backspace" => (vec![], Void),
        "chat::compose_len" => (vec![], I32),
        "chat::compose_char" => (vec![I32], I32),
        "chat::send" => (vec![], I32),
        // --- http (host_http): one-shot HTTP GET + HTML→text, the SAME POLL
        // MODEL as host_net (open a request → get a handle, poll `ready` until
        // the body lands, then read it out of cartridge memory). A cartridge
        // can't fetch arbitrary origins itself (no DOM, CORS), so the host
        // fetches through the platform's `/api/fetch` proxy — the SAME
        // CORS-bypassing route the agent `web_fetch` tool uses (https-only,
        // private/internal hosts denied, ≤3 redirects, 200KB body cap, textual
        // content only). Strings (URL in, body/text out) use the same
        // length-prefixed memory layout as host_net: 4 bytes LE length, then
        // UTF-8 payload, at the given cartridge-memory pointer.
        //
        // `get(url, url_len) -> handle`  start a GET of the URL at `url` — a
        //                             length-prefixed string pointer (a string
        //                             literal; same wire form as host_net). The
        //                             `url_len` i32 is ADVISORY (the layout is
        //                             self-describing). Returns a handle >= 0, or
        //                             -1 on a bad URL / cap hit.
        // `ready(handle) -> i32`      0 pending, 1 ready (body available), <0 on
        //                             error (-1 bad handle, -2 fetch failed/
        //                             denied/timed out).
        // `status(handle) -> i32`     the UPSTREAM site's HTTP status (200/404/…)
        //                             once ready, 0 while pending, -1 bad handle.
        //                             Check it before trusting the body.
        // `body_len(handle) -> i32`   byte length of the ready body (0 pending or
        //                             empty, -1 bad handle) — size an out buffer.
        // `read_body(handle, out_ptr, max) -> len`  copy the body (length-
        //                             prefixed) into memory at `out_ptr`, writing
        //                             at most `max` payload bytes; returns the
        //                             bytes written, 0 if not ready, -1 bad handle.
        // `parse_text(html, html_len, out_ptr, max) -> len`  PURE (no network):
        //                             strip HTML tags + decode common entities of
        //                             the length-prefixed HTML at `html` (a string
        //                             pointer) into plain text written length-
        //                             prefixed at `out_ptr` (≤ `max` payload
        //                             bytes); returns the bytes written. Turns a
        //                             fetched body into readable text. (`html_len`
        //                             advisory.) NB: the read pointers are typed
        //                             `String` so a string literal flows straight
        //                             in (the only pointer rustlite can produce);
        //                             the out pointer + lengths are i32.
        "http::get" => (vec![String, I32], I32),
        "http::ready" => (vec![I32], I32),
        "http::status" => (vec![I32], I32),
        "http::body_len" => (vec![I32], I32),
        "http::read_body" => (vec![I32, I32, I32], I32),
        "http::parse_text" => (vec![String, I32, I32, I32], I32),
        // body_lines(handle) -> line count; draw_line(handle, line, x, y, rgb, scale)
        // -> chars drawn. Render the HOST-HELD fetched body as TEXT by handle — no
        // cartridge buffer needed (rustlite can only produce a string-LITERAL
        // pointer, so read_body's out_ptr is unusable from rustlite). This is how a
        // data-driven cartridge shows LIVE fetched text. Lines are '\n'-delimited.
        "http::body_lines" => (vec![I32], I32),
        "http::draw_line" => (vec![I32, I32, I32, I32, I32, I32], I32),
        // --- audio (host_audio): Web Audio (AudioContext) playback. Integer
        // ABI, fire-and-forget like host_net. `wave`: 0 sine, 1 square,
        // 2 sawtooth, 3 triangle. A handle >= 0 names a voice for `stop`;
        // `stop(-1)` stops every voice. `set_volume(pct)` sets master gain
        // (0..=100). Audio is silent until the first user gesture (the browser
        // AudioContext rule) — a cartridge only runs after the user opens it,
        // so the first `tone` resumes the context. See `src/app/display.rs`
        // `mod audio` for the host implementation.
        //
        // `tone(freq_hz, dur_ms, wave) -> handle`        play a tone now.
        // `tone_at(freq_hz, dur_ms, wave, delay_ms) -> handle`  schedule a
        //                             tone `delay_ms` in the future (sequencing
        //                             a bar of notes from one frame).
        // `noise(dur_ms) -> handle`   white-noise burst (hats/explosions).
        // `stop(handle)`              stop one voice; `stop(-1)` stops all.
        // `set_volume(pct)`           master gain 0..=100 (clamped).
        "audio::tone" => (vec![I32, I32, I32], I32),
        "audio::tone_at" => (vec![I32, I32, I32, I32], I32),
        "audio::noise" => (vec![I32], I32),
        "audio::stop" => (vec![I32], Void),
        "audio::set_volume" => (vec![I32], Void),
        // --- agent (host_agent): the cartridge<->platform bridge (feedback
        // #66/#103). Lets a published cartridge reach the platform it runs
        // inside, within a deliberately narrow + safe v1 surface.
        //
        // `notify(title, body) -> i32`  show a LOCAL system notification to
        //   the CURRENT viewer (never other users — that P2P-subscriber push
        //   is the named follow-up). Strings are length-prefixed pointers
        //   (same ABI as host_net). Permission-GATED (never prompts; silently
        //   dropped if the viewer hasn't already allowed notifications) and
        //   RATE-LIMITED (~1 / 3s). Returns 1 if posted, 0 if dropped/limited.
        //   Use on a user gesture (a button press) — e.g. "Ready Up!".
        // `viewer_is_owner() -> i32`    1 if THIS device controls (owns) the
        //   subdomain the cartridge is published under, else 0 — gate
        //   host-only controls (the "host triggers" in a Ready-Up app).
        // `viewer_has_identity() -> i32` 1 if the viewer has a local wallet.
        "agent::notify" => (vec![String, String], I32),
        "agent::viewer_is_owner" => (vec![], I32),
        "agent::viewer_has_identity" => (vec![], I32),
        // --- subscriber feed (the "Ready Up" loop, feedback #103). The feed
        // is THIS cartridge's own subdomain (SubscribeFacet on-chain). Writes
        // are fire-and-forget (sponsored tx on the main thread); reads are
        // load-time context refreshed after a subscribe/unsubscribe.
        //
        // `subscribe() -> i32`    subscribe the viewer to this feed; 1 if the
        //   request went out, 0 if the viewer has no identity yet.
        // `unsubscribe() -> i32`  leave the feed.
        // `is_subscribed() -> i32` 1 if the viewer is on the feed (cached).
        // `subscriber_count() -> i32`  members on the feed (cached) — the
        //   "member count" for the UI.
        // `broadcast(title, body) -> i32`  THE READY UP: push a notification
        //   to EVERY subscriber's device (via the proxy). NOT owner-gated —
        //   anyone with an identity can fire it; rate-limited per feed. 1 if
        //   the request went out, 0 if no identity.
        // `broadcast_compose(title, default_body) -> i32`  `broadcast`, but
        //   the host first opens a text input over the canvas (prefilled with
        //   `default_body`) so the presser can type a CUSTOM message before it
        //   goes out — a cartridge is pixels-only and can't summon a mobile
        //   keyboard itself. [send] broadcasts the typed body under `title`;
        //   [cancel] sends nothing. 1 if the composer opened, 0 if no identity.
        // `request_identity() -> i32`  ensure the viewer has a wallet (creates
        //   a local one if missing — the "this app needs an identity" path);
        //   returns 1 if they now have one. Gate subscribe/broadcast on this
        //   for a sybil-resistant app.
        "agent::subscribe" => (vec![], I32),
        "agent::unsubscribe" => (vec![], I32),
        "agent::is_subscribed" => (vec![], I32),
        "agent::subscriber_count" => (vec![], I32),
        "agent::broadcast" => (vec![String, String], I32),
        "agent::broadcast_compose" => (vec![String, String], I32),
        "agent::request_identity" => (vec![], I32),
        // --- compose (host_compose): cartridge-in-cartridge composition. A
        // PARENT cartridge mounts another subdomain's published `app.wasm` as a
        // CHILD bound to a sub-rectangle of the parent's framebuffer, with the
        // child running in its OWN buffer (its native dims) and the host
        // blitting it into the rect (nearest-neighbour scale — see
        // `crate::compose::blit_child`). Pointer input routes into the focused
        // child via `crate::compose::map_pointer_into_child`. NO iframes — pure
        // pixel composition. Integer-only, poll-model, the SAME conventions as
        // host_net/host_agent: the child name is a length-prefixed string
        // pointer into the parent's linear memory (4-byte LE len + UTF-8). See
        // `web/cartridge-worker.js` host_compose + `src/app/display.rs`
        // (compose_spawn round-trip) + `design/host-compose.md` for the model.
        //
        // `spawn_module(name, x, y, w, h) -> handle`  fetch <name>'s on-chain
        //   app.wasm, instantiate a child bound to rect (x,y,w,h) in the CALLER's
        //   framebuffer coords. `name` is a string literal (a length-prefixed
        //   pointer into the caller's memory, same ABI as agent::notify).
        //   Returns a handle >= 0 (LOADING; it ticks once the bytes arrive) or a
        //   negative reject. RECURSIVE: the child gets its OWN compose table and
        //   may spawn grandchildren — handles are per-node (a child's handle 0 is
        //   distinct from the parent's). The fractal terminates at the depth cap
        //   (a node there returns -1); also bounded by per-node child / total-node
        //   / total-byte caps (ComposeBudget). A self-spawning cartridge nests
        //   into a Droste image.
        // `status(handle) -> i32`  -1 bad handle, 0 LOADING, 1 READY (ticking),
        //   2 FAILED (no app.wasm, bad bytes, trap, or budget-refused).
        // `move_module(handle, x, y, w, h) -> i32`  re-bind the child's rect
        //   (the window manager moves/resizes a panel). 1 ok, 0 bad handle.
        // `focus_module(handle) -> i32`  make `handle` the focused child — the
        //   only one fed pointer input. Pass -1 to focus the parent itself.
        //   1 ok, 0 bad handle.
        // `focused() -> i32`  the currently focused handle, or -1 for the parent.
        // `close_module(handle) -> i32`  tear the child down (drop its instance
        //   + buffer + rect; free the slot, never aliased). 1 ok, 0 bad handle.
        // `module_count() -> i32`  number of live (non-closed) children.
        "compose::spawn_module" => (vec![String, I32, I32, I32, I32], I32),
        "compose::status" => (vec![I32], I32),
        "compose::move_module" => (vec![I32, I32, I32, I32, I32], I32),
        "compose::focus_module" => (vec![I32], I32),
        "compose::focused" => (vec![], I32),
        "compose::close_module" => (vec![I32], I32),
        "compose::module_count" => (vec![], I32),
        _ => return None,
    };
    let (module, func) = key.split_once("::")?;
    Some((module.to_string(), func.to_string(), params, ret))
}

#[cfg(test)]
mod host_fn_tests {
    use super::*;

    #[test]
    fn host_agent_signatures_resolve() {
        use ResolvedType::{String as Str, I32};
        let (m, f, p, r) = resolve_host_fn("host::agent::notify").expect("notify resolves");
        assert_eq!((m.as_str(), f.as_str()), ("agent", "notify"));
        assert_eq!(p, vec![Str, Str]);
        assert_eq!(r, I32, "notify returns posted/dropped flag");

        for name in ["host::agent::viewer_is_owner", "host::agent::viewer_has_identity"] {
            let (m, _f, p, r) = resolve_host_fn(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(m, "agent");
            assert!(p.is_empty(), "{name} takes no args");
            assert_eq!(r, I32);
        }
        // The module-elision default (display) must NOT swallow agent fns.
        assert!(resolve_host_fn("agent::notify").is_some());
    }

    #[test]
    fn host_compose_signatures_resolve() {
        use ResolvedType::{String as Str, I32};
        // spawn_module(name, x, y, w, h) -> handle (name is a string literal).
        let (m, f, p, r) = resolve_host_fn("host::compose::spawn_module").expect("spawn resolves");
        assert_eq!((m.as_str(), f.as_str()), ("compose", "spawn_module"));
        assert_eq!(p, vec![Str, I32, I32, I32, I32], "name string + rect");
        assert_eq!(r, I32, "spawn returns a child handle");

        // Single-handle ops: status / focus / close return i32.
        for name in [
            "host::compose::status",
            "host::compose::focus_module",
            "host::compose::close_module",
        ] {
            let (m, _f, p, r) = resolve_host_fn(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(m, "compose");
            assert_eq!(p, vec![I32], "{name} takes one handle");
            assert_eq!(r, I32);
        }

        // move_module(handle, x, y, w, h) -> i32 (all integer).
        let (_m, _f, p, r) = resolve_host_fn("host::compose::move_module").unwrap();
        assert_eq!(p, vec![I32, I32, I32, I32, I32]);
        assert_eq!(r, I32);

        // No-arg readers: focused() / module_count() -> i32.
        for name in ["host::compose::focused", "host::compose::module_count"] {
            let (m, _f, p, r) = resolve_host_fn(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(m, "compose");
            assert!(p.is_empty(), "{name} takes no args");
            assert_eq!(r, I32);
        }
        // The module-elision default (display) must NOT swallow compose fns.
        assert!(resolve_host_fn("compose::spawn_module").is_some());
    }

    #[test]
    fn host_http_signatures_resolve() {
        use ResolvedType::{String as Str, I32};
        // get(url, url_len) -> handle. The url is a string-pointer (typed String
        // so a literal flows straight in, like agent::notify), plus an advisory
        // i32 length.
        let (m, f, p, r) = resolve_host_fn("host::http::get").expect("get resolves");
        assert_eq!((m.as_str(), f.as_str()), ("http", "get"));
        assert_eq!(p, vec![Str, I32], "url string + advisory len");
        assert_eq!(r, I32, "get returns a request handle");

        // Single-handle pollers: ready / status / body_len take one handle -> i32.
        for name in [
            "host::http::ready",
            "host::http::status",
            "host::http::body_len",
        ] {
            let (m, _f, p, r) = resolve_host_fn(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(m, "http");
            assert_eq!(p, vec![I32], "{name} takes one handle");
            assert_eq!(r, I32);
        }

        // read_body(handle, out_ptr, max) -> len (poll-model copy, like net::poll).
        let (_m, _f, p, r) = resolve_host_fn("host::http::read_body").unwrap();
        assert_eq!(p, vec![I32, I32, I32]);
        assert_eq!(r, I32);

        // parse_text(html, html_len, out_ptr, max) -> len (pure HTML->text); the
        // html read pointer is a String, the rest i32.
        let (_m, _f, p, r) = resolve_host_fn("host::http::parse_text").unwrap();
        assert_eq!(p, vec![Str, I32, I32, I32]);
        assert_eq!(r, I32);

        // The elided / `use host::http` spelling must resolve to the http
        // module, not get swallowed by the display-default.
        assert!(resolve_host_fn("http::get").is_some());
    }

    #[test]
    fn state_get_resolves_to_i32_in_every_spelling() {
        for name in [
            "state_get",
            "host::state_get",
            "display::state_get",
            "host::display::state_get",
        ] {
            let (module, func, params, ret) =
                resolve_host_fn(name).unwrap_or_else(|| panic!("{name} did not resolve"));
            assert_eq!(module, "display");
            assert_eq!(func, "state_get");
            assert_eq!(params, vec![ResolvedType::I32]);
            assert_eq!(ret, ResolvedType::I32, "{name} must return i32, not Void");
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
