//! Sync guard (tech-debt report §4): `AGENTS.md` and `CLAUDE.md` are the SAME
//! project map for two agents (Codex / Claude Code). Keeping both hand-edited let
//! them drift — a blanket Claude->Codex replace once turned the factual "Claude
//! Messages API" reference into a nonexistent "Codex Messages API", misinforming
//! whichever agent read AGENTS.md.
//!
//! This asserts that `AGENTS.md`, with ONLY its intentional per-agent
//! substitutions undone, is byte-identical to `CLAUDE.md`. A new section, or a bad
//! find-replace, in one but not the other now fails `cargo test`. Skips if either
//! file is absent (packaged crate).

use std::path::Path;

#[test]
fn agents_md_stays_in_sync_with_claude_md() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let agents_p = root.join("AGENTS.md");
    let claude_p = root.join("CLAUDE.md");
    if !agents_p.exists() || !claude_p.exists() {
        eprintln!("skip: AGENTS.md / CLAUDE.md not present");
        return;
    }
    let agents = std::fs::read_to_string(&agents_p).expect("read AGENTS.md");
    let claude = std::fs::read_to_string(&claude_p).expect("read CLAUDE.md");

    // The ONLY legitimate differences are the agent-name substitutions: the title
    // + "for <agent> sessions" line, and the doc's self-references (`AGENTS.md` ->
    // `CLAUDE.md`). Undo them, then require exact equality.
    let normalized = agents
        .replace("Codex sessions", "Claude Code sessions")
        .replace("AGENTS.md", "CLAUDE.md");

    assert_eq!(
        normalized, claude,
        "AGENTS.md drifted from CLAUDE.md beyond the intended agent-name \
         substitutions (title, 'Codex sessions', self-references). Edit BOTH in \
         lockstep, or fix the find-replace damage."
    );
}
