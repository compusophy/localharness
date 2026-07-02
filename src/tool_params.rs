//! Single-table tool parameters: ONE `tool_params!` declaration generates BOTH
//! the typed args struct AND the wire `input_schema` JSON, so schema↔parse
//! drift is impossible by construction — a field cannot exist in the schema
//! without existing in the struct, and vice versa.
//!
//! Why this shape (and not schemars / a proc-macro derive):
//! * **Zero new deps.** `macro_rules!` only — no companion proc-macro crate,
//!   no syn/quote, no schemars tree. wasm-clean (pure `serde_json`).
//! * **Gemini-safe by construction.** The macro grammar can only emit a single
//!   `type` string per property and can never emit `oneOf`/`anyOf`/`allOf`/
//!   `additionalProperties`/`$ref` — the exact shapes that 400 on Gemini and
//!   brick all chat (see `src/builtins/CLAUDE.md`). A hand-written `json!`
//!   schema has no such guarantee.
//! * **Byte-identical schemas.** `serde_json` (no `preserve_order`) serializes
//!   maps key-sorted, so a structurally-equal generated schema serializes
//!   byte-identically to the hand-written literal it replaces. Every migrated
//!   tool keeps a FROZEN copy of its original literal in a test asserting
//!   `to_string()` equality.
//! * **Two parse modes, no silent behavior change.** `: serde` emits a
//!   `#[derive(serde::Deserialize)]` struct (what the builtins already use —
//!   required fields error on missing, `Option` fields default to `None`);
//!   `: lenient` emits a plain struct + `fn lenient(&Value) -> Self`
//!   reproducing the browser chat tools' historical
//!   `.get(..).and_then(..).unwrap_or(default)` semantics exactly. A migration
//!   picks the mode matching the tool's CURRENT behavior.
//! * **Native-testable schemas for wasm-gated tools.** `src/app/chat/tools/`
//!   is compiled only under `browser-app`+wasm32 — outside every default
//!   check, which is where schema bricks historically hid. Migrated chat
//!   tools declare their table HERE (the `turn_flow` hoisting pattern), so
//!   plain `cargo test` byte-checks their wire schema for the first time.
//!
//! Migration is OPT-IN per tool. Tools whose schemas need shapes the grammar
//! doesn't cover (arrays of objects, enums, maximums) stay hand-written until
//! a kind is added — the escape hatch is "don't migrate yet", never "bend the
//! table".
//!
//! Field kinds: `req_str`, `opt_str`, `req_u32`, `opt_u32`, `opt_bool`,
//! each optionally followed by `min N` (JSON-Schema `minimum`). `req_*` kinds
//! land in the schema's `required` array in declaration order.
//!
//! ```rust
//! localharness::tool_params! {
//!     /// Args for a file-reading tool.
//!     struct Args: serde {
//!         path: req_str = "Absolute or relative file path.",
//!         start_line: opt_u32 min 1 = "1-indexed first line to return.",
//!     }
//! }
//! let schema = Args::schema(); // {"type":"object","properties":{...},"required":["path"]}
//! ```

/// Generate a typed tool-args struct + its wire `input_schema` from ONE table.
///
/// See the [module docs](crate::tool_params) for the grammar, the two parse
/// modes (`: serde` / `: lenient`), and the byte-identity migration rule.
/// Consumers need `serde` (with `derive`) and `serde_json` for `: serde` mode.
#[macro_export]
macro_rules! tool_params {
    // ---------- internal: Rust type per field kind ----------
    (@ty req_str) => { ::std::string::String };
    (@ty opt_str) => { ::core::option::Option<::std::string::String> };
    (@ty req_u32) => { u32 };
    (@ty opt_u32) => { ::core::option::Option<u32> };
    (@ty opt_bool) => { ::core::option::Option<bool> };
    // ---------- internal: JSON-Schema "type" per kind ----------
    (@json_ty req_str) => { "string" };
    (@json_ty opt_str) => { "string" };
    (@json_ty req_u32) => { "integer" };
    (@json_ty opt_u32) => { "integer" };
    (@json_ty opt_bool) => { "boolean" };
    // ---------- internal: required flag per kind ----------
    (@required req_str) => { true };
    (@required req_u32) => { true };
    (@required opt_str) => { false };
    (@required opt_u32) => { false };
    (@required opt_bool) => { false };
    // ---------- internal: lenient extraction per kind (the historical
    // `.get().and_then().unwrap_or()` chat-tool semantics, verbatim) ----------
    (@lenient req_str, $args:expr, $name:expr) => {
        $args.get($name).and_then(|v| v.as_str()).unwrap_or("").to_string()
    };
    (@lenient opt_str, $args:expr, $name:expr) => {
        $args.get($name).and_then(|v| v.as_str()).map(|s| s.to_string())
    };
    (@lenient req_u32, $args:expr, $name:expr) => {
        $args
            .get($name)
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0)
    };
    (@lenient opt_u32, $args:expr, $name:expr) => {
        $args
            .get($name)
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok())
    };
    (@lenient opt_bool, $args:expr, $name:expr) => {
        $args.get($name).and_then(|v| v.as_bool())
    };
    // ---------- internal: the shared `schema()` body ----------
    (@schema $vis:vis, $( $field:ident : $kind:ident $(min $min:literal)? = $desc:literal ),+) => {
        /// Wire `input_schema`, generated from the SAME table as the struct
        /// fields. Single `type` per property, no unions / `oneOf` /
        /// `additionalProperties` — Gemini-safe by construction.
        $vis fn schema() -> $crate::__private::serde_json::Value {
            use $crate::__private::serde_json::{Map, Value};
            let mut props = Map::new();
            $(
                let mut f = Map::new();
                f.insert(
                    "type".to_string(),
                    Value::String($crate::tool_params!(@json_ty $kind).to_string()),
                );
                $( f.insert("minimum".to_string(), Value::from($min)); )?
                f.insert("description".to_string(), Value::String($desc.to_string()));
                props.insert(stringify!($field).to_string(), Value::Object(f));
            )+
            let flags: &[(&str, bool)] =
                &[ $( (stringify!($field), $crate::tool_params!(@required $kind)) ),+ ];
            let required: ::std::vec::Vec<Value> = flags
                .iter()
                .filter(|(_, req)| *req)
                .map(|(name, _)| Value::String((*name).to_string()))
                .collect();
            let mut root = Map::new();
            root.insert("type".to_string(), Value::String("object".to_string()));
            root.insert("properties".to_string(), Value::Object(props));
            if !required.is_empty() {
                root.insert("required".to_string(), Value::Array(required));
            }
            Value::Object(root)
        }
    };
    // ---------- `: serde` mode (the builtins' existing parse semantics) ----------
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident : serde {
            $( $field:ident : $kind:ident $(min $min:literal)? = $desc:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(::serde::Deserialize)]
        $vis struct $name {
            $( #[doc = $desc] $vis $field: $crate::tool_params!(@ty $kind), )+
        }
        impl $name {
            $crate::tool_params!(@schema $vis, $( $field : $kind $(min $min)? = $desc ),+);
        }
    };
    // ---------- `: lenient` mode (the chat tools' existing parse semantics) ----------
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident : lenient {
            $( $field:ident : $kind:ident $(min $min:literal)? = $desc:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis struct $name {
            $( #[doc = $desc] $vis $field: $crate::tool_params!(@ty $kind), )+
        }
        impl $name {
            $crate::tool_params!(@schema $vis, $( $field : $kind $(min $min)? = $desc ),+);

            /// Lenient extraction: a missing or wrong-typed field falls back
            /// to its default (`""` / `None` / `0`) instead of erroring — the
            /// browser chat tools' historical `.get().and_then().unwrap_or()`
            /// semantics, preserved exactly (validation stays in the tool body).
            $vis fn lenient(args: &$crate::__private::serde_json::Value) -> Self {
                Self {
                    $( $field: $crate::tool_params!(@lenient $kind, args, stringify!($field)), )+
                }
            }
        }
    };
}

// =============================================================================
// Hoisted chat-tool param tables (the `turn_flow` pattern): the tools live in
// `src/app/chat/tools/` (browser-app + wasm32 only — outside every default
// check), but their TABLES live here so plain `cargo test` byte-checks the
// wire schemas. New chat-tool migrations add their table below.
// =============================================================================

crate::tool_params! {
    /// Args for the browser `send_lh` tool (`src/app/chat/tools/platform.rs`)
    /// — transfer real `$LH` to an address or a name's owner, typed-confirmation
    /// gated. Hoisted here so its schema is covered by native `cargo test`
    /// (byte-identity + Gemini-safety below); the tool body keeps its own
    /// trim/amount/confirmation validation unchanged.
    pub struct SendLhParams: lenient {
        recipient: req_str = "Who receives the $LH: either a raw 0x… 20-byte \
                    address, or a subdomain name like \"alice\" (the funds go to \
                    that subdomain's on-chain OWNER address).",
        amount: req_str = "Amount of $LH to send, as a decimal string \
                    (e.g. \"5\", \"1.5\", \"0.01\"). Must be greater than 0.",
        confirmation: opt_str = "Single-use confirmation code. OMIT (or pass \"\") on the \
                    first call — it returns a challenge code shown to the owner. Relay \
                    it, wait for the owner to TYPE the code in chat, then retry with it. \
                    Never invent it; only the platform issues it.",
    }
}

#[cfg(test)]
mod tests {
    use super::SendLhParams;
    use serde_json::{json, Value};

    crate::tool_params! {
        /// Exercises every kind + `min` in one table (serde mode).
        struct AllKinds: serde {
            name: req_str = "A required string.",
            note: opt_str = "An optional string.",
            count: req_u32 min 1 = "A required integer.",
            limit: opt_u32 min 2 = "An optional integer.",
            flag: opt_bool = "An optional boolean.",
        }
    }

    crate::tool_params! {
        /// Same table in lenient mode (defaults instead of errors).
        struct AllKindsLenient: lenient {
            name: req_str = "A required string.",
            note: opt_str = "An optional string.",
            count: req_u32 min 1 = "A required integer.",
            limit: opt_u32 min 2 = "An optional integer.",
            flag: opt_bool = "An optional boolean.",
        }
    }

    /// The macro grammar must be unable to emit the schema shapes that 400 on
    /// Gemini and brick all chat: array-valued `type` and the JSON-Schema meta
    /// keys (`oneOf`/`anyOf`/`allOf`/`additionalProperties`/`$ref`/`$schema`).
    /// This walks a generated schema the same way the builtins' schema-lint
    /// guard does — and unlike that guard it also covers the HOISTED chat-tool
    /// tables, which the hard-coded builtin list never reached.
    fn assert_gemini_safe(v: &Value, path: &str) {
        const FORBIDDEN: [&str; 6] =
            ["oneOf", "anyOf", "allOf", "additionalProperties", "$ref", "$schema"];
        match v {
            Value::Object(map) => {
                if let Some(t) = map.get("type") {
                    assert!(!t.is_array(), "array-valued type at {path}: {t}");
                }
                for k in FORBIDDEN {
                    assert!(!map.contains_key(k), "forbidden key `{k}` at {path}");
                }
                for (k, val) in map {
                    assert_gemini_safe(val, &format!("{path}.{k}"));
                }
            }
            Value::Array(arr) => {
                for (i, val) in arr.iter().enumerate() {
                    assert_gemini_safe(val, &format!("{path}[{i}]"));
                }
            }
            _ => {}
        }
    }

    #[test]
    fn generated_schemas_are_gemini_safe() {
        for (name, schema) in [
            ("AllKinds", AllKinds::schema()),
            ("AllKindsLenient", AllKindsLenient::schema()),
            ("SendLhParams", SendLhParams::schema()),
        ] {
            assert_gemini_safe(&schema, name);
        }
    }

    /// The generated schema covers every kind: correct JSON types, `minimum`
    /// carried through, `required` in declaration order, key omitted when no
    /// field is required.
    #[test]
    fn schema_shape_covers_all_kinds() {
        let s = AllKinds::schema();
        assert_eq!(s["properties"]["name"]["type"], "string");
        assert_eq!(s["properties"]["note"]["type"], "string");
        assert_eq!(s["properties"]["count"]["type"], "integer");
        assert_eq!(s["properties"]["count"]["minimum"], 1);
        assert_eq!(s["properties"]["limit"]["minimum"], 2);
        assert_eq!(s["properties"]["flag"]["type"], "boolean");
        assert_eq!(s["properties"]["name"]["description"], "A required string.");
        assert_eq!(s["required"], json!(["name", "count"]));
        // serde and lenient modes generate the SAME schema from the same table.
        assert_eq!(s.to_string(), AllKindsLenient::schema().to_string());
    }

    crate::tool_params! {
        /// No required fields → the `required` key must be OMITTED (not `[]`).
        struct AllOptional: serde {
            note: opt_str = "An optional string.",
        }
    }

    #[test]
    fn required_key_omitted_when_no_field_is_required() {
        assert!(AllOptional::schema().get("required").is_none());
        let p: AllOptional = serde_json::from_value(json!({"note": "x"})).unwrap();
        assert_eq!(p.note.as_deref(), Some("x"));
    }

    /// `: serde` mode keeps the builtins' EXACT existing parse semantics:
    /// required fields error on missing, `Option` fields default to `None`.
    #[test]
    fn serde_mode_parse_matches_builtin_semantics() {
        let ok: AllKinds =
            serde_json::from_value(json!({"name": "x", "count": 3})).unwrap();
        assert_eq!(ok.name, "x");
        assert_eq!(ok.count, 3);
        assert_eq!(ok.note, None);
        assert_eq!(ok.limit, None);
        assert_eq!(ok.flag, None);
        // Missing required field errors, naming the field — serde's message.
        let err = match serde_json::from_value::<AllKinds>(json!({"count": 3})) {
            Err(e) => e,
            Ok(_) => panic!("missing required `name` must error"),
        };
        assert!(err.to_string().contains("name"), "{err}");
    }

    /// `: lenient` mode reproduces the chat tools' historical
    /// `.get().and_then().unwrap_or()` semantics exactly: missing OR
    /// wrong-typed fields fall back to defaults, never error.
    #[test]
    fn lenient_mode_matches_historical_chat_tool_semantics() {
        let p = AllKindsLenient::lenient(&json!({}));
        assert_eq!(p.name, "");
        assert_eq!(p.note, None);
        assert_eq!(p.count, 0);
        assert_eq!(p.limit, None);
        assert_eq!(p.flag, None);
        // Wrong-typed values degrade to defaults, same as `.and_then(as_str)`.
        let p = AllKindsLenient::lenient(&json!({"name": 5, "count": "x", "flag": 1}));
        assert_eq!(p.name, "");
        assert_eq!(p.count, 0);
        assert_eq!(p.flag, None);
        let p = AllKindsLenient::lenient(
            &json!({"name": "a", "note": "n", "count": 2, "limit": 7, "flag": true}),
        );
        assert_eq!((p.name.as_str(), p.count, p.limit, p.flag), ("a", 2, Some(7), Some(true)));
    }

    /// BYTE-IDENTITY: the generated `send_lh` schema serializes byte-for-byte
    /// equal to the original hand-written literal it replaced in
    /// `src/app/chat/tools/platform.rs` (frozen verbatim below). The wire
    /// shape is model-behavior-load-bearing — this is the migration contract,
    /// and the first native-test coverage ANY chat-tool schema has had.
    #[test]
    fn send_lh_schema_is_byte_identical_to_the_frozen_original() {
        let frozen = json!({
            "type": "object",
            "properties": {
                "recipient": {
                    "type": "string",
                    "description": "Who receives the $LH: either a raw 0x… 20-byte \
                        address, or a subdomain name like \"alice\" (the funds go to \
                        that subdomain's on-chain OWNER address)."
                },
                "amount": {
                    "type": "string",
                    "description": "Amount of $LH to send, as a decimal string \
                        (e.g. \"5\", \"1.5\", \"0.01\"). Must be greater than 0."
                },
                "confirmation": {
                    "type": "string",
                    "description": "Single-use confirmation code. OMIT (or pass \"\") on the \
                        first call — it returns a challenge code shown to the owner. Relay \
                        it, wait for the owner to TYPE the code in chat, then retry with it. \
                        Never invent it; only the platform issues it."
                }
            },
            "required": ["recipient", "amount"]
        });
        assert_eq!(SendLhParams::schema().to_string(), frozen.to_string());
    }

    /// The lenient extraction feeds `send_lh`'s unchanged body validation the
    /// same values the old inline chains produced — including the edge cases
    /// (missing args, wrong types, whitespace confirmation).
    #[test]
    fn send_lh_lenient_matches_the_old_inline_extraction() {
        let p = SendLhParams::lenient(&json!({}));
        assert_eq!((p.recipient.as_str(), p.amount.as_str(), p.confirmation), ("", "", None));
        let p = SendLhParams::lenient(&json!({"recipient": 5, "amount": true}));
        assert_eq!((p.recipient.as_str(), p.amount.as_str()), ("", ""));
        let p = SendLhParams::lenient(
            &json!({"recipient": " alice ", "amount": "1.5", "confirmation": "  "}),
        );
        assert_eq!(p.recipient, " alice "); // body trims, exactly as before
        assert_eq!(p.amount, "1.5");
        // old: .map(|s| !s.trim().is_empty()).unwrap_or(false) → still false
        assert!(!p.confirmation.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false));
    }
}
