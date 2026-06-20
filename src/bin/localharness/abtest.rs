#[allow(unused_imports)]
use crate::*;

// ---- abtest (A/B testing with agentic personas, #22) ----------------------
//
// `abtest <prompt>` fans ONE prompt across N VARIANTS and prints the answers
// side-by-side so a builder can compare which model / which persona answers
// best — the every-agent-builder question "given a prompt, which variant
// wins?". A variant is one of the two axes the platform already exposes:
//   * MODEL — the same persona answered by Gemini vs Claude vs GPT (`--models`).
//   * PERSONA — different on-chain `<name>.localharness.xyz` system prompts
//     answering the SAME question on one model (`--personas`).
//
// It is a pure ORCHESTRATOR over the existing headless turn (`run_agent_turn`,
// shared with `call` + the MCP server): it adds NO new on-chain surface, no new
// proxy route, no new billing path. Each variant is ONE ordinary metered turn —
// run SEQUENTIALLY because each turn carries a one-shot x402 nonce valid for
// exactly one request (the `call.rs` INVARIANT), so variants must never share a
// connection / replay a nonce. A failed variant is reported in place (it never
// sinks the run), so the surviving variants still produce a comparison.
//
// Design: `design/ab-testing.md`. The parse / expand / format functions are
// pure + unit-tested (mirroring `call`/`colony`/`models`); only the fan-out
// loop touches the network.

pub(crate) const ABTEST_USAGE: &str = "\
usage: localharness abtest [--as <me>] <prompt…> (--models <a,b,c> | --model <id>…) | (--personas <x,y> | --persona <name>… [--model <id>])
  Run ONE prompt across N variants and print the answers side-by-side.
  Pick exactly ONE axis to vary:
    MODEL   --models gemini-3.5-flash,claude-opus-4-8   (or repeated --model <id>)
            same persona (your identity), answered by each model
    PERSONA --personas alice,bob                        (or repeated --persona <name>)
            each agent's on-chain persona, answered on one model (--model, default Gemini)
  Each variant is ONE ordinary metered turn billed to your identity (--as, or your
  sole key); the run prints the variant count up front so the spend is no surprise.
  A failed variant is reported in place — the others still produce a comparison.
  Comparisons are apples-to-apples: every variant answers the SAME prompt from a
  FRESH context (no saved call history is seeded in).";

/// The axis an A/B run varies. `Models` holds the parsed `--model`/`--models`
/// ids (the persona is the caller's identity); `Personas` holds the parsed
/// `--persona`/`--personas` names answered on a single `model`.
#[derive(Debug, PartialEq)]
pub(crate) enum AbAxis {
    /// Vary the MODEL: one persona (the caller), N model ids.
    Models(Vec<String>),
    /// Vary the PERSONA: N target agent names, all on one optional model id.
    Personas { names: Vec<String>, model: Option<String> },
}

/// A parsed `abtest` invocation: the caller identity, the prompt, and the axis.
#[derive(Debug, PartialEq)]
pub(crate) struct ParsedAbtest {
    pub caller: Option<String>,
    pub prompt: String,
    pub axis: AbAxis,
}

/// One concrete variant to run: a human `label` (the model id or the persona
/// name shown in the report), the `target` persona name to embody (the caller's
/// own identity for the model axis), and the `model` id (None = platform
/// default).
#[derive(Debug, PartialEq, Clone)]
pub(crate) struct Variant {
    pub label: String,
    pub target: String,
    pub model: Option<String>,
}

/// Split a comma-separated `--models`/`--personas` value into a de-duplicated,
/// order-preserving list of trimmed non-empty entries. `a, b ,,a,c` →
/// `[a,b,c]` — duplicates dropped (an A/B against the same variant twice is a
/// no-op), blanks dropped (a trailing comma is harmless). Pure. Mirrors
/// `call::parse_verify_keys` but with dedup (a repeated variant is meaningless).
pub(crate) fn parse_csv_dedup(spec: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if !out.iter().any(|e| e == item) {
            out.push(item.to_string());
        }
    }
    out
}

/// Parse `abtest` args: `--as` (any position, via `take_as_flag`), then the
/// variant flags (`--models`/`--model`/`--personas`/`--persona`), then the
/// remaining positionals join into the prompt. Pure (no I/O) so it is
/// unit-testable; `Err` carries the usage line.
///
/// Rules (each → the `ABTEST_USAGE` line on violation):
///   * EXACTLY one axis — model XOR persona (mixing `--models` with `--personas`
///     is ambiguous: which varies?).
///   * The chosen axis needs ≥ 2 DISTINCT variants (an A/B with one variant is
///     just a `call`).
///   * A non-empty prompt.
///   * `--model` on the PERSONA axis sets the single model all personas answer
///     on; `--model` on the MODEL axis is folded into the model set.
pub(crate) fn parse_abtest_args(rest: &[String]) -> Result<ParsedAbtest, String> {
    let (caller, rest) = take_as_flag(rest)?;

    let mut models: Vec<String> = Vec::new();
    let mut personas: Vec<String> = Vec::new();
    let mut single_model: Option<String> = None;
    let mut prompt_parts: Vec<String> = Vec::new();

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--models" => {
                let v = rest.get(i + 1).ok_or(ABTEST_USAGE)?;
                for m in parse_csv_dedup(v) {
                    if !models.contains(&m) {
                        models.push(m);
                    }
                }
                i += 2;
            }
            "--model" => {
                let v = rest.get(i + 1).ok_or(ABTEST_USAGE)?;
                let m = v.trim().to_string();
                if m.is_empty() {
                    return Err(ABTEST_USAGE.to_string());
                }
                // A single --model is the persona-axis model AND folds into the
                // model set (so `--model a --model b` is a valid model A/B).
                single_model = Some(m.clone());
                if !models.contains(&m) {
                    models.push(m);
                }
                i += 2;
            }
            "--personas" => {
                let v = rest.get(i + 1).ok_or(ABTEST_USAGE)?;
                for p in parse_csv_dedup(v) {
                    if !personas.contains(&p) {
                        personas.push(p);
                    }
                }
                i += 2;
            }
            "--persona" => {
                let v = rest.get(i + 1).ok_or(ABTEST_USAGE)?;
                let p = v.trim().to_string();
                if p.is_empty() {
                    return Err(ABTEST_USAGE.to_string());
                }
                if !personas.contains(&p) {
                    personas.push(p);
                }
                i += 2;
            }
            // Everything else is the prompt (collected in order). Unlike the
            // leading-flag parsers, the variant flags may interleave with the
            // prompt, so we don't `break` on the first positional.
            _ => {
                prompt_parts.push(rest[i].clone());
                i += 1;
            }
        }
    }

    let prompt = prompt_parts.join(" ");
    if prompt.trim().is_empty() {
        return Err(ABTEST_USAGE.to_string());
    }

    let varying_personas = !personas.is_empty();
    // Distinguish "varying models" from "a single model pinning the persona
    // axis": a lone `--model` (single_model set, exactly one model, no personas)
    // is NOT a model A/B — it's an under-specified persona axis (no personas) OR
    // a single-variant model run; either way it can't stand alone.
    let varying_models = models.len() >= 2;

    match (varying_models, varying_personas) {
        (true, true) => Err(ABTEST_USAGE.to_string()), // both axes — ambiguous
        (true, false) => Ok(ParsedAbtest {
            caller,
            prompt,
            axis: AbAxis::Models(models),
        }),
        (false, true) => {
            if personas.len() < 2 {
                return Err(ABTEST_USAGE.to_string());
            }
            Ok(ParsedAbtest {
                caller,
                prompt,
                axis: AbAxis::Personas { names: personas, model: single_model },
            })
        }
        (false, false) => Err(ABTEST_USAGE.to_string()), // no axis / single variant
    }
}

/// Expand a parsed run into the ordered list of concrete variants to execute.
/// `caller_label` is the caller's own identity (the persona embodied on the
/// MODEL axis). Pure + testable.
///
///   * MODEL axis → one variant per model id, each embodying `caller_label`,
///     labelled by the model id.
///   * PERSONA axis → one variant per persona name, each on the single `model`,
///     labelled by the persona name.
pub(crate) fn expand_variants(parsed: &ParsedAbtest, caller_label: &str) -> Vec<Variant> {
    match &parsed.axis {
        AbAxis::Models(models) => models
            .iter()
            .map(|m| Variant {
                label: m.clone(),
                target: caller_label.to_string(),
                model: Some(m.clone()),
            })
            .collect(),
        AbAxis::Personas { names, model } => names
            .iter()
            .map(|n| Variant {
                label: n.clone(),
                target: n.clone(),
                model: model.clone(),
            })
            .collect(),
    }
}

/// Render the collected `(variant_label, reply_or_error)` results as the
/// terminal A/B report: a labelled, ruled block per variant. `ok` = the reply
/// text; `Err` = a one-line failure (the caller folds in the actionable hint).
/// Pure (no I/O) so it is unit-testable, mirroring `models::format_models`.
pub(crate) fn format_abtest_report(prompt: &str, results: &[(String, Result<String, String>)]) -> String {
    let mut out = String::new();
    out.push_str(&format!("A/B run · {} variant(s)\nprompt: {}\n", results.len(), prompt.trim()));
    for (label, result) in results {
        out.push('\n');
        out.push_str(&format!("──── {label} ────\n"));
        match result {
            Ok(text) => {
                let t = text.trim();
                if t.is_empty() {
                    out.push_str("(no answer)\n");
                } else {
                    out.push_str(t);
                    out.push('\n');
                }
            }
            Err(e) => out.push_str(&format!("failed: {e}\n")),
        }
    }
    out
}

/// `localharness abtest <prompt> …` — fan ONE prompt across N variants and print
/// the answers side-by-side. Resolves the caller's identity key (it signs proxy
/// auth + pays each variant's metered turn), expands the variants, runs each
/// SEQUENTIALLY via the shared `run_agent_turn` (fresh context per variant — an
/// apples-to-apples comparison), captures each reply (or its error), and prints
/// the report. Returns the process exit code (0 if at least one variant
/// answered, 1 if every variant failed).
pub(crate) async fn abtest(rest: &[String]) -> i32 {
    let parsed = match parse_abtest_args(rest) {
        Ok(p) => p,
        Err(usage) => {
            eprintln!("{usage}");
            return 2;
        }
    };

    if let Err(e) = non_blank(&parsed.prompt, "abtest: prompt") {
        eprintln!("{e}");
        return 1;
    }

    // The caller's identity signs proxy auth and pays each variant's turn.
    let (_key_file, key_hex) = match resolve_caller_key(parsed.caller.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let caller_label = match resolve_caller_label(parsed.caller.as_deref()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };

    let variants = expand_variants(&parsed, &caller_label);
    // Defensive: the parser already guarantees ≥ 2 distinct variants.
    if variants.len() < 2 {
        eprintln!("{ABTEST_USAGE}");
        return 2;
    }

    println!(
        "abtest: running {} variant(s) — each is one metered turn billed to {caller_label}",
        variants.len()
    );

    // Run SEQUENTIALLY: each turn carries a one-shot x402 nonce (the `call.rs`
    // INVARIANT), so variants must not overlap / replay a nonce. A failed
    // variant is captured in place — it never aborts the run, so the surviving
    // variants still produce a comparison (the whole point of an A/B).
    let mut results: Vec<(String, Result<String, String>)> = Vec::with_capacity(variants.len());
    for v in &variants {
        // Fresh context per variant (prior_history = None) — an A/B comparison
        // must be apples-to-apples; a seeded thread would bleed one variant into
        // the next and make the comparison meaningless.
        let outcome =
            run_agent_turn(&key_hex, &v.target, &parsed.prompt, None, v.model.as_deref()).await;
        let captured = match outcome {
            Ok((text, _history)) if !text.trim().is_empty() => Ok(text),
            Ok(_) => Err("the agent returned no text".to_string()),
            Err(e) => {
                // Fold the actionable hint into the captured error so the report
                // is self-explanatory (the same hint `call` prints on failure).
                let msg = match hint_for_call_error(&e) {
                    Some(hint) => format!("{e} (hint: {hint})"),
                    None => e,
                };
                Err(msg)
            }
        };
        results.push((v.label.clone(), captured));
    }

    print!("{}", format_abtest_report(&parsed.prompt, &results));

    // Exit 0 if at least one variant answered; 1 only if EVERY variant failed
    // (a totally-failed run is an error, a partial run is a usable comparison).
    if results.iter().any(|(_, r)| r.is_ok()) {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_csv_dedup_trims_drops_blanks_and_dups() {
        assert_eq!(parse_csv_dedup("a,b,c"), vec!["a", "b", "c"]);
        // Whitespace trimmed; blanks (incl. trailing comma) dropped.
        assert_eq!(parse_csv_dedup(" a , b ,, c,"), vec!["a", "b", "c"]);
        // Duplicates dropped, first occurrence's order kept.
        assert_eq!(parse_csv_dedup("a,b,a,c,b"), vec!["a", "b", "c"]);
        // All-blank → empty.
        assert!(parse_csv_dedup(" , , ").is_empty());
        // A single entry.
        assert_eq!(parse_csv_dedup("gemini-3.5-flash"), vec!["gemini-3.5-flash"]);
    }

    #[test]
    fn parse_model_axis_via_models_flag() {
        let p = parse_abtest_args(&args(&[
            "--models",
            "gemini-3.5-flash,claude-opus-4-8",
            "explain",
            "recursion",
        ]))
        .unwrap();
        assert_eq!(p.caller, None);
        assert_eq!(p.prompt, "explain recursion");
        assert_eq!(
            p.axis,
            AbAxis::Models(vec![
                "gemini-3.5-flash".to_string(),
                "claude-opus-4-8".to_string()
            ])
        );
    }

    #[test]
    fn parse_model_axis_via_repeated_model_flag() {
        // Two `--model` flags form a model A/B (each folds into the model set).
        let p = parse_abtest_args(&args(&[
            "--model", "gemini-3.5-flash", "--model", "claude-opus-4-8", "hi",
        ]))
        .unwrap();
        assert_eq!(
            p.axis,
            AbAxis::Models(vec![
                "gemini-3.5-flash".to_string(),
                "claude-opus-4-8".to_string()
            ])
        );
        assert_eq!(p.prompt, "hi");
    }

    #[test]
    fn parse_persona_axis_via_personas_flag() {
        let p = parse_abtest_args(&args(&["--personas", "alice,bob", "what", "is", "rust"]))
            .unwrap();
        assert_eq!(p.prompt, "what is rust");
        assert_eq!(
            p.axis,
            AbAxis::Personas { names: vec!["alice".to_string(), "bob".to_string()], model: None }
        );
    }

    #[test]
    fn parse_persona_axis_pins_the_model_when_given() {
        // `--model` on the persona axis sets the single model all personas answer
        // on (it is NOT a model A/B because there are personas to vary).
        let p = parse_abtest_args(&args(&[
            "--personas", "alice,bob", "--model", "claude-opus-4-8", "hi",
        ]))
        .unwrap();
        assert_eq!(
            p.axis,
            AbAxis::Personas {
                names: vec!["alice".to_string(), "bob".to_string()],
                model: Some("claude-opus-4-8".to_string()),
            }
        );
    }

    #[test]
    fn parse_persona_axis_via_repeated_persona_flag() {
        let p = parse_abtest_args(&args(&[
            "--persona", "alice", "--persona", "bob", "compare",
        ]))
        .unwrap();
        assert_eq!(
            p.axis,
            AbAxis::Personas { names: vec!["alice".to_string(), "bob".to_string()], model: None }
        );
    }

    #[test]
    fn parse_extracts_as_flag_from_any_position() {
        let p = parse_abtest_args(&args(&[
            "--models", "a,b", "hi", "--as", "me",
        ]))
        .unwrap();
        assert_eq!(p.caller.as_deref(), Some("me"));
        assert_eq!(p.prompt, "hi");
    }

    #[test]
    fn parse_rejects_both_axes() {
        // Mixing model and persona axes is ambiguous (which varies?).
        assert!(parse_abtest_args(&args(&[
            "--models", "a,b", "--personas", "x,y", "hi"
        ]))
        .is_err());
    }

    #[test]
    fn parse_rejects_single_variant() {
        // One model is not an A/B (just a `call`).
        assert!(parse_abtest_args(&args(&["--models", "a", "hi"])).is_err());
        // A lone --model with no personas is not an A/B either.
        assert!(parse_abtest_args(&args(&["--model", "claude-opus-4-8", "hi"])).is_err());
        // One persona is not an A/B.
        assert!(parse_abtest_args(&args(&["--personas", "alice", "hi"])).is_err());
        assert!(parse_abtest_args(&args(&["--persona", "alice", "hi"])).is_err());
        // Two CSV entries that dedup to one → still single-variant → rejected.
        assert!(parse_abtest_args(&args(&["--models", "a,a", "hi"])).is_err());
    }

    #[test]
    fn parse_rejects_no_axis_and_empty_prompt() {
        // No variant flag at all.
        assert!(parse_abtest_args(&args(&["just", "a", "prompt"])).is_err());
        // Axis but no prompt.
        assert!(parse_abtest_args(&args(&["--models", "a,b"])).is_err());
        // Empty everything.
        assert!(parse_abtest_args(&args(&[])).is_err());
    }

    #[test]
    fn parse_rejects_dangling_flags() {
        assert!(parse_abtest_args(&args(&["--models"])).is_err());
        assert!(parse_abtest_args(&args(&["--model"])).is_err());
        assert!(parse_abtest_args(&args(&["--personas"])).is_err());
        assert!(parse_abtest_args(&args(&["--persona"])).is_err());
        // Empty flag value.
        assert!(parse_abtest_args(&args(&["--model", "  ", "hi"])).is_err());
        assert!(parse_abtest_args(&args(&["--persona", "  ", "hi"])).is_err());
    }

    #[test]
    fn expand_model_axis_embodies_caller_on_each_model() {
        let p = ParsedAbtest {
            caller: None,
            prompt: "hi".to_string(),
            axis: AbAxis::Models(vec!["m1".to_string(), "m2".to_string()]),
        };
        let v = expand_variants(&p, "claude");
        assert_eq!(
            v,
            vec![
                Variant { label: "m1".to_string(), target: "claude".to_string(), model: Some("m1".to_string()) },
                Variant { label: "m2".to_string(), target: "claude".to_string(), model: Some("m2".to_string()) },
            ]
        );
    }

    #[test]
    fn expand_persona_axis_runs_each_persona_on_the_pinned_model() {
        let p = ParsedAbtest {
            caller: None,
            prompt: "hi".to_string(),
            axis: AbAxis::Personas {
                names: vec!["alice".to_string(), "bob".to_string()],
                model: Some("claude-opus-4-8".to_string()),
            },
        };
        let v = expand_variants(&p, "me");
        assert_eq!(
            v,
            vec![
                Variant {
                    label: "alice".to_string(),
                    target: "alice".to_string(),
                    model: Some("claude-opus-4-8".to_string())
                },
                Variant {
                    label: "bob".to_string(),
                    target: "bob".to_string(),
                    model: Some("claude-opus-4-8".to_string())
                },
            ]
        );
    }

    #[test]
    fn expand_persona_axis_defaults_model_to_none() {
        let p = ParsedAbtest {
            caller: None,
            prompt: "hi".to_string(),
            axis: AbAxis::Personas { names: vec!["a".to_string(), "b".to_string()], model: None },
        };
        let v = expand_variants(&p, "me");
        assert!(v.iter().all(|x| x.model.is_none()));
    }

    #[test]
    fn format_report_lays_out_each_variant_with_its_label() {
        let results = vec![
            ("gemini".to_string(), Ok("recursion is when a function calls itself".to_string())),
            ("claude-opus-4-8".to_string(), Ok("a function defined in terms of itself".to_string())),
        ];
        let out = format_abtest_report("explain recursion", &results);
        // Header carries the variant count + the prompt.
        assert!(out.contains("2 variant(s)"));
        assert!(out.contains("prompt: explain recursion"));
        // Each variant's label and its answer appear.
        assert!(out.contains("gemini"));
        assert!(out.contains("recursion is when a function calls itself"));
        assert!(out.contains("claude-opus-4-8"));
        assert!(out.contains("a function defined in terms of itself"));
    }

    #[test]
    fn format_report_renders_a_failed_variant_in_place() {
        let results = vec![
            ("alice".to_string(), Ok("an answer".to_string())),
            ("bob".to_string(), Err("HTTP 402 (hint: fund first)".to_string())),
        ];
        let out = format_abtest_report("q", &results);
        // The good variant survives …
        assert!(out.contains("alice"));
        assert!(out.contains("an answer"));
        // … and the failed one is reported, not dropped.
        assert!(out.contains("bob"));
        assert!(out.contains("failed: HTTP 402 (hint: fund first)"));
    }

    #[test]
    fn format_report_marks_an_empty_answer() {
        let results = vec![
            ("a".to_string(), Ok("real".to_string())),
            ("b".to_string(), Ok("   ".to_string())),
        ];
        let out = format_abtest_report("q", &results);
        assert!(out.contains("(no answer)"));
    }
}
