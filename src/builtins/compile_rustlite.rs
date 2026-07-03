//! `compile_rustlite` — compile and run Rust-subset source code.
//!
//! The agent writes rustlite source, the tool compiles it to wasm
//! via `rustlite::compile`, instantiates it via the cartridge loader,
//! and calls the named function (default: `handle`). Returns the
//! result or compilation/runtime errors.

use std::sync::Arc;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::Result;
use crate::tools::{Tool, ToolContext};

pub struct CompileRustlite;

crate::tool_params! {
    /// ONE table generates both this struct and `input_schema` (see
    /// `crate::tool_params`); the schema byte-identity test is below.
    /// Lenient mode — the historical body defaulted missing/wrong-typed args
    /// (`""` source, `"handle"` function, empty `args`).
    struct Args: lenient {
        source: req_str = "Rustlite source code to compile",
        function: opt_str = "Function name to call after compilation (default: 'handle')",
        args: opt_i64_array = "i32 arguments to pass to the function",
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for CompileRustlite {
    fn name(&self) -> &str {
        "compile_rustlite"
    }

    fn description(&self) -> &str {
        "Compile-check rustlite (Rust-subset) source to wasm WITHOUT touching the \
         display — your compile-in-the-loop tool. Use it after each addition while \
         building a cartridge: it returns either a clean compile (then run_cartridge \
         / create_and_publish_app) or, on failure, `{ error: \"compilation failed\", \
         code: \"LHxxxx\", detail, location: \"line N, col M\", snippet: \"<the \
         offending source line with a ^ caret under the error>\", hint }` — READ \
         the location + snippet, fix that exact spot, and recompile before adding \
         more. rustlite supports structs, enums, fns, match (incl. ranges), \
         if/else, while/for/loop, arrays (read), const, recursion — but NO traits, \
         generics, references, heap types (Vec/String building/Box), or array \
         writes. If `function` is given it is also called and its i32 result \
         returned; for a plain compile-check just pass `source`."
    }

    fn input_schema(&self) -> Value {
        Args::schema()
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let p = Args::lenient(&args);
        let source = p.source;
        let function = p.function.as_deref().unwrap_or("handle").to_string();
        // Historical `filter_map(as_i64 → as i32)` — the table's `opt_i64_array`
        // is the same filter_map; only the i32 narrowing stays here.
        let fn_args: Vec<i32> = p
            .args
            .map(|v| v.into_iter().map(|n| n as i32).collect())
            .unwrap_or_default();

        if source.is_empty() {
            return Ok(json!({ "error": "source is required" }));
        }

        // Step 1: Compile
        let wasm_bytes = match crate::rustlite::compile(&source) {
            Ok(bytes) => bytes,
            Err(err) => {
                // The shared structured report (`builtins::compile_failure_report`):
                // stable LH0xxx code, the compiler message verbatim, a
                // `line N, col M` locator, the offending source line with a
                // caret marker, and the per-code fix hint — so the model
                // fixes-and-recompiles in the loop instead of giving up or,
                // worse, publishing the broken source anyway.
                let mut report = crate::builtins::compile_failure_report(&err, &source);
                report["exports"] = json!([]);
                return Ok(report);
            }
        };

        // Step 2: Load and run
        #[cfg(target_arch = "wasm32")]
        {
            use crate::rustlite::loader::Cartridge;

            let cartridge = match Cartridge::load(&wasm_bytes).await {
                Ok(c) => c,
                Err(err) => {
                    return Ok(json!({
                        "error": "load failed",
                        "detail": err.to_string(),
                        "wasm_size": wasm_bytes.len()
                    }));
                }
            };

            let exports = cartridge.exports();

            match cartridge.call_i32(&function, &fn_args) {
                Ok(result) => Ok(json!({
                    "result": result,
                    "function": function,
                    "exports": exports,
                    "wasm_size": wasm_bytes.len()
                })),
                Err(err) => Ok(json!({
                    "error": "execution failed",
                    "detail": err.to_string(),
                    "exports": exports,
                    "wasm_size": wasm_bytes.len()
                })),
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (&function, &fn_args);
            Ok(json!({
                "compiled": true,
                "wasm_size": wasm_bytes.len(),
                "note": "execution requires a browser — compiled successfully but cannot instantiate on native"
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BYTE-IDENTITY: the generated schema serializes byte-for-byte equal to
    /// the original hand-written literal it replaced (frozen verbatim below).
    #[test]
    fn schema_is_byte_identical_to_the_frozen_original() {
        let frozen = json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "Rustlite source code to compile"
                },
                "function": {
                    "type": "string",
                    "description": "Function name to call after compilation (default: 'handle')"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "description": "i32 arguments to pass to the function"
                }
            },
            "required": ["source"]
        });
        assert_eq!(Args::schema().to_string(), frozen.to_string());
    }

    /// Lenient parity: the table feeds the body the same values the old
    /// inline `.get().and_then().unwrap_or()` chains produced.
    #[test]
    fn lenient_matches_the_old_inline_extraction() {
        let p = Args::lenient(&serde_json::json!({}));
        assert_eq!((p.source.as_str(), p.function, p.args), ("", None, None));
        let p = Args::lenient(&serde_json::json!({
            "source": "fn handle() -> i32 { 1 }",
            "function": 7,                 // wrong type → the "handle" default
            "args": [1, -2, "3", 4.5],     // non-i64 entries drop, sign kept
        }));
        assert_eq!(p.function.as_deref().unwrap_or("handle"), "handle");
        let fn_args: Vec<i32> = p
            .args
            .map(|v| v.into_iter().map(|n| n as i32).collect())
            .unwrap_or_default();
        assert_eq!(fn_args, vec![1, -2]);
    }
}
