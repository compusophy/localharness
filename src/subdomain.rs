//! Pure subdomain-name validation — the single source of truth for the browser
//! create tools, kept in sync with the on-chain
//! `LocalharnessRegistryFacet._isValidName` rule. Native-tested (this is why it
//! lives at the crate root, not inside the wasm-only `app` module).
//!
//! The bug this closes (GitHub #66/#60): the create path used to `sanitize()` a
//! requested name by silently DROPPING any char outside `[a-z0-9-]`, so asking
//! to register `café-shop` quietly minted `caf-shop` — a DIFFERENT name than
//! requested — and a leading/trailing hyphen sailed past the client only to
//! revert on-chain. `validate` instead REJECTS a name that isn't already a
//! valid DNS-safe label, returning a human-readable reason the caller (the
//! AGENT, via a tool error) can act on, rather than guessing.

/// Is `name` a routable DNS label (the registry/DNS-gateway invariant)?
///
/// THE canonical rule, shared by every mint path so no caller can spend
/// sponsored gas on an unroutable "zombie" name (on-chain feedback, juno-qa:
/// the registry minted labels >63 chars that the DNS gateway then silently
/// choked on). A valid label is **1–63 chars** of `[a-z0-9-]` with no
/// leading/trailing hyphen and ASCII only — RFC 1035, matching the contract's
/// `_isValidName`. Takes the name AS-IS (no normalization): a caller wanting
/// "normalize-or-reject" semantics uses [`validate`], which is strictly
/// tighter (it also caps at 32 and lowercases first). The CLI's
/// `name_is_valid` delegates here so the binary and the browser agree.
pub fn is_valid_subdomain_label(name: &str) -> bool {
    let len = name.len(); // ASCII past the all-ascii char check, so byte len == char len
    (1..=63).contains(&len)
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Validate + normalize a requested subdomain label.
///
/// Lowercases and trims (unsurprising normalization), then requires the result
/// to be a valid label: 3–32 chars, `[a-z0-9-]` only, no leading/trailing
/// hyphen, ASCII only. Returns the normalized name, or `Err(reason)` describing
/// the first violation (surfaced to the agent as a tool error — NOT painted as
/// form text). The 3–32 bound matches the app's existing create range; the
/// character/hyphen rule matches the contract's `_isValidName` and the
/// canonical [`is_valid_subdomain_label`] (which this is strictly tighter than).
pub fn validate(input: &str) -> Result<String, String> {
    let name = input.trim().to_ascii_lowercase();
    if !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return Err(
            "use only lowercase letters, digits, and hyphens — no spaces, dots, or accented/unicode characters"
                .to_string(),
        );
    }
    // char count == byte count here (all-ASCII past the check above), but count
    // chars for a correct message regardless.
    let len = name.chars().count();
    if !(3..=32).contains(&len) {
        return Err(format!("name must be 3–32 characters (got {len})"));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err("name can't start or end with a hyphen".to_string());
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_labels() {
        assert_eq!(validate("alice").unwrap(), "alice");
        assert_eq!(validate("foo-bar").unwrap(), "foo-bar");
        assert_eq!(validate("a1b2c3").unwrap(), "a1b2c3");
        assert_eq!(validate(&"a".repeat(32)).unwrap(), "a".repeat(32));
    }

    #[test]
    fn normalizes_case_and_whitespace_unsurprisingly() {
        assert_eq!(validate("  Alice  ").unwrap(), "alice");
        assert_eq!(validate("MyAgent2").unwrap(), "myagent2");
    }

    #[test]
    fn rejects_unicode_instead_of_silently_mangling() {
        // The #66 repro: this used to mint "caf-shop"; now it's a clear error.
        assert!(validate("café-shop").is_err());
        assert!(validate("日本").is_err());
        assert!(validate("über").is_err());
    }

    #[test]
    fn rejects_spaces_and_dots() {
        assert!(validate("my cool app").is_err());
        assert!(validate("a.b.c").is_err());
        assert!(validate("under_score").is_err());
    }

    #[test]
    fn rejects_bad_length() {
        assert!(validate("ab").is_err()); // too short
        assert!(validate(&"a".repeat(33)).is_err()); // too long
        assert!(validate("").is_err());
    }

    #[test]
    fn rejects_edge_hyphens() {
        assert!(validate("-alice").is_err());
        assert!(validate("alice-").is_err());
        assert!(validate("--").is_err());
    }

    #[test]
    fn label_rule_blocks_unroutable_names() {
        // The juno-qa bug: a >63-char label is unroutable; the canonical rule
        // (1–63) must reject it BEFORE any mint spends sponsored gas.
        assert!(is_valid_subdomain_label("alice"));
        assert!(is_valid_subdomain_label("a")); // single char is a valid label
        assert!(is_valid_subdomain_label("a-b-c"));
        assert!(is_valid_subdomain_label(&"a".repeat(63))); // exactly the cap
        assert!(!is_valid_subdomain_label(&"a".repeat(64))); // the zombie — too long
        assert!(!is_valid_subdomain_label("")); // empty
        assert!(!is_valid_subdomain_label("Alice")); // uppercase
        assert!(!is_valid_subdomain_label("a_b")); // underscore
        assert!(!is_valid_subdomain_label("café")); // non-ascii
        assert!(!is_valid_subdomain_label("-foo")); // leading hyphen
        assert!(!is_valid_subdomain_label("foo-")); // trailing hyphen
        assert!(!is_valid_subdomain_label("-")); // only a hyphen
    }

    #[test]
    fn validate_is_strictly_tighter_than_the_label_rule() {
        // Anything `validate` accepts is a routable label (the mint invariant);
        // `validate` additionally caps at 32 and lowercases first.
        for ok in ["alice", "foo-bar", &"a".repeat(32)] {
            let normalized = validate(ok).unwrap();
            assert!(
                is_valid_subdomain_label(&normalized),
                "validate accepted an unroutable label: {normalized}"
            );
        }
        // 33–63 chars pass the label rule but `validate` rejects (the 32 cap).
        assert!(is_valid_subdomain_label(&"a".repeat(40)));
        assert!(validate(&"a".repeat(40)).is_err());
    }
}
