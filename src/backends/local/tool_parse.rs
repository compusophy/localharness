//! Best-effort textual tool-call parser for the local Gemma backend.
//!
//! Gemma 3 (the `-it` line, and FunctionGemma) is steered into the
//! community-standard philschmid / vLLM `gemma3` tool-call format: a
//! markdown fence tagged `tool_code` wrapping a **Python-style call**
//! expression (NOT JSON):
//!
//! ````text
//! ```tool_code
//! view_file(path="main.rs", limit=10)
//! ```
//! ````
//!
//! Results are fed back the same way under a ```` ```tool_output ```` fence.
//!
//! This module extracts the FIRST `tool_code` fence and parses the call into
//! `(name, serde_json::Value args-object)` WITHOUT `eval()` (the reference
//! impl `eval()`s — unsafe). The parse is intentionally shallow: flat keyword
//! arguments, top-level comma splitting, simple scalars/strings — exactly what
//! the fs builtins need (`path` / `limit` / `content` strings). Nested
//! lists/dicts or escaped quotes inside strings may mis-split; that is an
//! accepted limitation, never an `eval`.
//!
//! The base `unsloth/gemma-3-270m` checkpoint shipping today is a *base* model
//! and will rarely emit a well-formed fence at all — so this is strictly a
//! fast-path. Callers MUST treat a `None` return as "plain text, emit as-is"
//! (graceful fall-through). Gated on `feature = "local"` via the module it
//! lives in.

use serde_json::{json, Value};

/// Parse the first ```` ```tool_code ```` fence out of `text` into a
/// `(name, args)` pair. Returns `None` when there is no well-formed fenced
/// call — the caller then treats the model output as plain text.
pub fn parse_tool_code(text: &str) -> Option<(String, Value)> {
    // Locate the opening fence and the code between it and the next ```` ``` ````.
    let after_tag = text.find("```tool_code")? + "```tool_code".len();
    let rest = &text[after_tag..];
    let end = rest.find("```")?;
    let code = rest[..end].trim();
    if code.is_empty() {
        return None;
    }

    // Split `name(args...)`.
    let open = code.find('(')?;
    let name = code[..open].trim().to_string();
    if name.is_empty() || !is_ident(&name) {
        return None;
    }
    // Strip the trailing `)` of the call (tolerate a missing one).
    let args_src = code[open + 1..].trim_end();
    let inner = args_src.strip_suffix(')').unwrap_or(args_src);

    let mut args = serde_json::Map::new();
    for kv in split_top_level_commas(inner) {
        let kv = kv.trim();
        if kv.is_empty() {
            continue;
        }
        // Only keyword args (`key=value`). Positional args are ignored — the
        // fs builtins are all keyword-shaped.
        if let Some(eq) = find_top_level_eq(kv) {
            let key = kv[..eq].trim();
            if key.is_empty() || !is_ident(key) {
                continue;
            }
            let raw = kv[eq + 1..].trim();
            args.insert(key.to_string(), py_value_to_json(raw));
        }
    }

    Some((name, Value::Object(args)))
}

/// A bare identifier: ASCII alnum + `_`, not starting with a digit. Keeps the
/// parser from mistaking arbitrary prose for a function call.
fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Convert a single Python literal RHS into a JSON value. Handles
/// `True`/`False`/`None`, single- and double-quoted strings, numbers, and
/// falls back to a bare string for anything else.
fn py_value_to_json(raw: &str) -> Value {
    match raw {
        "True" => return json!(true),
        "False" => return json!(false),
        "None" => return json!(null),
        _ => {}
    }
    // Quoted string (single or double).
    if raw.len() >= 2
        && ((raw.starts_with('"') && raw.ends_with('"'))
            || (raw.starts_with('\'') && raw.ends_with('\'')))
    {
        return Value::String(raw[1..raw.len() - 1].to_string());
    }
    // Number (or any valid JSON scalar), else a bare string.
    serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

/// Find the first `=` that is NOT `==`/`!=`/`<=`/`>=` and is at bracket/quote
/// depth 0 — the keyword-argument separator.
fn find_top_level_eq(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut prev = '\0';
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"') | (None, '\'') => quote = Some(c),
            (None, '(') | (None, '[') | (None, '{') => depth += 1,
            (None, ')') | (None, ']') | (None, '}') => depth -= 1,
            (None, '=') if depth == 0 => {
                let next = bytes.get(i + 1).copied().unwrap_or(0);
                // Skip comparison operators (==, !=, <=, >=).
                if next != b'=' && prev != '=' && prev != '!' && prev != '<' && prev != '>' {
                    return Some(i);
                }
            }
            _ => {}
        }
        prev = c;
    }
    None
}

/// Split on commas that are not inside quotes or brackets. Good enough for the
/// flat keyword args the fs builtins take.
fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), _) if c == q => {
                quote = None;
                buf.push(c);
            }
            (Some(_), _) => buf.push(c),
            (None, '"') | (None, '\'') => {
                quote = Some(c);
                buf.push(c);
            }
            (None, '(') | (None, '[') | (None, '{') => {
                depth += 1;
                buf.push(c);
            }
            (None, ')') | (None, ']') | (None, '}') => {
                depth -= 1;
                buf.push(c);
            }
            (None, ',') if depth == 0 => out.push(std::mem::take(&mut buf)),
            _ => buf.push(c),
        }
    }
    if !buf.trim().is_empty() {
        out.push(buf);
    }
    out
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn parses_pythonic_tool_code() {
        let txt = "sure, let me look\n```tool_code\nview_file(path=\"main.rs\", limit=10)\n```";
        let (name, args) = parse_tool_code(txt).expect("a parsed call");
        assert_eq!(name, "view_file");
        assert_eq!(args["path"], "main.rs");
        assert_eq!(args["limit"], 10);
    }

    #[test]
    fn parses_single_quoted_and_bools() {
        let txt = "```tool_code\ncreate_file(path='a.txt', overwrite=True, note=None)\n```";
        let (name, args) = parse_tool_code(txt).unwrap();
        assert_eq!(name, "create_file");
        assert_eq!(args["path"], "a.txt");
        assert_eq!(args["overwrite"], true);
        assert!(args["note"].is_null());
    }

    #[test]
    fn no_fence_is_none() {
        assert!(parse_tool_code("just a plain text answer, no fence").is_none());
        assert!(parse_tool_code("```tool_output\n42\n```").is_none());
    }

    #[test]
    fn empty_args_ok() {
        let (name, args) = parse_tool_code("```tool_code\nfinish()\n```").unwrap();
        assert_eq!(name, "finish");
        assert!(args.as_object().unwrap().is_empty());
    }

    #[test]
    fn comma_inside_string_is_not_split() {
        let txt = "```tool_code\ncreate_file(path=\"a.txt\", content=\"a, b, c\")\n```";
        let (_, args) = parse_tool_code(txt).unwrap();
        assert_eq!(args["content"], "a, b, c");
        assert_eq!(args["path"], "a.txt");
    }

    #[test]
    fn prose_with_parens_is_rejected() {
        // No fence at all → None. Guards against treating arbitrary prose like
        // "I think (maybe)" as a call.
        assert!(parse_tool_code("I think (maybe) the answer is 4").is_none());
    }
}
