# Agent-writes-Rust — design notes

Working notes for the subsystem that lets a localharness agent write a Rust
source file, have it compiled (in the browser) to wasm, and loaded as a
hot-swappable cartridge alongside the running host.

Persistent design surface — built up across hourly loop iterations starting
2026-05-26. Each iteration appends one focused section; nothing here is
landed code. Read top-to-bottom for current state; the iteration log at the
bottom is a chronological summary.

> **Status: in design, not implemented.** Source under `src/` is unchanged.
> Subject to revision by the user when they wake up.

---

## Immutable constraints

- One crate. Everything ships inside `localharness`. No sibling crates.
- Hand-rolled, zero-dep parser + typechecker + codegen. Skip `syn`, skip
  `cranelift`. We control the grammar; the grammar is what our parser accepts.
- No off-chain infrastructure. Substrate is the Tempo chain + the user's
  browser tab. No compile servers.
- No JS, no Python in the cartridge story. Source language is a Rust subset;
  output is wasm32 bytes.
- Host + cartridges architecture (not per-agent monolithic wasm). The bundle
  is a stable runtime; the agent's "soul" is its cartridge graph.

## Open questions

(Maintained as a live backlog. Items get checked off when worked through;
items get added when later sections surface them.)

- [x] **End-to-end strawman.** 30 lines of imaginary rustlite walked
  parse → typed IR → wasm bytes → instantiate → invoke. (Worked 2026-05-26.)
- [x] **Rustlite v0.1 grammar.** One-page EBNF, brutal subset.
  (Worked 2026-05-26. Revised: arena-per-invocation, so no `Rc`, no
  references, no lifetimes, no `&` at all in v0.1.)
- [x] **Cartridge ABI.** Host-module catalog + wasm-level wire format.
  (Worked 2026-05-26. v0.1 ABI = 9 modules, monomorphic types per module,
  manifest-gated capabilities, length-prefixed strings.)
- [ ] **Stdlib question.** `harness-std` cartridge vs host functions vs
  both. Where do `String`, `Vec`, `HashMap` live?
- [ ] **Compiler location in bundle.** Eager-loaded module (every visitor
  downloads the compiler) vs lazy cartridge (loaded only when an agent
  actually needs to compile).
- [ ] **Cartridge identity.** Content-addressed (sha256 of wasm bytes)?
  Signed by the authoring agent's TBA? On-chain registered in the diamond?
  Combination?
- [ ] **Cartridge versioning + rollback.** How does an agent revert a bad
  cartridge it just shipped? Per-cartridge version pointer in OPFS?
- [ ] **Neural-net-as-compiler.** Prior art survey (AlphaCode, Codex,
  neural superoptimizers, the user's tempo-x402 codegen model). What's
  the realistic 6-month research path?
- [ ] **Cartridge testing.** Unit harness in-browser? Property tests?
  How does the agent know its cartridge works before publishing?
- [ ] **Cartridge composition.** Capability tokens vs plain function
  imports vs message passing for cartridge-to-cartridge calls.
- [ ] **Hot-swap semantics.** What happens to in-flight invocations when
  a cartridge is swapped? State migration between versions?
- [ ] **Memory + fuel limits per cartridge.** Default budgets; how does
  an agent request more; what does OOM look like to the cartridge.
- [ ] **Macros.** Rustlite has none, but agents will want `format!` and
  friends. Hardcode a small set into the compiler, or expand them in a
  preprocessor pass? Or expose them as host functions?
- [ ] **Drop / RAII.** Without a borrow checker, what manages resource
  cleanup? Explicit `close()`? `Rc<T>` everywhere with `Drop` impls?
  *(Likely moot under arena-per-invocation, but revisit if long-running
  cartridges arrive in v0.2.)*
- [ ] **Char type.** v0.1 has none — `String` is the only character-
  shaped type, indexed as bytes. When do agents need UTF-8 char iteration?
- [ ] **`usize` equivalent.** Grammar has i32/i64/f32/f64/bool. Wasm
  pointers/lengths are i32; agents iterating arrays want a "natural
  number" type. Pick one and stick with it.
- [ ] **Async / blocking semantics.** Cartridges call host fns as
  blocking; host runs async runtime underneath. Acceptable for short
  invocations; revisit when a cartridge wants to await a 30s LLM call
  and have its memory survive that pause.
- [ ] **Manifest schema versioning.** Capability manifest is JSON v1;
  add `"manifest_version": 1` field and decide migration policy when
  v2 arrives.
- [ ] **Cross-invocation byte data.** `Bytes` handles are arena-scoped;
  storing one in KV needs explicit `bytes_to_string` (base64) round-trip.
  Decide whether to ship `host::str::base64_encode/decode` in v0.1 or
  defer.
- [ ] **Fuel budget origin.** Manifest declares `fuel_limit`. Who
  authorizes higher-than-default? Per-agent setting in OPFS? On-chain
  budget tied to credits? Resolve when wiring up actual enforcement.

---

## 2026-05-26 — End-to-end strawman

**Why this first:** the cheapest way to find what's missing in the design
is to write a plausible 30-line agent cartridge and walk it through every
stage of the pipeline. Every "wait, how does that actually work?" is an
unresolved decision worth surfacing now rather than after a parser is half
written.

### The source

A modest cartridge an agent might plausibly write on its first day:
something that takes a request, calls the host LLM, and returns a reply.

```rust
// summarize.rl     ("rl" = rustlite)

use host::llm;
use host::log;

struct Request {
    text: String,
}

struct Response {
    summary: String,
}

fn handle(req: Request) -> Response {
    let prompt = concat("Summarize in one sentence: ", req.text);
    let reply = llm::call(prompt);
    log::info("summarized");
    Response { summary: reply }
}
```

Twelve lines of actual logic. Already exposes a half-dozen open questions.

### What each stage looks like

#### 1. Source ingest

Agent writes the source to OPFS at e.g. `cartridges/summarize/src.rl`. The
runtime's cartridge tooling exposes a host function the agent calls:

```text
cartridge::build(source_path: "cartridges/summarize/src.rl")
  -> Result<CartridgeId, BuildError>
```

The runtime reads the file, runs parse → typecheck → codegen, writes the
wasm bytes to `cartridges/summarize/v1.wasm` next to the source, and
returns a CartridgeId the agent uses to load + invoke it.

**Decision:** cartridge source AND compiled wasm both live in the agent's
own OPFS. Source is durable evidence of what the agent intended; wasm is
the cached compile artifact. Recompile rebuilds the wasm from source.

#### 2. Parse

Hand-rolled recursive descent. Token stream from a tiny lexer.

The strawman above tokenizes to roughly:

```text
USE IDENT("host") COLONCOLON IDENT("llm") SEMI
USE IDENT("host") COLONCOLON IDENT("log") SEMI
STRUCT IDENT("Request") LBRACE
  IDENT("text") COLON IDENT("String") COMMA
RBRACE
STRUCT IDENT("Response") LBRACE
  IDENT("summary") COLON IDENT("String") COMMA
RBRACE
FN IDENT("handle") LPAREN
  IDENT("req") COLON IDENT("Request")
RPAREN ARROW IDENT("Response") LBRACE
  LET IDENT("prompt") EQ IDENT("concat") LPAREN
    STRING("Summarize in one sentence: ") COMMA
    IDENT("req") DOT IDENT("text")
  RPAREN SEMI
  LET IDENT("reply") EQ IDENT("llm") COLONCOLON IDENT("call") LPAREN
    IDENT("prompt")
  RPAREN SEMI
  IDENT("log") COLONCOLON IDENT("info") LPAREN
    STRING("summarized")
  RPAREN SEMI
  IDENT("Response") LBRACE
    IDENT("summary") COLON IDENT("reply")
  RBRACE
RBRACE
```

Output is an AST of:

```text
Module {
  uses: [Path("host::llm"), Path("host::log")],
  items: [
    Struct { name: "Request", fields: [(text, String)] },
    Struct { name: "Response", fields: [(summary, String)] },
    Fn {
      name: "handle",
      params: [(req, Request)],
      ret: Response,
      body: Block {
        stmts: [
          Let { name: "prompt", expr: Call(concat, [...]) },
          Let { name: "reply", expr: PathCall(llm::call, [Var(prompt)]) },
          ExprStmt(PathCall(log::info, [Str("summarized")])),
        ],
        tail: StructLit { name: "Response", fields: [(summary, Var(reply))] },
      },
    },
  ],
}
```

**Decision surfaced:** the parser must handle `concat(a, b)` as a normal
function call. This implies `concat` is part of the stdlib surface and is
NOT a built-in operator. See open question on stdlib.

**Decision surfaced:** `req.text` is a field access expression; the
parser must support `.` chaining alongside `::` path resolution. Both are
postfix operators on expressions. Standard.

#### 3. Typecheck + lower to IR

Walk the AST top-down with an environment. Every `use` is resolved to a
host-module table (compiler-known). Every struct is registered in a type
table. Every fn is registered with its signature.

For each fn body:
- Each `let` introduces a binding into the local environment.
- Each expression infers a type; mismatches fail compilation.
- Method/path calls (`llm::call`, `log::info`) are resolved against the
  host-module table; their signatures are known and checked.

For the strawman, the type table is:

```text
String       (built-in, wasm representation: (i32 ptr, i32 len) pair)
Request      { text: String }
Response     { summary: String }

host::llm::call(String) -> String     (host fn, imports)
host::log::info(String) -> ()         (host fn, imports)
concat(String, String) -> String      (stdlib, will be inlined as host fn for v0.1)
```

Lowered IR (a simple flat list of typed operations, SSA-ish):

```text
fn handle(req: Request) -> Response {
  block 0:
    %0 = struct_field req .text          : String
    %1 = const "Summarize in one sentence: " : String
    %2 = call concat(%1, %0)             : String
    %3 = call host::llm::call(%2)        : String
    %4 = const "summarized"              : String
    %5 = call host::log::info(%4)        : ()
    %6 = struct_new Response { summary: %3 } : Response
    return %6
}
```

**Decision surfaced:** strings as `(ptr, len)` pairs is a big simplification.
The runtime owns string memory; cartridges allocate via a host function
`alloc(n: i32) -> i32`. Strings are immutable byte sequences. No UTF-8
validation at the cartridge boundary (host validates on input).

**Decision surfaced:** every string literal becomes a data segment in the
output wasm module, addressed by offset. No string interning yet (v0.1).

**Decision surfaced:** struct returns are by-value. The wasm calling
convention will pass the return address as an implicit first param
(C-style sret). Caller allocates space for the result.

#### 4. Codegen to wasm bytes

We emit a wasm binary directly — no `wasm-encoder` dep, no `cranelift`.
The wasm binary format is small: ~30 opcodes we'll actually use.

Output sections (typical):

```text
Magic + version: 0x00 0x61 0x73 0x6d 0x01 0x00 0x00 0x00

Type section: declare function signatures
  type 0: (i32, i32) -> i32        ; concat(ptr, ptr_to_request) -> ptr
  type 1: (i32) -> i32             ; llm::call(ptr) -> ptr
  type 2: (i32) -> ()              ; log::info(ptr)
  type 3: (i32) -> i32             ; handle(req_ptr) -> resp_ptr

Import section: pull in host functions
  import "host" "alloc"      type i32 -> i32
  import "host" "concat"     type 0  (stdlib lowered to host import in v0.1)
  import "host" "llm_call"   type 1
  import "host" "log_info"   type 2

Function section: declare local fns by type index
  func 0: type 3   ; handle

Memory section: 1 page initial, no max (host caps it)

Data section: string literals
  data 0 @ offset 0:  "Summarize in one sentence: "
  data 1 @ offset 27: "summarized"

Export section: expose handle to the host
  export "handle" -> func 0
  export "memory" -> memory 0

Code section: function bodies
  func 0 (handle):
    ; load req.text field (offset 0 in Request struct: ptr to string)
    local.get 0
    i32.load offset=0
    ; push static "Summarize in one sentence: " string pointer
    i32.const 0
    ; call concat(static_str_ptr, req.text_ptr)
    call $concat
    local.set 1            ; %prompt
    local.get 1
    call $llm_call
    local.set 2            ; %reply
    i32.const 27
    call $log_info
    ; allocate Response struct (4 bytes: just the summary pointer)
    i32.const 4
    call $alloc
    local.tee 3            ; resp_ptr
    local.get 2
    i32.store offset=0
    local.get 3
    return
```

**Decision surfaced:** for v0.1 we punt the entire stdlib (including
`concat`) to host imports. Cartridges call the host for every primitive
op above basic arithmetic. This makes the compiler tiny and the host
fat. Acceptable for v0.1; v0.2 can move some primitives into cartridge-
side wasm code.

**Decision surfaced:** memory layout — every struct is laid out as
pointer-sized fields in declaration order. String fields = (ptr, len)
pairs OR just `ptr` if length is stored at `*ptr - 4` (length-prefix
convention). We'll use pointer-with-length-prefix because it makes
struct layout trivially fixed-size: every field is i32.

**Open question added:** drop / cleanup. Who frees the memory the
cartridge allocates? For v0.1, the cleanest answer is: cartridge memory
is sandboxed and freed wholesale when the invocation returns. No
per-allocation free. Cost: cartridges can't run for long lived state.
Benefit: no manual memory management in rustlite, no Drop, no Rc.
Records this in the open-questions list.

#### 5. Instantiate

```text
let bytes = read_opfs("cartridges/summarize/v1.wasm");
let module = WebAssembly.compile(bytes);
let imports = {
  host: {
    alloc: |n| host_alloc(n),
    concat: |a, b| host_concat(a, b),
    llm_call: |p| host_llm_call(p),
    log_info: |p| host_log_info(p),
  },
};
let instance = WebAssembly.instantiate(module, imports);
let exports = instance.exports;
```

The wasm runs in the same process as the host (the browser tab). Host
functions are Rust closures bridged via wasm-bindgen, identical to how
the existing localharness builtins are bridged.

#### 6. Invoke

```text
let req_ptr = encode_request_to_wasm_memory(exports.memory, Request {
    text: "long form text here...".to_string(),
});
let resp_ptr = exports.handle(req_ptr);
let resp = decode_response_from_wasm_memory(exports.memory, resp_ptr);
```

The host owns the marshaling between Rust-side values and wasm-side
pointers. This is the entire reason cartridges are useful — the host
provides the safe shell, the cartridge provides the bespoke logic.

### What this strawman surfaced

New items added to the open-questions backlog above:

- **Stdlib in v0.1 = host imports.** `concat`, `format`, every string op,
  every collection op — all imports. Compiler stays tiny. Revisit in v0.2.
- **Memory model = arena per invocation.** No drop, no Rc, no manual
  free. Wholesale reset on return. Implies no long-lived in-cartridge
  state — that's what host `kv` is for. (Already in backlog as "Drop / RAII".)
- **`use host::xyz` is the only allowed `use`.** No multi-module cartridges
  in v0.1; everything is one file. No external dependencies (since there's
  no crate system inside rustlite yet).
- **The cartridge ABI is roughly fixed by this exercise:** a single exported
  `handle(req_ptr) -> resp_ptr` entry point, with request/response types
  agreed at compile time. Future versions may expand to multi-entry-point
  cartridges (tick + handle + render, etc.), but v0.1 is just `handle`.
- **The host is doing a lot of heavy lifting.** This is correct for v0.1.
  The simplicity of the compiler is bought by the richness of the host.

### What's NOT addressed yet

This strawman intentionally skipped:
- Error types — `Result<T, E>` not used. v0.1 may panic-trap on any error;
  v0.2 will need a real Result equivalent.
- Async — `llm::call` is shown as blocking, which means the wasm call
  blocks until the host's async future resolves. That requires the host
  to spin a microtask loop, OR rustlite needs an async syntax. **TBD.**
- Generics — none here. Will need `Vec<u8>` eventually; v0.1 punts by
  having the host expose monomorphic `byte_vec_new()`, `byte_vec_push()`,
  etc. Ugly but works.
- Pattern matching — no `match` in the strawman. The grammar item is
  still on the backlog.

---

## 2026-05-26 — Rustlite v0.1 grammar

**Why this matters now:** the previous iteration's strawman walked
*one* example through the pipeline. The grammar is the contract — the
exact set of programs the parser will accept. Pinning it down here
fixes the scope of everything downstream: parser size, typechecker
complexity, codegen surface, what agents can express. v0.1 is
deliberately brutal so the implementation stays small enough to ship
inside the localharness bundle.

### Design rule

If a Rust feature requires a borrow checker, lifetime annotations, a
trait resolver, monomorphization, or macro expansion to implement,
**it is not in v0.1.** That cuts most of Rust's complexity in one
swing. What remains is a small imperative-functional language with
sum types and structural pattern matching — basically OCaml/Reason
with curly braces and Rust spelling.

### EBNF

```ebnf
(* module *)
module          = use_decl* item* ;
use_decl        = "use" path ";" ;
path            = ident ("::" ident)* ;

(* items *)
item            = struct_decl | enum_decl | fn_decl | const_decl ;
struct_decl     = "struct" ident "{" field_list "}" ;
enum_decl       = "enum" ident "{" variant_list "}" ;
fn_decl         = "fn" ident "(" param_list? ")" ret_type? block ;
const_decl      = "const" ident ":" type "=" expr ";" ;

field_list      = (field ",")* field? ;
field           = ident ":" type ;

variant_list    = (variant ",")* variant? ;
variant         = ident                              (* unit:   None *)
                | ident "(" type_list ")"            (* tuple:  Some(T) *)
                | ident "{" field_list "}" ;         (* struct: Point { x, y } *)

param_list      = (param ",")* param ;
param           = ident ":" type ;
ret_type        = "->" type ;
type_list       = (type ",")* type ;

(* types — only what can be checked without inference, traits, or generics *)
type            = "i32" | "i64" | "f32" | "f64" | "bool"
                | "String"
                | ident                              (* user struct/enum *)
                | "(" type ("," type)+ ")" ;         (* tuple, arity >= 2 *)

(* statements *)
block           = "{" stmt* expr? "}" ;             (* tail expr = block value *)
stmt            = "let" "mut"? ident (":" type)? "=" expr ";"
                | place "=" expr ";"                 (* assignment *)
                | "return" expr? ";"
                | expr ";" ;

place           = ident ("." ident)* ;               (* lvalues: var or var.field.field *)

(* expressions, in increasing precedence *)
expr            = expr_or ;
expr_or         = expr_and ("||" expr_and)* ;
expr_and        = expr_cmp ("&&" expr_cmp)* ;
expr_cmp        = expr_sum (cmp_op expr_sum)? ;
expr_sum        = expr_term (("+" | "-") expr_term)* ;
expr_term       = expr_unary (("*" | "/" | "%") expr_unary)* ;
expr_unary      = ("-" | "!")* expr_postfix ;
expr_postfix    = expr_atom postfix* ;
postfix         = "." ident                          (* field access *)
                | "." ident "(" arg_list? ")"        (* method call — desugars *)
                | "(" arg_list? ")" ;                (* call *)
expr_atom       = literal
                | path                               (* variable / fn name / variant *)
                | path "{" field_init_list "}"       (* struct or struct-variant literal *)
                | "if" expr block ("else" (block | if_expr))?
                | "match" expr "{" match_arm+ "}"
                | "while" expr block
                | "loop" block
                | "break" expr?
                | "continue"
                | "(" expr ")"
                | "(" expr ("," expr)+ ")" ;         (* tuple literal *)

if_expr         = "if" expr block ("else" (block | if_expr))? ;
cmp_op          = "==" | "!=" | "<" | ">" | "<=" | ">=" ;

arg_list        = (expr ",")* expr ;
field_init_list = (field_init ",")* field_init? ;
field_init      = ident ":" expr | ident ;           (* shorthand: `summary` == `summary: summary` *)

(* patterns *)
match_arm       = pattern "=>" (expr "," | block) ;
pattern         = literal
                | "_"
                | ident                              (* binding or unit-variant match *)
                | path                               (* variant constructor with no payload *)
                | path "(" pattern_list ")"          (* tuple variant *)
                | path "{" field_pat_list "}" ;      (* struct variant *)
pattern_list    = (pattern ",")* pattern ;
field_pat_list  = (field_pat ",")* field_pat? ;
field_pat       = ident                              (* shorthand: bind to same name *)
                | ident ":" pattern ;

(* tokens *)
literal         = INT | FLOAT | STRING | "true" | "false" ;
ident           = /[a-zA-Z_][a-zA-Z0-9_]*/ ;
INT             = /[0-9]+/ ("i32" | "i64")? ;        (* default: i32 *)
FLOAT           = /[0-9]+\.[0-9]+/ ("f32" | "f64")? ; (* default: f64 *)
STRING          = /"([^"\\]|\\.)*"/ ;                (* C-style escapes *)

(* trivia *)
line_comment    = "//" .* end_of_line ;
(* no block comments, no doc comments in v0.1 *)
```

That's about 100 lines of grammar. The parser implementing it should
land under 1500 lines of Rust.

### What's in (and worth noting)

- **Structs and enums with three variant shapes.** Unit, tuple, struct.
  This is the entire sum-type story; agents will use enums everywhere
  (Result/Option ship as stdlib enums, see stdlib question).
- **`let mut` exists, but no `&mut`.** Local variables can be
  reassigned. Place expressions (`x`, `x.field`) can be assigned to.
  No references means no aliasing problem, so we get mutation for free.
- **Method syntax via desugaring, not `impl`.** `x.foo(a)` is exactly
  equivalent to `Bar::foo(x, a)` where `Bar` is `x`'s static type. No
  trait dispatch, no method tables, no inherent impls. The typechecker
  resolves the call site, codegen emits a direct call. Agents get to
  write `req.text.len()` shape code without us building method dispatch.
- **Pattern matching is the only conditional-on-shape construct.**
  No `if let`, no `while let`. Just `match`. Keeps the parser small.
  Match arms return values (last block expr or post-`=>` expr); the
  match itself is an expression.
- **Tail expressions in blocks.** Both `fn` bodies and `if`/`match`/
  `loop` arms can omit `return` by leaving the value as the tail expr.
  This is the one syntactic non-triviality kept from Rust; it pays
  for itself by making expression-oriented agent code idiomatic.
- **Field-init shorthand.** `Point { x, y }` works the same as Rust.
  Big quality-of-life win for almost no parser cost.

### What's deliberately out — and why

Each of these has a one-line rationale; they're not arbitrary.

- **No references (`&T`, `&mut T`).** Arena-per-invocation makes
  references unnecessary and removes borrow-checking entirely. v0.2
  may add `&` for cartridges that need shared in-memory state.
- **No lifetimes (`'a`).** Without references, lifetimes have nothing
  to annotate.
- **No traits, no `impl` blocks.** Trait dispatch needs vtables OR
  monomorphization; both are big implementation projects. Method
  desugaring delivers 80% of the ergonomic value.
- **No generics.** Monomorphization is the single largest piece of a
  Rust compiler. Punt to v0.2; for now, agents call monomorphic
  host functions like `byte_vec_push` instead of `Vec::<u8>::push`.
- **No closures.** Closure capture analysis + heap-promotion is a
  whole subsystem. Agents pass plain function pointers when they
  need to (host functions accept `fn(i32) -> i32` style callbacks).
- **No `unsafe`.** Cartridges live in a wasm sandbox; there's no
  "below safe Rust" to escape into. The keyword would be theatre.
- **No `async`/`await`.** Host functions block from the cartridge's
  point of view; the host runs the async runtime. This means
  cartridge code is simpler but cartridge invocations can be long.
  Acceptable for v0.1; revisit when long-running cartridges hurt.
- **No macros (`format!`, `println!`, `vec![]`).** Each replaced by a
  host function call: `format(template_str, args)` (variadic at host
  side), `vec_new()`. Ugly but tiny. v0.2 may add a fixed set of
  builtin macros expanded in a pre-parse pass.
- **No modules / `mod`.** One file per cartridge, all items in the
  top-level scope. `use host::xyz` is the only `use` allowed, and it
  resolves against the compiler-known host module table.
- **No `pub` / visibility.** Everything top-level is exported as a
  potential cartridge entry point (though by default the host only
  invokes `handle`). All fields of all structs are public.
- **No attribute macros, no derives.** `Eq`, `Clone`, `Debug` are
  auto-generated by codegen for every struct; agents never write
  `#[derive(...)]`. (For v0.1: `Clone` = bitwise copy, since arena
  memory means no fancy ownership; `Eq` = field-wise compare; `Debug`
  = a host function `debug_fmt(any) -> String` instead.)
- **No `as` casts.** Numeric conversion is via explicit functions:
  `i32_to_i64(x)`, `f32_to_i32_trunc(x)`. Forces the agent to think
  about precision loss.
- **No range syntax `..`.** Use `for_each(start, end, |i| ...)` once
  closures exist; for now use a `while` with explicit counter.
- **No `if let` / `while let`.** `match` covers it.
- **No tuple structs (`struct Foo(i32)`).** Use a named-field struct
  with one field, or a plain tuple. Removes a parsing case.
- **No `where` clauses.** Nothing to constrain without generics.
- **No turbofish (`::<T>`).** Same.
- **No `?` operator.** When `Result` arrives as a stdlib enum,
  pattern-match it. The `?` desugar wants traits (`From`) to work
  cleanly.
- **No raw strings, no string interpolation.** Plain `"..."` with
  `\n`/`\t`/`\\`/`\"` escapes. Build multi-line strings with `concat`.

### Why this is enough

The first realistic agent cartridge the strawman sketched
(`summarize.rl`) needs: `use`, `struct`, `fn`, `let`, function calls,
field access, string literals, return-via-tail-expression. All in.

Cartridges that build on it — pagination, error handling, structured
LLM responses, conditional logic, list processing — need: `enum`,
`match`, `if`/`else`, `while`, mutable locals, tuples. All in.

What we lose: bespoke types like a custom hashmap, ergonomic
iteration, generic helpers. All deferred to host imports or v0.2.

### Things this surfaced for the backlog

- **`String` literal vs `String` value.** In Rust, `"foo"` is `&str`,
  not `String`. We can't keep that distinction without references.
  Decision: `"foo"` is `String` directly. The compiler emits code
  that copies the data segment bytes into arena memory at the use
  site, so every `String` is owned. Cheap, simple, slightly wasteful.
- **Where does `String::len`, `String::push_str`, etc. live?** Since
  methods desugar to free functions, we need a `String` "module" of
  free functions: `String::len(s)`, `String::push_str(s, t)`, etc.
  These are host imports in v0.1. Goes in the host-module table next
  to `host::llm`, `host::log`.
- **What does `concat("a", "b")` resolve to?** Open whether it's a
  bare top-level fn or `String::concat`. Leaning `String::concat(a, b)`
  with `concat(a, b)` as a global alias for ergonomics. **TBD,
  resolves with stdlib question.**
- **What about `i32::MAX`?** Path expressions can resolve to constants
  on primitive types. The compiler ships these as built-in constants;
  agents write `i32::MAX` as syntactic sugar for a literal. Cost:
  zero. Benefit: idiomatic.
- **Char type.** Not in v0.1. `String` is the only character-shaped
  type. Agents index strings with byte offsets; UTF-8 handling is the
  host's problem.
- **Float NaN / Infinity literals.** `f64::NAN`, `f64::INFINITY` as
  built-in constants, like `i32::MAX`.

These get added to the backlog at the top of the doc.

---

## 2026-05-26 — Cartridge ABI v0.1

**Why this third:** the strawman established that the host carries the
stdlib; the grammar established that `use host::X` is the only `use`
allowed. Both punted to "the cartridge ABI." This iteration nails it:
exactly which modules exist, exactly what types and functions they
expose, exactly what the wasm-level import signatures look like, and
how a cartridge declares which capabilities it needs.

The ABI is the contract every cartridge sees. Making it explicit means
the compiler's symbol table is just a static encoding of this document.

### Design rules

1. **Monomorphic everything.** No `Result<T, E>` at the ABI boundary;
   instead each module declares its own concrete error enum (e.g.
   `host::llm::LlmResult`). This is ugly but matches the no-generics
   grammar constraint and keeps the compiler's resolution rules
   trivial.
2. **One i32 per cross-boundary value.** All non-primitive values
   (strings, structs, byte arrays) cross the wasm boundary as a
   single i32 pointer into the cartridge's linear memory. The host
   reads/writes through that pointer using known layout rules.
3. **Length-prefixed strings.** A `String` at the ABI is `i32 ptr`
   where the 4 bytes at `*ptr` are the length, and bytes at
   `*ptr + 4` are the UTF-8 payload. No null terminator. Host
   validates UTF-8 on read; cartridge code must produce valid UTF-8.
4. **Manifest-gated capabilities.** A cartridge declares the host
   modules it imports in a manifest custom section. The host
   provides ONLY the declared imports at instantiate time. Trying
   to call a non-declared host fn = wasm validation error =
   instantiation fails. Capability-based, not ambient.
5. **No callback-into-cartridge ABI.** Host fns are pure import +
   return. No fn pointers passed from cartridge to host. This means
   no event listeners, no streaming responses to cartridge, no
   "iterator" style protocols where the host pumps the cartridge.
   When v0.1 needs streaming, the cartridge polls.

### Host-module catalog

Nine modules in v0.1. Five of them are mandatory (every cartridge
gets them); four are opt-in through the manifest.

| Module          | Capability   | Notes                              |
|-----------------|--------------|------------------------------------|
| `host::log`     | ambient      | always available                   |
| `host::time`    | ambient      | always available                   |
| `host::str`     | ambient      | string ops (no I/O)                |
| `host::random`  | ambient      | crypto-secure RNG                  |
| `host::abort`   | ambient      | trap / panic / fuel introspection  |
| `host::kv`      | declared     | per-cartridge durable KV in OPFS   |
| `host::fs`      | declared     | cartridge-sandboxed OPFS subtree   |
| `host::llm`     | declared     | LLM backend (Gemini etc.)          |
| `host::http`    | declared     | gated by origin allowlist          |
| `host::chain`   | declared     | Tempo Moderato RPC + sponsored tx  |

### Module specs

Each module is given in two forms: the rustlite signature the agent
writes, and the wasm-level import signature the compiler emits.

#### `host::log`  (ambient)

```rust
// rustlite signatures
fn info(msg: String);
fn warn(msg: String);
fn error(msg: String);
fn debug(msg: String);
```

```wat
;; wasm imports — single i32 = string pointer
(import "host_log" "info"  (func (param i32)))
(import "host_log" "warn"  (func (param i32)))
(import "host_log" "error" (func (param i32)))
(import "host_log" "debug" (func (param i32)))
```

Wasm module names use `host_log` not `host::log` because wasm import
names can't contain colons in practice (the compiler chooses the
mangling; this is the simplest).

#### `host::time`  (ambient)

```rust
fn now_unix_ms() -> i64;
fn monotonic_ms() -> i64;
```

```wat
(import "host_time" "now_unix_ms"   (func (result i64)))
(import "host_time" "monotonic_ms"  (func (result i64)))
```

#### `host::str`  (ambient)

The string standard library. Lives in the host so the compiler
doesn't need to emit string-manipulation wasm.

```rust
fn len(s: String) -> i32;
fn concat(a: String, b: String) -> String;
fn slice(s: String, start: i32, end: i32) -> String;
fn contains(s: String, needle: String) -> bool;
fn starts_with(s: String, prefix: String) -> bool;
fn ends_with(s: String, suffix: String) -> bool;
fn replace(s: String, from: String, to: String) -> String;
fn split(s: String, sep: String) -> StringIter;       // see iterator pattern
fn trim(s: String) -> String;
fn to_lower(s: String) -> String;
fn to_upper(s: String) -> String;
fn parse_i32(s: String) -> ParseI32Result;
fn parse_i64(s: String) -> ParseI64Result;
fn parse_f64(s: String) -> ParseF64Result;
fn i32_to_string(n: i32) -> String;
fn i64_to_string(n: i64) -> String;
fn f64_to_string(n: f64) -> String;

// Monomorphic result enums
enum ParseI32Result { Ok { value: i32 }, Err }
enum ParseI64Result { Ok { value: i64 }, Err }
enum ParseF64Result { Ok { value: f64 }, Err }

// Iterator: opaque handle, polled via `next`
struct StringIter { handle: i32 }
fn next(it: StringIter) -> StringIterNext;
enum StringIterNext { Item { value: String }, Done }
```

Wasm-level: every fn takes/returns i32 (string ptr) or primitive.
Result/iterator enums marshal as a tag byte + payload pointer.

#### `host::random`  (ambient)

```rust
fn u32() -> i32;       // i32 because rustlite has no u32; reinterpret bits
fn i64() -> i64;
fn bytes(n: i32) -> Bytes;

struct Bytes { handle: i32 }   // opaque byte-vector handle
fn bytes_at(b: Bytes, i: i32) -> i32;
fn bytes_len(b: Bytes) -> i32;
```

Backed by `crypto.getRandomValues` in the browser. Not deterministic;
test-mode determinism is a v0.2 question.

#### `host::abort`  (ambient)

```rust
fn panic(msg: String) -> !;            // wasm trap, cartridge dies
fn fuel_remaining() -> i64;            // current fuel budget
fn memory_bytes() -> i32;              // bytes currently allocated in arena
```

`panic` returns `!` (never type) — the typechecker treats subsequent
code as unreachable. Wasm-level: emits `unreachable` after the call.

#### `host::kv`  (declared)

Per-cartridge namespaced KV. Persists across invocations in the
cartridge's OPFS subtree. Not shared across cartridges (a different
cartridge writing to the same key sees a different namespace).

```rust
fn get(key: String) -> KvGetResult;
fn set(key: String, value: String);
fn remove(key: String);
fn list_keys(prefix: String) -> StringIter;

enum KvGetResult { Found { value: String }, NotFound }
```

Maximum value size: 64 KiB (TBD; arbitrary cap). Maximum key length:
256 bytes. Beyond limits = panic.

#### `host::fs`  (declared)

Cartridge-sandboxed OPFS subtree. Path `"foo.txt"` resolves to
`/cartridges/<cart-id>/data/foo.txt` in OPFS. Paths cannot escape
the subtree (`..` is rejected).

```rust
fn read_text(path: String) -> FsReadResult;
fn write_text(path: String, contents: String) -> FsWriteResult;
fn read_bytes(path: String) -> FsReadBytesResult;
fn write_bytes(path: String, contents: Bytes) -> FsWriteResult;
fn exists(path: String) -> bool;
fn remove(path: String) -> FsWriteResult;
fn list(path: String) -> StringIter;     // dir entries

enum FsReadResult       { Ok { text: String },  Err { code: i32 } }
enum FsReadBytesResult  { Ok { bytes: Bytes },   Err { code: i32 } }
enum FsWriteResult      { Ok,                    Err { code: i32 } }
```

Error codes: 1=not found, 2=permission denied, 3=quota exceeded,
4=invalid path, 99=other. Documented in host comments; no enum
variant per error to keep the surface small.

#### `host::llm`  (declared)

```rust
fn call(prompt: String) -> LlmResult;
fn call_system(system: String, user: String) -> LlmResult;
fn call_with_tools(prompt: String, tools_json: String) -> LlmResult;

enum LlmResult {
    Ok { text: String },
    Err { code: i32, message: String },
}
```

Tools are passed as a JSON string the cartridge has serialized — the
ABI does not understand tool schemas natively. The returned text may
include tool-call markers the cartridge must parse. This is uglier
than a structured tool API but avoids hardcoding tool-call wire
format into the ABI; the host can swap LLM backends without ABI
churn.

Backed by whichever `Connection` the host has installed (Gemini in
v0.1; Anthropic later).

#### `host::http`  (declared)

Gated by manifest-declared origin allowlist. A cartridge declares
`origins: ["https://api.example.com"]` and calls to other origins
return `Err { code: 99, message: "origin not in allowlist" }`.

```rust
fn get(url: String, headers_json: String) -> HttpResult;
fn post(url: String, body: String, headers_json: String) -> HttpResult;
fn put(url: String, body: String, headers_json: String) -> HttpResult;
fn delete(url: String, headers_json: String) -> HttpResult;

enum HttpResult {
    Ok { status: i32, body: String, headers_json: String },
    Err { code: i32, message: String },
}
```

Headers passed as a JSON string of key→value because rustlite has no
generic map type. Body is `String` for v0.1 (no binary upload);
v0.2 adds a `bytes`-variant overload.

#### `host::chain`  (declared)

Talks to the Tempo Moderato testnet through the bundle's existing
`registry` module. All write paths go through the sponsor (see
`src/app/sponsor.rs`); the cartridge cannot specify a fee_payer.

```rust
fn read(addr: String, calldata_hex: String) -> ChainReadResult;
fn write(addr: String, calldata_hex: String) -> ChainWriteResult;
fn block_number() -> i64;
fn my_tba() -> String;             // the cartridge's owning agent's TBA addr
fn my_owner() -> String;            // the agent's wallet addr (MAIN holder)

enum ChainReadResult  { Ok { return_data_hex: String },  Err { message: String } }
enum ChainWriteResult { Ok { tx_hash: String },           Err { message: String } }
```

`write` blocks until the sponsored Tempo tx confirms (or fails).
Calldata is hex-encoded — the cartridge is responsible for ABI
encoding (the host doesn't ship a Solidity encoder for cartridges
in v0.1; v0.2 may add `host::chain::encode_call(sig, args_json)`).

### Cross-boundary wire format

Every i32 passed across the ABI is a pointer into the cartridge's
wasm linear memory. The host reads/writes per the layout rules below.

#### Strings

```text
   ptr ─→ [ len: i32 ][ b0 b1 b2 ... b(len-1) ]
```

UTF-8 payload. Host validates on read from cartridge → host. Host
writes valid UTF-8 on cartridge ← host. Length is bytes, not chars.

#### Structs and struct enums

Laid out as i32-aligned fields in declaration order. Each field
occupies 4 bytes (or 8 for i64/f64), in declaration order:

```text
struct Response { summary: String }   →  4 bytes: ptr to String

enum LlmResult {
    Ok { text: String },        ;; tag = 0
    Err { code: i32, message: String }, ;; tag = 1
}                                  →  4-byte tag + max(payload_a, payload_b)
                                  →  for Ok: tag(0) + ptr_to_text       (8 bytes)
                                  →  for Err: tag(1) + code + ptr_to_msg (12 bytes)
                                  →  overall enum slot = max = 12 bytes
```

Padding aligned to 4. Unused tail bytes in shorter variants are
zero-filled.

#### Tuples

Same as structs but fields are unnamed and indexed.

#### Opaque handles (`Bytes`, `StringIter`)

A single i32 field holding the host-side resource ID. The cartridge
treats it as opaque; the host translates back to a real resource on
each call. Resources are scoped to the invocation — when `handle`
returns, all handles created during it are dropped server-side.

#### Primitives

`bool` = i32, 0 or 1. `i32`, `i64`, `f32`, `f64` = native wasm types
in registers, no pointer indirection.

### Manifest

A cartridge declares its needed capabilities (and any other metadata)
in a JSON manifest written to OPFS at
`cartridges/<name>/manifest.json`:

```json
{
  "name": "summarize",
  "version": "0.1.0",
  "entry": "handle",
  "request_type": "Request",
  "response_type": "Response",
  "capabilities": {
    "llm": true,
    "kv": false,
    "fs": false,
    "http": { "origins": [] },
    "chain": false
  },
  "fuel_limit": 1000000,
  "memory_pages_max": 16
}
```

The cartridge build step (`cartridge::build(source_path)`) parses
the source for `use host::X` statements and writes the manifest
automatically. The agent doesn't write the manifest by hand;
declared capabilities are derived from imports.

Loading: when the host instantiates the wasm, it reads the manifest,
constructs the import object containing ONLY the declared modules,
and instantiates. Any `use host::X` the source doesn't actually
import but doesn't appear in the manifest → cartridge build fails.

### What this surfaced for the backlog

- **Manifest schema versioning.** The above JSON shape is v1. Need
  a `"manifest_version": 1` field at the top so we can evolve later.
  Added to backlog.
- **`Bytes` value lifetime.** Bytes handles are scoped to one
  invocation. If a cartridge stores a `Bytes` in KV between
  invocations, what happens? Probably the handle is invalid;
  serializing Bytes for KV needs an explicit `bytes_to_string`
  (base64) round-trip. Added to backlog.
- **Streaming.** `llm::call` is one-shot, no streaming token output
  to the cartridge. The host can stream to the UI (existing
  behavior) but the cartridge sees only the final string. Acceptable
  for v0.1; v0.2 may add a polling iterator.
- **No tool-calling from cartridges in v0.1.** The `call_with_tools`
  signature exists but the JSON convention is unspecified. Marking
  as "experimental, format TBD" in the actual ABI doc.
- **Fuel budget origin.** The manifest declares `fuel_limit` but who
  authorizes more than the default? An agent-controlled setting?
  An on-chain limit tied to credits? Open. Tied to existing
  "Memory + fuel limits" backlog item.

### Why this is enough

A cartridge can: log, read time, manipulate strings, generate
random numbers, abort. (Ambient.) It can also, if declared:
persist KV, read/write its OPFS subtree, call the LLM, make
allowlisted HTTP requests, and interact with the Tempo chain via
its owning agent's TBA. That covers ~all of what the existing
13 builtins do, with the difference that cartridges can compose
the primitives into arbitrary new tools.

What we lose vs. a "full" ABI: tool-call structure, streaming,
cartridge-to-cartridge calls, callbacks. All deferred. Composition
between cartridges is the next-next item on the backlog and is
itself a substantial design surface.

---

## Iteration log

- **2026-05-26** — End-to-end strawman walked through.
  Result: cartridge ABI shape decided (single `handle` entry point, host-imported
  stdlib, arena memory per invocation, length-prefixed strings as i32 ptrs).
  Next: grammar EBNF — pin down what rustlite v0.1 source actually looks like.

- **2026-05-26** — Rustlite v0.1 grammar pinned.
  Result: ~100 lines of EBNF; structs/enums/fns/match/if/while/loop, no traits/
  generics/closures/macros/refs/lifetimes/unsafe/async/`?`/derives. Method syntax
  via free-fn desugar. Parser estimated <1500 LOC.
  Next: cartridge ABI — exact host-function table the compiler resolves `use host::*` against.

- **2026-05-26** — Cartridge ABI v0.1 nailed.
  Result: 9 modules (5 ambient + 4 declared), monomorphic types per module,
  capability-gated via OPFS manifest derived from `use` statements, length-
  prefixed UTF-8 strings, opaque per-invocation handles for Bytes/Iter.
  Next: stdlib question — formally collapse it ("v0.1 stdlib = host modules"
  already implicit; document explicitly) OR compiler-location.

