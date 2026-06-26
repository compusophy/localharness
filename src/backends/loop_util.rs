//! Shared helpers for the streaming-backend turn loops.
//!
//! Per the subsystem spec ("Fix plumbing in the SHARED core, not per-backend":
//! if a fix would be copy-pasted into two backends, it belongs here), these are
//! the small loop helpers that were byte-identical across gemini / anthropic /
//! openai. Hoisted to ONE home so a fix (e.g. canonical-path handling, the
//! malformed-args convention) can't drift between backends.
//!
//! NOTE: currently consumed only by the OpenAI loop, so the module is gated on
//! `feature = "openai"` to stay dead-code-free. As the gemini (always-on) and
//! anthropic loops migrate to these impls, widen the gate in `backends/mod.rs`
//! to unconditional (`extract_canonical_path` is also used by gemini) and
//! `resolve_tool_args` to `any(feature = "anthropic", feature = "openai")`.

use serde_json::{json, Value};
use tracing::warn;

/// Resolve a tool call's concatenated streamed `arguments` fragment into parsed
/// args. An EMPTY/absent fragment is a valid no-arg call → `({}, None)`. A
/// NON-EMPTY fragment that fails to parse returns `({}, Some(error))`: the
/// caller surfaces that error to the model as a tool error rather than running
/// the tool with empty args silently.
pub(crate) fn resolve_tool_args(name: &str, args_json: &str) -> (Value, Option<String>) {
    if args_json.trim().is_empty() {
        return (json!({}), None);
    }
    match serde_json::from_str(args_json) {
        Ok(v) => (v, None),
        Err(e) => {
            let msg = format!("malformed tool arguments for '{name}': {e} (got: {args_json})");
            warn!(error = %e, name = %name, "tool_call args not valid JSON; surfacing tool error");
            (json!({}), Some(msg))
        }
    }
}

/// Canonicalize a tool call's `"path"` arg so the `workspace_only` containment
/// policy has an absolute path to check. Existing targets canonicalize
/// directly; for a not-yet-existent target (e.g. `create_file`) the parent is
/// canonicalized and the file name re-joined. `None` when there's no path arg
/// or the parent can't be resolved.
pub(crate) fn extract_canonical_path(args: &Value) -> Option<String> {
    let path_str = args.get("path").and_then(|v| v.as_str())?;
    let path = std::path::Path::new(path_str);
    if let Ok(p) = dunce::canonicalize(path) {
        return Some(p.display().to_string());
    }
    let parent = path.parent()?;
    let file = path.file_name()?;
    let parent = if parent.as_os_str().is_empty() {
        std::path::Path::new(".")
    } else {
        parent
    };
    dunce::canonicalize(parent)
        .ok()
        .map(|p| p.join(file).display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_tool_args_valid_empty_and_malformed() {
        // Valid JSON parses.
        let (args, err) = resolve_tool_args("view_file", r#"{"path":"main.rs"}"#);
        assert!(err.is_none());
        assert_eq!(args["path"], "main.rs");
        // Empty / whitespace is a valid no-arg call, NOT a parse error.
        for empty in ["", "   "] {
            let (args, err) = resolve_tool_args("list_subdomains", empty);
            assert!(err.is_none(), "empty args must NOT be treated as malformed");
            assert_eq!(args, json!({}));
        }
        // A non-empty fragment that fails to parse surfaces an error (no silent {}).
        let (args, err) = resolve_tool_args("edit_file", r#"{"path":"a.rs","content":"#);
        assert!(err.unwrap().contains("malformed tool arguments for 'edit_file'"));
        assert_eq!(args, json!({}));
    }
}
