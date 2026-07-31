//! ROUTER INTENT EVAL — the measured business case for a local intent
//! classifier (design/local-models.md). Offline + deterministic: no network,
//! no model, no wallet. Scores the shipped [`HeuristicClassifier`] over
//! `datasets/router/cases.json` through the SAME call the app makes —
//! `app::chat::router_wire::pre_route` checks `parse_router_cmd` first, then
//! hands the RAW prompt to `classify()` (trim/normalize live inside it), so
//! the eval entry point is exactly `classify()` plus a hygiene guard that no
//! case would be swallowed as a `/router` command upstream.
//!
//! Reported: free-class RECALL per class + overall (a miss costs the user
//! ~1 $LH, never a wrong answer) and metered PRECISION (MUST be 1.0 — any
//! expected-metered case routed Free is a real router bug, printed loudly).
//!
//! Run: `cargo run --example router_eval` — exits non-zero iff a false-free.

use localharness::router::{
    parse_router_cmd, AdminTopic, DocsTopic, FreeAction, HeuristicClassifier, IntentClassifier,
    Route, UiCommand,
};
use serde::Deserialize;
use std::process::ExitCode;

const CASES_JSON: &str = include_str!("../datasets/router/cases.json");

#[derive(Deserialize)]
struct Case {
    text: String,
    expect: String,
}

/// Report order for the free classes (labels are the dataset's `expect` names).
const FREE_LABELS: &[&str] = &[
    "free_balance",
    "free_ui_open_files",
    "free_ui_open_display",
    "free_ui_open_terminal",
    "free_ui_theme_light",
    "free_ui_theme_dark",
    "free_ui_view_desktop",
    "free_ui_view_mobile",
    "free_admin_settings",
    "free_admin_identity",
    "free_admin_model",
    "free_admin_public_face",
    "free_admin_funds",
    "free_admin_devices",
    "free_docs_pricing",
    "free_docs_funding",
    "free_docs_what_is_this",
];

/// Route -> dataset label (the eval-side mirror of the `Route` tree).
fn observed_label(route: Route) -> &'static str {
    match route {
        Route::Metered => "metered",
        Route::Free(FreeAction::BalanceQuery) => "free_balance",
        Route::Free(FreeAction::UiCommand(c)) => match c {
            UiCommand::OpenFiles => "free_ui_open_files",
            UiCommand::OpenDisplay => "free_ui_open_display",
            UiCommand::OpenTerminal => "free_ui_open_terminal",
            UiCommand::ThemeLight => "free_ui_theme_light",
            UiCommand::ThemeDark => "free_ui_theme_dark",
            UiCommand::ViewDesktop => "free_ui_view_desktop",
            UiCommand::ViewMobile => "free_ui_view_mobile",
        },
        Route::Free(FreeAction::AdminCard(t)) => match t {
            AdminTopic::Settings => "free_admin_settings",
            AdminTopic::Identity => "free_admin_identity",
            AdminTopic::Model => "free_admin_model",
            AdminTopic::PublicFace => "free_admin_public_face",
            AdminTopic::Funds => "free_admin_funds",
            AdminTopic::Devices => "free_admin_devices",
        },
        Route::Free(FreeAction::DocsAnswer(t)) => match t {
            DocsTopic::Pricing => "free_docs_pricing",
            DocsTopic::Funding => "free_docs_funding",
            DocsTopic::WhatIsThis => "free_docs_what_is_this",
        },
    }
}

fn pct(hit: usize, total: usize) -> f64 {
    if total == 0 { 0.0 } else { 100.0 * hit as f64 / total as f64 }
}

fn main() -> ExitCode {
    let cases: Vec<Case> = serde_json::from_str(CASES_JSON).expect("cases.json parses");

    // ── dataset hygiene (a bad row would silently skew the numbers) ──
    let mut seen = std::collections::HashSet::new();
    for c in &cases {
        assert!(seen.insert(c.text.as_str()), "duplicate case {:?}", c.text);
        assert!(
            FREE_LABELS.contains(&c.expect.as_str()) || c.expect == "metered" || c.expect == "metered_force",
            "unknown label {:?} on {:?}",
            c.expect,
            c.text
        );
        // pre_route intercepts `/router …` before the classifier — such a case
        // would never reach classify() in the app, so it must not be here.
        assert!(parse_router_cmd(&c.text).is_none(), "{:?} is a /router command", c.text);
        assert_eq!(
            c.expect == "metered_force",
            c.text.trim_start().starts_with('!'),
            "'!' prefix must pair with metered_force: {:?}",
            c.text
        );
    }

    // ── classify every case through the app's entry point ──
    let clf = HeuristicClassifier;
    let mut per_class: Vec<(usize, usize)> = vec![(0, 0); FREE_LABELS.len()]; // (hit, total)
    let mut misses: Vec<(&str, &str)> = Vec::new(); // (expect, text) — free routed metered
    let mut wrong_action: Vec<(&str, &str, &str)> = Vec::new(); // free but wrong class
    let mut false_free: Vec<(&str, &str, &str)> = Vec::new(); // THE bug class
    let (mut metered_held, mut metered_total) = (0usize, 0usize);
    let (mut force_held, mut force_total) = (0usize, 0usize);

    for c in &cases {
        let got = observed_label(clf.classify(&c.text));
        match c.expect.as_str() {
            "metered" | "metered_force" => {
                let (held, total) = if c.expect == "metered_force" {
                    (&mut force_held, &mut force_total)
                } else {
                    (&mut metered_held, &mut metered_total)
                };
                *total += 1;
                if got == "metered" {
                    *held += 1;
                } else {
                    false_free.push((c.expect.as_str(), c.text.as_str(), got));
                }
            }
            expect => {
                let i = FREE_LABELS.iter().position(|l| *l == expect).unwrap();
                per_class[i].1 += 1;
                if got == expect {
                    per_class[i].0 += 1;
                } else if got == "metered" {
                    misses.push((expect, c.text.as_str()));
                } else {
                    wrong_action.push((expect, c.text.as_str(), got));
                }
            }
        }
    }

    // ── report ──
    println!("router intent eval — datasets/router/cases.json ({} cases)", cases.len());
    println!("entry point: HeuristicClassifier::classify (same call as router_wire::pre_route)\n");

    println!("FREE-CLASS RECALL (natural should-be-free phrasings routed to their free action)");
    let (mut hits, mut totals) = (0, 0);
    for (i, label) in FREE_LABELS.iter().enumerate() {
        let (h, t) = per_class[i];
        hits += h;
        totals += t;
        println!("  {label:<24} {h:>2}/{t:<3} {:>5.1}%", pct(h, t));
    }
    println!("  {:<24} {hits:>2}/{totals:<3} {:>5.1}%\n", "OVERALL", pct(hits, totals));

    println!("METERED PRECISION (expected-metered cases; any free-routed one is a BUG)");
    println!("  must-be-metered          {metered_held:>2}/{metered_total:<3} held");
    println!("  '!'-forced               {force_held:>2}/{force_total:<3} held");
    let prec_total = metered_total + force_total;
    let prec_held = metered_held + force_held;
    println!("  precision                {:.3}\n", prec_held as f64 / prec_total as f64);

    if !false_free.is_empty() {
        println!("*** BUG: FALSE-FREE ROUTES (metered asks answered locally — real router defects) ***");
        for (expect, text, got) in &false_free {
            println!("  [{expect}] {text:?} -> {got}");
        }
        println!();
    }
    if !wrong_action.is_empty() {
        println!("FREE-CLASS CONFUSIONS (free, but the WRONG action)");
        for (expect, text, got) in &wrong_action {
            println!("  [{expect}] {text:?} -> {got}");
        }
        println!();
    }
    println!("MISSES (should-be-free routed metered — each costs the user ~1 $LH, never a wrong answer)");
    for (expect, text) in &misses {
        println!("  [{expect:<22}] {text:?}");
    }

    println!("\nSUMMARY");
    println!(
        "  cases: {} (free-expected {totals}, metered-expected {metered_total}, '!'-forced {force_total})",
        cases.len()
    );
    println!("  free-class recall: {hits}/{totals} = {:.3}", hits as f64 / totals as f64);
    println!(
        "  metered precision: {:.3} ({} false-free)",
        prec_held as f64 / prec_total as f64,
        false_free.len()
    );
    if false_free.is_empty() {
        println!(
            "  verdict: heuristic is SAFE (precision 1.0) but leaves {:.0}% of natural \
             free-intent phrasings metered — the local-classifier headroom.",
            100.0 - pct(hits, totals)
        );
        ExitCode::SUCCESS
    } else {
        println!("  verdict: PRECISION BROKEN — fix the router before widening anything.");
        ExitCode::FAILURE
    }
}
