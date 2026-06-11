//! Self-recorded agent lessons — the native-testable core of the LESSONS LOOP.
//!
//! Agents record one short lesson after a REAL error, failed tool call, or
//! user correction (the browser `record_lesson` tool); the lessons blob is
//! plain text, ONE lesson per line, persisted on-chain under
//! `keccak256("localharness.lessons")` plus an OPFS working copy
//! (`.lh_lessons.txt`). [`compose_section`] folds it into the system prompt
//! on EVERY surface — browser session, headless CLI `call`, and the proxy
//! scheduler worker — so a corrected mistake stays corrected across sessions
//! and devices.
//!
//! Pure functions over the blob, no I/O — the `raster.rs`/`compose.rs`
//! pattern of native-testable cores for browser features, so the bounds
//! (dedup, last-[`MAX_LESSONS`], [`MAX_BLOB_BYTES`]) run under `cargo test`.

/// Maximum number of retained lessons — the newest win; older ones drop.
pub const MAX_LESSONS: usize = 10;

/// Maximum length of a single lesson, in chars (longer lessons truncate).
pub const MAX_LESSON_CHARS: usize = 240;

/// Maximum total blob size in bytes — on-chain `setMetadata` costs
/// ~8.5k gas/byte, so the blob is hard-capped; oldest lines drop first.
pub const MAX_BLOB_BYTES: usize = 2000;

/// Header line of the prompt section produced by [`compose_section`].
pub const LESSONS_HEADER: &str = "=== Lessons (self-recorded) ===";

/// Normalize one lesson: trim, collapse internal newlines to single spaces,
/// truncate to [`MAX_LESSON_CHARS`] chars (char-boundary safe).
fn normalize(lesson: &str) -> String {
    let collapsed: String = lesson
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    collapsed.chars().take(MAX_LESSON_CHARS).collect()
}

/// Parse a lessons blob into its non-empty, trimmed lines.
fn lines_of(blob: &str) -> Vec<&str> {
    blob.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect()
}

/// Merge `new_lesson` into the `existing` lessons blob (one lesson per line).
///
/// The new lesson is trimmed, internal newlines collapse to spaces, and it is
/// truncated to [`MAX_LESSON_CHARS`] chars. An empty lesson, or an EXACT
/// duplicate of an existing line, is rejected — `existing` is returned
/// unchanged. Otherwise it appends; only the LAST [`MAX_LESSONS`] lessons are
/// kept, and the whole blob is capped at [`MAX_BLOB_BYTES`] bytes by dropping
/// the oldest lines until it fits.
pub fn merge_lesson(existing: &str, new_lesson: &str) -> String {
    let lesson = normalize(new_lesson);
    let mut lines = lines_of(existing);
    if lesson.is_empty() || lines.iter().any(|l| *l == lesson) {
        return existing.to_string();
    }
    lines.push(&lesson);
    // Keep only the LAST MAX_LESSONS entries.
    if lines.len() > MAX_LESSONS {
        lines.drain(..lines.len() - MAX_LESSONS);
    }
    // Byte cap: drop the OLDEST lines until the joined blob fits. A single
    // lesson is <= 240 chars (<= 960 bytes UTF-8), so this always terminates
    // with at least the newest lesson retained.
    let joined_len =
        |ls: &[&str]| ls.iter().map(|l| l.len()).sum::<usize>() + ls.len().saturating_sub(1);
    while lines.len() > 1 && joined_len(&lines) > MAX_BLOB_BYTES {
        lines.remove(0);
    }
    lines.join("\n")
}

/// Render the lessons blob as a system-prompt section under
/// [`LESSONS_HEADER`]. `None` when the blob has no lessons — callers append
/// nothing rather than an empty header.
pub fn compose_section(lessons: &str) -> Option<String> {
    let lines = lines_of(lessons);
    if lines.is_empty() {
        return None;
    }
    Some(format!("{LESSONS_HEADER}\n{}", lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_appends_to_empty() {
        assert_eq!(merge_lesson("", "always compile first"), "always compile first");
    }

    #[test]
    fn merge_appends_after_existing() {
        let blob = merge_lesson("lesson one", "lesson two");
        assert_eq!(blob, "lesson one\nlesson two");
    }

    #[test]
    fn merge_rejects_empty_and_whitespace() {
        assert_eq!(merge_lesson("a", ""), "a");
        assert_eq!(merge_lesson("a", "   "), "a");
        assert_eq!(merge_lesson("a", "\n\r\n  \n"), "a");
        // Empty into empty stays empty.
        assert_eq!(merge_lesson("", "  "), "");
    }

    #[test]
    fn merge_rejects_exact_duplicate() {
        let existing = "lesson one\nlesson two";
        assert_eq!(merge_lesson(existing, "lesson one"), existing);
        assert_eq!(merge_lesson(existing, "lesson two"), existing);
        // Whitespace-padded duplicate is still a duplicate after trimming.
        assert_eq!(merge_lesson(existing, "  lesson one  "), existing);
        // A duplicate that only matches AFTER newline collapsing is rejected too.
        assert_eq!(merge_lesson(existing, "lesson\none"), existing);
        // `existing` comes back byte-identical, untouched.
        let messy = "  lesson one  \n\nlesson two";
        assert_eq!(merge_lesson(messy, "lesson one"), messy);
    }

    #[test]
    fn merge_collapses_internal_newlines() {
        let blob = merge_lesson("", "first part\nsecond part\r\nthird part");
        assert_eq!(blob, "first part second part third part");
        // Blank interior lines vanish rather than doubling spaces.
        let blob = merge_lesson("", "a\n\n\nb");
        assert_eq!(blob, "a b");
    }

    #[test]
    fn merge_caps_lesson_at_240_chars() {
        let long = "x".repeat(500);
        let blob = merge_lesson("", &long);
        assert_eq!(blob.chars().count(), MAX_LESSON_CHARS);
        // Char-boundary safe with multibyte input (no panic, exact char count).
        let emoji = "é".repeat(500);
        let blob = merge_lesson("", &emoji);
        assert_eq!(blob.chars().count(), MAX_LESSON_CHARS);
    }

    #[test]
    fn merge_truncation_then_duplicate_check() {
        // Two long lessons identical in their first 240 chars normalize to the
        // same line — the second is a duplicate and is rejected.
        let a = format!("{}{}", "x".repeat(240), "AAA");
        let b = format!("{}{}", "x".repeat(240), "BBB");
        let blob = merge_lesson("", &a);
        assert_eq!(merge_lesson(&blob, &b), blob);
    }

    #[test]
    fn merge_keeps_only_last_10() {
        let mut blob = String::new();
        for i in 0..12 {
            blob = merge_lesson(&blob, &format!("lesson {i}"));
        }
        let lines: Vec<&str> = blob.lines().collect();
        assert_eq!(lines.len(), MAX_LESSONS);
        // Oldest two dropped; newest retained, order preserved.
        assert_eq!(lines[0], "lesson 2");
        assert_eq!(lines[9], "lesson 11");
    }

    #[test]
    fn merge_caps_blob_at_2000_bytes_dropping_oldest() {
        // Ten 240-char lessons = 2409 bytes joined — over the cap, so the
        // oldest lines must drop until the blob fits.
        let mut blob = String::new();
        for i in 0..10 {
            blob = merge_lesson(&blob, &format!("{i}{}", "x".repeat(239)));
        }
        assert!(blob.len() <= MAX_BLOB_BYTES, "blob is {} bytes", blob.len());
        let lines: Vec<&str> = blob.lines().collect();
        assert_eq!(lines.len(), 8, "two oldest dropped to fit 2000 bytes");
        assert!(lines[0].starts_with('2'), "oldest surviving lesson is #2");
        assert!(lines[7].starts_with('9'), "newest lesson always survives");
    }

    #[test]
    fn merge_never_drops_the_newest_lesson() {
        // Even when existing is near the cap, the just-recorded lesson stays.
        let mut blob = String::new();
        for i in 0..9 {
            blob = merge_lesson(&blob, &format!("{i}{}", "y".repeat(239)));
        }
        let newest = "z".repeat(240);
        let merged = merge_lesson(&blob, &newest);
        assert!(merged.lines().any(|l| l == newest));
        assert!(merged.len() <= MAX_BLOB_BYTES);
    }

    #[test]
    fn merge_normalizes_messy_existing_blob() {
        // Blank/padded lines in a hand-edited blob are dropped on merge.
        let merged = merge_lesson("  a  \n\n\nb\n", "c");
        assert_eq!(merged, "a\nb\nc");
    }

    #[test]
    fn compose_section_none_when_empty() {
        assert_eq!(compose_section(""), None);
        assert_eq!(compose_section("   \n \n"), None);
    }

    #[test]
    fn compose_section_renders_header_and_lines() {
        let s = compose_section("lesson one\nlesson two").unwrap();
        assert_eq!(s, "=== Lessons (self-recorded) ===\nlesson one\nlesson two");
        // Blank lines in the blob don't leak into the prompt.
        let s = compose_section("\na\n\nb\n").unwrap();
        assert_eq!(s, format!("{LESSONS_HEADER}\na\nb"));
    }

    #[test]
    fn merge_then_compose_round_trip() {
        let blob = merge_lesson(&merge_lesson("", "first"), "second");
        let section = compose_section(&blob).unwrap();
        assert!(section.starts_with(LESSONS_HEADER));
        assert!(section.contains("first"));
        assert!(section.ends_with("second"));
    }
}
