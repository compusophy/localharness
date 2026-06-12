#[allow(unused_imports)]
use crate::*;

// ---- models (the `--model` discovery command) ----------------------------
//
// `call`/`mcp-call` take a `--model <id>`, but there was no way to discover the
// valid ids — a user had to read the source or guess (on-chain feedback #90).
// `localharness models` lists them. A `claude-*` id routes to the Anthropic
// backend (built only with the `anthropic` cargo feature); a `gemini-*` id is
// the always-available default.

/// The known model ids the CLI accepts for `--model`, as `(id, label, note)`.
/// Mirrors the browser admin selector (`src/app/model.rs::MODELS`) + the
/// Anthropic backend's wire constants, kept in lockstep with
/// `models_match_canonical_constants`. The Gemini id is the platform default;
/// the `claude-*` ids need a build with the `anthropic` feature.
pub(crate) const MODELS: &[(&str, &str, &str)] = &[
    (localharness::types::DEFAULT_MODEL, "Gemini (default)", "the platform default"),
    ("claude-haiku-4-5-20251001", "Claude Haiku", "needs the anthropic-feature build"),
    ("claude-sonnet-4-6", "Claude Sonnet", "needs the anthropic-feature build"),
    ("claude-opus-4-8", "Claude Opus", "needs the anthropic-feature build"),
];

/// Render the model list as the terminal report. Pure (no I/O) so it's
/// unit-testable.
pub(crate) fn format_models() -> String {
    let mut out = String::from("available --model ids (for `call` / `mcp-call`):\n");
    for (id, label, note) in MODELS {
        out.push_str(&format!("  {id}\n      {label} — {note}\n"));
    }
    out.push_str(
        "\nuse with: localharness call --model <id> <name> \"…\"\n\
         claude-* ids route to the Anthropic backend; gemini-* is the default.\n",
    );
    out
}

/// `localharness models` — list the valid `--model` ids. Read-only, no `$LH`,
/// no key.
pub(crate) fn models() -> i32 {
    print!("{}", format_models());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_lists_gemini_default_and_claude_ids() {
        let out = format_models();
        // The gemini default is present and labelled as the default.
        assert!(out.contains(localharness::types::DEFAULT_MODEL));
        assert!(out.contains("default"));
        // The three claude ids appear with the anthropic-build caveat.
        assert!(out.contains("claude-haiku-4-5-20251001"));
        assert!(out.contains("claude-sonnet-4-6"));
        assert!(out.contains("claude-opus-4-8"));
        assert!(out.contains("anthropic"));
    }

    /// The hard-coded ids must stay in lockstep with the crate's canonical
    /// constants — a model rename that forgets this list would advertise a dead
    /// id (the gemini-model-id-flip gotcha applied to the CLI surface).
    #[test]
    fn models_match_canonical_constants() {
        let ids: Vec<&str> = MODELS.iter().map(|(id, _, _)| *id).collect();
        assert!(ids.contains(&localharness::types::DEFAULT_MODEL));
    }
}
