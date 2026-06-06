use crate::rustlite::CompileError;
use crate::rustlite::ast::{BinOp, UnaryOp};
use crate::rustlite::typecheck::*;

pub fn emit(module: &TypedModule) -> Result<Vec<u8>, CompileError> {
    let mut emitter = WasmEmitter::new();
    emitter.emit_module(module)?;
    Ok(emitter.finish())
}

// Wasm binary format constants
const WASM_MAGIC: &[u8] = b"\0asm";
const WASM_VERSION: &[u8] = &[1, 0, 0, 0];

// Section IDs
const SEC_TYPE: u8 = 1;
const SEC_IMPORT: u8 = 2;
const SEC_FUNCTION: u8 = 3;
const SEC_MEMORY: u8 = 5;
const SEC_EXPORT: u8 = 7;
const SEC_CODE: u8 = 10;
const SEC_DATA: u8 = 11;

// Value types
const WASM_I32: u8 = 0x7F;
const WASM_I64: u8 = 0x7E;
const WASM_F32: u8 = 0x7D;
const WASM_F64: u8 = 0x7C;

// Opcodes
const _OP_UNREACHABLE: u8 = 0x00;
const _OP_NOP: u8 = 0x01;
const OP_BLOCK: u8 = 0x02;
const OP_LOOP: u8 = 0x03;
const OP_IF: u8 = 0x04;
const OP_ELSE: u8 = 0x05;
const OP_END: u8 = 0x0B;
const OP_BR: u8 = 0x0C;
const OP_BR_IF: u8 = 0x0D;
const OP_RETURN: u8 = 0x0F;
const OP_CALL: u8 = 0x10;
const OP_DROP: u8 = 0x1A;
const OP_LOCAL_GET: u8 = 0x20;
const OP_LOCAL_SET: u8 = 0x21;
const _OP_LOCAL_TEE: u8 = 0x22;
const OP_I32_LOAD: u8 = 0x28;
const _OP_I64_LOAD: u8 = 0x29;
const _OP_F32_LOAD: u8 = 0x2A;
const _OP_F64_LOAD: u8 = 0x2B;
const _OP_I32_STORE: u8 = 0x36;
const _OP_I64_STORE: u8 = 0x37;
const _OP_F32_STORE: u8 = 0x38;
const _OP_F64_STORE: u8 = 0x39;
const OP_I32_CONST: u8 = 0x41;
const OP_I64_CONST: u8 = 0x42;
const _OP_F32_CONST: u8 = 0x43;
const OP_F64_CONST: u8 = 0x44;
const OP_I32_EQZ: u8 = 0x45;
const OP_I32_EQ: u8 = 0x46;
const OP_I32_NE: u8 = 0x47;
const OP_I32_LT_S: u8 = 0x48;
const OP_I32_GT_S: u8 = 0x4A;
const OP_I32_LE_S: u8 = 0x4C;
const OP_I32_GE_S: u8 = 0x4E;
const OP_I64_EQ: u8 = 0x51;
const OP_I64_NE: u8 = 0x52;
const OP_I64_LT_S: u8 = 0x53;
const OP_I64_GT_S: u8 = 0x55;
const OP_I64_LE_S: u8 = 0x57;
const OP_I64_GE_S: u8 = 0x59;
const OP_F64_EQ: u8 = 0x61;
const OP_F64_NE: u8 = 0x62;
const OP_F64_LT: u8 = 0x63;
const OP_F64_GT: u8 = 0x64;
const OP_F64_LE: u8 = 0x65;
const OP_F64_GE: u8 = 0x66;
const OP_I32_ADD: u8 = 0x6A;
const OP_I32_SUB: u8 = 0x6B;
const OP_I32_MUL: u8 = 0x6C;
const OP_I32_DIV_S: u8 = 0x6D;
const OP_I32_REM_S: u8 = 0x6F;
const OP_I64_ADD: u8 = 0x7C;
const OP_I64_SUB: u8 = 0x7D;
const OP_I64_MUL: u8 = 0x7E;
const OP_I64_DIV_S: u8 = 0x7F;
const OP_I64_REM_S: u8 = 0x81;
const OP_F64_ADD: u8 = 0xA0;
const OP_F64_SUB: u8 = 0xA1;
const OP_F64_MUL: u8 = 0xA2;
const OP_F64_DIV: u8 = 0xA3;
const OP_F64_NEG: u8 = 0x9A;
// Bitwise + shift (integer only)
const OP_I32_AND: u8 = 0x71;
const OP_I32_OR: u8 = 0x72;
const OP_I32_XOR: u8 = 0x73;
const OP_I32_SHL: u8 = 0x74;
const OP_I32_SHR_S: u8 = 0x75;
const OP_I64_AND: u8 = 0x83;
const OP_I64_OR: u8 = 0x84;
const OP_I64_XOR: u8 = 0x85;
const OP_I64_SHL: u8 = 0x86;
const OP_I64_SHR_S: u8 = 0x87;

const BLOCK_VOID: u8 = 0x40;

pub struct WasmModule {
    pub bytes: Vec<u8>,
}

struct _FuncInfo {
    _type_idx: u32,
    _local_count: u32,
}

struct WasmEmitter {
    types: Vec<Vec<u8>>,
    functions: Vec<FuncBody>,
    exports: Vec<(String, u8, u32)>,
    data_segments: Vec<(u32, Vec<u8>)>,
    data_offset: u32,

    // Host imports. Wasm puts imported functions at function indices
    // 0..import_count, so every local function index (calls, exports)
    // is offset by `import_count`. `host_import_map` keys are the
    // resolved "module::func" name (e.g. "display::clear").
    imports: Vec<ImportEntry>,
    host_import_map: std::collections::HashMap<String, u32>,
    import_count: u32,

    // Per-function state
    fn_map: std::collections::HashMap<String, u32>,
    local_map: Vec<std::collections::HashMap<String, u32>>,
    local_types: Vec<u8>,
    string_map: std::collections::HashMap<String, (u32, u32)>,
    // Control frames (if/match) currently open BETWEEN the emit point and the
    // innermost enclosing loop's body. `break`/`continue` add this to their
    // branch depth so they reach the loop's block/loop frame even when nested
    // inside conditionals (br targets are relative). Saved/reset around each
    // loop, balanced inc/dec around each `if`/match arm → returns to 0.
    extra_depth: u32,
}

struct ImportEntry {
    module: String,
    field: String,
    type_idx: u32,
}

struct FuncBody {
    type_idx: u32,
    locals: Vec<u8>,
    code: Vec<u8>,
}

impl WasmEmitter {
    fn new() -> Self {
        Self {
            types: Vec::new(),
            functions: Vec::new(),
            exports: Vec::new(),
            data_segments: Vec::new(),
            data_offset: 1024, // start data segment at 1KB
            imports: Vec::new(),
            host_import_map: std::collections::HashMap::new(),
            import_count: 0,
            fn_map: std::collections::HashMap::new(),
            local_map: Vec::new(),
            local_types: Vec::new(),
            string_map: std::collections::HashMap::new(),
            extra_depth: 0,
        }
    }

    fn emit_module(&mut self, module: &TypedModule) -> Result<(), CompileError> {
        // Register all functions first (for forward references)
        for (i, f) in module.functions.iter().enumerate() {
            self.fn_map.insert(f.name.clone(), i as u32);
        }

        // Collect host imports up front. Their wasm function indices are
        // 0..import_count, and their types occupy the low type indices,
        // so this must run before any local function emits its type.
        for f in &module.functions {
            self.scan_block_imports(&f.body);
        }
        self.import_count = self.imports.len() as u32;

        // Emit each function. Local function index = import_count + its
        // position in the module, so exports point past the imports.
        for f in &module.functions {
            self.emit_function(f)?;
            let local_pos = self.fn_map[&f.name];
            self.exports.push((f.name.clone(), 0x00, self.import_count + local_pos));
        }

        Ok(())
    }

    /// Register a host import (idempotent) and intern its wasm type.
    /// The wasm import module name is `host_<module>` to match the
    /// loader's import object (see `src/app/display.rs`).
    fn register_import(&mut self, module: &str, func: &str, params: &[ResolvedType], ret: &ResolvedType) {
        let key = format!("{module}::{func}");
        if self.host_import_map.contains_key(&key) {
            return;
        }
        let type_idx = self.intern_functype(params, ret);
        let import_idx = self.imports.len() as u32;
        self.imports.push(ImportEntry {
            module: format!("host_{module}"),
            field: func.to_string(),
            type_idx,
        });
        self.host_import_map.insert(key, import_idx);
    }

    fn intern_functype(&mut self, params: &[ResolvedType], ret: &ResolvedType) -> u32 {
        let mut sig = vec![0x60];
        sig.push(params.len() as u8);
        for p in params {
            sig.push(resolved_to_wasm(p));
        }
        if *ret == ResolvedType::Void {
            sig.push(0);
        } else {
            sig.push(1);
            sig.push(resolved_to_wasm(ret));
        }
        let idx = self.types.len() as u32;
        self.types.push(sig);
        idx
    }

    fn scan_block_imports(&mut self, block: &TypedBlock) {
        for stmt in &block.stmts {
            match stmt {
                TypedStmt::Let { init, .. } => self.scan_expr_imports(init),
                TypedStmt::Assign { value, .. } => self.scan_expr_imports(value),
                TypedStmt::Return { value } => {
                    if let Some(v) = value {
                        self.scan_expr_imports(v);
                    }
                }
                TypedStmt::Expr { expr } => self.scan_expr_imports(expr),
            }
        }
        if let Some(tail) = &block.tail {
            self.scan_expr_imports(tail);
        }
    }

    fn scan_expr_imports(&mut self, expr: &TypedExpr) {
        match &expr.kind {
            TypedExprKind::HostCall { module, func, args, ret_ty } => {
                let param_tys: Vec<ResolvedType> = args.iter().map(|a| a.ty.clone()).collect();
                self.register_import(module, func, &param_tys, ret_ty);
                for a in args {
                    self.scan_expr_imports(a);
                }
            }
            TypedExprKind::Call { func, args } => {
                self.scan_expr_imports(func);
                for a in args {
                    self.scan_expr_imports(a);
                }
            }
            TypedExprKind::MethodCall { object, args, .. } => {
                self.scan_expr_imports(object);
                for a in args {
                    self.scan_expr_imports(a);
                }
            }
            TypedExprKind::FieldAccess { object, .. } => self.scan_expr_imports(object),
            TypedExprKind::StructLit { fields, .. } => {
                for (_, v) in fields {
                    self.scan_expr_imports(v);
                }
            }
            TypedExprKind::TupleLit(exprs) => {
                for e in exprs {
                    self.scan_expr_imports(e);
                }
            }
            TypedExprKind::BinOp { lhs, rhs, .. } => {
                self.scan_expr_imports(lhs);
                self.scan_expr_imports(rhs);
            }
            TypedExprKind::UnaryOp { operand, .. } => self.scan_expr_imports(operand),
            TypedExprKind::If { cond, then_block, else_block } => {
                self.scan_expr_imports(cond);
                self.scan_block_imports(then_block);
                match else_block {
                    Some(TypedElse::Block(b)) => self.scan_block_imports(b),
                    Some(TypedElse::If(e)) => self.scan_expr_imports(e),
                    None => {}
                }
            }
            TypedExprKind::Match { scrutinee, arms, .. } => {
                self.scan_expr_imports(scrutinee);
                for arm in arms {
                    self.scan_expr_imports(&arm.body);
                }
            }
            TypedExprKind::While { cond, body } => {
                self.scan_expr_imports(cond);
                self.scan_block_imports(body);
            }
            TypedExprKind::Loop { body } => self.scan_block_imports(body),
            TypedExprKind::Break { value } => {
                if let Some(v) = value {
                    self.scan_expr_imports(v);
                }
            }
            TypedExprKind::Block(block) => self.scan_block_imports(block),
            TypedExprKind::IntLit(_)
            | TypedExprKind::FloatLit(_)
            | TypedExprKind::StringLit(_)
            | TypedExprKind::BoolLit(_)
            | TypedExprKind::Var(_)
            | TypedExprKind::Path(_)
            | TypedExprKind::Continue => {}
        }
    }

    fn emit_function(&mut self, f: &TypedFn) -> Result<(), CompileError> {
        // Build type signature
        let mut sig = Vec::new();
        sig.push(0x60); // func type
        // Params
        sig.push(f.params.len() as u8);
        for (_, ty) in &f.params {
            sig.push(resolved_to_wasm(ty));
        }
        // Returns
        if f.ret_type == ResolvedType::Void {
            sig.push(0); // no return
        } else {
            sig.push(1);
            sig.push(resolved_to_wasm(&f.ret_type));
        }
        let type_idx = self.types.len() as u32;
        self.types.push(sig);

        // Set up locals
        self.local_map.push(std::collections::HashMap::new());
        self.local_types = Vec::new();

        // Params are locals 0..n
        for (i, (name, _ty)) in f.params.iter().enumerate() {
            self.local_map.last_mut().unwrap().insert(name.clone(), i as u32);
        }

        // Emit body
        let mut code = Vec::new();
        self.emit_block_code(&f.body, &mut code)?;
        code.push(OP_END);

        // Build locals section for the function body
        let mut locals_encoded = Vec::new();
        if !self.local_types.is_empty() {
            // Group consecutive locals of the same type
            let mut groups: Vec<(u32, u8)> = Vec::new();
            for &ty in &self.local_types {
                if let Some(last) = groups.last_mut() {
                    if last.1 == ty {
                        last.0 += 1;
                        continue;
                    }
                }
                groups.push((1, ty));
            }
            leb128_u32(groups.len() as u32, &mut locals_encoded);
            for (count, ty) in groups {
                leb128_u32(count, &mut locals_encoded);
                locals_encoded.push(ty);
            }
        } else {
            leb128_u32(0, &mut locals_encoded);
        }

        self.functions.push(FuncBody {
            type_idx,
            locals: locals_encoded,
            code,
        });

        self.local_map.pop();
        Ok(())
    }

    fn alloc_local(&mut self, name: &str, ty: &ResolvedType) -> u32 {
        // The function uses ONE flat local map (params inserted first,
        // then every declared local), so the next wasm local index is
        // simply the current map size. Adding `local_types.len()` here
        // double-counts the declared locals already in the map and makes
        // indices for the 2nd+ local invalid.
        let wasm_ty = resolved_to_wasm(ty);
        let local_idx = self.local_map.last().unwrap().len() as u32;
        self.local_types.push(wasm_ty);
        self.local_map.last_mut().unwrap().insert(name.to_string(), local_idx);
        local_idx
    }

    fn emit_block_code(&mut self, block: &TypedBlock, code: &mut Vec<u8>) -> Result<(), CompileError> {
        for stmt in &block.stmts {
            self.emit_stmt(stmt, code)?;
        }
        if let Some(tail) = &block.tail {
            self.emit_expr(tail, code)?;
        }
        Ok(())
    }

    fn emit_stmt(&mut self, stmt: &TypedStmt, code: &mut Vec<u8>) -> Result<(), CompileError> {
        match stmt {
            TypedStmt::Let { name, ty, init, .. } => {
                let local_idx = self.alloc_local(name, ty);
                self.emit_expr(init, code)?;
                code.push(OP_LOCAL_SET);
                leb128_u32(local_idx, code);
            }
            TypedStmt::Assign { place, value, .. } => {
                self.emit_expr(value, code)?;
                let local_idx = *self.local_map.last().unwrap().get(&place.root)
                    .ok_or_else(|| CompileError::new(format!("undefined local '{}'", place.root)))?;
                code.push(OP_LOCAL_SET);
                leb128_u32(local_idx, code);
            }
            TypedStmt::Return { value } => {
                if let Some(val) = value {
                    self.emit_expr(val, code)?;
                }
                code.push(OP_RETURN);
            }
            TypedStmt::Expr { expr } => {
                self.emit_expr(expr, code)?;
                if expr.ty != ResolvedType::Void {
                    code.push(OP_DROP);
                }
            }
        }
        Ok(())
    }

    fn emit_expr(&mut self, expr: &TypedExpr, code: &mut Vec<u8>) -> Result<(), CompileError> {
        match &expr.kind {
            TypedExprKind::IntLit(n) => {
                code.push(OP_I32_CONST);
                leb128_i32(*n as i32, code);
            }
            TypedExprKind::FloatLit(n) => {
                code.push(OP_F64_CONST);
                code.extend_from_slice(&n.to_le_bytes());
            }
            TypedExprKind::BoolLit(b) => {
                code.push(OP_I32_CONST);
                leb128_i32(if *b { 1 } else { 0 }, code);
            }
            TypedExprKind::StringLit(s) => {
                // Store string data in data segment, push pointer
                let (ptr, _len) = self.intern_string(s);
                code.push(OP_I32_CONST);
                leb128_i32(ptr as i32, code);
            }
            TypedExprKind::Var(name) => {
                let local_idx = *self.local_map.last().unwrap().get(name)
                    .ok_or_else(|| CompileError::new(format!("undefined local '{name}'")))?;
                code.push(OP_LOCAL_GET);
                leb128_u32(local_idx, code);
            }
            TypedExprKind::Path(_segments) => {
                // Unit enum variant — represented as tag value (i32)
                code.push(OP_I32_CONST);
                leb128_i32(0, code);
            }
            TypedExprKind::FieldAccess { object, field_index, .. } => {
                // For now: emit the object, then load from offset
                self.emit_expr(object, code)?;
                // Field access on stack-allocated structs: the object is
                // a pointer; load field at known offset
                code.push(OP_I32_CONST);
                leb128_i32((*field_index as i32) * 4, code);
                code.push(OP_I32_ADD);
                code.push(OP_I32_LOAD);
                code.push(2); // alignment
                code.push(0); // offset
            }
            TypedExprKind::Call { func, args } => {
                for arg in args {
                    self.emit_expr(arg, code)?;
                }
                let fn_name = match &func.kind {
                    TypedExprKind::Var(name) => name.clone(),
                    TypedExprKind::Path(p) => p.join("::"),
                    _ => return Err(CompileError::new("cannot call non-function")),
                };
                let fn_idx = *self.fn_map.get(&fn_name)
                    .ok_or_else(|| CompileError::new(format!("undefined function '{fn_name}'")))?;
                code.push(OP_CALL);
                leb128_u32(self.import_count + fn_idx, code);
            }
            TypedExprKind::HostCall { module, func, args, .. } => {
                for arg in args {
                    self.emit_expr(arg, code)?;
                }
                let key = format!("{module}::{func}");
                let import_idx = *self.host_import_map.get(&key)
                    .ok_or_else(|| CompileError::new(format!("unregistered host import '{key}'")))?;
                code.push(OP_CALL);
                leb128_u32(import_idx, code);
            }
            TypedExprKind::MethodCall { object, method: _, args } => {
                // Desugar to Type::method(object, args...)
                self.emit_expr(object, code)?;
                for arg in args {
                    self.emit_expr(arg, code)?;
                }
                // For now, method calls go to host imports (unresolved at this stage)
                code.push(OP_I32_CONST);
                leb128_i32(0, code);
            }
            TypedExprKind::StructLit { fields, .. } => {
                // For now, push each field value onto stack
                // In a real impl, allocate arena memory and store fields
                for (_, val) in fields {
                    self.emit_expr(val, code)?;
                }
                // If more than one field, only keep the last (simplified)
                for _ in 1..fields.len() {
                    // In the real version, we'd store each to memory
                }
            }
            TypedExprKind::TupleLit(exprs) => {
                for e in exprs {
                    self.emit_expr(e, code)?;
                }
            }
            TypedExprKind::BinOp { op: op @ (BinOp::And | BinOp::Or), lhs, rhs } => {
                // Short-circuit boolean ops. Operands are bool (i32 0/1).
                // `a && b` ≡ `if a { b } else { 0 }`; `a || b` ≡
                // `if a { 1 } else { b }`. Emitting an `if (result i32)`
                // block means `rhs` only runs when needed — matching Rust
                // semantics (so e.g. `i != 0 && 100/i > 1` can't divide by
                // zero). The previous codegen left the stack imbalanced and
                // ignored `lhs`, producing invalid wasm.
                self.emit_expr(lhs, code)?;
                code.push(OP_IF);
                code.push(resolved_to_wasm(&ResolvedType::I32));
                match op {
                    BinOp::And => {
                        self.emit_expr(rhs, code)?;
                        code.push(OP_ELSE);
                        code.push(OP_I32_CONST);
                        leb128_i32(0, code);
                    }
                    BinOp::Or => {
                        code.push(OP_I32_CONST);
                        leb128_i32(1, code);
                        code.push(OP_ELSE);
                        self.emit_expr(rhs, code)?;
                    }
                    _ => unreachable!(),
                }
                code.push(OP_END);
            }
            TypedExprKind::BinOp { op, lhs, rhs } => {
                self.emit_expr(lhs, code)?;
                self.emit_expr(rhs, code)?;
                let opcode = match (&lhs.ty, op) {
                    (ResolvedType::I32, BinOp::Add) => OP_I32_ADD,
                    (ResolvedType::I32, BinOp::Sub) => OP_I32_SUB,
                    (ResolvedType::I32, BinOp::Mul) => OP_I32_MUL,
                    (ResolvedType::I32, BinOp::Div) => OP_I32_DIV_S,
                    (ResolvedType::I32, BinOp::Mod) => OP_I32_REM_S,
                    (ResolvedType::I32, BinOp::Eq) => OP_I32_EQ,
                    (ResolvedType::I32, BinOp::Ne) => OP_I32_NE,
                    (ResolvedType::I32, BinOp::Lt) => OP_I32_LT_S,
                    (ResolvedType::I32, BinOp::Gt) => OP_I32_GT_S,
                    (ResolvedType::I32, BinOp::Le) => OP_I32_LE_S,
                    (ResolvedType::I32, BinOp::Ge) => OP_I32_GE_S,
                    (ResolvedType::I32, BinOp::BitAnd) => OP_I32_AND,
                    (ResolvedType::I32, BinOp::BitOr) => OP_I32_OR,
                    (ResolvedType::I32, BinOp::BitXor) => OP_I32_XOR,
                    (ResolvedType::I32, BinOp::Shl) => OP_I32_SHL,
                    (ResolvedType::I32, BinOp::Shr) => OP_I32_SHR_S,
                    (ResolvedType::I64, BinOp::Add) => OP_I64_ADD,
                    (ResolvedType::I64, BinOp::Sub) => OP_I64_SUB,
                    (ResolvedType::I64, BinOp::Mul) => OP_I64_MUL,
                    (ResolvedType::I64, BinOp::Div) => OP_I64_DIV_S,
                    (ResolvedType::I64, BinOp::Mod) => OP_I64_REM_S,
                    (ResolvedType::I64, BinOp::Eq) => OP_I64_EQ,
                    (ResolvedType::I64, BinOp::Ne) => OP_I64_NE,
                    (ResolvedType::I64, BinOp::Lt) => OP_I64_LT_S,
                    (ResolvedType::I64, BinOp::Gt) => OP_I64_GT_S,
                    (ResolvedType::I64, BinOp::Le) => OP_I64_LE_S,
                    (ResolvedType::I64, BinOp::Ge) => OP_I64_GE_S,
                    (ResolvedType::I64, BinOp::BitAnd) => OP_I64_AND,
                    (ResolvedType::I64, BinOp::BitOr) => OP_I64_OR,
                    (ResolvedType::I64, BinOp::BitXor) => OP_I64_XOR,
                    (ResolvedType::I64, BinOp::Shl) => OP_I64_SHL,
                    (ResolvedType::I64, BinOp::Shr) => OP_I64_SHR_S,
                    (ResolvedType::F64, BinOp::Add) => OP_F64_ADD,
                    (ResolvedType::F64, BinOp::Sub) => OP_F64_SUB,
                    (ResolvedType::F64, BinOp::Mul) => OP_F64_MUL,
                    (ResolvedType::F64, BinOp::Div) => OP_F64_DIV,
                    (ResolvedType::F64, BinOp::Eq) => OP_F64_EQ,
                    (ResolvedType::F64, BinOp::Ne) => OP_F64_NE,
                    (ResolvedType::F64, BinOp::Lt) => OP_F64_LT,
                    (ResolvedType::F64, BinOp::Gt) => OP_F64_GT,
                    (ResolvedType::F64, BinOp::Le) => OP_F64_LE,
                    (ResolvedType::F64, BinOp::Ge) => OP_F64_GE,
                    // And/Or are handled by the short-circuit arm above.
                    _ => return Err(CompileError::new(format!("unsupported binop {:?} for {:?}", op, lhs.ty))),
                };
                code.push(opcode);
            }
            TypedExprKind::UnaryOp { op, operand } => {
                match op {
                    UnaryOp::Neg => {
                        match &operand.ty {
                            ResolvedType::I32 => {
                                code.push(OP_I32_CONST);
                                leb128_i32(0, code);
                                self.emit_expr(operand, code)?;
                                code.push(OP_I32_SUB);
                            }
                            ResolvedType::I64 => {
                                code.push(OP_I64_CONST);
                                leb128_i64(0, code);
                                self.emit_expr(operand, code)?;
                                code.push(OP_I64_SUB);
                            }
                            ResolvedType::F64 => {
                                self.emit_expr(operand, code)?;
                                code.push(OP_F64_NEG);
                            }
                            _ => return Err(CompileError::new("neg on non-numeric")),
                        }
                    }
                    UnaryOp::Not => {
                        self.emit_expr(operand, code)?;
                        code.push(OP_I32_EQZ);
                    }
                }
            }
            TypedExprKind::If { cond, then_block, else_block } => {
                self.emit_expr(cond, code)?;
                let block_ty = if then_block.ty == ResolvedType::Void {
                    BLOCK_VOID
                } else {
                    resolved_to_wasm(&then_block.ty)
                };
                code.push(OP_IF);
                code.push(block_ty);
                // Inside the if frame: break/continue must step one frame further.
                self.extra_depth += 1;
                self.emit_block_code(then_block, code)?;
                if let Some(else_branch) = else_block {
                    code.push(OP_ELSE);
                    match else_branch {
                        TypedElse::Block(b) => self.emit_block_code(b, code)?,
                        TypedElse::If(e) => self.emit_expr(e, code)?,
                    }
                }
                self.extra_depth -= 1;
                code.push(OP_END);
            }
            TypedExprKind::Match { scrutinee, arms, result_ty } => {
                // Simplified: emit as chained if-else for now
                // A real impl would use br_table for dense integer matches
                let scrutinee_local = self.alloc_local("__match_scrutinee", &scrutinee.ty);
                self.emit_expr(scrutinee, code)?;
                code.push(OP_LOCAL_SET);
                leb128_u32(scrutinee_local, code);

                let block_ty = if *result_ty == ResolvedType::Void {
                    BLOCK_VOID
                } else {
                    resolved_to_wasm(result_ty)
                };

                // Nested if-else chain
                for (i, arm) in arms.iter().enumerate() {
                    let is_last = i == arms.len() - 1;
                    if !is_wildcard(&arm.pattern) && !is_last {
                        // Emit condition check
                        self.emit_pattern_check(&arm.pattern, scrutinee_local, &scrutinee.ty, code)?;
                        code.push(OP_IF);
                        code.push(block_ty);
                        self.extra_depth += 1; // arm body is one if-frame deeper
                    }
                    self.emit_expr(&arm.body, code)?;
                    if !is_wildcard(&arm.pattern) && !is_last {
                        code.push(OP_ELSE);
                    }
                }
                // Close all the if-else chains
                for (i, arm) in arms.iter().enumerate() {
                    let is_last = i == arms.len() - 1;
                    if !is_wildcard(&arm.pattern) && !is_last {
                        code.push(OP_END);
                        self.extra_depth -= 1;
                    }
                }
            }
            TypedExprKind::While { cond, body } => {
                // New loop → the body's break/continue base level. Save + reset
                // `extra_depth` so nested-if accounting starts fresh, restore after.
                let saved = self.extra_depth;
                self.extra_depth = 0;
                code.push(OP_BLOCK);
                code.push(BLOCK_VOID);
                code.push(OP_LOOP);
                code.push(BLOCK_VOID);
                // Check condition (emitted directly in the loop body — depth 0)
                self.emit_expr(cond, code)?;
                code.push(OP_I32_EQZ);
                code.push(OP_BR_IF);
                leb128_u32(1, code); // break out of block
                // Body
                self.emit_block_code(body, code)?;
                code.push(OP_BR);
                leb128_u32(0, code); // continue loop
                code.push(OP_END); // end loop
                code.push(OP_END); // end block
                self.extra_depth = saved;
            }
            TypedExprKind::Loop { body } => {
                let saved = self.extra_depth;
                self.extra_depth = 0;
                code.push(OP_BLOCK);
                code.push(BLOCK_VOID);
                code.push(OP_LOOP);
                code.push(BLOCK_VOID);
                self.emit_block_code(body, code)?;
                code.push(OP_BR);
                leb128_u32(0, code);
                code.push(OP_END);
                code.push(OP_END);
                self.extra_depth = saved;
            }
            TypedExprKind::Break { .. } => {
                code.push(OP_BR);
                // Exit the loop's block, stepping past any enclosing if/match
                // frames (br targets are relative to the current frame nesting).
                leb128_u32(self.extra_depth + 1, code);
            }
            TypedExprKind::Continue => {
                code.push(OP_BR);
                // Re-enter the loop, past any enclosing if/match frames.
                leb128_u32(self.extra_depth, code);
            }
            TypedExprKind::Block(block) => {
                self.emit_block_code(block, code)?;
            }
        }
        Ok(())
    }

    fn emit_pattern_check(&mut self, pattern: &crate::rustlite::ast::Pattern, scrutinee_local: u32, _scrutinee_ty: &ResolvedType, code: &mut Vec<u8>) -> Result<(), CompileError> {
        match &pattern.kind {
            crate::rustlite::ast::PatternKind::Literal(lit) => {
                code.push(OP_LOCAL_GET);
                leb128_u32(scrutinee_local, code);
                match lit {
                    crate::rustlite::ast::LitPattern::Int(n) => {
                        code.push(OP_I32_CONST);
                        leb128_i32(*n as i32, code);
                        code.push(OP_I32_EQ);
                    }
                    crate::rustlite::ast::LitPattern::Bool(b) => {
                        code.push(OP_I32_CONST);
                        leb128_i32(if *b { 1 } else { 0 }, code);
                        code.push(OP_I32_EQ);
                    }
                    _ => {
                        code.push(OP_I32_CONST);
                        leb128_i32(1, code);
                    }
                }
            }
            _ => {
                // Binding or wildcard: always matches
                code.push(OP_I32_CONST);
                leb128_i32(1, code);
            }
        }
        Ok(())
    }

    fn intern_string(&mut self, s: &str) -> (u32, u32) {
        if let Some(&cached) = self.string_map.get(s) {
            return cached;
        }
        let ptr = self.data_offset;
        let len = s.len() as u32;
        // Length-prefixed: 4 bytes len + payload
        let mut data = Vec::with_capacity(4 + s.len());
        data.extend_from_slice(&len.to_le_bytes());
        data.extend_from_slice(s.as_bytes());
        self.data_segments.push((ptr, data));
        self.data_offset += 4 + len;
        // Align to 4
        let padding = (4 - (self.data_offset % 4)) % 4;
        self.data_offset += padding;
        self.string_map.insert(s.to_string(), (ptr, len));
        (ptr, len)
    }

    fn finish(self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(WASM_MAGIC);
        out.extend_from_slice(WASM_VERSION);

        // Type section
        {
            let mut sec = Vec::new();
            leb128_u32(self.types.len() as u32, &mut sec);
            for ty in &self.types {
                sec.extend_from_slice(ty);
            }
            write_section(SEC_TYPE, &sec, &mut out);
        }

        // Import section — host functions occupy function indices
        // 0..import_count, ahead of all local functions.
        if !self.imports.is_empty() {
            let mut sec = Vec::new();
            leb128_u32(self.imports.len() as u32, &mut sec);
            for imp in &self.imports {
                leb128_u32(imp.module.len() as u32, &mut sec);
                sec.extend_from_slice(imp.module.as_bytes());
                leb128_u32(imp.field.len() as u32, &mut sec);
                sec.extend_from_slice(imp.field.as_bytes());
                sec.push(0x00); // import kind: func
                leb128_u32(imp.type_idx, &mut sec);
            }
            write_section(SEC_IMPORT, &sec, &mut out);
        }

        // Function section (maps each local func to its type index)
        {
            let mut sec = Vec::new();
            leb128_u32(self.functions.len() as u32, &mut sec);
            for func in &self.functions {
                leb128_u32(func.type_idx, &mut sec);
            }
            write_section(SEC_FUNCTION, &sec, &mut out);
        }

        // Memory section — 1 page minimum
        {
            let mut sec = Vec::new();
            leb128_u32(1, &mut sec); // 1 memory
            sec.push(0x00); // no max
            leb128_u32(1, &mut sec); // 1 page initial
            write_section(SEC_MEMORY, &sec, &mut out);
        }

        // Export section
        {
            let mut sec = Vec::new();
            // Export memory
            let total_exports = self.exports.len() + 1;
            leb128_u32(total_exports as u32, &mut sec);

            // Memory export
            let mem_name = "memory";
            leb128_u32(mem_name.len() as u32, &mut sec);
            sec.extend_from_slice(mem_name.as_bytes());
            sec.push(0x02); // memory
            leb128_u32(0, &mut sec);

            for (name, kind, idx) in &self.exports {
                leb128_u32(name.len() as u32, &mut sec);
                sec.extend_from_slice(name.as_bytes());
                sec.push(*kind);
                leb128_u32(*idx, &mut sec);
            }
            write_section(SEC_EXPORT, &sec, &mut out);
        }

        // Code section
        {
            let mut sec = Vec::new();
            leb128_u32(self.functions.len() as u32, &mut sec);
            for func in &self.functions {
                let mut body = Vec::new();
                body.extend_from_slice(&func.locals);
                body.extend_from_slice(&func.code);
                // Body size
                leb128_u32(body.len() as u32, &mut sec);
                sec.extend_from_slice(&body);
            }
            write_section(SEC_CODE, &sec, &mut out);
        }

        // Data section
        if !self.data_segments.is_empty() {
            let mut sec = Vec::new();
            leb128_u32(self.data_segments.len() as u32, &mut sec);
            for (offset, data) in &self.data_segments {
                sec.push(0x00); // active, memory 0
                sec.push(OP_I32_CONST);
                leb128_i32(*offset as i32, &mut sec);
                sec.push(OP_END);
                leb128_u32(data.len() as u32, &mut sec);
                sec.extend_from_slice(data);
            }
            write_section(SEC_DATA, &sec, &mut out);
        }

        out
    }
}

fn is_wildcard(pattern: &crate::rustlite::ast::Pattern) -> bool {
    matches!(pattern.kind, crate::rustlite::ast::PatternKind::Wildcard | crate::rustlite::ast::PatternKind::Binding(_))
}

fn resolved_to_wasm(ty: &ResolvedType) -> u8 {
    match ty {
        ResolvedType::I32 | ResolvedType::Bool => WASM_I32,
        ResolvedType::I64 => WASM_I64,
        ResolvedType::F32 => WASM_F32,
        ResolvedType::F64 => WASM_F64,
        ResolvedType::String => WASM_I32, // pointer
        _ => WASM_I32, // structs/enums are pointers or tags
    }
}

fn write_section(id: u8, data: &[u8], out: &mut Vec<u8>) {
    out.push(id);
    leb128_u32(data.len() as u32, out);
    out.extend_from_slice(data);
}

fn leb128_u32(mut val: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (val & 0x7F) as u8;
        val >>= 7;
        if val != 0 { byte |= 0x80; }
        out.push(byte);
        if val == 0 { break; }
    }
}

fn leb128_i32(mut val: i32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (val & 0x7F) as u8;
        val >>= 7;
        let more = !((val == 0 && byte & 0x40 == 0) || (val == -1 && byte & 0x40 != 0));
        if more { byte |= 0x80; }
        out.push(byte);
        if !more { break; }
    }
}

fn leb128_i64(mut val: i64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (val & 0x7F) as u8;
        val >>= 7;
        let more = !((val == 0 && byte & 0x40 == 0) || (val == -1 && byte & 0x40 != 0));
        if more { byte |= 0x80; }
        out.push(byte);
        if !more { break; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rustlite::{lexer, parser, typecheck};

    fn compile_to_wasm(source: &str) -> Vec<u8> {
        let tokens = lexer::lex(source).unwrap();
        let module = parser::parse(&tokens).unwrap();
        let typed = typecheck::check(&module).unwrap();
        emit(&typed).unwrap()
    }

    #[test]
    fn emit_simple_add() {
        let wasm = compile_to_wasm("fn add(a: i32, b: i32) -> i32 { a + b }");
        // Check wasm magic
        assert_eq!(&wasm[0..4], WASM_MAGIC);
        assert_eq!(&wasm[4..8], WASM_VERSION);
        assert!(wasm.len() > 8);
    }

    #[test]
    fn emit_const_fn() {
        let wasm = compile_to_wasm("fn answer() -> i32 { 42 }");
        assert_eq!(&wasm[0..4], WASM_MAGIC);
    }

    #[test]
    fn emit_if_else() {
        let wasm = compile_to_wasm("fn abs(x: i32) -> i32 { if x > 0 { x } else { 0 - x } }");
        assert_eq!(&wasm[0..4], WASM_MAGIC);
    }

    #[test]
    fn emit_short_circuit_bool() {
        // `&&` / `||` previously emitted stack-imbalanced (invalid) wasm.
        // They now compile to short-circuit `if`-blocks. This guards the
        // compile path; correct *execution* (incl. div-by-zero short-
        // circuit) was validated by instantiating the output in node.
        let wasm = compile_to_wasm(
            "fn t(a: i32, b: i32) -> i32 { if a > 0 && b > 0 { 1 } else { 0 } }\n\
             fn u(a: i32, b: i32) -> i32 { if a > 0 || b > 0 { 1 } else { 0 } }",
        );
        assert_eq!(&wasm[0..4], WASM_MAGIC);
        assert!(wasm.len() > 16);
    }

    #[test]
    fn emit_while_loop() {
        let wasm = compile_to_wasm(r#"
            fn sum_to(n: i32) -> i32 {
                let mut total: i32 = 0;
                let mut i: i32 = 1;
                while i <= n {
                    total = total + i;
                    i = i + 1;
                }
                total
            }
        "#);
        assert_eq!(&wasm[0..4], WASM_MAGIC);
    }

    #[test]
    fn emit_string_data() {
        let wasm = compile_to_wasm(r#"fn greet() -> String { "hello world" }"#);
        // Should contain the string in data section
        let hello = b"hello world";
        let found = wasm.windows(hello.len()).any(|w| w == hello);
        assert!(found, "wasm should contain string data");
    }

    /// Walk a wasm module's top-level sections, returning their ids in
    /// order. Reliable presence check (vs. scanning for a raw byte,
    /// which collides with leb/opcode bytes).
    fn section_ids(wasm: &[u8]) -> Vec<u8> {
        let mut ids = Vec::new();
        let mut i = 8; // skip magic + version
        while i < wasm.len() {
            let id = wasm[i];
            i += 1;
            // decode unsigned LEB128 size
            let mut size = 0u32;
            let mut shift = 0;
            loop {
                let byte = wasm[i];
                i += 1;
                size |= ((byte & 0x7f) as u32) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }
            ids.push(id);
            i += size as usize;
        }
        ids
    }

    #[test]
    fn emit_host_display_import() {
        let wasm = compile_to_wasm(
            r#"
            use host::display;
            fn frame(t: i32) {
                display::clear(0);
                display::fill_rect(t, 0, 10, 10, 16777215);
                display::present();
            }
        "#,
        );
        assert_eq!(&wasm[0..4], WASM_MAGIC);
        assert!(section_ids(&wasm).contains(&SEC_IMPORT), "expected an import section");
        // The wasm import module name + fields the loader provides.
        for needle in [&b"host_display"[..], b"clear", b"fill_rect", b"present"] {
            assert!(
                wasm.windows(needle.len()).any(|w| w == needle),
                "wasm should reference {:?}",
                std::str::from_utf8(needle).unwrap(),
            );
        }
    }

    #[test]
    fn emit_host_net_import() {
        // A cartridge that opens a WebSocket, sends a message, and drains
        // its inbox each frame — the multiplayer/sync primitive. Asserts
        // the `host_net` import module + fields the loader provides land
        // in the wasm import section.
        let wasm = compile_to_wasm(
            r#"
            fn frame(t: i32) {
                let sock: i32 = host::net::open(0);
                if host::net::status(sock) == 1 {
                    host::net::send(sock, 8);
                    let n: i32 = host::net::poll(sock, 64, 256);
                    host::net::close(sock);
                }
            }
        "#,
        );
        assert_eq!(&wasm[0..4], WASM_MAGIC);
        assert!(section_ids(&wasm).contains(&SEC_IMPORT), "expected an import section");
        for needle in [&b"host_net"[..], b"open", b"send", b"poll", b"status", b"close"] {
            assert!(
                wasm.windows(needle.len()).any(|w| w == needle),
                "wasm should reference {:?}",
                std::str::from_utf8(needle).unwrap(),
            );
        }
    }

    #[test]
    fn emit_host_audio_import() {
        // A cartridge that plays a tone, schedules a delayed note, fires a
        // noise burst, sets volume, and stops a voice — the Web Audio
        // primitives. Asserts the `host_audio` import module + fields the
        // loader/display host provides land in the wasm import section.
        // Codegen is generic over `host_<module>`, so the only compiler
        // change is the typecheck signature table — this guards that wiring.
        let wasm = compile_to_wasm(
            r#"
            use host::audio;
            fn frame(t: i32) {
                let v: i32 = audio::tone(440, 200, 0);
                audio::tone_at(660, 120, 1, 100);
                audio::noise(80);
                audio::set_volume(50);
                audio::stop(v);
            }
        "#,
        );
        assert_eq!(&wasm[0..4], WASM_MAGIC);
        assert!(section_ids(&wasm).contains(&SEC_IMPORT), "expected an import section");
        for needle in
            [&b"host_audio"[..], b"tone", b"tone_at", b"noise", b"stop", b"set_volume"]
        {
            assert!(
                wasm.windows(needle.len()).any(|w| w == needle),
                "wasm should reference {:?}",
                std::str::from_utf8(needle).unwrap(),
            );
        }
    }

    #[test]
    fn emit_host_display_3d_import() {
        // A cartridge drawing software 3D primitives: a flat-filled triangle
        // and a line. Asserts the new host_display 3D fields (FB#12b) land in
        // the wasm import section so codegen auto-derives the signatures from
        // the typecheck table.
        let wasm = compile_to_wasm(
            r#"
            use host::display;
            fn frame(t: i32) {
                display::fill_triangle(0, 0, 50, 0, 0, 50, 255);
                display::draw_line(0, 0, t, 143, 16777215);
            }
        "#,
        );
        assert_eq!(&wasm[0..4], WASM_MAGIC);
        assert!(section_ids(&wasm).contains(&SEC_IMPORT), "expected an import section");
        for needle in [
            &b"host_display"[..],
            b"draw_line",
            b"fill_triangle",
        ] {
            assert!(
                wasm.windows(needle.len()).any(|w| w == needle),
                "wasm should reference {:?}",
                std::str::from_utf8(needle).unwrap(),
            );
        }
    }

    #[test]
    fn no_imports_when_no_host_calls() {
        let wasm = compile_to_wasm("fn add(a: i32, b: i32) -> i32 { a + b }");
        // Backward-compat: a module with no host calls has no import
        // section, so function indices are unshifted.
        assert!(!section_ids(&wasm).contains(&SEC_IMPORT), "no host calls => no import section");
    }
}
