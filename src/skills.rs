//! Agent-defined named skills — the native-testable core of the SKILLS LOOP.
//!
//! A skill is a NAMED, reusable instruction fragment the agent defines at
//! runtime (the browser `create_skill` tool): `{ name, instructions }`. The
//! skill set is a JSON array persisted on-chain under
//! `keccak256("localharness.skills")` plus an OPFS working copy
//! (`.lh_skills.json`). [`compose_section`] folds the set into the system
//! prompt on EVERY surface — browser session, headless CLI `call`, and the
//! proxy scheduler worker — so a skill the agent teaches itself once stays
//! available across sessions and devices, and it can invoke the skill later
//! by name.
//!
//! Pure functions over the blob, no I/O — the `lessons.rs`/`raster.rs`/
//! `compose.rs` pattern of native-testable cores for browser features, so the
//! bounds (name normalization, dedup/upsert, [`MAX_SKILLS`],
//! [`MAX_INSTRUCTION_CHARS`], [`MAX_BLOB_BYTES`]) run under `cargo test`.

use serde::{Deserialize, Serialize};

/// Maximum number of retained skills — adding past the cap drops the OLDEST.
pub const MAX_SKILLS: usize = 16;

/// Maximum length of a single skill's instructions, in chars (truncated).
pub const MAX_INSTRUCTION_CHARS: usize = 600;

/// Maximum length of a skill name, in chars (truncated).
pub const MAX_NAME_CHARS: usize = 48;

/// Maximum total serialized blob size in bytes — on-chain `setMetadata` costs
/// ~8.5k gas/byte, so the blob is hard-capped; oldest skills drop first.
pub const MAX_BLOB_BYTES: usize = 4000;

/// Header line of the prompt section produced by [`compose_section`].
pub const SKILLS_HEADER: &str = "=== Your skills ===";

/// One agent-defined skill: a short named instruction fragment the agent can
/// invoke later by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    /// The invocable handle (normalized: trimmed, internal whitespace
    /// collapsed, lowercased, truncated to [`MAX_NAME_CHARS`]).
    pub name: String,
    /// The instruction/prompt fragment that defines what the skill does
    /// (trimmed, internal newlines collapsed to spaces, truncated to
    /// [`MAX_INSTRUCTION_CHARS`]).
    pub instructions: String,
}

/// Normalize a skill name: trim, collapse internal whitespace to single
/// spaces, lowercase, truncate to [`MAX_NAME_CHARS`] chars (char-boundary safe).
fn normalize_name(name: &str) -> String {
    let collapsed: String = name
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    collapsed.chars().take(MAX_NAME_CHARS).collect()
}

/// Normalize instructions: trim, collapse internal newlines to single spaces,
/// truncate to [`MAX_INSTRUCTION_CHARS`] chars (char-boundary safe).
fn normalize_instructions(instructions: &str) -> String {
    let collapsed: String = instructions
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    collapsed.chars().take(MAX_INSTRUCTION_CHARS).collect()
}

/// Parse a skills blob (JSON array of `{name, instructions}`) into a `Vec`.
/// Tolerant: a malformed / empty blob yields an empty list (so a corrupt slot
/// never bricks the session), and each entry is re-normalized + bad ones
/// dropped so parsing is idempotent with [`serialize`].
pub fn parse(blob: &str) -> Vec<Skill> {
    let raw: Vec<Skill> = serde_json::from_str(blob.trim()).unwrap_or_default();
    let mut out: Vec<Skill> = Vec::new();
    for s in raw {
        let name = normalize_name(&s.name);
        let instructions = normalize_instructions(&s.instructions);
        if name.is_empty() || instructions.is_empty() {
            continue;
        }
        // De-dup by name, last-wins (mirrors upsert semantics).
        if let Some(existing) = out.iter_mut().find(|e| e.name == name) {
            existing.instructions = instructions;
        } else {
            out.push(Skill { name, instructions });
        }
    }
    out
}

/// Serialize a skills list to a compact JSON array string.
pub fn serialize(skills: &[Skill]) -> String {
    serde_json::to_string(skills).unwrap_or_else(|_| "[]".to_string())
}

/// Enforce the count + byte bounds on a skills list IN PLACE: drop the OLDEST
/// skills until both `len <= MAX_SKILLS` and the serialized blob fits
/// [`MAX_BLOB_BYTES`]. The newest skill always survives (a single skill is
/// well under the cap).
fn enforce_bounds(skills: &mut Vec<Skill>) {
    if skills.len() > MAX_SKILLS {
        let drop = skills.len() - MAX_SKILLS;
        skills.drain(..drop);
    }
    while skills.len() > 1 && serialize(skills).len() > MAX_BLOB_BYTES {
        skills.remove(0);
    }
}

/// Add or UPSERT a skill into the `existing` blob. The name + instructions are
/// normalized (trim, collapse, truncate); an empty name OR empty instructions
/// is rejected — `existing` is parsed and re-serialized unchanged. If a skill
/// with the same (normalized) name exists, its instructions are REPLACED in
/// place (order preserved). Otherwise it appends. Bounds: only the newest
/// [`MAX_SKILLS`] are kept and the blob is capped at [`MAX_BLOB_BYTES`] bytes,
/// dropping the oldest skills first. Returns the new serialized blob.
pub fn upsert(existing: &str, name: &str, instructions: &str) -> String {
    let mut skills = parse(existing);
    let name = normalize_name(name);
    let instructions = normalize_instructions(instructions);
    if name.is_empty() || instructions.is_empty() {
        return serialize(&skills);
    }
    if let Some(s) = skills.iter_mut().find(|s| s.name == name) {
        s.instructions = instructions;
    } else {
        skills.push(Skill { name, instructions });
    }
    enforce_bounds(&mut skills);
    serialize(&skills)
}

/// Remove the skill named `name` (normalized match) from the `existing` blob.
/// Returns `(new_blob, removed)` — `removed` is false when no such skill
/// existed (the blob is still re-serialized/normalized).
pub fn remove(existing: &str, name: &str) -> (String, bool) {
    let mut skills = parse(existing);
    let name = normalize_name(name);
    let before = skills.len();
    skills.retain(|s| s.name != name);
    let removed = skills.len() != before;
    (serialize(&skills), removed)
}

/// The skill names in a blob, in stored order — for a compact `list_skills`
/// summary.
pub fn names(blob: &str) -> Vec<String> {
    parse(blob).into_iter().map(|s| s.name).collect()
}

/// Render the skills blob as a system-prompt section under [`SKILLS_HEADER`].
/// `None` when the blob has no skills — callers append nothing rather than an
/// empty header. Each skill renders as `• <name>: <instructions>`.
pub fn compose_section(blob: &str) -> Option<String> {
    let skills = parse(blob);
    if skills.is_empty() {
        return None;
    }
    let body = skills
        .iter()
        .map(|s| format!("• {}: {}", s.name, s.instructions))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "{SKILLS_HEADER}\nNamed skills you defined for yourself. Invoke one by name when its description fits the task.\n{body}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_adds_to_empty() {
        let blob = upsert("", "greet", "say hello warmly");
        let skills = parse(&blob);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "greet");
        assert_eq!(skills[0].instructions, "say hello warmly");
    }

    #[test]
    fn upsert_appends_after_existing() {
        let blob = upsert("", "a", "first");
        let blob = upsert(&blob, "b", "second");
        let skills = parse(&blob);
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "a");
        assert_eq!(skills[1].name, "b");
    }

    #[test]
    fn upsert_rejects_empty_name_or_instructions() {
        let blob = upsert("", "a", "first");
        // Empty name → unchanged.
        assert_eq!(upsert(&blob, "", "x"), blob);
        assert_eq!(upsert(&blob, "   ", "x"), blob);
        // Empty instructions → unchanged.
        assert_eq!(upsert(&blob, "b", ""), blob);
        assert_eq!(upsert(&blob, "b", "\n\r \n"), blob);
        // Empty into empty stays empty.
        assert_eq!(parse(&upsert("", "  ", "x")).len(), 0);
    }

    #[test]
    fn upsert_replaces_same_name_in_place() {
        let blob = upsert("", "a", "first");
        let blob = upsert(&blob, "b", "second");
        // Re-defining "a" REPLACES its instructions, keeps order, no dup.
        let blob = upsert(&blob, "a", "first revised");
        let skills = parse(&blob);
        assert_eq!(skills.len(), 2, "no duplicate name");
        assert_eq!(skills[0].name, "a");
        assert_eq!(skills[0].instructions, "first revised");
        assert_eq!(skills[1].name, "b");
    }

    #[test]
    fn name_normalized_for_dedup() {
        let blob = upsert("", "My Skill", "v1");
        // "  my  skill  " normalizes to the same handle "my skill" → upsert.
        let blob = upsert(&blob, "  MY   SKILL ", "v2");
        let skills = parse(&blob);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my skill");
        assert_eq!(skills[0].instructions, "v2");
    }

    #[test]
    fn instructions_collapse_newlines() {
        let blob = upsert("", "s", "line one\nline two\r\nline three");
        assert_eq!(parse(&blob)[0].instructions, "line one line two line three");
        // Blank interior lines vanish rather than doubling spaces.
        let blob = upsert("", "s", "a\n\n\nb");
        assert_eq!(parse(&blob)[0].instructions, "a b");
    }

    #[test]
    fn name_capped_and_instructions_capped() {
        let long_name = "x".repeat(200);
        let long_instr = "y".repeat(2000);
        let blob = upsert("", &long_name, &long_instr);
        let s = &parse(&blob)[0];
        assert_eq!(s.name.chars().count(), MAX_NAME_CHARS);
        assert_eq!(s.instructions.chars().count(), MAX_INSTRUCTION_CHARS);
        // Char-boundary safe with multibyte input (no panic, exact char count).
        let blob = upsert("", &"é".repeat(200), &"é".repeat(2000));
        let s = &parse(&blob)[0];
        assert_eq!(s.name.chars().count(), MAX_NAME_CHARS);
        assert_eq!(s.instructions.chars().count(), MAX_INSTRUCTION_CHARS);
    }

    #[test]
    fn upsert_keeps_only_last_max_skills() {
        let mut blob = String::new();
        for i in 0..(MAX_SKILLS + 3) {
            blob = upsert(&blob, &format!("s{i}"), &format!("instr {i}"));
        }
        let skills = parse(&blob);
        assert_eq!(skills.len(), MAX_SKILLS);
        // Oldest three dropped; newest retained, order preserved.
        assert_eq!(skills[0].name, "s3");
        assert_eq!(skills[MAX_SKILLS - 1].name, format!("s{}", MAX_SKILLS + 2));
    }

    #[test]
    fn upsert_caps_blob_bytes_dropping_oldest() {
        // Each skill ~600-char instructions → a handful blow past 4000 bytes.
        let mut blob = String::new();
        for i in 0..MAX_SKILLS {
            blob = upsert(&blob, &format!("s{i}"), &"z".repeat(MAX_INSTRUCTION_CHARS));
        }
        assert!(blob.len() <= MAX_BLOB_BYTES, "blob is {} bytes", blob.len());
        let skills = parse(&blob);
        assert!(!skills.is_empty());
        // Newest always survives.
        assert_eq!(skills.last().unwrap().name, format!("s{}", MAX_SKILLS - 1));
    }

    #[test]
    fn upsert_never_drops_the_newest_skill() {
        let mut blob = String::new();
        for i in 0..MAX_SKILLS {
            blob = upsert(&blob, &format!("s{i}"), &"z".repeat(MAX_INSTRUCTION_CHARS));
        }
        let newest_instr = "w".repeat(MAX_INSTRUCTION_CHARS);
        let blob = upsert(&blob, "newest", &newest_instr);
        let skills = parse(&blob);
        assert!(skills.iter().any(|s| s.name == "newest" && s.instructions == newest_instr));
        assert!(blob.len() <= MAX_BLOB_BYTES);
    }

    #[test]
    fn remove_existing_and_missing() {
        let blob = upsert("", "a", "first");
        let blob = upsert(&blob, "b", "second");
        // Remove existing (normalized match).
        let (blob2, removed) = remove(&blob, " A ");
        assert!(removed);
        let skills = parse(&blob2);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "b");
        // Remove missing → unchanged, removed=false.
        let (blob3, removed) = remove(&blob2, "nope");
        assert!(!removed);
        assert_eq!(parse(&blob3).len(), 1);
    }

    #[test]
    fn names_lists_in_order() {
        let blob = upsert(&upsert("", "first", "x"), "second", "y");
        assert_eq!(names(&blob), vec!["first".to_string(), "second".to_string()]);
        assert!(names("").is_empty());
    }

    #[test]
    fn parse_tolerates_garbage_and_is_idempotent() {
        assert!(parse("").is_empty());
        assert!(parse("not json").is_empty());
        assert!(parse("{}").is_empty());
        assert!(parse("[]").is_empty());
        // Entries with empty fields are dropped.
        assert!(parse(r#"[{"name":"","instructions":"x"}]"#).is_empty());
        assert!(parse(r#"[{"name":"a","instructions":""}]"#).is_empty());
        // Idempotent: parse → serialize → parse round-trips.
        let blob = upsert(&upsert("", "a", "one"), "b", "two");
        assert_eq!(serialize(&parse(&blob)), blob);
        // Duplicate names in a hand-edited blob collapse last-wins on parse.
        let dup = r#"[{"name":"a","instructions":"one"},{"name":"a","instructions":"two"}]"#;
        let skills = parse(dup);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].instructions, "two");
    }

    #[test]
    fn compose_section_none_when_empty() {
        assert_eq!(compose_section(""), None);
        assert_eq!(compose_section("[]"), None);
        assert_eq!(compose_section("garbage"), None);
    }

    #[test]
    fn compose_section_renders_header_and_skills() {
        let blob = upsert(&upsert("", "greet", "say hi"), "summarize", "condense text");
        let section = compose_section(&blob).unwrap();
        assert!(section.starts_with(SKILLS_HEADER));
        assert!(section.contains("• greet: say hi"));
        assert!(section.ends_with("• summarize: condense text"));
    }

    #[test]
    fn upsert_then_compose_round_trip() {
        let blob = upsert("", "alpha", "do alpha things");
        let section = compose_section(&blob).unwrap();
        assert!(section.contains("alpha"));
        assert!(section.contains("do alpha things"));
    }
}
