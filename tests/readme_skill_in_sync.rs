//! Sync guard (maintainer feedback #56): `README.md` and `web/skill.md` are ONE
//! document. They used to be independently hand-written — the README minimal,
//! skill.md the agent onboarding page — which let them drift apart and grow the
//! "major dup issues" the maintainer flagged. Now `web/skill.md` is the SOURCE
//! and `README.md` is a pure DERIVED COPY of its filled output (`gen-docs` fills
//! skill.md's GEN blocks and writes the result to README.md).
//!
//! This asserts README.md is byte-identical to the FILLED skill.md, so a stray
//! hand-edit to either — or forgetting to rerun `gen-docs` — fails `cargo test`.
//! Requires `--features wallet` (the manifest reads `registry::chain`). Skips if
//! a file is absent (packaged crate).

#![cfg(feature = "wallet")]

use localharness::docs_manifest;
use std::path::Path;

#[test]
fn readme_is_identical_to_filled_skill_md() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let skill_p = root.join("web/skill.md");
    let readme_p = root.join("README.md");
    if !skill_p.exists() || !readme_p.exists() {
        eprintln!("skip: README.md / web/skill.md not present");
        return;
    }
    let skill = std::fs::read_to_string(&skill_p).expect("read web/skill.md");
    let readme = std::fs::read_to_string(&readme_p).expect("read README.md");

    // README.md must equal skill.md with its GEN blocks filled (the derived
    // form gen-docs writes). Comparing against the FILLED source also catches a
    // skill.md whose own GEN blocks are stale.
    let (filled_skill, _) = docs_manifest::fill(&skill);

    assert_eq!(
        readme, filled_skill,
        "README.md drifted from web/skill.md. They are ONE document (#56): edit \
         web/skill.md, then run `cargo run --bin gen-docs --features wallet` to \
         resync README.md."
    );
}
