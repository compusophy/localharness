//! Pure core for `batch_create_subdomains`' `{name, source}` ARRAY BATCH
//! (telemetry #85's remaining ask, following the #86 one-tool precedent:
//! a `source` is what makes an item an app). Native-testable per the
//! `turn_flow`/`relay_chunk` hoisting pattern — the tool body itself is
//! `browser-app`+wasm32-gated and outside every default check.
//!
//! Owns the three pieces that must not drift silently:
//! - the `names`/`items` UNION parse (the wire contract: EITHER the legacy
//!   `names: [string]` OR `items: [{name, source?}]`, never both),
//! - the COMPILE-FIRST partition (every provided source compiles through the
//!   SAME rustlite path `create_subdomain` uses BEFORE any registration tx —
//!   a bad cartridge fails ITS item up front and never spends),
//! - the per-item STATUS fold mapping the chunked-registration outcome
//!   ([`crate::relay_chunk::BatchFold`], positions over the REGISTRATION
//!   SUBSET) back onto the full item list.
//!
//! The hand-written `input_schema` also lives here ([`input_schema`]) so
//! plain `cargo test` guards its Gemini-safety — the tool left the
//! `tool_params!` table because `items` is an array of NESTED OBJECTS (the
//! `batch_send_lh` resident shape the macro grammar deliberately can't
//! express).

/// One requested batch item: a subdomain `name` (trimmed, as requested — NOT
/// normalized) plus an optional rustlite `source` that also publishes the
/// compiled cartridge as that subdomain's app. A blank/whitespace source
/// means "no app" (the same rule as `create_subdomain`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive] // pub SDK surface — a new field must not be semver-breaking (the BatchFold rule)
pub struct BatchItem {
    pub name: String,
    pub source: Option<String>,
}

/// Parse the tool args' `names`/`items` UNION into one item list.
///
/// - `items: [{name, source?}]` — the register-AND-publish shape. A
///   non-object element or an empty `name` is a hard error (a source-bearing
///   item is spend-relevant; silently dropping one would lose work).
/// - `names: [string]` — the legacy shape, kept lenient verbatim (non-string
///   elements drop, blanks drop after trim — the historical
///   `req_str_array` + trim/filter extraction).
/// - Both non-empty → error (the schema says one OR the other); neither →
///   error.
/// - DUPLICATES are a hard error in EITHER shape: two items sanitizing to the
///   same registered name (`crate::subdomain::sanitize` — so "App" and "app!"
///   collide) would double-publish one app and misreport `count`; the error
///   names BOTH indices.
pub fn parse_items(args: &serde_json::Value) -> Result<Vec<BatchItem>, String> {
    let names = args.get("names").and_then(|v| v.as_array());
    let items = args.get("items").and_then(|v| v.as_array());
    if names.map(|a| !a.is_empty()).unwrap_or(false)
        && items.map(|a| !a.is_empty()).unwrap_or(false)
    {
        return Err("pass `names` OR `items`, not both".into());
    }
    if let Some(arr) = items.filter(|a| !a.is_empty()) {
        let mut out = Vec::with_capacity(arr.len());
        for (i, v) in arr.iter().enumerate() {
            let Some(obj) = v.as_object() else {
                return Err(format!("items[{i}] must be an object {{ name, source? }}"));
            };
            let name = obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if name.is_empty() {
                return Err(format!("items[{i}].name is empty"));
            }
            let source = obj
                .get("source")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            out.push(BatchItem { name, source });
        }
        reject_duplicates(&out)?;
        return Ok(out);
    }
    let out: Vec<BatchItem> = names
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| BatchItem { name: s.to_string(), source: None })
                .collect()
        })
        .unwrap_or_default();
    if out.is_empty() {
        return Err(
            "names cannot be empty — pass `names` ([\"a\", …]) or `items` \
             ([{name, source?}, …])"
                .into(),
        );
    }
    reject_duplicates(&out)?;
    Ok(out)
}

/// Duplicate CLEANED names are a HARD error, not a silent collapse: both
/// copies would pass the availability pre-check, the second register would
/// no-op or revert, and a source item would publish TWICE while the result
/// claimed `count: 1` against two urls. Keyed on `crate::subdomain::sanitize`
/// (what the events path actually registers). Items sanitizing to `""` are
/// exempt — each reports its own invalid-name outcome instead of a bogus
/// "duplicate of another garbage name".
fn reject_duplicates(items: &[BatchItem]) -> Result<(), String> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (j, it) in items.iter().enumerate() {
        let c = crate::subdomain::sanitize(&it.name);
        if c.is_empty() {
            continue;
        }
        if let Some(first) = seen.insert(c.clone(), j) {
            return Err(format!(
                "duplicate name: item {first} (\"{}\") and item {j} (\"{}\") both \
                 register \"{c}\" — list each name ONCE",
                items[first].name, it.name
            ));
        }
    }
    Ok(())
}

/// COMPILE-FIRST: compile one item's rustlite `source` through the same path
/// `create_subdomain` uses (`crate::rustlite::compile` + the store size cap),
/// with the same error text — the full diagnostic rendering (LH code +
/// line/col + caret) so the agent can fix the exact spot. Runs BEFORE any
/// registration tx; a failure fails THAT item up front and removes it from
/// the registration set.
pub fn compile_source(source: &str, max_wasm: usize) -> Result<Vec<u8>, String> {
    let wasm = crate::rustlite::compile(source)
        .map_err(|e| format!("compile failed: {}", e.render(source)))?;
    if wasm.len() > max_wasm {
        return Err(format!(
            "app wasm too large to publish: {} bytes (max {max_wasm})",
            wasm.len()
        ));
    }
    Ok(wasm)
}

/// Where one item stands BEFORE the chunked registration runs (the compile +
/// ownership pre-pass routes each item here).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive] // pub SDK surface — a new plan state must not be semver-breaking (the BatchFold rule)
pub enum ItemPlan {
    /// Goes into the chunked registration set (source-less items always;
    /// source items whose name is free).
    Register,
    /// The caller ALREADY OWNS the name (source given) — publish in place,
    /// no registration tx (the `create_subdomain` update-in-place mirror).
    UpdateInPlace,
    /// The source failed to compile — nothing on-chain is attempted; carries
    /// the compiler diagnostic.
    CompileFailed(String),
    /// Skipped before any attempt (invalid name / owned by someone else with
    /// a source — its app must NOT publish); carries the reason.
    PreSkipped(String),
}

/// Final per-item status after the chunked registration fold (publish
/// outcomes layer on top in the tool body — publishing is off-chain and its
/// failure never changes these registration claims).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive] // pub SDK surface — a new outcome state must not be semver-breaking (the BatchFold rule)
pub enum ItemState {
    /// The item's chunk landed AND its cleaned name registered this call.
    Registered,
    /// Publish-only (already owned by the caller); no tx was needed.
    UpdateInPlace,
    /// Failed compile-first; carries the diagnostic.
    CompileFailed(String),
    /// Did not register (pre-skipped, or its chunk landed but the name was
    /// filtered as taken/invalid); carries the reason.
    Skipped(String),
    /// Its chunk's tx FAILED — the name did NOT register.
    Failed,
    /// Its chunk's receipt TIMED OUT — the tx MAY still land.
    Unconfirmed,
    /// Never tried (the batch stopped early).
    Unattempted,
}

/// The registration subset: item indices with [`ItemPlan::Register`], in
/// order — the list the chunk partition runs over. `BatchFold` positions
/// index into THIS list.
pub fn register_set(plan: &[ItemPlan]) -> Vec<usize> {
    plan.iter()
        .enumerate()
        .filter(|(_, p)| matches!(p, ItemPlan::Register))
        .map(|(i, _)| i)
        .collect()
}

/// Map the chunked-registration outcome back onto the FULL item list.
///
/// `fold` indexes positions of [`register_set`]`(plan)`; `cleaned[i]` is item
/// i's sanitized name (what the events path would register); `registered` is
/// the cleaned names that actually registered across the landed chunks. A
/// landed position whose cleaned name did NOT register was filtered by the
/// events path (taken/invalid) → [`ItemState::Skipped`], the legacy
/// semantics.
///
/// # Panics
///
/// The fold must MATCH the plan: every position in `fold` must index into
/// [`register_set`]`(plan)` (i.e. the fold came from
/// [`crate::relay_chunk::fold_outcomes`] over
/// [`crate::relay_chunk::chunk_ranges`] of exactly the register set's
/// length), and `cleaned` must cover every item (`cleaned.len() >=
/// plan.len()`). A mismatched fold or a short `cleaned` panics on the
/// out-of-bounds index rather than silently misattributing an on-chain
/// outcome to the wrong item.
pub fn item_states(
    plan: &[ItemPlan],
    fold: &crate::relay_chunk::BatchFold,
    cleaned: &[String],
    registered: &[String],
) -> Vec<ItemState> {
    let reg = register_set(plan);
    let mut states: Vec<ItemState> = plan
        .iter()
        .map(|p| match p {
            // Provisional: overwritten below for every position the fold
            // covers; a position it doesn't cover was never attempted.
            ItemPlan::Register => ItemState::Unattempted,
            ItemPlan::UpdateInPlace => ItemState::UpdateInPlace,
            ItemPlan::CompileFailed(e) => ItemState::CompileFailed(e.clone()),
            ItemPlan::PreSkipped(r) => ItemState::Skipped(r.clone()),
        })
        .collect();
    for &pos in &fold.landed {
        let idx = reg[pos];
        states[idx] = if registered.iter().any(|n| n == &cleaned[idx]) {
            ItemState::Registered
        } else {
            ItemState::Skipped("taken or invalid — skipped at registration".into())
        };
    }
    for &pos in &fold.failed {
        states[reg[pos]] = ItemState::Failed;
    }
    for &pos in &fold.unconfirmed {
        states[reg[pos]] = ItemState::Unconfirmed;
    }
    states
}

/// The hand-written `input_schema` for `batch_create_subdomains` — hoisted
/// here (out of the wasm-gated tool file) so plain `cargo test` guards it.
/// Gemini-safe by test below: nested-object array items are fine (the
/// `batch_send_lh` precedent); no unions / `additionalProperties` / `$ref`.
pub fn input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "names": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Name-only registrations, e.g. [\"alice\",\"bob\"] -> \
                    alice.localharness.xyz, bob.localharness.xyz. Each: 3-32 chars, \
                    lowercase letters, digits, hyphens. Already-taken or invalid names \
                    are skipped and reported back; listing the SAME name twice is a \
                    hard error. Give `names` OR `items`, never both. \
                    More than 7 are split across multiple sponsored txs automatically; \
                    at most 28 per call — split a bigger request into separate calls."
            },
            "items": {
                "type": "array",
                "description": "Register-AND-publish batch: one { name, source? } object \
                    per subdomain. An item WITH a rustlite `source` ALSO publishes it as \
                    that subdomain's app in the same call (the create_subdomain `source` \
                    behavior, batched). Every source compiles FIRST — a compile failure \
                    fails THAT item before any registration and spends nothing. A name \
                    you ALREADY OWN is updated in place (published, no re-register, no \
                    fee); the SAME name twice is a hard error. Give `items` OR `names`, \
                    never both; at most 28 items per call.",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "The subdomain name (3-32 chars, lowercase \
                                letters, digits, hyphens)."
                        },
                        "source": {
                            "type": "string",
                            "description": "OPTIONAL rustlite cartridge source (the SAME \
                                dialect as run_cartridge / create_subdomain). Compiled up \
                                front; published OFF-CHAIN (free, no gas) as the \
                                subdomain's fullscreen public face once its name \
                                registers (or in place when you already own it)."
                        }
                    },
                    "required": ["name"]
                }
            },
            "confirmation": {
                "type": "string",
                "description": "Single-use confirmation code. OMIT (or pass \"\") on \
                    the first call — it returns a challenge code shown to the owner \
                    (each registration costs real $LH on mainnet). List the names, ask \
                    the owner to TYPE the code in chat, then retry with it. Never \
                    invent it; only the platform issues it."
            }
        },
        "required": []
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay_chunk::{fold_outcomes, ChunkOutcome};
    use serde_json::json;

    #[test]
    fn parse_legacy_names_keeps_the_lenient_extraction_verbatim() {
        // Trim, drop blanks, drop non-strings — the historical req_str_array
        // + trim/filter chain.
        let items = parse_items(&json!({"names": [" a ", "", 3, "bob"]})).unwrap();
        assert_eq!(
            items,
            vec![
                BatchItem { name: "a".into(), source: None },
                BatchItem { name: "bob".into(), source: None },
            ]
        );
        // Missing / empty / all-blank → the legacy empty error.
        for args in [json!({}), json!({"names": []}), json!({"names": ["", "  "]})] {
            assert!(parse_items(&args).unwrap_err().contains("names cannot be empty"));
        }
    }

    #[test]
    fn parse_items_union_accepts_sources_and_rejects_bad_shapes() {
        let items = parse_items(&json!({
            "items": [
                {"name": " app1 ", "source": "fn f() -> i32 { 1 }"},
                {"name": "bare"},
                {"name": "blank-src", "source": "   "},
            ]
        }))
        .unwrap();
        assert_eq!(items[0].name, "app1");
        assert_eq!(items[0].source.as_deref(), Some("fn f() -> i32 { 1 }"));
        assert_eq!(items[1], BatchItem { name: "bare".into(), source: None });
        // Blank source = "no app", the create_subdomain rule.
        assert_eq!(items[2].source, None);
        // Spend-relevant shapes hard-error instead of silently dropping.
        assert!(parse_items(&json!({"items": ["oops"]})).unwrap_err().contains("items[0]"));
        assert!(parse_items(&json!({"items": [{"source": "x"}]}))
            .unwrap_err()
            .contains("items[0].name"));
        // Both non-empty → error; an EMPTY other arm doesn't block.
        assert!(parse_items(&json!({"names": ["a"], "items": [{"name": "b"}]}))
            .unwrap_err()
            .contains("not both"));
        assert!(parse_items(&json!({"names": [], "items": [{"name": "b"}]})).is_ok());
    }

    #[test]
    fn parse_rejects_duplicate_cleaned_names_naming_both_indices() {
        // Same literal twice (either arm) — a silent collapse would register
        // once but report/publish twice.
        let err = parse_items(&json!({"names": ["alpha", "beta", "alpha"]})).unwrap_err();
        assert!(err.contains("item 0") && err.contains("item 2"), "{err}");
        assert!(err.contains("\"alpha\""), "{err}");
        // Different literals SANITIZING to the same registered name collide
        // too ("App" / "app!" → "app") — the double-publish shape.
        let err = parse_items(&json!({
            "items": [{"name": "App", "source": "fn f() -> i32 { 1 }"}, {"name": "app!"}]
        }))
        .unwrap_err();
        assert!(err.contains("item 0") && err.contains("item 1"), "{err}");
        assert!(err.contains("\"app\""), "{err}");
        // Names sanitizing to "" never collide with each other — each gets
        // its own invalid-name outcome downstream instead.
        assert!(parse_items(&json!({"names": ["!!!", "???", "ok-name"]})).is_ok());
    }

    #[test]
    fn compile_first_partitions_good_bad_and_oversize_sources() {
        // A good source compiles to real wasm through the same rustlite path.
        let wasm = compile_source("fn helper(n: i32) -> i32 { n + 1 }", usize::MAX).unwrap();
        assert!(!wasm.is_empty());
        // A bad source fails with the FULL diagnostic rendering (the
        // create_subdomain error shape the agent knows how to fix).
        let err = compile_source("fn {", usize::MAX).unwrap_err();
        assert!(err.starts_with("compile failed: "), "{err}");
        // The store size cap applies at compile time — before any tx.
        let err = compile_source("fn helper(n: i32) -> i32 { n + 1 }", 4).unwrap_err();
        assert!(err.contains("too large"), "{err}");
    }

    #[test]
    fn item_states_scatters_the_subset_fold_onto_the_full_list() {
        // items: 0=register 1=compile_failed 2=register 3=in_place
        //        4=pre_skipped 5=register 6=register
        let plan = vec![
            ItemPlan::Register,
            ItemPlan::CompileFailed("compile failed: LH0001".into()),
            ItemPlan::Register,
            ItemPlan::UpdateInPlace,
            ItemPlan::PreSkipped("owned by 0xabc, not you".into()),
            ItemPlan::Register,
            ItemPlan::Register,
        ];
        assert_eq!(register_set(&plan), vec![0, 2, 5, 6]);
        let cleaned: Vec<String> =
            ["zero", "one", "two", "three", "four", "five", "six"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        // Chunked over the 4-item subset: chunk A = positions 0..2 landed,
        // chunk B = position 2..3 failed, position 3 never attempted. "two"
        // (position 1) landed but was filtered by the events path (taken).
        let ranges = vec![0..2, 2..3, 3..4];
        let fold = fold_outcomes(
            &ranges,
            &[ChunkOutcome::Landed("0xa".into()), ChunkOutcome::Failed("boom".into())],
        );
        let states = item_states(&plan, &fold, &cleaned, &["zero".to_string()]);
        assert_eq!(states[0], ItemState::Registered);
        assert_eq!(states[1], ItemState::CompileFailed("compile failed: LH0001".into()));
        assert_eq!(
            states[2],
            ItemState::Skipped("taken or invalid — skipped at registration".into())
        );
        assert_eq!(states[3], ItemState::UpdateInPlace);
        assert_eq!(states[4], ItemState::Skipped("owned by 0xabc, not you".into()));
        assert_eq!(states[5], ItemState::Failed);
        assert_eq!(states[6], ItemState::Unattempted);
    }

    #[test]
    fn item_states_marks_unconfirmed_receipt_timeouts() {
        let plan = vec![ItemPlan::Register, ItemPlan::Register];
        let fold = fold_outcomes(&[0..1, 1..2], &[ChunkOutcome::Unconfirmed("0xbeef".into())]);
        let states = item_states(
            &plan,
            &fold,
            &["a".to_string(), "b".to_string()],
            &[],
        );
        assert_eq!(states, vec![ItemState::Unconfirmed, ItemState::Unattempted]);
    }

    /// The hand-written schema must stay GEMINI-SAFE (a violation 400s and
    /// bricks all chat — `src/builtins/CLAUDE.md`): single `type` strings,
    /// no unions, none of the meta keys. Nested-object array items are fine.
    #[test]
    fn input_schema_is_gemini_safe_and_carries_the_union() {
        fn walk(v: &serde_json::Value) {
            match v {
                serde_json::Value::Object(map) => {
                    for banned in
                        ["oneOf", "anyOf", "allOf", "additionalProperties", "$ref", "$schema"]
                    {
                        assert!(map.get(banned).is_none(), "banned key {banned}");
                    }
                    if let Some(t) = map.get("type") {
                        assert!(t.is_string(), "union type: {t}");
                    }
                    map.values().for_each(walk);
                }
                serde_json::Value::Array(a) => a.iter().for_each(walk),
                _ => {}
            }
        }
        let s = input_schema();
        walk(&s);
        // The union: BOTH arms present, NEITHER required (one or the other).
        assert_eq!(s["required"], json!([]));
        assert_eq!(s["properties"]["names"]["type"], "array");
        assert_eq!(s["properties"]["items"]["items"]["type"], "object");
        assert_eq!(s["properties"]["items"]["items"]["required"], json!(["name"]));
        // The live batch bound rides the model-facing text of BOTH arms — a
        // whole-schema contains() would let one arm's literal go stale behind
        // the other's hit (the drift class relay_chunk's file sweep guards).
        let n = crate::relay_chunk::MAX_BATCH_ITEMS.to_string();
        for arm in ["names", "items"] {
            let d = s["properties"][arm]["description"].as_str().unwrap();
            assert!(
                d.contains(&format!("at most {n}")),
                "{arm} description dropped the live batch bound {n}"
            );
        }
    }
}
