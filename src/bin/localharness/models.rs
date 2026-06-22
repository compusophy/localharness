
// ---- models (the `--model` discovery command) ----------------------------
//
// `call`/`mcp-call` take a `--model <id>`, but there was no way to discover the
// valid ids — a user had to read the source or guess (on-chain feedback #90).
// `localharness models` lists them. A `claude-*` id routes to the Anthropic
// backend (built only with the `anthropic` cargo feature); a `gpt-*` id routes
// to the OpenAI backend (the `openai` feature); a `gemini-*` id is the
// always-available default. The multi-provider proxy routes all three on $LH.
//
// KEEP IN SYNC with the provider ids advertised in web/llms.txt — this command
// is what that doc points users to.

/// The known model ids the CLI accepts for `--model`, as `(id, label, note)`.
/// Mirrors the browser admin selector (`src/app/model.rs::MODELS`) + the
/// Anthropic/OpenAI backend wire constants, kept in lockstep with
/// `models_match_canonical_constants`. The Gemini id is the platform default;
/// `claude-*` ids need the `anthropic` feature, `gpt-*` ids the `openai`
/// feature (the credit proxy routes any of them).
pub(crate) const MODELS: &[(&str, &str, &str)] = &[
    (localharness::types::DEFAULT_MODEL, "Gemini (default)", "the platform default"),
    ("claude-haiku-4-5-20251001", "Claude Haiku", "needs the anthropic-feature build"),
    ("claude-sonnet-4-6", "Claude Sonnet", "needs the anthropic-feature build"),
    ("claude-opus-4-8", "Claude Opus", "needs the anthropic-feature build"),
    ("gpt-5-nano", "GPT-5 nano", "needs the openai-feature build"),
    ("gpt-5-mini", "GPT-5 mini", "needs the openai-feature build"),
    ("gpt-5.1", "GPT-5.1", "needs the openai-feature build"),
    ("gpt-5-pro", "GPT-5 pro", "needs the openai-feature build"),
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
         claude-* → Anthropic, gpt-* → OpenAI, gemini-* (default) → Gemini; \
         the credit proxy routes all three on $LH.\n",
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
    fn models_lists_gemini_default_and_claude_and_gpt_ids() {
        let out = format_models();
        // The gemini default is present and labelled as the default.
        assert!(out.contains(localharness::types::DEFAULT_MODEL));
        assert!(out.contains("default"));
        // The three claude ids appear with the anthropic-build caveat.
        assert!(out.contains("claude-haiku-4-5-20251001"));
        assert!(out.contains("claude-sonnet-4-6"));
        assert!(out.contains("claude-opus-4-8"));
        assert!(out.contains("anthropic"));
        // The OpenAI ids appear with the openai-build caveat (added when the
        // proxy gained /v1/chat/completions routing + the SDK backend).
        assert!(out.contains("gpt-5-nano"));
        assert!(out.contains("gpt-5.1"));
        assert!(out.contains("openai"));
    }

    /// The hard-coded ids must stay in lockstep with the crate's canonical
    /// constants — a model rename that forgets this list would advertise a dead
    /// id (the gemini-model-id-flip gotcha applied to the CLI surface). The CLI
    /// lists ids ACROSS feature builds (with a "needs the X-feature build"
    /// caveat), so it can't const-reference the feature-gated backend wire
    /// constants the way the browser selector does — instead these tests pin
    /// the literals against those constants WHEN the feature is present, so any
    /// build that includes a backend catches a drifted id.
    #[test]
    fn models_match_canonical_constants() {
        let ids: Vec<&str> = MODELS.iter().map(|(id, _, _)| *id).collect();
        assert!(ids.contains(&localharness::types::DEFAULT_MODEL));
    }

    #[cfg(feature = "anthropic")]
    #[test]
    fn anthropic_ids_match_backend_wire_constants() {
        let ids: Vec<&str> = MODELS.iter().map(|(id, _, _)| *id).collect();
        assert!(ids.contains(&localharness::backends::anthropic::DEFAULT_MODEL));
        assert!(ids.contains(&localharness::backends::anthropic::SONNET_MODEL));
        assert!(ids.contains(&localharness::backends::anthropic::OPUS_MODEL));
    }

    #[cfg(feature = "openai")]
    #[test]
    fn openai_ids_match_backend_wire_constants() {
        let ids: Vec<&str> = MODELS.iter().map(|(id, _, _)| *id).collect();
        assert!(ids.contains(&localharness::backends::openai::DEFAULT_MODEL));
        assert!(ids.contains(&localharness::backends::openai::MINI_MODEL));
        assert!(ids.contains(&localharness::backends::openai::PRO_MODEL));
    }

    /// The CLI's non-Gemini advertised ids (Gemini is flat-priced / per-token, so
    /// the proxy tables carry no claude-/gpt-style row for it).
    fn cli_non_gemini_ids() -> std::collections::BTreeSet<&'static str> {
        MODELS
            .iter()
            .map(|(id, _, _)| *id)
            .filter(|id| id.starts_with("claude-") || id.starts_with("gpt-"))
            .collect()
    }

    /// claude-/gpt- single-quoted record keys (`'<id>':`) in a proxy TS table.
    /// Those quotes only appear as PRICE_*/RATE_USD record keys; `None` if the
    /// proxy tree isn't present (a packaged crate).
    fn proxy_claude_gpt_keys(rel: &str) -> Option<std::collections::BTreeSet<String>> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        let src = std::fs::read_to_string(&path).ok()?;
        let mut out = std::collections::BTreeSet::new();
        for line in src.lines() {
            let Some(rest) = line.trim_start().strip_prefix('\'') else { continue };
            let Some(end) = rest.find('\'') else { continue };
            let id = &rest[..end];
            if rest[end + 1..].trim_start().starts_with(':')
                && (id.starts_with("claude-") || id.starts_with("gpt-"))
            {
                out.insert(id.to_string());
            }
        }
        Some(out)
    }

    /// SSOT drift guard (tech-debt report §2): the proxy per-model PRICE table
    /// (`proxy/api/_prices.ts`) must price EXACTLY the non-Gemini ids the CLI
    /// advertises in `MODELS`. A model renamed in the backend/CLI but not here
    /// would silently fall to the proxy's UNKNOWN-model default tier (a
    /// mis-billing); a stale id priced here would advertise a dead model.
    /// `MODELS` is already pinned to the backend wire constants above, closing the
    /// loop to the off-chain table. Skips if the proxy tree isn't present.
    #[test]
    fn proxy_price_table_matches_cli_models() {
        let Some(proxy) = proxy_claude_gpt_keys("proxy/api/_prices.ts") else {
            eprintln!("skip: proxy/api/_prices.ts not present (packaged crate?)");
            return;
        };
        let proxy_refs: std::collections::BTreeSet<&str> = proxy.iter().map(String::as_str).collect();
        assert_eq!(
            cli_non_gemini_ids(),
            proxy_refs,
            "proxy _prices.ts model ids drifted from CLI MODELS: {proxy_refs:?}"
        );
    }

    /// Same guard for the token-RATE table (`proxy/api/_usage.ts`, the usage-based
    /// metering path). It's flag-gated OFF in prod, so drift there is LATENT — this
    /// catches a renamed claude/gpt model before metering is ever flipped on.
    /// (Gemini has per-model rate rows there that `MODELS` doesn't enumerate, so
    /// only the claude-/gpt- subset is checked.)
    #[test]
    fn proxy_usage_table_matches_cli_models() {
        let Some(proxy) = proxy_claude_gpt_keys("proxy/api/_usage.ts") else {
            eprintln!("skip: proxy/api/_usage.ts not present (packaged crate?)");
            return;
        };
        let proxy_refs: std::collections::BTreeSet<&str> = proxy.iter().map(String::as_str).collect();
        assert_eq!(
            cli_non_gemini_ids(),
            proxy_refs,
            "proxy _usage.ts rate-table ids drifted from CLI MODELS: {proxy_refs:?}"
        );
    }
}
