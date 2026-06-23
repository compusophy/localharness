//! README minimalism guard (maintainer feedback — the user was FURIOUS when the
//! README became a bloated copy of the full docs: "worst readme ive ever seen").
//!
//! The README is the FRONT DOOR, not the manual. It is HAND-WRITTEN and minimal;
//! it is deliberately NOT coupled to `web/skill.md` (an earlier `#56` "one
//! document" experiment re-bloated it back to the 268-line onboarding doc and
//! kept re-introducing testnet, which the maintainer rejected — telemetry #26).
//!
//! This asserts the README stays small + clean so a future change can't quietly
//! turn it back into the full doc: no testnet, no GEN-block machinery, bounded
//! length. Detail belongs in docs.rs + `web/llms.txt`, not here.

use std::path::Path;

#[test]
fn readme_stays_minimal_and_testnet_free() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme_p = root.join("README.md");
    if !readme_p.exists() {
        eprintln!("skip: README.md not present");
        return;
    }
    let readme = std::fs::read_to_string(&readme_p).expect("read README.md");
    let lower = readme.to_lowercase();

    // The front door, not the manual: keep it short.
    let lines = readme.lines().count();
    assert!(
        lines <= 60,
        "README.md is {lines} lines — keep it MINIMAL (~30, a front door, not the \
         manual). Detail goes in docs.rs / web/llms.txt, never the README."
    );

    // NEVER testnet (telemetry #26 — "remove all the testnet stuff from the readme").
    for needle in ["testnet", "moderato", "42431", "--dev"] {
        assert!(
            !lower.contains(needle),
            "README.md mentions {needle:?} — the README must have ZERO testnet references."
        );
    }

    // No GEN-block machinery — the README is hand-written, not generated.
    assert!(
        !readme.contains("<!-- GEN:"),
        "README.md must not carry GEN blocks — it is hand-written, not gen-managed."
    );
}
