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
//! doesn't cover (arrays of objects, runtime-derived enums/descriptions) stay
//! hand-written until a kind is added — the escape hatch is "don't migrate
//! yet", never "bend the table".
//!
//! # Permanent residents (the sweep is COMPLETE — these stay hand-written BY DESIGN)
//!
//! The migration sweep is DONE. Every tool below keeps its hand-written
//! `input_schema` deliberately; do NOT "finish" migrating them without first
//! removing the blocking reason (usually by changing the wire schema — which
//! is a model-behavior change, not a refactor):
//!
//! * `ask_question` (builtin) — `options` is an array of NESTED OBJECTS
//!   (`{label, description}`); the grammar has no object-items kind.
//! * `batch_send_lh` (chat) — `transfers` is an array of NESTED OBJECTS
//!   (`{recipient, amount}` with its own `required`).
//! * `finish` (builtin) — its `output` property is deliberately TYPE-LESS
//!   (accepts any JSON); every table kind emits a single `type`.
//! * `consult_model` (chat) — the `model` enum AND the tool description are
//!   RUNTIME-derived from the `CONSULT_MODELS` allowlist; a literal table
//!   would fork that single source.
//! * `stake_validation` (chat) — its required-arg errors are FIELD-SPECIFIC
//!   (`"subject (tokenId) is required"`, `"valid (true/false verdict) is
//!   required"`); the generated accessors can only say `"<field> is required"`.
//! * `discover_bounties`, `discover_parties`, `list_my_guilds`,
//!   `bulk_release_subdomains` (chat) — their hand-written schemas carry a
//!   LITERAL `"required": []`, which can never byte-match the macro's
//!   omit-when-empty rule.
//! * The zero-arg tools (`{"type":"object","properties":{}}` — no table to
//!   declare): `current_time` + `read_self_docs`, `list_subdomains`,
//!   `check_balances`, `evm_chains`, `clear_context`, `compact_context`,
//!   `clear_notifications`, `list_notifications`, `consolidate_lessons`,
//!   `list_skills`, `shared_state_list`.
//!
//! Partially-inline extractions (migrated, but one field's PARSE deliberately
//! stays in the tool body while the table owns its schema): `attest.rating`
//! (numeric-string coercion), `evm_call.args` (non-string entries are
//! STRINGIFIED, not dropped), `form_party.shares` (error ORDER/messages over
//! the raw array), `found_company.roles` (`resolve_roles` reads the raw
//! `Value`), `configure_agent` (absent-vs-`null` distinctions need the raw
//! args).
//!
//! Field kinds: `req_str`, `opt_str`, `req_u32`, `req_u64`, `opt_u32`,
//! `opt_u64`, `opt_bool`, `req_bool`, plus the flat array kinds
//! `req_str_array`, `opt_str_array`, `opt_i64_array` (JSON-Schema
//! `"type":"array"` with single-type `items`; lenient parse = the historical
//! `.as_array()` + `filter_map` element extraction). Each kind is optionally
//! followed by `min N` / `max N` (JSON-Schema `minimum`/`maximum` — on the
//! ITEMS for array kinds, on the field for scalars) and — for string kinds —
//! `enum ["a", "b"]`, which emits the schema's exact `"enum": [...]` array.
//! `enum` shapes only the SCHEMA (it constrains the model); the lenient parse
//! stays a plain string extraction and out-of-enum validation stays in the
//! tool body, exactly as the hand-written tools behaved.
//! `req_*` kinds land in the schema's `required` array in declaration order.
//! `req_u64` is the LENIENT-mode required integer: stored as `Option<u64>`
//! and read via a generated `fn <field>() -> Result<u64>` accessor that
//! errors `"<field> is required"` on a missing or non-integer value instead
//! of defaulting — a real id 0 must never be conflated with "missing" (the
//! bounty/guild/proposal id semantics). `req_bool` is its boolean sibling:
//! `Option<bool>` + an accessor erroring the tools' historical
//! `"<field> (true/false) is required"`. In serde tables use plain required
//! kinds; serde's own missing-field error already covers them.
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
    (@ty req_u64) => { ::core::option::Option<u64> };
    (@ty opt_u32) => { ::core::option::Option<u32> };
    (@ty opt_u64) => { ::core::option::Option<u64> };
    (@ty opt_bool) => { ::core::option::Option<bool> };
    (@ty req_bool) => { ::core::option::Option<bool> };
    (@ty req_str_array) => { ::std::vec::Vec<::std::string::String> };
    (@ty opt_str_array) => { ::core::option::Option<::std::vec::Vec<::std::string::String>> };
    (@ty opt_i64_array) => { ::core::option::Option<::std::vec::Vec<i64>> };
    // ---------- internal: JSON-Schema "type" per kind ----------
    (@json_ty req_str) => { "string" };
    (@json_ty opt_str) => { "string" };
    (@json_ty req_u32) => { "integer" };
    (@json_ty req_u64) => { "integer" };
    (@json_ty opt_u32) => { "integer" };
    (@json_ty opt_u64) => { "integer" };
    (@json_ty opt_bool) => { "boolean" };
    (@json_ty req_bool) => { "boolean" };
    // ---------- internal: required flag per kind ----------
    (@required req_str) => { true };
    (@required req_u32) => { true };
    (@required req_u64) => { true };
    (@required req_bool) => { true };
    (@required req_str_array) => { true };
    (@required opt_str) => { false };
    (@required opt_u32) => { false };
    (@required opt_u64) => { false };
    (@required opt_bool) => { false };
    (@required opt_str_array) => { false };
    (@required opt_i64_array) => { false };
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
    (@lenient req_u64, $args:expr, $name:expr) => {
        $args.get($name).and_then(|v| v.as_u64())
    };
    (@lenient opt_u32, $args:expr, $name:expr) => {
        $args
            .get($name)
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok())
    };
    (@lenient opt_u64, $args:expr, $name:expr) => {
        $args.get($name).and_then(|v| v.as_u64())
    };
    (@lenient opt_bool, $args:expr, $name:expr) => {
        $args.get($name).and_then(|v| v.as_bool())
    };
    (@lenient req_bool, $args:expr, $name:expr) => {
        $args.get($name).and_then(|v| v.as_bool())
    };
    (@lenient req_str_array, $args:expr, $name:expr) => {
        $args
            .get($name)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(::std::string::ToString::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    (@lenient opt_str_array, $args:expr, $name:expr) => {
        $args.get($name).and_then(|v| v.as_array()).map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(::std::string::ToString::to_string)
                .collect()
        })
    };
    (@lenient opt_i64_array, $args:expr, $name:expr) => {
        $args
            .get($name)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
    };
    // ---------- internal: per-field REQUIRED accessor. `req_u64` generates a
    // method (field-named — methods and fields share no namespace) that errors
    // on missing/non-integer instead of defaulting, reproducing the historical
    // `.as_u64().ok_or_else(..)` chat-tool arm verbatim (id 0 stays a real id).
    // Every other kind generates nothing. ----------
    (@req_accessor $vis:vis, req_u64, $field:ident) => {
        /// Required integer, lenient-extracted: `Ok` when present and
        /// integer-typed (0 is a REAL value), else the tools' historical
        /// `"<field> is required"` error — never a silent default.
        $vis fn $field(&self) -> ::core::result::Result<u64, $crate::error::Error> {
            self.$field.ok_or_else(|| {
                $crate::error::Error::other(concat!(stringify!($field), " is required"))
            })
        }
    };
    (@req_accessor $vis:vis, req_bool, $field:ident) => {
        /// Required boolean, lenient-extracted: `Ok` when present and
        /// bool-typed, else the tools' historical
        /// `"<field> (true/false) is required"` error — never a silent default.
        $vis fn $field(&self) -> ::core::result::Result<bool, $crate::error::Error> {
            self.$field.ok_or_else(|| {
                $crate::error::Error::other(concat!(
                    stringify!($field),
                    " (true/false) is required"
                ))
            })
        }
    };
    (@req_accessor $vis:vis, $other:ident, $field:ident) => {};
    // ---------- internal: per-field schema map. The three array kinds emit
    // `"type":"array","items":{"type":...}` and route `min`/`max` into the
    // ITEMS (JSON-Schema bounds on each element — the form_party `shares`
    // shape); scalar kinds keep them on the field itself. NOTHING else about
    // items is expressible (no descriptions, no nested objects, no minItems) —
    // deliberately, so the grammar stays Gemini-safe and small. ----------
    (@field_map req_str_array $(, min $min:literal)? $(, max $max:literal)?) => {
        $crate::tool_params!(@array_field_map "string" $(, min $min)? $(, max $max)?)
    };
    (@field_map opt_str_array $(, min $min:literal)? $(, max $max:literal)?) => {
        $crate::tool_params!(@array_field_map "string" $(, min $min)? $(, max $max)?)
    };
    (@field_map opt_i64_array $(, min $min:literal)? $(, max $max:literal)?) => {
        $crate::tool_params!(@array_field_map "integer" $(, min $min)? $(, max $max)?)
    };
    (@field_map $kind:ident $(, min $min:literal)? $(, max $max:literal)?) => {{
        let mut f = $crate::__private::serde_json::Map::new();
        f.insert(
            "type".to_string(),
            $crate::__private::serde_json::Value::String(
                $crate::tool_params!(@json_ty $kind).to_string(),
            ),
        );
        $( f.insert("minimum".to_string(), $crate::__private::serde_json::Value::from($min)); )?
        $( f.insert("maximum".to_string(), $crate::__private::serde_json::Value::from($max)); )?
        f
    }};
    (@array_field_map $item_ty:literal $(, min $min:literal)? $(, max $max:literal)?) => {{
        let mut items = $crate::__private::serde_json::Map::new();
        items.insert(
            "type".to_string(),
            $crate::__private::serde_json::Value::String($item_ty.to_string()),
        );
        $( items.insert("minimum".to_string(), $crate::__private::serde_json::Value::from($min)); )?
        $( items.insert("maximum".to_string(), $crate::__private::serde_json::Value::from($max)); )?
        let mut f = $crate::__private::serde_json::Map::new();
        f.insert(
            "type".to_string(),
            $crate::__private::serde_json::Value::String("array".to_string()),
        );
        f.insert("items".to_string(), $crate::__private::serde_json::Value::Object(items));
        f
    }};
    // ---------- internal: the shared `schema()` body ----------
    (@schema $vis:vis, $( $field:ident : $kind:ident $(min $min:literal)? $(max $max:literal)? $(enum [$($ev:literal),+ $(,)?])? = $desc:literal ),+) => {
        /// Wire `input_schema`, generated from the SAME table as the struct
        /// fields. Single `type` per property, no unions / `oneOf` /
        /// `additionalProperties` — Gemini-safe by construction.
        $vis fn schema() -> $crate::__private::serde_json::Value {
            use $crate::__private::serde_json::{Map, Value};
            let mut props = Map::new();
            $(
                let mut f = $crate::tool_params!(@field_map $kind $(, min $min)? $(, max $max)?);
                $(
                    f.insert(
                        "enum".to_string(),
                        Value::Array(vec![ $( Value::String($ev.to_string()) ),+ ]),
                    );
                )?
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
            $( $field:ident : $kind:ident $(min $min:literal)? $(max $max:literal)? $(enum [$($ev:literal),+ $(,)?])? = $desc:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(::serde::Deserialize)]
        $vis struct $name {
            $( #[doc = $desc] $vis $field: $crate::tool_params!(@ty $kind), )+
        }
        impl $name {
            $crate::tool_params!(@schema $vis, $( $field : $kind $(min $min)? $(max $max)? $(enum [$($ev),+])? = $desc ),+);
            $( $crate::tool_params!(@req_accessor $vis, $kind, $field); )+
        }
    };
    // ---------- `: lenient` mode (the chat tools' existing parse semantics) ----------
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident : lenient {
            $( $field:ident : $kind:ident $(min $min:literal)? $(max $max:literal)? $(enum [$($ev:literal),+ $(,)?])? = $desc:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis struct $name {
            $( #[doc = $desc] $vis $field: $crate::tool_params!(@ty $kind), )+
        }
        impl $name {
            $crate::tool_params!(@schema $vis, $( $field : $kind $(min $min)? $(max $max)? $(enum [$($ev),+])? = $desc ),+);
            $( $crate::tool_params!(@req_accessor $vis, $kind, $field); )+

            /// Lenient extraction: a missing or wrong-typed field falls back
            /// to its default (`""` / `None` / `0`) instead of erroring — the
            /// browser chat tools' historical `.get().and_then().unwrap_or()`
            /// semantics, preserved exactly (validation stays in the tool body).
            /// `req_u64` fields extract to `None` here; the error fires at
            /// their generated accessor, matching the old inline `ok_or_else`.
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

crate::tool_params! {
    /// Args for the browser `create_subdomain` tool
    /// (`src/app/chat/tools/platform.rs`) — sponsored name mint + optional
    /// actor-model persona/prefund. Body keeps its own validate/trim logic.
    pub struct CreateSubdomainParams: lenient {
        name: req_str = "Subdomain to register, e.g. \"alice\" becomes \
                    alice.localharness.xyz. 3-32 chars; lowercase letters, digits, \
                    and hyphens only.",
        persona: opt_str = "OPTIONAL system instruction / persona for the new \
                    agent — published on-chain as its system prompt (the persona \
                    that headless `call`s and the public face read). Omit to leave \
                    the default.",
        prefund_lh: opt_str = "OPTIONAL amount of $LH to prefund the new agent with, \
                    as a decimal string (\"5\", \"1.5\"). Transferred from YOUR \
                    wallet to the new subdomain's token-bound account (its own \
                    spendable wallet — used to pay other agents via x402). Omit, or \
                    pass \"0\", to skip. Must not exceed your $LH balance.",
    }
}

crate::tool_params! {
    /// Args for the browser `create_and_publish_app` tool
    /// (`src/app/chat/tools/platform.rs`) — one-shot compile + register + publish.
    pub struct CreateAndPublishAppParams: lenient {
        name: req_str = "Subdomain to register, e.g. \"clock\" becomes \
                    clock.localharness.xyz. 3-32 chars; lowercase letters, digits, \
                    and hyphens only.",
        source: req_str = "rustlite cartridge source — the SAME dialect as \
                    run_cartridge. Exports `fn frame(t: i32)` (animated) or \
                    `fn render()` and draws via `use host::display;`. This becomes \
                    the subdomain's fullscreen public face.",
        persona: opt_str = "OPTIONAL system instruction / persona for the new \
                    agent — published on-chain as its system prompt (read by \
                    headless `call`s). Omit to leave the default.",
        prefund_lh: opt_str = "OPTIONAL amount of $LH to prefund the new agent with, \
                    as a decimal string (\"5\", \"1.5\"). Transferred from YOUR \
                    wallet to the new subdomain's token-bound account (its own \
                    spendable wallet). Omit, or pass \"0\", to skip. Must not exceed \
                    your $LH balance.",
    }
}

crate::tool_params! {
    /// Args for the browser `publish_app_to` tool
    /// (`src/app/chat/tools/platform.rs`) — update-from-MAIN publish, confirm-gated.
    pub struct PublishAppToParams: lenient {
        name: req_str = "The subdomain to publish to — MUST be one you already \
                    own (e.g. \"clock\" → clock.localharness.xyz). Can be different from \
                    the subdomain you are currently on. To create a NEW subdomain, use \
                    create_and_publish_app instead.",
        source: req_str = "rustlite cartridge source — the SAME dialect as \
                    run_cartridge / create_and_publish_app. Exports `fn frame(t: i32)` \
                    (animated) or `fn render()` and draws via `use host::display;`. \
                    Becomes the target subdomain's fullscreen public face.",
        confirmation: opt_str = "Single-use confirmation code. OMIT (or pass \"\") on the \
                    first call — it returns a challenge code shown to the owner. State \
                    which subdomain you will update, ask the owner to TYPE the code in \
                    chat, then retry with it. Never invent it; only the platform issues it.",
    }
}

crate::tool_params! {
    /// Args for the browser `embed_app` tool (`src/app/chat/tools/platform.rs`).
    pub struct EmbedAppParams: lenient {
        name: req_str = "Subdomain whose published cartridge to embed, \
                    e.g. \"pong\" embeds pong.localharness.xyz's app inline.",
    }
}

crate::tool_params! {
    /// Args for the browser `publish_public_face` tool
    /// (`src/app/chat/tools/platform.rs`).
    pub struct PublishPublicFaceParams: lenient {
        choice: req_str = "Which face to publish: \"app\" (compile + publish \
                    this device's local app.rl as a fullscreen cartridge), \
                    \"html\" (publish local index.html), or \"directory\" (a \
                    profile landing listing your sibling agents).",
    }
}

crate::tool_params! {
    /// Args for the browser `release_subdomain` tool
    /// (`src/app/chat/tools/platform.rs`) — DESTRUCTIVE burn, confirm-gated.
    pub struct ReleaseSubdomainParams: lenient {
        name: req_str = "Subdomain to release/recycle — burns the NFT, frees the name.",
        confirmation: opt_str = "Single-use confirmation code. OMIT (or pass \"\") on the \
                    first call — it returns a challenge code that is shown to the owner. \
                    Relay it, wait for the owner to TYPE that code in chat, then retry \
                    with the code here. Never invent it; only the platform issues it.",
    }
}

crate::tool_params! {
    /// Args for the browser `discover_agents` tool
    /// (`src/app/chat/tools/platform.rs`) — read-only registry scan.
    pub struct DiscoverAgentsParams: lenient {
        query: req_str = "What to look for — capabilities, topics, or \
                    keywords matched (case-insensitively) against agent names \
                    and personas. Several keywords are ORed and ranked by \
                    overlap (e.g. \"solidity audit security\"). \
                    Empty returns recent agents.",
    }
}

crate::tool_params! {
    /// Args for the browser `query_balance` tool
    /// (`src/app/chat/tools/platform.rs`) — read-only balance lookup.
    pub struct QueryBalanceParams: lenient {
        target: req_str = "an agent NAME (e.g. \"binglescan\") or a 0x address",
    }
}

crate::tool_params! {
    /// Args for the browser `post_bounty` tool (`src/app/chat/tools/bounty.rs`)
    /// — escrow $LH behind an on-chain task. Body keeps parse/positivity checks.
    pub struct PostBountyParams: lenient {
        task: req_str = "The task to be done — a clear, self-contained \
                    description of what a claimant must deliver to earn the reward.",
        reward_lh: req_str = "Reward in $LH, as a decimal string (\"5\", \"1.5\"). \
                    Escrowed from YOUR wallet when the bounty is posted; paid out to \
                    the claimant when you accept their result. Must be > 0.",
        ttl_hours: opt_str = "OPTIONAL lifetime in hours before the bounty expires \
                    (decimal). Omit for the 24h default.",
    }
}

crate::tool_params! {
    /// Args for the browser `set_persona` tool (`src/app/chat/tools/misc.rs`)
    /// — SELF-EDIT of the agent's own system instruction.
    pub struct SetPersonaParams: lenient {
        text: req_str = "The new system instruction / persona for YOURSELF — \
                    your role, personality, and constraints. This becomes both your \
                    on-chain published persona AND your local custom system prompt; it \
                    takes effect on your next session. Keep it focused.",
    }
}

crate::tool_params! {
    /// Args for the browser `record_lesson` tool (`src/app/chat/tools/misc.rs`)
    /// — the write half of the LESSONS LOOP.
    pub struct RecordLessonParams: lenient {
        lesson: req_str = "ONE short lesson (a single sentence, max 240 chars) \
                    learned from a REAL error, failed tool call, or user correction. \
                    Make it concrete and actionable (what to do differently next \
                    time), not a description of what happened.",
    }
}

crate::tool_params! {
    /// Args for the browser `notify` tool (`src/app/chat/tools/misc.rs`)
    /// — local device notification or cross-agent inbox push.
    pub struct NotifyParams: lenient {
        title: req_str = "Short notification title, e.g. \"timer done\" or \
                    \"new message from dex\".",
        body: opt_str = "Optional body text shown under the title. Keep it \
                    to a sentence.",
        vibrate: opt_bool = "Also vibrate the device (mobile only; silently \
                    ignored where unsupported).",
        to: opt_str = "CROSS-AGENT: deliver to ANOTHER agent's \
                    notification inbox instead of this device — the target \
                    subdomain name, e.g. \"krafto\". Routed via the platform \
                    proxy (costs the per-request $LH like a model call); the \
                    push title is stamped with YOUR identity so the recipient \
                    sees who pinged them. Omit for a local notification on \
                    this device.",
    }
}

crate::tool_params! {
    /// Args for the browser `claim_bounty` tool (`src/app/chat/tools/bounty.rs`)
    /// — claim an open bounty as THIS agent. `bounty_id` uses the `req_u64`
    /// required-accessor (id 0 is real; missing/wrong-type errors).
    pub struct ClaimBountyParams: lenient {
        bounty_id: req_u64 min 0 = "The id of the open bounty to claim (from \
                    discover_bounties / the bounty board).",
    }
}

crate::tool_params! {
    /// Args for the browser `submit_result` tool (`src/app/chat/tools/bounty.rs`).
    pub struct SubmitResultParams: lenient {
        bounty_id: req_u64 min 0 = "The id of the bounty you previously claimed.",
        result: req_str = "Your deliverable / result for the bounty — the work \
                    product the poster will review before accepting + paying out.",
    }
}

crate::tool_params! {
    /// Args for the browser `accept_result` tool (`src/app/chat/tools/bounty.rs`)
    /// — releases escrow, so the preflight/signing body stays unchanged.
    pub struct AcceptResultParams: lenient {
        bounty_id: req_u64 min 0 = "The id of a bounty YOU posted whose submitted result \
                    you want to accept (releases the escrowed $LH to the claimant).",
    }
}

crate::tool_params! {
    /// Args for the browser `create_guild` tool (`src/app/chat/tools/guild.rs`).
    pub struct CreateGuildParams: lenient {
        name: req_str = "Display name for the guild (a short label for the org).",
    }
}

crate::tool_params! {
    /// Args for the browser `invite_to_guild` tool (`src/app/chat/tools/guild.rs`).
    pub struct InviteToGuildParams: lenient {
        guild_id: req_u64 min 0 = "The id of the guild you administer.",
        member: req_str = "Who to invite — a raw 0x… address OR a subdomain name \
                    (resolved to that name's on-chain owner).",
    }
}

crate::tool_params! {
    /// Args for the browser `fund_guild` tool (`src/app/chat/tools/guild.rs`).
    pub struct FundGuildParams: lenient {
        guild_id: req_u64 min 0 = "The id of the guild to fund.",
        amount_lh: req_str = "Amount of $LH to contribute, as a decimal string \
                    (\"5\", \"1.5\"). Pulled from YOUR wallet into the shared treasury. \
                    Must be > 0.",
    }
}

crate::tool_params! {
    /// Args for the browser `spend_treasury` tool (`src/app/chat/tools/guild.rs`)
    /// — pays $LH OUT of a guild treasury, confirm-gated.
    pub struct SpendTreasuryParams: lenient {
        guild_id: req_u64 min 0 = "The id of the guild whose treasury to spend from.",
        to: req_str = "Recipient — a raw 0x… address OR a subdomain name \
                    (resolved to that name's on-chain owner).",
        amount_lh: req_str = "Amount of $LH to pay out, as a decimal string. Must be > 0.",
        memo: opt_str = "OPTIONAL note recorded with the payment (what it's for).",
        confirmation: opt_str = "Single-use confirmation code. OMIT (or pass \"\") on the \
                    first call — it returns a challenge code shown to the owner. Relay \
                    it, wait for the owner to TYPE the code in chat, then retry with it. \
                    Never invent it; only the platform issues it.",
    }
}

crate::tool_params! {
    /// Args for the browser `propose_measure` tool
    /// (`src/app/chat/tools/governance.rs`).
    pub struct ProposeMeasureParams: lenient {
        guild_id: req_u64 min 0 = "The id of the guild whose treasury the proposal would spend from.",
        to: req_str = "Spend recipient if the proposal passes — a raw 0x… \
                    address OR a subdomain name (resolved to that name's on-chain owner).",
        amount_lh: req_str = "Amount of $LH the proposal would pay out from the \
                    treasury, as a decimal string (\"5\", \"1.5\"). Must be > 0.",
        memo: opt_str = "OPTIONAL description of what the spend is for — recorded \
                    on-chain so voters know what they're approving.",
        period_hours: opt_str = "OPTIONAL voting window in hours (decimal). Omit for the \
                    48h default. Members can vote until the deadline; only then can a \
                    passing proposal be executed.",
    }
}

crate::tool_params! {
    /// Args for the browser `execute_proposal` tool
    /// (`src/app/chat/tools/governance.rs`).
    pub struct ExecuteProposalParams: lenient {
        proposal_id: req_u64 min 0 = "The id of a passed proposal whose voting deadline has \
                    elapsed (executing it pays out the treasury spend).",
    }
}

crate::tool_params! {
    /// Args for the browser `list_proposals` tool
    /// (`src/app/chat/tools/governance.rs`) — read-only.
    pub struct ListProposalsParams: lenient {
        guild_id: req_u64 min 0 = "The id of the guild whose proposals to list.",
    }
}

crate::tool_params! {
    /// Args for the browser `web_fetch` tool (`src/app/chat/tools/misc.rs`)
    /// — proxy-metered external HTTPS fetch.
    pub struct WebFetchParams: lenient {
        url: req_str = "Absolute https:// URL to fetch — a docs page, \
                    GitHub README (use raw.githubusercontent.com for raw \
                    content), or JSON API endpoint. http://, private/internal \
                    hosts, and raw-IP targets are rejected.",
    }
}

crate::tool_params! {
    /// Args for the browser `submit_feedback` tool (`src/app/chat/tools/misc.rs`).
    pub struct SubmitFeedbackParams: lenient {
        text: req_str = "The feedback text. Filed off-chain with full \
                    conversation + device/settings context. (If the owner enabled \
                    on-chain mirroring, the SHORT note is also written on-chain, where \
                    a 2048-byte cap applies — summarize rather than pasting a long report.)",
    }
}

crate::tool_params! {
    /// Args for the browser `set_lessons` tool (`src/app/chat/tools/misc.rs`)
    /// — the guarded WRITE half of a consolidate_lessons pass.
    pub struct SetLessonsParams: lenient {
        lessons: req_str = "The FULL replacement lessons list — one lesson \
                    per line, newline-separated, max 10 lines of max 240 chars \
                    each. This REPLACES every existing lesson, so it must \
                    still contain (verbatim or strengthened) every lesson \
                    worth keeping; anything omitted is forgotten.",
    }
}

crate::tool_params! {
    /// Args for the browser `create_skill` tool (`src/app/chat/tools/misc.rs`)
    /// — the write half of the SKILLS LOOP (upsert by name).
    pub struct CreateSkillParams: lenient {
        name: req_str = "A short handle for the skill (e.g. \"summarize\", \
                    \"daily-standup\"), max 48 chars. Re-using an existing name \
                    REPLACES that skill's instructions.",
        instructions: req_str = "The reusable instruction/prompt fragment that defines \
                    what the skill does when invoked — a focused recipe (max 600 \
                    chars). Make it self-contained and actionable.",
    }
}

crate::tool_params! {
    /// Args for the browser `delete_skill` tool (`src/app/chat/tools/misc.rs`).
    pub struct DeleteSkillParams: lenient {
        name: req_str = "The name of the skill to remove (use list_skills to \
                    see your defined skills).",
    }
}

crate::tool_params! {
    /// Args for the browser `cancel_task` tool (`src/app/chat/tools/misc.rs`)
    /// — off-chain job teardown; the body keeps the trim/empty validation.
    pub struct CancelTaskParams: lenient {
        job_id: req_str = "The id of the scheduled job to cancel — the `job_id` \
                    string schedule_task returned.",
    }
}

crate::tool_params! {
    /// Args for the browser `execute_script` tool (`src/app/chat/tools/misc.rs`)
    /// — one-pass bashlite over the tenant's OPFS sandbox.
    pub struct ExecuteScriptParams: lenient {
        source: req_str = "A bashlite script to run over your OPFS sandbox. \
                    Supports: variables (x=value, x=$(cmd)), $VAR / ${VAR} / $? \
                    interpolation, pipes (a | b | c), && / || short-circuit \
                    chaining, if/elif/else/fi, for NAME in WORDS; do …; done \
                    (`for f in $(…)` splits on whitespace), while …; do …; done, \
                    [ … ] tests (string =/!=/-z/-n, int -eq/-ne/-lt/-le/-gt/-ge, \
                    file -e/-f/-d PATH), \
                    command substitution $(…), and `run FILE.bl` / `source FILE.bl` \
                    to compose another script. Builtins (filesystem): \
                    echo, cd, pwd, ls, cat, grep PATTERN (literal substring; \
                    -i/-v/-c), find [path] [-name GLOB] [-type f|d], wc [-l|-w|-c] \
                    (of stdin), head/tail [-n N] (first/last N stdin lines), \
                    mkdir, write/create PATH CONTENT (create-only — \
                    refuses to overwrite), true/false. NO value-moving / lh-* \
                    commands, NO networking, NO process spawning.",
    }
}

crate::tool_params! {
    /// Args for the browser `spawn_recursive_subagent` tool
    /// (`src/app/chat/tools/misc.rs`) — reduced-surface tool-bearing subagent.
    pub struct SpawnRecursiveSubagentParams: lenient {
        system_instructions: req_str = "System prompt for the subagent — describes its persona, \
                    scope, and any constraints. Often \"you are a focused worker \
                    that does X and returns just the result\".",
        prompt: req_str = "The user message to send to the subagent.",
    }
}

crate::tool_params! {
    /// Args for the browser `company_status` tool
    /// (`src/app/chat/tools/company.rs`) — read-only org snapshot.
    pub struct CompanyStatusParams: lenient {
        company: req_str = "Which company/guild to report on — a numeric guild id \
                    (e.g. \"67\") OR a guild display name you belong to.",
    }
}

crate::tool_params! {
    /// Args for the browser `shared_state_set` tool (`src/app/chat/tools/room.rs`).
    pub struct SharedStateSetParams: lenient {
        key: req_str = "The key to write in the shared volume, e.g. \
                    \"task_status\" or \"worker_1/progress\".",
        value: req_str = "The value to store under `key` (UTF-8 text).",
    }
}

crate::tool_params! {
    /// Args for the browser `shared_state_get` tool (`src/app/chat/tools/room.rs`).
    pub struct SharedStateGetParams: lenient {
        key: req_str = "The key to read from the shared volume.",
    }
}

crate::tool_params! {
    /// Args for the browser `evm_balance` tool (`src/app/chat/tools/evm.rs`)
    /// — read-only multi-chain native/ERC-20 balance.
    pub struct EvmBalanceParams: lenient {
        chain: req_str = "Which chain: ethereum, base, optimism, arbitrum, \
                    polygon, or tempo (aliases: eth/mainnet, op, arb, matic). Call \
                    evm_chains() if unsure.",
        address: req_str = "The 0x… account address to read the balance OF.",
        token: opt_str = "OPTIONAL ERC-20 token contract address (0x…). Given \
                    → returns that token's balanceOf(address) with best-effort \
                    symbol + decimals; omitted → the chain's NATIVE coin balance.",
    }
}

crate::tool_params! {
    /// Args for the browser `resolve_ens` tool (`src/app/chat/tools/evm.rs`).
    pub struct ResolveEnsParams: lenient {
        name: req_str = "An ENS name to resolve, e.g. \"vitalik.eth\".",
    }
}

crate::tool_params! {
    /// Args for the browser `challenge_validation` tool
    /// (`src/app/chat/tools/validation.rs`).
    pub struct ChallengeValidationParams: lenient {
        validation_id: req_u64 min 0 = "The id of the OPEN validation to challenge (from \
                    get_validation).",
    }
}

crate::tool_params! {
    /// Args for the browser `resolve_validation` tool
    /// (`src/app/chat/tools/validation.rs`) — resolver-only ruling.
    pub struct ResolveValidationParams: lenient {
        validation_id: req_u64 min 0 = "The id of the CHALLENGED validation to resolve.",
        winner: req_str = "Who wins, paid BOTH stakes: \"validator\" (the original \
                    verdict stands) or \"challenger\" (the counter-verdict stands).",
    }
}

crate::tool_params! {
    /// Args for the browser `reclaim_validation` tool
    /// (`src/app/chat/tools/validation.rs`).
    pub struct ReclaimValidationParams: lenient {
        validation_id: req_u64 min 0 = "The id of the validation to refund (its window must \
                    have passed).",
    }
}

crate::tool_params! {
    /// Args for the browser `get_validation` tool
    /// (`src/app/chat/tools/validation.rs`) — read-only record fetch.
    pub struct GetValidationParams: lenient {
        validation_id: req_u64 min 0 = "The id of the validation to read.",
    }
}

crate::tool_params! {
    /// Args for the browser `join_party` tool (`src/app/chat/tools/party.rs`).
    pub struct JoinPartyParams: lenient {
        party_id: req_u64 min 0 = "The id of the party to consent to (from \
                    discover_parties / get_party).",
    }
}

crate::tool_params! {
    /// Args for the browser `fund_party` tool (`src/app/chat/tools/party.rs`)
    /// — escrows $LH, so the parse/positivity body stays unchanged.
    pub struct FundPartyParams: lenient {
        party_id: req_u64 min 0 = "The id of the party whose pot to fund.",
        amount_lh: req_str = "Amount of $LH to contribute, as a decimal string (\"5\", \
                    \"1.5\"). Pulled from YOUR wallet into the party pot; refunded exactly \
                    on disband/expiry, split to the members on complete. Must be > 0.",
    }
}

crate::tool_params! {
    /// Args for the browser `complete_party` tool (`src/app/chat/tools/party.rs`).
    pub struct CompletePartyParams: lenient {
        party_id: req_u64 min 0 = "The id of a party YOU formed (Active, all seats consented) \
                    whose pot you want to split to the members' TBAs.",
    }
}

crate::tool_params! {
    /// Args for the browser `disband_party` tool (`src/app/chat/tools/party.rs`).
    pub struct DisbandPartyParams: lenient {
        party_id: req_u64 min 0 = "The id of the party to disband. As the creator you may \
                    disband any live party; anyone may once its ttl has expired.",
    }
}

crate::tool_params! {
    /// Args for the browser `get_party` tool (`src/app/chat/tools/party.rs`).
    pub struct GetPartyParams: lenient {
        party_id: req_u64 min 0 = "The id of the party to inspect.",
    }
}

crate::tool_params! {
    /// Args for the browser `set_role` tool (`src/app/chat/tools/guild.rs`)
    /// — privilege escalation, confirm-gated. `role`'s `enum` constrains the
    /// MODEL only; the body's `GuildRole::parse` validation stays unchanged.
    pub struct SetRoleParams: lenient {
        guild_id: req_u64 min 0 = "The id of the guild you administer.",
        member: req_str = "Whose role to set — a raw 0x… address OR a subdomain \
                    name (resolved to that name's on-chain owner).",
        role: req_str enum ["member", "officer", "admin"] = "The rank to assign: \
                    \"member\", \"officer\", or \
                    \"admin\". (\"none\"/removal is not settable here.)",
        confirmation: opt_str = "Single-use confirmation code. OMIT (or pass \"\") on the \
                    first call — it returns a challenge code shown to the owner. Relay \
                    it, wait for the owner to TYPE the code in chat, then retry with it. \
                    Never invent it; only the platform issues it.",
    }
}

crate::tool_params! {
    /// Args for the browser `cast_vote` tool (`src/app/chat/tools/governance.rs`)
    /// — `support` uses the `req_bool` required-accessor (missing/wrong type
    /// errors the historical `"support (true/false) is required"`).
    pub struct CastVoteParams: lenient {
        proposal_id: req_u64 min 0 = "The id of the open proposal to vote on (from list_proposals).",
        support: req_bool = "true to vote FOR the proposal, false to vote AGAINST it.",
    }
}

crate::tool_params! {
    /// Args for the browser `schedule_task` tool (`src/app/chat/tools/misc.rs`)
    /// — off-chain job creation. `kind`'s `enum` constrains the MODEL only; the
    /// body's `== Some("agent")` classification (anything else = reminder)
    /// stays unchanged.
    pub struct ScheduleTaskParams: lenient {
        task: req_str = "What to do each fire. For a REMINDER, the note to push \
                    you. For an AGENT job, a self-contained prompt. Prefix with \
                    \"GOAL: \" for a goal-loop that ends early once done.",
        interval: req_str = "Delay / cadence: \"60s\", \"15m\", \"1h\" (a bare number \
                    = seconds; minimum 60s). For a ONE-SHOT (\"in 15 minutes\") set this \
                    to the delay and `runs` to 1.",
        runs: opt_u64 min 1 = "How many times to fire (default 1 — a single delayed \
                    task). Higher = a recurring job.",
        kind: opt_str enum ["reminder", "agent"] = "\"reminder\" (default) = just push you the task text \
                    (free, no agent run, no $LH). \"agent\" = run an agent each fire \
                    (bills your meter per run).",
        target: opt_str = "AGENT jobs only: the subdomain to run each fire \
                    (defaults to THIS agent). Ignored for a reminder.",
    }
}

crate::tool_params! {
    /// Args for the browser `attest` tool (`src/app/chat/tools/bounty.rs`)
    /// — durable one-shot reputation write, confirm-gated. `rating`'s table row
    /// covers the SCHEMA (`min 1 max 5`); its extraction stays INLINE in the
    /// body because the historical chain also coerces numeric strings
    /// (`"3"` → 3), which `req_u64`'s integer-only accessor must not replace.
    pub struct AttestParams: lenient {
        subject: req_str = "Who you are rating — a subdomain NAME (resolved to its \
                    on-chain tokenId) OR a raw numeric tokenId. Cannot be yourself.",
        rating: req_u64 min 1 max 5 = "Quality rating, an integer 1 (worst) to 5 (best).",
        work_ref: opt_str = "OPTIONAL bounty id this attestation is about (a decimal \
                    integer), so the rating ties to specific work. Omit for a general \
                    attestation.",
        confirmation: opt_str = "Single-use confirmation code. OMIT (or pass \"\") on the \
                    first call — it returns a challenge code shown to the owner. Relay \
                    it, wait for the owner to TYPE the code in chat, then retry with it. \
                    Never invent it; only the platform issues it.",
    }
}

crate::tool_params! {
    /// Args for the browser `dwell` tool (`src/app/chat/tools/misc.rs`) — the
    /// in-loop wait. The body reads the `Option<u64>` FIELD directly
    /// (`.unwrap_or(0).clamp(1, 300)` — dwell historically defaulted, never
    /// errored), so the generated required-accessor goes unused by design.
    pub struct DwellParams: lenient {
        seconds: req_u64 min 1 max 300 = "How long to wait, in seconds (1-300).",
    }
}

crate::tool_params! {
    /// Args for the browser `form_party` tool (`src/app/chat/tools/party.rs`)
    /// — propose a squad + fixed bps split. `shares` rides `opt_i64_array`
    /// for the SCHEMA only (items bounded 1..10000); its extraction stays
    /// INLINE in the body, which validates length/sum over the RAW array so
    /// the historical error order + messages survive.
    pub struct FormPartyParams: lenient {
        members: req_str_array = "The member identities — subdomain names (\"alice\") or \
                    token ids (\"#7\" / \"7\"). Each becomes a seat that must consent \
                    (via join_party) before the party can complete.",
        shares: opt_i64_array min 1 max 10000 = "OPTIONAL parallel array of each member's share in basis \
                    points (1..10000), in the SAME order as `members`; MUST sum to \
                    10000. Omit entirely for an equal split (remainder to the first \
                    member). If given, its length must match `members`.",
        ttl_hours: opt_str = "OPTIONAL lifetime in hours before the party expires \
                    (decimal). Omit for the 168h (7d) default.",
    }
}

crate::tool_params! {
    /// Args for the browser `evm_call` tool (`src/app/chat/tools/evm.rs`)
    /// — generic read-only eth_call. `args` rides `opt_str_array` for the
    /// SCHEMA only: the body's historical extraction STRINGIFIES non-string
    /// entries (`x.to_string()`, e.g. a numeric arg still encodes) where the
    /// lenient kind would drop them, so it stays inline.
    pub struct EvmCallParams: lenient {
        chain: req_str = "Which chain to call on (see evm_chains): ethereum, \
                    base, optimism, arbitrum, polygon, tempo.",
        to: req_str = "The 0x… contract address to call.",
        function_signature: req_str = "The view/pure function as a human signature, e.g. \
                    \"balanceOf(address)\", \"totalSupply()\", \"ownerOf(uint256)\". \
                    Supported arg types: address, bool, uintN/intN (decimal or 0x), \
                    bytes32. NO dynamic types (string/bytes/arrays) as args.",
        args: opt_str_array = "OPTIONAL args, one string per parameter, in order \
                    (e.g. [\"0xabc…\"] for balanceOf(address)). Omit for a no-arg call.",
    }
}

crate::tool_params! {
    /// Args for the browser `batch_create_subdomains` tool
    /// (`src/app/chat/tools/platform.rs`) — N registrations in ONE sponsored
    /// tx. The body keeps its own trim/empty/cap validation.
    pub struct BatchCreateSubdomainsParams: lenient {
        names: req_str_array = "Subdomain names to register in ONE tx, e.g. \
                    [\"alice\",\"bob\"] -> alice.localharness.xyz, \
                    bob.localharness.xyz. Each: 3-32 chars, lowercase letters, \
                    digits, hyphens. Already-taken or invalid names are skipped \
                    and reported back. Max 20 per call.",
    }
}

crate::tool_params! {
    /// Args for the browser `run_wasm_cli` tool (`src/app/chat/tools/misc.rs`)
    /// — the WASI-subset CLI sandbox. Fully table-parsed (`args` is the plain
    /// filter-map string-array extraction, exactly the historical chain).
    pub struct RunWasmCliParams: lenient {
        path: req_str = "OPFS path to a compiled `.wasm` CLI module — a \
                    wasm32-wasi COMMAND that exports `_start` (the standard output \
                    of `clang --target=wasm32-wasi`, `rustc --target wasm32-wasi`, \
                    TinyGo, etc.). The committed demo is \"examples/cli/hello.wasm\" \
                    if present in OPFS; otherwise point at a `.wasm` you placed in \
                    OPFS.",
        args: opt_str_array = "OPTIONAL command-line arguments passed as argv \
                    (argv[0] is a synthetic program name; these follow it).",
    }
}

crate::tool_params! {
    /// Args for the browser `found_company` tool
    /// (`src/app/chat/tools/company.rs`) — guild + role subdomains + personas
    /// in one confirm-gated founding. `roles` rides `opt_str_array` for the
    /// SCHEMA only; `resolve_roles` keeps reading the raw `Value` (its
    /// None/empty → seven-defaults arm predates the table).
    pub struct FoundCompanyParams: lenient {
        name: req_str = "The company's display name (also the guild name). A \
                    subdomain slug is derived from it for each role, e.g. \"acme\" → \
                    acme-exec, acme-pm, …",
        mission: req_str = "One or two sentences: what the company exists to do. \
                    Seeded into the shared backlog so every role works to the same plan.",
        roles: opt_str_array = "OPTIONAL list of roles to staff, e.g. [\"executive\", \
                    \"coder\", \"reviewer\"]. Omit for the seven defaults (executive, pm, \
                    coder, reviewer, accounting, hr, marketing). Unknown roles are \
                    slugified with a generic persona.",
        seed_treasury_lh: opt_str = "OPTIONAL $LH to deposit into the company treasury from YOUR \
                    wallet at founding, as a decimal string (\"10\", \"2.5\"). Omit or \
                    \"0\" to skip.",
        prefund_each_lh: opt_str = "OPTIONAL $LH to prefund EACH role's token-bound account \
                    (its own spendable wallet) with, as a decimal string. Total pulled = \
                    this × number of roles, from YOUR wallet. Omit or \"0\" to skip.",
        confirmation: opt_str = "Single-use confirmation code. OMIT (or pass \"\") on the \
                    first call — it returns a challenge code shown to the owner. State the \
                    company name, roles, and any $LH it will spend, ask the owner to TYPE \
                    the code in chat, then retry with it. Never invent it; only the \
                    platform issues it.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            ("CreateSubdomainParams", CreateSubdomainParams::schema()),
            ("CreateAndPublishAppParams", CreateAndPublishAppParams::schema()),
            ("PublishAppToParams", PublishAppToParams::schema()),
            ("EmbedAppParams", EmbedAppParams::schema()),
            ("PublishPublicFaceParams", PublishPublicFaceParams::schema()),
            ("ReleaseSubdomainParams", ReleaseSubdomainParams::schema()),
            ("DiscoverAgentsParams", DiscoverAgentsParams::schema()),
            ("QueryBalanceParams", QueryBalanceParams::schema()),
            ("PostBountyParams", PostBountyParams::schema()),
            ("SetPersonaParams", SetPersonaParams::schema()),
            ("RecordLessonParams", RecordLessonParams::schema()),
            ("NotifyParams", NotifyParams::schema()),
            ("ClaimBountyParams", ClaimBountyParams::schema()),
            ("SubmitResultParams", SubmitResultParams::schema()),
            ("AcceptResultParams", AcceptResultParams::schema()),
            ("CreateGuildParams", CreateGuildParams::schema()),
            ("InviteToGuildParams", InviteToGuildParams::schema()),
            ("FundGuildParams", FundGuildParams::schema()),
            ("SpendTreasuryParams", SpendTreasuryParams::schema()),
            ("ProposeMeasureParams", ProposeMeasureParams::schema()),
            ("ExecuteProposalParams", ExecuteProposalParams::schema()),
            ("ListProposalsParams", ListProposalsParams::schema()),
            ("WebFetchParams", WebFetchParams::schema()),
            ("SubmitFeedbackParams", SubmitFeedbackParams::schema()),
            ("SetLessonsParams", SetLessonsParams::schema()),
            ("CreateSkillParams", CreateSkillParams::schema()),
            ("DeleteSkillParams", DeleteSkillParams::schema()),
            ("CancelTaskParams", CancelTaskParams::schema()),
            ("ExecuteScriptParams", ExecuteScriptParams::schema()),
            ("SpawnRecursiveSubagentParams", SpawnRecursiveSubagentParams::schema()),
            ("CompanyStatusParams", CompanyStatusParams::schema()),
            ("SharedStateSetParams", SharedStateSetParams::schema()),
            ("SharedStateGetParams", SharedStateGetParams::schema()),
            ("EvmBalanceParams", EvmBalanceParams::schema()),
            ("ResolveEnsParams", ResolveEnsParams::schema()),
            ("ChallengeValidationParams", ChallengeValidationParams::schema()),
            ("ResolveValidationParams", ResolveValidationParams::schema()),
            ("ReclaimValidationParams", ReclaimValidationParams::schema()),
            ("GetValidationParams", GetValidationParams::schema()),
            ("JoinPartyParams", JoinPartyParams::schema()),
            ("FundPartyParams", FundPartyParams::schema()),
            ("CompletePartyParams", CompletePartyParams::schema()),
            ("DisbandPartyParams", DisbandPartyParams::schema()),
            ("GetPartyParams", GetPartyParams::schema()),
            ("SetRoleParams", SetRoleParams::schema()),
            ("CastVoteParams", CastVoteParams::schema()),
            ("ScheduleTaskParams", ScheduleTaskParams::schema()),
            ("AttestParams", AttestParams::schema()),
            ("DwellParams", DwellParams::schema()),
            ("FormPartyParams", FormPartyParams::schema()),
            ("EvmCallParams", EvmCallParams::schema()),
            ("BatchCreateSubdomainsParams", BatchCreateSubdomainsParams::schema()),
            ("RunWasmCliParams", RunWasmCliParams::schema()),
            ("FoundCompanyParams", FoundCompanyParams::schema()),
        ] {
            assert_gemini_safe(&schema, name);
        }
    }

    crate::tool_params! {
        /// Exercises the batch-5 (FINAL) kind additions: the three flat array
        /// kinds, including `min`/`max` routed into the ITEMS.
        struct ArrayKinds: lenient {
            tags: req_str_array = "A required string array.",
            extras: opt_str_array = "An optional string array.",
            weights: opt_i64_array min 1 max 10000 = "Bounded integer items.",
            plain: opt_i64_array = "Unbounded integer items.",
        }
    }

    /// Array kinds emit `"type":"array"` + a single-type `items` map, route
    /// `min`/`max` into the ITEMS (never the field), and stay Gemini-safe.
    /// The lenient parse is the historical `.as_array()` + `filter_map`
    /// element extraction: missing/wrong-typed → default (`[]` for the
    /// required kind, `None` for optionals), non-conforming ELEMENTS drop.
    #[test]
    fn array_kinds_shape_schema_and_lenient_parse() {
        let s = ArrayKinds::schema();
        assert_eq!(s["properties"]["tags"]["type"], "array");
        assert_eq!(s["properties"]["tags"]["items"], json!({"type": "string"}));
        assert!(s["properties"]["tags"].get("minimum").is_none());
        assert_eq!(s["properties"]["extras"]["items"], json!({"type": "string"}));
        assert_eq!(
            s["properties"]["weights"]["items"],
            json!({"type": "integer", "minimum": 1, "maximum": 10000})
        );
        assert!(s["properties"]["weights"].get("minimum").is_none());
        assert_eq!(s["properties"]["plain"]["items"], json!({"type": "integer"}));
        assert_eq!(s["required"], json!(["tags"]));

        let p = ArrayKinds::lenient(&json!({}));
        assert_eq!(p.tags, Vec::<String>::new());
        assert_eq!(p.extras, None);
        assert_eq!(p.weights, None);
        assert_eq!(p.plain, None);
        let p = ArrayKinds::lenient(&json!({
            "tags": ["a", 7, "b"],          // non-string element drops
            "extras": "not-an-array",       // wrong type → None
            "weights": [1, "2", 3.5, -4],   // non-i64 elements drop
        }));
        assert_eq!(p.tags, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(p.extras, None);
        assert_eq!(p.weights, Some(vec![1, -4]));
        // Out-of-bounds ELEMENTS pass the parse untouched — items `min`/`max`
        // constrain the MODEL; the tool body validates (form_party's sum/range
        // checks stay in the body).
        let p = ArrayKinds::lenient(&json!({"weights": [99999]}));
        assert_eq!(p.weights, Some(vec![99999]));
    }

    crate::tool_params! {
        /// Exercises the batch-4 kind additions: `max`, string `enum` on both
        /// required and optional strings, `opt_u64`, and `req_bool`.
        struct NewKinds: lenient {
            level: req_u64 min 1 max 5 = "A bounded required integer.",
            mode: req_str enum ["fast", "slow"] = "A required enum string.",
            hint: opt_str enum ["a", "b", "c"] = "An optional enum string.",
            budget: opt_u64 min 1 = "An optional u64.",
            yes: req_bool = "A required boolean.",
        }
    }

    /// `max` emits JSON-Schema `maximum`, `enum` emits the exact allowed-values
    /// array (on required AND optional strings), and both stay Gemini-safe.
    /// The LENIENT parse is untouched by the modifiers: enum fields extract as
    /// plain strings (out-of-enum values pass through to the tool body's own
    /// validation, exactly like the hand-written schemas behaved) and bounded
    /// integers are NOT clamped by the parse.
    #[test]
    fn max_and_enum_shape_the_schema_without_touching_the_parse() {
        let s = NewKinds::schema();
        assert_eq!(s["properties"]["level"]["minimum"], 1);
        assert_eq!(s["properties"]["level"]["maximum"], 5);
        assert_eq!(s["properties"]["mode"]["enum"], json!(["fast", "slow"]));
        assert_eq!(s["properties"]["mode"]["type"], "string");
        assert_eq!(s["properties"]["hint"]["enum"], json!(["a", "b", "c"]));
        assert_eq!(s["properties"]["budget"]["type"], "integer");
        assert_eq!(s["properties"]["budget"]["minimum"], 1);
        assert!(s["properties"]["budget"].get("maximum").is_none());
        assert_eq!(s["properties"]["yes"]["type"], "boolean");
        assert_eq!(s["required"], json!(["level", "mode", "yes"]));
        // Parse side: an out-of-enum string and an out-of-range integer both
        // pass through untouched — the schema constrains the MODEL, the body
        // validates.
        let p = NewKinds::lenient(&json!({"level": 99, "mode": "warp", "hint": "z"}));
        assert_eq!(p.level().unwrap(), 99);
        assert_eq!(p.mode, "warp");
        assert_eq!(p.hint.as_deref(), Some("z"));
    }

    /// `opt_u64`: full-u64-range optional integer (no u32 narrowing — the
    /// historical `.and_then(as_u64)` chains the kind replaces); `req_bool`:
    /// the required-accessor errors the tools' exact historical
    /// `"<field> (true/false) is required"` on missing/wrong type, while both
    /// real values round-trip.
    #[test]
    fn opt_u64_and_req_bool_match_the_historical_semantics() {
        let p = NewKinds::lenient(&json!({}));
        assert_eq!(p.budget, None);
        assert_eq!(p.yes().unwrap_err().to_string(), "yes (true/false) is required");
        assert!(NewKinds::lenient(&json!({"yes": "true"})).yes().is_err());
        assert!(NewKinds::lenient(&json!({"yes": 1})).yes().is_err());
        assert!(!NewKinds::lenient(&json!({"yes": false})).yes().unwrap());
        assert!(NewKinds::lenient(&json!({"yes": true})).yes().unwrap());
        assert_eq!(NewKinds::lenient(&json!({"budget": u64::MAX})).budget, Some(u64::MAX));
        assert_eq!(NewKinds::lenient(&json!({"budget": "7"})).budget, None);
    }

    crate::tool_params! {
        /// Exercises the lenient-mode REQUIRED integer kind (`req_u64`).
        struct ReqIntLenient: lenient {
            id: req_u64 min 0 = "A required integer id.",
            note: opt_str = "An optional string.",
        }
    }

    /// `req_u64`: the required-accessor ERRORS on missing/invalid with the
    /// chat tools' exact historical message, while 0 and u64::MAX stay REAL
    /// values — the tick-20 skip reason (default-0 conflated missing with a
    /// real id 0) is what this kind exists to fix.
    #[test]
    fn req_u64_errors_on_missing_or_invalid_instead_of_defaulting() {
        // Missing → the historical `<field> is required` error, verbatim.
        let p = ReqIntLenient::lenient(&json!({}));
        assert_eq!(p.id().unwrap_err().to_string(), "id is required");
        // Wrong-typed (string / bool / float / negative) → same error, exactly
        // like the old inline `.and_then(|v| v.as_u64())` failing.
        assert!(ReqIntLenient::lenient(&json!({"id": "7"})).id().is_err());
        assert!(ReqIntLenient::lenient(&json!({"id": true})).id().is_err());
        assert!(ReqIntLenient::lenient(&json!({"id": 1.5})).id().is_err());
        assert!(ReqIntLenient::lenient(&json!({"id": -1})).id().is_err());
        // 0 is a REAL id — must round-trip, never read as "missing".
        assert_eq!(ReqIntLenient::lenient(&json!({"id": 0})).id().unwrap(), 0);
        // Sibling optional fields keep their lenient defaults alongside it.
        let p = ReqIntLenient::lenient(&json!({"id": 3, "note": "n"}));
        assert_eq!((p.id().unwrap(), p.note.as_deref()), (3, Some("n")));
        // Full u64 range (the old arm never narrowed to u32).
        assert_eq!(
            ReqIntLenient::lenient(&json!({"id": u64::MAX})).id().unwrap(),
            u64::MAX
        );
        // Schema side: integer + `minimum` carried + in `required`.
        let s = ReqIntLenient::schema();
        assert_eq!(s["properties"]["id"]["type"], "integer");
        assert_eq!(s["properties"]["id"]["minimum"], 0);
        assert_eq!(s["required"], json!(["id"]));
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

    /// BYTE-IDENTITY for the chat-tools wave: each generated schema serializes
    /// byte-for-byte equal to the hand-written literal it replaced in
    /// `src/app/chat/tools/{platform,bounty,misc}.rs` (frozen verbatim below) —
    /// the same migration contract as `send_lh` above.
    #[test]
    fn chat_tool_schemas_are_byte_identical_to_the_frozen_originals() {
        let cases: [(&str, Value, Value); 12] = [
            ("create_subdomain", CreateSubdomainParams::schema(), json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Subdomain to register, e.g. \"alice\" \
                            becomes alice.localharness.xyz. 3-32 chars; lowercase \
                            letters, digits, and hyphens only."
                    },
                    "persona": {
                        "type": "string",
                        "description": "OPTIONAL system instruction / persona for the new \
                            agent — published on-chain as its system prompt (the persona \
                            that headless `call`s and the public face read). Omit to leave \
                            the default."
                    },
                    "prefund_lh": {
                        "type": "string",
                        "description": "OPTIONAL amount of $LH to prefund the new agent with, \
                            as a decimal string (\"5\", \"1.5\"). Transferred from YOUR \
                            wallet to the new subdomain's token-bound account (its own \
                            spendable wallet — used to pay other agents via x402). Omit, or \
                            pass \"0\", to skip. Must not exceed your $LH balance."
                    }
                },
                "required": ["name"]
            })),
            ("create_and_publish_app", CreateAndPublishAppParams::schema(), json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Subdomain to register, e.g. \"clock\" \
                            becomes clock.localharness.xyz. 3-32 chars; lowercase \
                            letters, digits, and hyphens only."
                    },
                    "source": {
                        "type": "string",
                        "description": "rustlite cartridge source — the SAME dialect as \
                            run_cartridge. Exports `fn frame(t: i32)` (animated) or \
                            `fn render()` and draws via `use host::display;`. This becomes \
                            the subdomain's fullscreen public face."
                    },
                    "persona": {
                        "type": "string",
                        "description": "OPTIONAL system instruction / persona for the new \
                            agent — published on-chain as its system prompt (read by \
                            headless `call`s). Omit to leave the default."
                    },
                    "prefund_lh": {
                        "type": "string",
                        "description": "OPTIONAL amount of $LH to prefund the new agent with, \
                            as a decimal string (\"5\", \"1.5\"). Transferred from YOUR \
                            wallet to the new subdomain's token-bound account (its own \
                            spendable wallet). Omit, or pass \"0\", to skip. Must not exceed \
                            your $LH balance."
                    }
                },
                "required": ["name", "source"]
            })),
            ("publish_app_to", PublishAppToParams::schema(), json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The subdomain to publish to — MUST be one you already \
                            own (e.g. \"clock\" → clock.localharness.xyz). Can be different from \
                            the subdomain you are currently on. To create a NEW subdomain, use \
                            create_and_publish_app instead."
                    },
                    "source": {
                        "type": "string",
                        "description": "rustlite cartridge source — the SAME dialect as \
                            run_cartridge / create_and_publish_app. Exports `fn frame(t: i32)` \
                            (animated) or `fn render()` and draws via `use host::display;`. \
                            Becomes the target subdomain's fullscreen public face."
                    },
                    "confirmation": {
                        "type": "string",
                        "description": "Single-use confirmation code. OMIT (or pass \"\") on the \
                            first call — it returns a challenge code shown to the owner. State \
                            which subdomain you will update, ask the owner to TYPE the code in \
                            chat, then retry with it. Never invent it; only the platform issues it."
                    }
                },
                "required": ["name", "source"]
            })),
            ("embed_app", EmbedAppParams::schema(), json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Subdomain whose published cartridge to embed, \
                            e.g. \"pong\" embeds pong.localharness.xyz's app inline."
                    }
                },
                "required": ["name"]
            })),
            ("publish_public_face", PublishPublicFaceParams::schema(), json!({
                "type": "object",
                "properties": {
                    "choice": {
                        "type": "string",
                        "description": "Which face to publish: \"app\" (compile + publish \
                            this device's local app.rl as a fullscreen cartridge), \
                            \"html\" (publish local index.html), or \"directory\" (a \
                            profile landing listing your sibling agents)."
                    }
                },
                "required": ["choice"]
            })),
            ("release_subdomain", ReleaseSubdomainParams::schema(), json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Subdomain to release/recycle — burns the NFT, frees the name."
                    },
                    "confirmation": {
                        "type": "string",
                        "description": "Single-use confirmation code. OMIT (or pass \"\") on the \
                            first call — it returns a challenge code that is shown to the owner. \
                            Relay it, wait for the owner to TYPE that code in chat, then retry \
                            with the code here. Never invent it; only the platform issues it."
                    }
                },
                "required": ["name"]
            })),
            ("discover_agents", DiscoverAgentsParams::schema(), json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What to look for — capabilities, topics, or \
                            keywords matched (case-insensitively) against agent names \
                            and personas. Several keywords are ORed and ranked by \
                            overlap (e.g. \"solidity audit security\"). \
                            Empty returns recent agents."
                    }
                },
                "required": ["query"]
            })),
            ("query_balance", QueryBalanceParams::schema(), json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "an agent NAME (e.g. \"binglescan\") or a 0x address"
                    }
                },
                "required": ["target"]
            })),
            ("post_bounty", PostBountyParams::schema(), json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The task to be done — a clear, self-contained \
                            description of what a claimant must deliver to earn the reward."
                    },
                    "reward_lh": {
                        "type": "string",
                        "description": "Reward in $LH, as a decimal string (\"5\", \"1.5\"). \
                            Escrowed from YOUR wallet when the bounty is posted; paid out to \
                            the claimant when you accept their result. Must be > 0."
                    },
                    "ttl_hours": {
                        "type": "string",
                        "description": "OPTIONAL lifetime in hours before the bounty expires \
                            (decimal). Omit for the 24h default."
                    }
                },
                "required": ["task", "reward_lh"]
            })),
            ("set_persona", SetPersonaParams::schema(), json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "The new system instruction / persona for YOURSELF — \
                            your role, personality, and constraints. This becomes both your \
                            on-chain published persona AND your local custom system prompt; it \
                            takes effect on your next session. Keep it focused."
                    }
                },
                "required": ["text"]
            })),
            ("record_lesson", RecordLessonParams::schema(), json!({
                "type": "object",
                "properties": {
                    "lesson": {
                        "type": "string",
                        "description": "ONE short lesson (a single sentence, max 240 chars) \
                            learned from a REAL error, failed tool call, or user correction. \
                            Make it concrete and actionable (what to do differently next \
                            time), not a description of what happened."
                    }
                },
                "required": ["lesson"]
            })),
            ("notify", NotifyParams::schema(), json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Short notification title, e.g. \"timer done\" or \
                            \"new message from dex\"."
                    },
                    "body": {
                        "type": "string",
                        "description": "Optional body text shown under the title. Keep it \
                            to a sentence."
                    },
                    "vibrate": {
                        "type": "boolean",
                        "description": "Also vibrate the device (mobile only; silently \
                            ignored where unsupported)."
                    },
                    "to": {
                        "type": "string",
                        "description": "CROSS-AGENT: deliver to ANOTHER agent's \
                            notification inbox instead of this device — the target \
                            subdomain name, e.g. \"krafto\". Routed via the platform \
                            proxy (costs the per-request $LH like a model call); the \
                            push title is stamped with YOUR identity so the recipient \
                            sees who pinged them. Omit for a local notification on \
                            this device."
                    }
                },
                "required": ["title"]
            })),
        ];
        for (name, generated, frozen) in cases {
            assert_eq!(generated.to_string(), frozen.to_string(), "schema drift: {name}");
        }
    }

    /// Lenient parity for the chat-tools wave: the extraction feeds each tool's
    /// unchanged body validation the same values the old inline
    /// `.get().and_then().unwrap_or()` chains produced, including the edges
    /// (missing args, wrong types, empty/whitespace optionals).
    #[test]
    fn chat_tool_lenient_matches_the_old_inline_extraction() {
        // create_subdomain: missing/wrong-typed → defaults; want_persona /
        // want_prefund logic sees identical values.
        let p = CreateSubdomainParams::lenient(&json!({"name": 7, "persona": true}));
        assert_eq!((p.name.as_str(), p.persona, p.prefund_lh), ("", None, None));
        let p = CreateSubdomainParams::lenient(
            &json!({"name": " alice ", "persona": "  ", "prefund_lh": "0"}),
        );
        assert_eq!(p.name.trim(), "alice");
        // old: persona.map(|p| !p.trim().is_empty()).unwrap_or(false) → false
        assert!(!p.persona.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false));
        // old: prefund.map(|p| { let t = p.trim(); !t.is_empty() && t != "0" }) → false
        let t = p.prefund_lh.as_deref().unwrap().trim();
        assert!(t.is_empty() || t == "0");

        // create_and_publish_app / publish_app_to: req_str "" default keeps the
        // body's empty-source error path reachable exactly as before.
        let p = CreateAndPublishAppParams::lenient(&json!({"name": "x"}));
        assert_eq!(p.source, "");
        let p = PublishAppToParams::lenient(&json!({"name": "x", "source": "s"}));
        assert!(!p.confirmation.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false));
        let p = PublishAppToParams::lenient(&json!({"confirmation": "c0de"}));
        assert!(p.confirmation.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false));

        // Single-required-string tools: missing OR wrong-typed → "".
        assert_eq!(EmbedAppParams::lenient(&json!({})).name, "");
        assert_eq!(PublishPublicFaceParams::lenient(&json!({"choice": 3})).choice, "");
        assert_eq!(PublishPublicFaceParams::lenient(&json!({"choice": "APP "})).choice, "APP ");
        assert_eq!(DiscoverAgentsParams::lenient(&json!({})).query, "");
        assert_eq!(QueryBalanceParams::lenient(&json!({"target": " k "})).target, " k ");
        assert_eq!(SetPersonaParams::lenient(&json!({"text": 1})).text, "");
        assert_eq!(RecordLessonParams::lenient(&json!({})).lesson, "");
        let p = ReleaseSubdomainParams::lenient(&json!({"name": " z "}));
        assert_eq!(p.name.trim().to_string(), "z");
        assert_eq!(p.confirmation, None);

        // post_bounty: ttl_hours missing/blank → the body's 24h default arm.
        let p = PostBountyParams::lenient(&json!({"task": " t ", "reward_lh": "1.5"}));
        assert_eq!((p.task.trim(), p.reward_lh.trim()), ("t", "1.5"));
        assert!(p.ttl_hours.is_none());
        let p = PostBountyParams::lenient(&json!({"ttl_hours": "  "}));
        // old match arm: Some(s) if !s.trim().is_empty() → falls to the 24h default
        assert!(!matches!(p.ttl_hours.as_deref(), Some(s) if !s.trim().is_empty()));

        // notify: body defaults to "", vibrate wrong-typed → false, `to` empty
        // string filtered out (local path), populated `to` normalized by the body.
        let p = NotifyParams::lenient(&json!({"title": "hi", "vibrate": 1, "to": ""}));
        assert_eq!(p.body.as_deref().unwrap_or(""), "");
        assert!(!p.vibrate.unwrap_or(false));
        assert_eq!(
            p.to.map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()),
            None
        );
        let p = NotifyParams::lenient(&json!({"to": " Krafto ", "vibrate": true}));
        assert_eq!(
            p.to.map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()),
            Some("krafto".to_string())
        );
        assert!(p.vibrate.unwrap_or(false));
    }

    /// BYTE-IDENTITY for the SECOND chat-tools wave (the `req_u64` unlock):
    /// each generated schema serializes byte-for-byte equal to the hand-written
    /// literal it replaced in `src/app/chat/tools/{bounty,guild,governance,misc}.rs`
    /// (frozen verbatim below) — the same migration contract as wave 1.
    #[test]
    fn chat_tool_wave2_schemas_are_byte_identical_to_the_frozen_originals() {
        let cases: [(&str, Value, Value); 12] = [
            ("claim_bounty", ClaimBountyParams::schema(), json!({
                "type": "object",
                "properties": {
                    "bounty_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the open bounty to claim (from \
                            discover_bounties / the bounty board)."
                    }
                },
                "required": ["bounty_id"]
            })),
            ("submit_result", SubmitResultParams::schema(), json!({
                "type": "object",
                "properties": {
                    "bounty_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the bounty you previously claimed."
                    },
                    "result": {
                        "type": "string",
                        "description": "Your deliverable / result for the bounty — the work \
                            product the poster will review before accepting + paying out."
                    }
                },
                "required": ["bounty_id", "result"]
            })),
            ("accept_result", AcceptResultParams::schema(), json!({
                "type": "object",
                "properties": {
                    "bounty_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of a bounty YOU posted whose submitted result \
                            you want to accept (releases the escrowed $LH to the claimant)."
                    }
                },
                "required": ["bounty_id"]
            })),
            ("create_guild", CreateGuildParams::schema(), json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Display name for the guild (a short label for the org)."
                    }
                },
                "required": ["name"]
            })),
            ("invite_to_guild", InviteToGuildParams::schema(), json!({
                "type": "object",
                "properties": {
                    "guild_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the guild you administer."
                    },
                    "member": {
                        "type": "string",
                        "description": "Who to invite — a raw 0x… address OR a subdomain name \
                            (resolved to that name's on-chain owner)."
                    }
                },
                "required": ["guild_id", "member"]
            })),
            ("fund_guild", FundGuildParams::schema(), json!({
                "type": "object",
                "properties": {
                    "guild_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the guild to fund."
                    },
                    "amount_lh": {
                        "type": "string",
                        "description": "Amount of $LH to contribute, as a decimal string \
                            (\"5\", \"1.5\"). Pulled from YOUR wallet into the shared treasury. \
                            Must be > 0."
                    }
                },
                "required": ["guild_id", "amount_lh"]
            })),
            ("spend_treasury", SpendTreasuryParams::schema(), json!({
                "type": "object",
                "properties": {
                    "guild_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the guild whose treasury to spend from."
                    },
                    "to": {
                        "type": "string",
                        "description": "Recipient — a raw 0x… address OR a subdomain name \
                            (resolved to that name's on-chain owner)."
                    },
                    "amount_lh": {
                        "type": "string",
                        "description": "Amount of $LH to pay out, as a decimal string. Must be > 0."
                    },
                    "memo": {
                        "type": "string",
                        "description": "OPTIONAL note recorded with the payment (what it's for)."
                    },
                    "confirmation": {
                        "type": "string",
                        "description": "Single-use confirmation code. OMIT (or pass \"\") on the \
                            first call — it returns a challenge code shown to the owner. Relay \
                            it, wait for the owner to TYPE the code in chat, then retry with it. \
                            Never invent it; only the platform issues it."
                    }
                },
                "required": ["guild_id", "to", "amount_lh"]
            })),
            ("propose_measure", ProposeMeasureParams::schema(), json!({
                "type": "object",
                "properties": {
                    "guild_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the guild whose treasury the proposal would spend from."
                    },
                    "to": {
                        "type": "string",
                        "description": "Spend recipient if the proposal passes — a raw 0x… \
                            address OR a subdomain name (resolved to that name's on-chain owner)."
                    },
                    "amount_lh": {
                        "type": "string",
                        "description": "Amount of $LH the proposal would pay out from the \
                            treasury, as a decimal string (\"5\", \"1.5\"). Must be > 0."
                    },
                    "memo": {
                        "type": "string",
                        "description": "OPTIONAL description of what the spend is for — recorded \
                            on-chain so voters know what they're approving."
                    },
                    "period_hours": {
                        "type": "string",
                        "description": "OPTIONAL voting window in hours (decimal). Omit for the \
                            48h default. Members can vote until the deadline; only then can a \
                            passing proposal be executed."
                    }
                },
                "required": ["guild_id", "to", "amount_lh"]
            })),
            ("execute_proposal", ExecuteProposalParams::schema(), json!({
                "type": "object",
                "properties": {
                    "proposal_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of a passed proposal whose voting deadline has \
                            elapsed (executing it pays out the treasury spend)."
                    }
                },
                "required": ["proposal_id"]
            })),
            ("list_proposals", ListProposalsParams::schema(), json!({
                "type": "object",
                "properties": {
                    "guild_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the guild whose proposals to list."
                    }
                },
                "required": ["guild_id"]
            })),
            ("web_fetch", WebFetchParams::schema(), json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Absolute https:// URL to fetch — a docs page, \
                            GitHub README (use raw.githubusercontent.com for raw \
                            content), or JSON API endpoint. http://, private/internal \
                            hosts, and raw-IP targets are rejected."
                    }
                },
                "required": ["url"]
            })),
            ("submit_feedback", SubmitFeedbackParams::schema(), json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "The feedback text. Filed off-chain with full \
                            conversation + device/settings context. (If the owner enabled \
                            on-chain mirroring, the SHORT note is also written on-chain, where \
                            a 2048-byte cap applies — summarize rather than pasting a long report.)"
                    }
                },
                "required": ["text"]
            })),
        ];
        for (name, generated, frozen) in cases {
            assert_eq!(generated.to_string(), frozen.to_string(), "schema drift: {name}");
        }
    }

    /// Lenient parity for wave 2: the extraction (plus the `req_u64` accessor)
    /// feeds each tool's unchanged body validation the same values — and the
    /// same errors, with the same messages — the old inline chains produced.
    #[test]
    fn chat_tool_wave2_lenient_matches_the_old_inline_extraction() {
        // Bounty trio: missing/wrong-typed bounty_id errors with the tools'
        // EXACT historical message; 0 stays a real id (the tick-20 skip reason).
        let p = ClaimBountyParams::lenient(&json!({}));
        assert_eq!(p.bounty_id().unwrap_err().to_string(), "bounty_id is required");
        assert_eq!(ClaimBountyParams::lenient(&json!({"bounty_id": 0})).bounty_id().unwrap(), 0);
        let p = SubmitResultParams::lenient(&json!({"bounty_id": "3", "result": " r "}));
        assert!(p.bounty_id().is_err()); // string id fails, as `.as_u64()` did
        assert_eq!(p.result, " r "); // body trims, exactly as before
        let p = SubmitResultParams::lenient(&json!({"bounty_id": 7}));
        assert_eq!((p.bounty_id().unwrap(), p.result.as_str()), (7, ""));
        assert_eq!(
            AcceptResultParams::lenient(&json!({})).bounty_id().unwrap_err().to_string(),
            "bounty_id is required"
        );

        // Guild tools: ids share the accessor; strings/optionals keep the
        // historical defaults the bodies re-validate.
        assert_eq!(CreateGuildParams::lenient(&json!({"name": 9})).name, "");
        let p = InviteToGuildParams::lenient(&json!({"member": " Alice "}));
        assert_eq!(p.guild_id().unwrap_err().to_string(), "guild_id is required");
        assert_eq!(p.member, " Alice "); // body trims
        let p = FundGuildParams::lenient(&json!({"guild_id": 2, "amount_lh": " 1.5 "}));
        assert_eq!((p.guild_id().unwrap(), p.amount_lh.trim()), (2, "1.5"));
        let p = SpendTreasuryParams::lenient(&json!({"guild_id": 1, "to": "bob", "amount_lh": "2"}));
        assert_eq!(p.memo.as_deref().unwrap_or(""), "");
        // old: .map(|s| !s.trim().is_empty()).unwrap_or(false) → still false
        assert!(!p.confirmation.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false));

        // Governance: period_hours blank → the body's 48h default arm.
        let p = ProposeMeasureParams::lenient(&json!({"guild_id": 4, "period_hours": "  "}));
        assert_eq!(p.guild_id().unwrap(), 4);
        assert!(!matches!(p.period_hours.as_deref(), Some(s) if !s.trim().is_empty()));
        assert_eq!(
            ExecuteProposalParams::lenient(&json!({"proposal_id": true}))
                .proposal_id()
                .unwrap_err()
                .to_string(),
            "proposal_id is required"
        );
        assert_eq!(ListProposalsParams::lenient(&json!({"guild_id": 11})).guild_id().unwrap(), 11);

        // misc: "" defaults keep the bodies' empty-check error paths reachable.
        assert_eq!(WebFetchParams::lenient(&json!({})).url, "");
        assert_eq!(WebFetchParams::lenient(&json!({"url": " https://x "})).url.trim(), "https://x");
        assert_eq!(SubmitFeedbackParams::lenient(&json!({"text": 1})).text, "");
        assert_eq!(SubmitFeedbackParams::lenient(&json!({"text": " ok "})).text.trim(), "ok");
    }

    /// BYTE-IDENTITY for the THIRD chat-tools wave (the straggler sweep):
    /// each generated schema serializes byte-for-byte equal to the hand-written
    /// literal it replaced in `src/app/chat/tools/{misc,company,room,evm,
    /// validation,party}.rs` (frozen verbatim below) — the same migration
    /// contract as waves 1 and 2.
    #[test]
    fn chat_tool_wave3_schemas_are_byte_identical_to_the_frozen_originals() {
        let cases: [(&str, Value, Value); 20] = [
            ("set_lessons", SetLessonsParams::schema(), json!({
                "type": "object",
                "properties": {
                    "lessons": {
                        "type": "string",
                        "description": "The FULL replacement lessons list — one lesson \
                            per line, newline-separated, max 10 lines of max 240 chars \
                            each. This REPLACES every existing lesson, so it must \
                            still contain (verbatim or strengthened) every lesson \
                            worth keeping; anything omitted is forgotten."
                    }
                },
                "required": ["lessons"]
            })),
            ("create_skill", CreateSkillParams::schema(), json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "A short handle for the skill (e.g. \"summarize\", \
                            \"daily-standup\"), max 48 chars. Re-using an existing name \
                            REPLACES that skill's instructions."
                    },
                    "instructions": {
                        "type": "string",
                        "description": "The reusable instruction/prompt fragment that defines \
                            what the skill does when invoked — a focused recipe (max 600 \
                            chars). Make it self-contained and actionable."
                    }
                },
                "required": ["name", "instructions"]
            })),
            ("delete_skill", DeleteSkillParams::schema(), json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The name of the skill to remove (use list_skills to \
                            see your defined skills)."
                    }
                },
                "required": ["name"]
            })),
            ("cancel_task", CancelTaskParams::schema(), json!({
                "type": "object",
                "properties": {
                    "job_id": {
                        "type": "string",
                        "description": "The id of the scheduled job to cancel — the `job_id` \
                            string schedule_task returned."
                    }
                },
                "required": ["job_id"]
            })),
            ("execute_script", ExecuteScriptParams::schema(), json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "A bashlite script to run over your OPFS sandbox. \
                            Supports: variables (x=value, x=$(cmd)), $VAR / ${VAR} / $? \
                            interpolation, pipes (a | b | c), && / || short-circuit \
                            chaining, if/elif/else/fi, for NAME in WORDS; do …; done \
                            (`for f in $(…)` splits on whitespace), while …; do …; done, \
                            [ … ] tests (string =/!=/-z/-n, int -eq/-ne/-lt/-le/-gt/-ge, \
                            file -e/-f/-d PATH), \
                            command substitution $(…), and `run FILE.bl` / `source FILE.bl` \
                            to compose another script. Builtins (filesystem): \
                            echo, cd, pwd, ls, cat, grep PATTERN (literal substring; \
                            -i/-v/-c), find [path] [-name GLOB] [-type f|d], wc [-l|-w|-c] \
                            (of stdin), head/tail [-n N] (first/last N stdin lines), \
                            mkdir, write/create PATH CONTENT (create-only — \
                            refuses to overwrite), true/false. NO value-moving / lh-* \
                            commands, NO networking, NO process spawning."
                    }
                },
                "required": ["source"]
            })),
            ("spawn_recursive_subagent", SpawnRecursiveSubagentParams::schema(), json!({
                "type": "object",
                "properties": {
                    "system_instructions": {
                        "type": "string",
                        "description": "System prompt for the subagent — describes its persona, \
                            scope, and any constraints. Often \"you are a focused worker \
                            that does X and returns just the result\"."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The user message to send to the subagent."
                    }
                },
                "required": ["system_instructions", "prompt"]
            })),
            ("company_status", CompanyStatusParams::schema(), json!({
                "type": "object",
                "properties": {
                    "company": {
                        "type": "string",
                        "description": "Which company/guild to report on — a numeric guild id \
                            (e.g. \"67\") OR a guild display name you belong to."
                    }
                },
                "required": ["company"]
            })),
            ("shared_state_set", SharedStateSetParams::schema(), json!({
                "type": "object",
                "properties": {
                    "key": {
                        "type": "string",
                        "description": "The key to write in the shared volume, e.g. \
                            \"task_status\" or \"worker_1/progress\"."
                    },
                    "value": {
                        "type": "string",
                        "description": "The value to store under `key` (UTF-8 text)."
                    }
                },
                "required": ["key", "value"]
            })),
            ("shared_state_get", SharedStateGetParams::schema(), json!({
                "type": "object",
                "properties": {
                    "key": {
                        "type": "string",
                        "description": "The key to read from the shared volume."
                    }
                },
                "required": ["key"]
            })),
            ("evm_balance", EvmBalanceParams::schema(), json!({
                "type": "object",
                "properties": {
                    "chain": {
                        "type": "string",
                        "description": "Which chain: ethereum, base, optimism, arbitrum, \
                            polygon, or tempo (aliases: eth/mainnet, op, arb, matic). Call \
                            evm_chains() if unsure."
                    },
                    "address": {
                        "type": "string",
                        "description": "The 0x… account address to read the balance OF."
                    },
                    "token": {
                        "type": "string",
                        "description": "OPTIONAL ERC-20 token contract address (0x…). Given \
                            → returns that token's balanceOf(address) with best-effort \
                            symbol + decimals; omitted → the chain's NATIVE coin balance."
                    }
                },
                "required": ["chain", "address"]
            })),
            ("resolve_ens", ResolveEnsParams::schema(), json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "An ENS name to resolve, e.g. \"vitalik.eth\"."
                    }
                },
                "required": ["name"]
            })),
            ("challenge_validation", ChallengeValidationParams::schema(), json!({
                "type": "object",
                "properties": {
                    "validation_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the OPEN validation to challenge (from \
                            get_validation)."
                    }
                },
                "required": ["validation_id"]
            })),
            ("resolve_validation", ResolveValidationParams::schema(), json!({
                "type": "object",
                "properties": {
                    "validation_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the CHALLENGED validation to resolve."
                    },
                    "winner": {
                        "type": "string",
                        "description": "Who wins, paid BOTH stakes: \"validator\" (the original \
                            verdict stands) or \"challenger\" (the counter-verdict stands)."
                    }
                },
                "required": ["validation_id", "winner"]
            })),
            ("reclaim_validation", ReclaimValidationParams::schema(), json!({
                "type": "object",
                "properties": {
                    "validation_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the validation to refund (its window must \
                            have passed)."
                    }
                },
                "required": ["validation_id"]
            })),
            ("get_validation", GetValidationParams::schema(), json!({
                "type": "object",
                "properties": {
                    "validation_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the validation to read."
                    }
                },
                "required": ["validation_id"]
            })),
            ("join_party", JoinPartyParams::schema(), json!({
                "type": "object",
                "properties": {
                    "party_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the party to consent to (from \
                            discover_parties / get_party)."
                    }
                },
                "required": ["party_id"]
            })),
            ("fund_party", FundPartyParams::schema(), json!({
                "type": "object",
                "properties": {
                    "party_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the party whose pot to fund."
                    },
                    "amount_lh": {
                        "type": "string",
                        "description": "Amount of $LH to contribute, as a decimal string (\"5\", \
                            \"1.5\"). Pulled from YOUR wallet into the party pot; refunded exactly \
                            on disband/expiry, split to the members on complete. Must be > 0."
                    }
                },
                "required": ["party_id", "amount_lh"]
            })),
            ("complete_party", CompletePartyParams::schema(), json!({
                "type": "object",
                "properties": {
                    "party_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of a party YOU formed (Active, all seats consented) \
                            whose pot you want to split to the members' TBAs."
                    }
                },
                "required": ["party_id"]
            })),
            ("disband_party", DisbandPartyParams::schema(), json!({
                "type": "object",
                "properties": {
                    "party_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the party to disband. As the creator you may \
                            disband any live party; anyone may once its ttl has expired."
                    }
                },
                "required": ["party_id"]
            })),
            ("get_party", GetPartyParams::schema(), json!({
                "type": "object",
                "properties": {
                    "party_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the party to inspect."
                    }
                },
                "required": ["party_id"]
            })),
        ];
        for (name, generated, frozen) in cases {
            assert_eq!(generated.to_string(), frozen.to_string(), "schema drift: {name}");
        }
    }

    /// Lenient parity for wave 3: the extraction (plus the `req_u64` accessor)
    /// feeds each tool's unchanged body validation the same values — and the
    /// same errors, with the same messages — the old inline chains produced.
    #[test]
    fn chat_tool_wave3_lenient_matches_the_old_inline_extraction() {
        // set_lessons / create_skill / delete_skill: "" defaults keep the
        // bodies' empty-check error paths reachable; bodies trim, as before.
        assert_eq!(SetLessonsParams::lenient(&json!({})).lessons, "");
        assert_eq!(SetLessonsParams::lenient(&json!({"lessons": 3})).lessons, "");
        assert_eq!(SetLessonsParams::lenient(&json!({"lessons": "a\nb"})).lessons, "a\nb");
        let p = CreateSkillParams::lenient(&json!({"name": " s ", "instructions": true}));
        assert_eq!((p.name.trim(), p.instructions.trim()), ("s", ""));
        assert_eq!(DeleteSkillParams::lenient(&json!({})).name, "");

        // cancel_task: the old `.map(trim).filter(!empty).ok_or_else(..)` chain
        // errored on missing/wrong-typed/blank — the "" default + the body's
        // trim/empty check reproduce exactly that (and pass through real ids).
        assert!(CancelTaskParams::lenient(&json!({})).job_id.trim().is_empty());
        assert!(CancelTaskParams::lenient(&json!({"job_id": 7})).job_id.trim().is_empty());
        assert!(CancelTaskParams::lenient(&json!({"job_id": "  "})).job_id.trim().is_empty());
        assert_eq!(
            CancelTaskParams::lenient(&json!({"job_id": " j-1 "})).job_id.trim(),
            "j-1"
        );

        // execute_script / spawn_recursive_subagent: "" defaults preserved.
        assert_eq!(ExecuteScriptParams::lenient(&json!({})).source, "");
        assert_eq!(ExecuteScriptParams::lenient(&json!({"source": "ls | wc -l"})).source, "ls | wc -l");
        let p = SpawnRecursiveSubagentParams::lenient(&json!({"prompt": 9}));
        assert_eq!((p.system_instructions.as_str(), p.prompt.as_str()), ("", ""));
        let p = SpawnRecursiveSubagentParams::lenient(&json!({"system_instructions": "s", "prompt": "p"}));
        assert_eq!((p.system_instructions.as_str(), p.prompt.as_str()), ("s", "p"));

        // company_status / shared-state: bodies trim keys and re-validate.
        assert_eq!(CompanyStatusParams::lenient(&json!({"company": 67})).company, "");
        assert_eq!(CompanyStatusParams::lenient(&json!({"company": " 67 "})).company.trim(), "67");
        let p = SharedStateSetParams::lenient(&json!({"key": " k ", "value": 1}));
        assert_eq!((p.key.trim(), p.value.as_str()), ("k", ""));
        assert_eq!(SharedStateGetParams::lenient(&json!({})).key, "");

        // evm tools: token empty/whitespace filters to the native-balance arm,
        // exactly like the old `.map(str::trim).filter(!empty)`.
        let p = EvmBalanceParams::lenient(&json!({"chain": " base ", "address": "0xA", "token": " "}));
        assert_eq!((p.chain.trim(), p.address.trim()), ("base", "0xA"));
        assert_eq!(p.token.as_deref().map(str::trim).filter(|s| !s.is_empty()), None);
        let p = EvmBalanceParams::lenient(&json!({"token": "0xT"}));
        assert_eq!(p.token.as_deref().map(str::trim).filter(|s| !s.is_empty()), Some("0xT"));
        assert_eq!(ResolveEnsParams::lenient(&json!({})).name, "");

        // validation ids: the accessor errors with the tools' EXACT historical
        // message on missing/wrong type; 0 stays a real id.
        assert_eq!(
            ChallengeValidationParams::lenient(&json!({})).validation_id().unwrap_err().to_string(),
            "validation_id is required"
        );
        assert_eq!(
            ChallengeValidationParams::lenient(&json!({"validation_id": 0})).validation_id().unwrap(),
            0
        );
        let p = ResolveValidationParams::lenient(&json!({"validation_id": 4, "winner": " Validator "}));
        assert_eq!(p.validation_id().unwrap(), 4);
        assert_eq!(p.winner.trim().to_ascii_lowercase(), "validator");
        assert!(ResolveValidationParams::lenient(&json!({"validation_id": "4"}))
            .validation_id()
            .is_err());
        assert_eq!(
            ReclaimValidationParams::lenient(&json!({"validation_id": true}))
                .validation_id()
                .unwrap_err()
                .to_string(),
            "validation_id is required"
        );
        assert_eq!(GetValidationParams::lenient(&json!({"validation_id": 11})).validation_id().unwrap(), 11);

        // party ids share the accessor ("party_id is required"); fund_party's
        // amount keeps the "" default its parse body re-validates.
        assert_eq!(
            JoinPartyParams::lenient(&json!({})).party_id().unwrap_err().to_string(),
            "party_id is required"
        );
        let p = FundPartyParams::lenient(&json!({"party_id": 2, "amount_lh": " 1.5 "}));
        assert_eq!((p.party_id().unwrap(), p.amount_lh.trim()), (2, "1.5"));
        assert_eq!(FundPartyParams::lenient(&json!({"party_id": 2})).amount_lh, "");
        assert_eq!(CompletePartyParams::lenient(&json!({"party_id": 0})).party_id().unwrap(), 0);
        assert_eq!(
            DisbandPartyParams::lenient(&json!({"party_id": 1.5})).party_id().unwrap_err().to_string(),
            "party_id is required"
        );
        assert_eq!(GetPartyParams::lenient(&json!({"party_id": 9})).party_id().unwrap(), 9);
    }

    /// BYTE-IDENTITY for the FOURTH chat-tools wave (the `max`/`enum`/`req_bool`
    /// unlock): each generated schema serializes byte-for-byte equal to the
    /// hand-written literal it replaced in `src/app/chat/tools/{guild,
    /// governance,misc,bounty}.rs` (frozen verbatim below) — the same migration
    /// contract as waves 1-3.
    #[test]
    fn chat_tool_wave4_schemas_are_byte_identical_to_the_frozen_originals() {
        let cases: [(&str, Value, Value); 5] = [
            ("set_role", SetRoleParams::schema(), json!({
                "type": "object",
                "properties": {
                    "guild_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the guild you administer."
                    },
                    "member": {
                        "type": "string",
                        "description": "Whose role to set — a raw 0x… address OR a subdomain \
                            name (resolved to that name's on-chain owner)."
                    },
                    "role": {
                        "type": "string",
                        "enum": ["member", "officer", "admin"],
                        "description": "The rank to assign: \"member\", \"officer\", or \
                            \"admin\". (\"none\"/removal is not settable here.)"
                    },
                    "confirmation": {
                        "type": "string",
                        "description": "Single-use confirmation code. OMIT (or pass \"\") on the \
                            first call — it returns a challenge code shown to the owner. Relay \
                            it, wait for the owner to TYPE the code in chat, then retry with it. \
                            Never invent it; only the platform issues it."
                    }
                },
                "required": ["guild_id", "member", "role"]
            })),
            ("cast_vote", CastVoteParams::schema(), json!({
                "type": "object",
                "properties": {
                    "proposal_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The id of the open proposal to vote on (from list_proposals)."
                    },
                    "support": {
                        "type": "boolean",
                        "description": "true to vote FOR the proposal, false to vote AGAINST it."
                    }
                },
                "required": ["proposal_id", "support"]
            })),
            ("schedule_task", ScheduleTaskParams::schema(), json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "What to do each fire. For a REMINDER, the note to push \
                            you. For an AGENT job, a self-contained prompt. Prefix with \
                            \"GOAL: \" for a goal-loop that ends early once done."
                    },
                    "interval": {
                        "type": "string",
                        "description": "Delay / cadence: \"60s\", \"15m\", \"1h\" (a bare number \
                            = seconds; minimum 60s). For a ONE-SHOT (\"in 15 minutes\") set this \
                            to the delay and `runs` to 1."
                    },
                    "runs": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "How many times to fire (default 1 — a single delayed \
                            task). Higher = a recurring job."
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["reminder", "agent"],
                        "description": "\"reminder\" (default) = just push you the task text \
                            (free, no agent run, no $LH). \"agent\" = run an agent each fire \
                            (bills your meter per run)."
                    },
                    "target": {
                        "type": "string",
                        "description": "AGENT jobs only: the subdomain to run each fire \
                            (defaults to THIS agent). Ignored for a reminder."
                    }
                },
                "required": ["task", "interval"]
            })),
            ("attest", AttestParams::schema(), json!({
                "type": "object",
                "properties": {
                    "subject": {
                        "type": "string",
                        "description": "Who you are rating — a subdomain NAME (resolved to its \
                            on-chain tokenId) OR a raw numeric tokenId. Cannot be yourself."
                    },
                    "rating": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 5,
                        "description": "Quality rating, an integer 1 (worst) to 5 (best)."
                    },
                    "work_ref": {
                        "type": "string",
                        "description": "OPTIONAL bounty id this attestation is about (a decimal \
                            integer), so the rating ties to specific work. Omit for a general \
                            attestation."
                    },
                    "confirmation": {
                        "type": "string",
                        "description": "Single-use confirmation code. OMIT (or pass \"\") on the \
                            first call — it returns a challenge code shown to the owner. Relay \
                            it, wait for the owner to TYPE the code in chat, then retry with it. \
                            Never invent it; only the platform issues it."
                    }
                },
                "required": ["subject", "rating"]
            })),
            ("dwell", DwellParams::schema(), json!({
                "type": "object",
                "properties": {
                    "seconds": {
                        "type": "integer",
                        "description": "How long to wait, in seconds (1-300).",
                        "minimum": 1,
                        "maximum": 300
                    }
                },
                "required": ["seconds"]
            })),
        ];
        for (name, generated, frozen) in cases {
            assert_eq!(generated.to_string(), frozen.to_string(), "schema drift: {name}");
        }
    }

    /// Lenient parity for wave 4: the extraction (plus the `req_u64`/`req_bool`
    /// accessors) feeds each tool's unchanged body validation the same values —
    /// and the same errors, with the same messages — the old inline chains
    /// produced. The enum'd string fields stay PLAIN extractions: out-of-enum
    /// values reach the bodies' own validation exactly as before.
    #[test]
    fn chat_tool_wave4_lenient_matches_the_old_inline_extraction() {
        // set_role: guild_id accessor message matches the old inline
        // `ok_or_else`; role/member keep the "" defaults the body re-validates
        // (an out-of-enum role still reaches GuildRole::parse).
        let p = SetRoleParams::lenient(&json!({"member": " Alice ", "role": "emperor"}));
        assert_eq!(p.guild_id().unwrap_err().to_string(), "guild_id is required");
        assert_eq!(p.member.trim(), "Alice"); // body trims
        assert_eq!(p.role.trim(), "emperor"); // body's GuildRole::parse rejects
        let p = SetRoleParams::lenient(&json!({"guild_id": 0, "role": " Officer "}));
        assert_eq!(p.guild_id().unwrap(), 0); // 0 stays a real id
        assert_eq!(p.role.trim(), "Officer");
        assert!(!p.confirmation.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false));

        // cast_vote: both accessors error the EXACT historical messages;
        // support=false is a REAL vote, never conflated with "missing".
        let p = CastVoteParams::lenient(&json!({}));
        assert_eq!(p.proposal_id().unwrap_err().to_string(), "proposal_id is required");
        assert_eq!(p.support().unwrap_err().to_string(), "support (true/false) is required");
        let p = CastVoteParams::lenient(&json!({"proposal_id": 3, "support": false}));
        assert_eq!((p.proposal_id().unwrap(), p.support().unwrap()), (3, false));
        assert!(CastVoteParams::lenient(&json!({"support": "true"})).support().is_err());

        // schedule_task: task/interval "" defaults keep the body's error arms;
        // runs reproduces `.map(|r| r.max(1) as u32).unwrap_or(1)`; kind's
        // `== Some("agent")` classification (anything else = reminder) and
        // target's trim/lowercase/filter chain see identical values.
        let p = ScheduleTaskParams::lenient(&json!({}));
        assert!(p.task.trim().is_empty());
        assert_eq!(p.interval, "");
        assert_eq!(p.runs.map(|r| r.max(1) as u32).unwrap_or(1), 1);
        assert_ne!(p.kind.as_deref(), Some("agent")); // → "reminder"
        let p = ScheduleTaskParams::lenient(
            &json!({"task": " t ", "interval": "15m", "runs": 0, "kind": "AGENT", "target": " Bob "}),
        );
        assert_eq!(p.task.trim(), "t");
        assert_eq!(p.runs.map(|r| r.max(1) as u32).unwrap_or(1), 1); // 0 → max(1)
        assert_ne!(p.kind.as_deref(), Some("agent")); // case-sensitive, as before
        assert_eq!(
            p.target.as_deref().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()),
            Some("bob".to_string())
        );
        let p = ScheduleTaskParams::lenient(&json!({"kind": "agent", "runs": 5, "target": "  "}));
        assert_eq!(p.kind.as_deref(), Some("agent"));
        assert_eq!(p.runs.map(|r| r.max(1) as u32).unwrap_or(1), 5);
        assert_eq!(
            p.target.as_deref().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()),
            None // blank target → the body's current_name fallback, as before
        );

        // attest: subject/work_ref/confirmation ride the table; the rating
        // COERCION (int OR numeric string) deliberately stays inline in the
        // body — the table's Option<u64> only mirrors the integer case.
        let p = AttestParams::lenient(&json!({"subject": " dex ", "rating": "4"}));
        assert_eq!(p.subject.trim(), "dex");
        assert_eq!(p.rating, None); // string "4" → the body's inline parse
        assert_eq!(p.work_ref.as_deref().unwrap_or("").trim(), "");
        assert!(!p.confirmation.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false));
        let p = AttestParams::lenient(&json!({"rating": 5, "work_ref": " #12 "}));
        assert_eq!(p.rating, Some(5));
        assert_eq!(p.work_ref.as_deref().unwrap_or("").trim(), "#12");

        // dwell: the body reads the FIELD (`.unwrap_or(0).clamp(1, 300)`) —
        // missing/wrong-typed → 0 → clamped to 1, oversized → 300, exactly the
        // historical `.and_then(as_u64).unwrap_or(0).clamp(1, 300)`.
        assert_eq!(DwellParams::lenient(&json!({})).seconds.unwrap_or(0).clamp(1, 300), 1);
        assert_eq!(DwellParams::lenient(&json!({"seconds": "9"})).seconds.unwrap_or(0).clamp(1, 300), 1);
        assert_eq!(DwellParams::lenient(&json!({"seconds": 30})).seconds.unwrap_or(0).clamp(1, 300), 30);
        assert_eq!(DwellParams::lenient(&json!({"seconds": 9999})).seconds.unwrap_or(0).clamp(1, 300), 300);
    }

    /// BYTE-IDENTITY for the FIFTH — FINAL — chat-tools wave (the array-kinds
    /// unlock): each generated schema serializes byte-for-byte equal to the
    /// hand-written literal it replaced in `src/app/chat/tools/{party,evm,
    /// platform,misc,company}.rs` (frozen verbatim below) — the same migration
    /// contract as waves 1-4.
    #[test]
    fn chat_tool_wave5_schemas_are_byte_identical_to_the_frozen_originals() {
        let cases: [(&str, Value, Value); 5] = [
            ("form_party", FormPartyParams::schema(), json!({
                "type": "object",
                "properties": {
                    "members": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "The member identities — subdomain names (\"alice\") or \
                            token ids (\"#7\" / \"7\"). Each becomes a seat that must consent \
                            (via join_party) before the party can complete."
                    },
                    "shares": {
                        "type": "array",
                        "items": { "type": "integer", "minimum": 1, "maximum": 10000 },
                        "description": "OPTIONAL parallel array of each member's share in basis \
                            points (1..10000), in the SAME order as `members`; MUST sum to \
                            10000. Omit entirely for an equal split (remainder to the first \
                            member). If given, its length must match `members`."
                    },
                    "ttl_hours": {
                        "type": "string",
                        "description": "OPTIONAL lifetime in hours before the party expires \
                            (decimal). Omit for the 168h (7d) default."
                    }
                },
                "required": ["members"]
            })),
            ("evm_call", EvmCallParams::schema(), json!({
                "type": "object",
                "properties": {
                    "chain": {
                        "type": "string",
                        "description": "Which chain to call on (see evm_chains): ethereum, \
                            base, optimism, arbitrum, polygon, tempo."
                    },
                    "to": {
                        "type": "string",
                        "description": "The 0x… contract address to call."
                    },
                    "function_signature": {
                        "type": "string",
                        "description": "The view/pure function as a human signature, e.g. \
                            \"balanceOf(address)\", \"totalSupply()\", \"ownerOf(uint256)\". \
                            Supported arg types: address, bool, uintN/intN (decimal or 0x), \
                            bytes32. NO dynamic types (string/bytes/arrays) as args."
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "OPTIONAL args, one string per parameter, in order \
                            (e.g. [\"0xabc…\"] for balanceOf(address)). Omit for a no-arg call."
                    }
                },
                "required": ["chain", "to", "function_signature"]
            })),
            ("batch_create_subdomains", BatchCreateSubdomainsParams::schema(), json!({
                "type": "object",
                "properties": {
                    "names": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Subdomain names to register in ONE tx, e.g. \
                            [\"alice\",\"bob\"] -> alice.localharness.xyz, \
                            bob.localharness.xyz. Each: 3-32 chars, lowercase letters, \
                            digits, hyphens. Already-taken or invalid names are skipped \
                            and reported back. Max 20 per call."
                    }
                },
                "required": ["names"]
            })),
            ("run_wasm_cli", RunWasmCliParams::schema(), json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "OPFS path to a compiled `.wasm` CLI module — a \
                            wasm32-wasi COMMAND that exports `_start` (the standard output \
                            of `clang --target=wasm32-wasi`, `rustc --target wasm32-wasi`, \
                            TinyGo, etc.). The committed demo is \"examples/cli/hello.wasm\" \
                            if present in OPFS; otherwise point at a `.wasm` you placed in \
                            OPFS."
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "OPTIONAL command-line arguments passed as argv \
                            (argv[0] is a synthetic program name; these follow it)."
                    }
                },
                "required": ["path"]
            })),
            ("found_company", FoundCompanyParams::schema(), json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The company's display name (also the guild name). A \
                            subdomain slug is derived from it for each role, e.g. \"acme\" → \
                            acme-exec, acme-pm, …"
                    },
                    "mission": {
                        "type": "string",
                        "description": "One or two sentences: what the company exists to do. \
                            Seeded into the shared backlog so every role works to the same plan."
                    },
                    "roles": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "OPTIONAL list of roles to staff, e.g. [\"executive\", \
                            \"coder\", \"reviewer\"]. Omit for the seven defaults (executive, pm, \
                            coder, reviewer, accounting, hr, marketing). Unknown roles are \
                            slugified with a generic persona."
                    },
                    "seed_treasury_lh": {
                        "type": "string",
                        "description": "OPTIONAL $LH to deposit into the company treasury from YOUR \
                            wallet at founding, as a decimal string (\"10\", \"2.5\"). Omit or \
                            \"0\" to skip."
                    },
                    "prefund_each_lh": {
                        "type": "string",
                        "description": "OPTIONAL $LH to prefund EACH role's token-bound account \
                            (its own spendable wallet) with, as a decimal string. Total pulled = \
                            this × number of roles, from YOUR wallet. Omit or \"0\" to skip."
                    },
                    "confirmation": {
                        "type": "string",
                        "description": "Single-use confirmation code. OMIT (or pass \"\") on the \
                            first call — it returns a challenge code shown to the owner. State the \
                            company name, roles, and any $LH it will spend, ask the owner to TYPE \
                            the code in chat, then retry with it. Never invent it; only the \
                            platform issues it."
                    }
                },
                "required": ["name", "mission"]
            })),
        ];
        for (name, generated, frozen) in cases {
            assert_eq!(generated.to_string(), frozen.to_string(), "schema drift: {name}");
        }
    }

    /// Lenient parity for wave 5: the extraction feeds each tool's unchanged
    /// body validation the same values the old inline chains produced. The
    /// deliberately-INLINE fields (form_party.shares, evm_call.args,
    /// found_company.roles) are exercised here only as far as the tables
    /// carry them; their raw-`Value` parses stay in the tool bodies.
    #[test]
    fn chat_tool_wave5_lenient_matches_the_old_inline_extraction() {
        // form_party: members historically trimmed + dropped empties INSIDE
        // the extraction; the table yields the raw strings and the body now
        // applies the identical trim/filter — same final vec, same empty-error.
        let p = FormPartyParams::lenient(&json!({"members": [" alice ", 7, "", "#9"]}));
        let members_arg: Vec<String> = p
            .members
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(members_arg, vec!["alice".to_string(), "#9".to_string()]);
        assert!(FormPartyParams::lenient(&json!({})).members.is_empty());
        // ttl_hours blank → the body's 168h default arm, exactly as before.
        let p = FormPartyParams::lenient(&json!({"ttl_hours": "  "}));
        assert!(!matches!(p.ttl_hours.as_deref(), Some(s) if !s.trim().is_empty()));

        // evm_call: the three strings ride the table ("" defaults keep the
        // body's error arms); `args` deliberately does NOT — the historical
        // chain STRINGIFIES non-string entries instead of dropping them.
        let p = EvmCallParams::lenient(&json!({"chain": " base ", "to": "0xC"}));
        assert_eq!((p.chain.trim(), p.to.trim(), p.function_signature.as_str()), ("base", "0xC", ""));

        // batch_create_subdomains: raw strings out, body trims/filters/caps.
        let p = BatchCreateSubdomainsParams::lenient(&json!({"names": [" a ", "", 3, "bob"]}));
        let requested: Vec<String> = p
            .names
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(requested, vec!["a".to_string(), "bob".to_string()]);
        assert!(BatchCreateSubdomainsParams::lenient(&json!({})).names.is_empty());

        // run_wasm_cli: path "" default keeps the empty-error reachable; argv
        // is the plain filter-map — non-strings drop, exactly as before.
        let p = RunWasmCliParams::lenient(&json!({"args": ["-v", 2, "x"]}));
        assert_eq!(p.path, "");
        assert_eq!(p.args, Some(vec!["-v".to_string(), "x".to_string()]));
        assert_eq!(RunWasmCliParams::lenient(&json!({})).args, None);

        // found_company: strings match the historical
        // `.and_then(as_str).unwrap_or("").trim()` / optional chains.
        let p = FoundCompanyParams::lenient(&json!({
            "name": " Acme ", "mission": 7, "seed_treasury_lh": " 10 ",
        }));
        assert_eq!((p.name.trim(), p.mission.trim()), ("Acme", ""));
        assert_eq!(p.seed_treasury_lh.as_deref().map(str::trim), Some("10"));
        // prefund: `.map(trim.to_string()).filter(!empty && != "0")` parity.
        let p = FoundCompanyParams::lenient(&json!({"prefund_each_lh": " 0 "}));
        let prefund = p
            .prefund_each_lh
            .as_deref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "0");
        assert_eq!(prefund, None);
        assert!(!FoundCompanyParams::lenient(&json!({}))
            .confirmation
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false));
    }
}
