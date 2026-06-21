//! Source guard (tech-debt report §8): every `data-action="…"` literal a browser
//! template/handler emits MUST have a matching `=> Action::…` arm in
//! `Action::parse` — otherwise the button is dead (a click resolves to no Action).
//!
//! `src/app` is wasm32-only (`cfg(all(feature="browser-app", target_arch="wasm32"))`)
//! so this can't unit-test `Action::parse` directly on a native target. Instead it
//! cross-checks the SOURCE as text, which runs natively on every `cargo test` (and
//! so gates releases via verify.sh). Skips cleanly if `src/app` isn't present.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Every `data-action="<name>"` static literal under `src/app`.
fn emitted_actions(app_dir: &Path) -> BTreeSet<String> {
    const PREFIX: &str = "data-action=\"";
    let mut files = Vec::new();
    rs_files(app_dir, &mut files);
    let mut out = BTreeSet::new();
    for f in files {
        let src = std::fs::read_to_string(&f).unwrap_or_default();
        let mut rest = src.as_str();
        while let Some(i) = rest.find(PREFIX) {
            rest = &rest[i + PREFIX.len()..];
            if let Some(j) = rest.find('"') {
                let name = &rest[..j];
                // Real action names are kebab-case; this also drops the
                // `data-action="..."` placeholders that appear in doc comments.
                if !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                {
                    out.insert(name.to_string());
                }
                rest = &rest[j + 1..];
            } else {
                break;
            }
        }
    }
    out
}

/// Every `"<name>" => Action::…` arm in `Action::parse`.
fn parse_arms(events_src: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in events_src.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix('"') else { continue };
        let Some(j) = rest.find('"') else { continue };
        if rest[j + 1..].trim_start().starts_with("=> Action::") {
            out.insert(rest[..j].to_string());
        }
    }
    out
}

#[test]
fn every_template_data_action_has_a_parse_arm() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let app = root.join("src/app");
    if !app.exists() {
        eprintln!("skip: {} not present (packaged crate?)", app.display());
        return;
    }
    let emitted = emitted_actions(&app);
    let events =
        std::fs::read_to_string(app.join("events/mod.rs")).expect("read src/app/events/mod.rs");
    let handled = parse_arms(&events);

    // Guard the extractors themselves — an empty set would make the check vacuous.
    assert!(emitted.len() > 40, "too few data-action literals ({}) — extractor broke", emitted.len());
    assert!(handled.len() > 40, "too few parse arms ({}) — extractor broke", handled.len());

    let dead: Vec<&String> = emitted.iter().filter(|a| !handled.contains(*a)).collect();
    assert!(
        dead.is_empty(),
        "data-action(s) emitted by a template with NO `=> Action::` arm in Action::parse \
         (dead buttons): {dead:?}"
    );
}
