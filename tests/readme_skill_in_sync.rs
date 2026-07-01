//! README guard. The README is HAND-WRITTEN and DECOUPLED from gen-docs (no GEN
//! blocks; not a derived copy of `web/skill.md` — the `#56` "one document"
//! experiment re-bloated it into the 268-line onboarding doc and kept
//! reintroducing testnet, both rejected: telemetry #26, "worst readme ive ever
//! seen").
//!
//! As of 2026-07 the README is a SUBSTANTIVE, sectioned front door (buffa-style:
//! Why / Features / How it works / Quickstart / What it isn't / Limitations /
//! Stability) — the maintainer asked for more depth than the earlier ~30-line
//! stub. It is still GUARDED: zero testnet references, NO images/screenshots
//! (removed twice as "zombie shit"), NO GEN machinery, and a length ceiling so it
//! can't balloon back into the full manual. Exhaustive tool/CLI lists still belong
//! in docs.rs / `web/llms.txt`, never here.

use std::path::Path;

#[test]
fn readme_is_substantive_but_guarded() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme_p = root.join("README.md");
    if !readme_p.exists() {
        eprintln!("skip: README.md not present");
        return;
    }
    let readme = std::fs::read_to_string(&readme_p).expect("read README.md");
    let lower = readme.to_lowercase();

    // Substantive, but not the full manual — bounded so it can't re-bloat into a
    // copy of web/skill.md (the #56 "one document" regression).
    let lines = readme.lines().count();
    assert!(
        lines <= 220,
        "README.md is {lines} lines — it's the front door, not the manual (cap ~220). \
         Exhaustive tool/CLI lists go in docs.rs / web/llms.txt, never the README."
    );

    // NEVER testnet (telemetry #26 — "remove all the testnet stuff from the readme").
    for needle in ["testnet", "moderato", "42431", "--dev"] {
        assert!(
            !lower.contains(needle),
            "README.md mentions {needle:?} — the README must have ZERO testnet references."
        );
    }

    // No images / screenshots — a standing maintainer rule (removed twice; text only).
    assert!(
        !readme.contains("!["),
        "README.md must have NO images/screenshots (markdown `![...]`) — text only."
    );

    // No GEN-block machinery — the README is hand-written, not generated.
    assert!(
        !readme.contains("<!-- GEN:"),
        "README.md must not carry GEN blocks — it is hand-written, not gen-managed."
    );
}
