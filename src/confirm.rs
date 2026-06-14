//! Typed-confirmation challenge gate — the pure core of the
//! destructive-action convention, enforced at the DISPATCH layer.
//!
//! The prompt-level rule ("never auto-fill the confirmation") proved
//! unenforceable: a live E2E showed the model calling
//! `release_subdomain(name: "x", confirmation: "x")` in one turn, supplying
//! the confirmation itself. This module makes auto-fill impossible:
//!
//! 1. A destructive call WITHOUT a valid code never executes. The gate
//!    issues a short single-use NONCE (random — not derivable from the
//!    conversation), bound to that exact tool + arguments, and the caller
//!    relays it to the user.
//! 2. The call executes only when retried with the matching nonce for the
//!    SAME tool + arguments.
//! 3. The model sees the nonce in the tool result, so it could echo it back
//!    unprompted. To close that, the nonce must ALSO appear in the latest
//!    USER message — i.e. the user actually typed it.
//!
//! Hoisted to `src/` (the `turn_flow` pattern) so the state machine unit-tests
//! natively; the browser hook (`app::chat::confirm_guard`) supplies the
//! randomness, the last user message, and the UI surfacing.

/// Length of a confirmation code.
pub const NONCE_LEN: usize = 6;

/// Code alphabet — uppercase alphanumerics minus the ambiguous I/L/O/0/1.
const NONCE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// Map random bytes to a confirmation code (one byte consumed per char,
/// `NONCE_LEN` chars). Callers supply CSPRNG bytes (`getrandom`/`OsRng`);
/// the slight modulo bias is irrelevant at this threat level (the nonce
/// guards against MODEL auto-fill, not an offline attacker).
pub fn nonce_from_bytes(bytes: &[u8; NONCE_LEN]) -> String {
    bytes
        .iter()
        .map(|b| NONCE_ALPHABET[*b as usize % NONCE_ALPHABET.len()] as char)
        .collect()
}

/// Canonical fingerprint of one destructive action: the tool name plus its
/// arguments with the `confirmation` key removed (so the challenge call and
/// the confirming retry fingerprint identically).
pub fn fingerprint(tool: &str, args: &serde_json::Value) -> String {
    let mut stripped = args.clone();
    if let Some(map) = stripped.as_object_mut() {
        map.remove("confirmation");
    }
    format!("{tool}:{stripped}")
}

/// How one gate check resolved.
#[derive(Debug, PartialEq, Eq)]
pub enum ConfirmOutcome {
    /// No valid confirmation was supplied (missing, wrong, stale, or for
    /// different arguments). A NEW challenge was issued — any prior one is
    /// dead — and `nonce` must be relayed to the user to type.
    Challenge {
        /// The freshly issued single-use code.
        nonce: String,
    },
    /// The nonce matched the pending challenge AND appears in the latest
    /// user message: execute. The challenge is consumed (single-use).
    Approved,
    /// The nonce matched the pending challenge but does NOT appear in the
    /// latest user message — the model echoed the code without the user
    /// typing it. The challenge stays pending so the SAME code works once
    /// the user actually types it.
    NotTypedByUser,
}

struct Pending {
    fingerprint: String,
    nonce: String,
}

/// The challenge state: at most ONE pending confirmation at a time, scoped
/// to an exact action + arguments.
#[derive(Default)]
pub struct ConfirmGate {
    pending: Option<Pending>,
}

impl ConfirmGate {
    /// An empty gate (no pending challenge).
    pub fn new() -> Self {
        Self::default()
    }

    /// Run one destructive call through the gate.
    ///
    /// * `fingerprint` — [`fingerprint`] of the call being attempted.
    /// * `confirmation` — the code the model supplied (may be empty).
    /// * `last_user_msg` — the text of the most recent REAL user message.
    /// * `fresh_nonce` — a new random code, used only if a challenge is
    ///   (re)issued.
    pub fn check(
        &mut self,
        fingerprint: &str,
        confirmation: &str,
        last_user_msg: &str,
        fresh_nonce: String,
    ) -> ConfirmOutcome {
        let supplied = confirmation.trim();
        let matches_pending = self
            .pending
            .as_ref()
            .is_some_and(|p| {
                p.fingerprint == fingerprint
                    && !supplied.is_empty()
                    && supplied.eq_ignore_ascii_case(&p.nonce)
            });
        if !matches_pending {
            // Missing / wrong / stale code, or the arguments changed since
            // the challenge: burn any prior challenge (single-use; issuing
            // a new one expires the old) and issue a fresh one.
            self.pending = Some(Pending {
                fingerprint: fingerprint.to_string(),
                nonce: fresh_nonce.clone(),
            });
            return ConfirmOutcome::Challenge { nonce: fresh_nonce };
        }
        // Code + args match. The model could have copied the code out of the
        // tool result, so additionally require that the USER typed it: the
        // code must appear in the latest user message as a STANDALONE token
        // (case-insensitive). A substring `contains` would approve on an
        // incidental match inside an unrelated word/hash/URL — too weak for the
        // last line of defense on irreversible actions, so split on
        // non-alphanumerics (the alphabet is alnum) and compare whole tokens.
        if last_user_msg
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|tok| tok.eq_ignore_ascii_case(supplied))
        {
            self.pending = None; // consume — single-use
            ConfirmOutcome::Approved
        } else {
            // Keep the challenge pending: once the user actually types the
            // code, the same retry succeeds.
            ConfirmOutcome::NotTypedByUser
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(name: &str) -> String {
        fingerprint(
            "release_subdomain",
            &serde_json::json!({ "name": name, "confirmation": "anything" }),
        )
    }

    #[test]
    fn fingerprint_ignores_confirmation_arg() {
        let a = fingerprint(
            "release_subdomain",
            &serde_json::json!({ "name": "fsmoke", "confirmation": "" }),
        );
        let b = fingerprint(
            "release_subdomain",
            &serde_json::json!({ "name": "fsmoke", "confirmation": "ABC234" }),
        );
        assert_eq!(a, b);
        let c = fingerprint(
            "release_subdomain",
            &serde_json::json!({ "name": "other", "confirmation": "ABC234" }),
        );
        assert_ne!(a, c);
    }

    #[test]
    fn first_call_issues_challenge() {
        let mut gate = ConfirmGate::new();
        let out = gate.check(&fp("fsmoke"), "", "please release fsmoke", "AAAAAA".into());
        assert_eq!(out, ConfirmOutcome::Challenge { nonce: "AAAAAA".into() });
    }

    #[test]
    fn auto_filled_confirmation_cannot_skip_the_challenge() {
        // The live bug: the model invents a confirmation on the FIRST call.
        let mut gate = ConfirmGate::new();
        let out = gate.check(&fp("fsmoke"), "fsmoke", "please release fsmoke", "AAAAAA".into());
        assert_eq!(out, ConfirmOutcome::Challenge { nonce: "AAAAAA".into() });
    }

    #[test]
    fn wrong_nonce_rejected_and_reissues() {
        let mut gate = ConfirmGate::new();
        gate.check(&fp("fsmoke"), "", "msg", "AAAAAA".into());
        let out = gate.check(&fp("fsmoke"), "ZZZZZZ", "ZZZZZZ", "BBBBBB".into());
        assert_eq!(out, ConfirmOutcome::Challenge { nonce: "BBBBBB".into() });
        // The original code is now stale — reissue expired it.
        let out = gate.check(&fp("fsmoke"), "AAAAAA", "AAAAAA", "CCCCCC".into());
        assert_eq!(out, ConfirmOutcome::Challenge { nonce: "CCCCCC".into() });
    }

    #[test]
    fn args_mismatch_rejected() {
        let mut gate = ConfirmGate::new();
        gate.check(&fp("fsmoke"), "", "msg", "AAAAAA".into());
        // Right code, DIFFERENT target — must re-challenge, not execute.
        let out = gate.check(&fp("other"), "AAAAAA", "AAAAAA", "BBBBBB".into());
        assert_eq!(out, ConfirmOutcome::Challenge { nonce: "BBBBBB".into() });
    }

    #[test]
    fn model_echo_without_user_typing_is_rejected_then_user_typed_succeeds() {
        let mut gate = ConfirmGate::new();
        gate.check(&fp("fsmoke"), "", "please release fsmoke", "AAAAAA".into());
        // Model copies the code out of the tool result; the latest USER
        // message does not contain it → denied, challenge kept.
        let out = gate.check(&fp("fsmoke"), "AAAAAA", "please release fsmoke", "BBBBBB".into());
        assert_eq!(out, ConfirmOutcome::NotTypedByUser);
        // The user then actually types it (lowercase is fine) → approved.
        let out = gate.check(&fp("fsmoke"), "AAAAAA", "ok: aaaaaa", "CCCCCC".into());
        assert_eq!(out, ConfirmOutcome::Approved);
    }

    #[test]
    fn incidental_substring_does_not_approve() {
        let mut gate = ConfirmGate::new();
        gate.check(&fp("fsmoke"), "", "please release fsmoke", "AB23CD".into());
        // The code appears only as a SUBSTRING of an unrelated token (a hash),
        // never typed standalone by the user → must NOT approve.
        let out = gate.check(
            &fp("fsmoke"),
            "AB23CD",
            "see commit 9fAB23CDee for context",
            "ZZZZZZ".into(),
        );
        assert_eq!(out, ConfirmOutcome::NotTypedByUser);
        // Typed as its own token (any delimiter) → approved.
        let out = gate.check(&fp("fsmoke"), "AB23CD", "code: AB23CD.", "YYYYYY".into());
        assert_eq!(out, ConfirmOutcome::Approved);
    }

    #[test]
    fn happy_path_then_single_use() {
        let mut gate = ConfirmGate::new();
        gate.check(&fp("fsmoke"), "", "please release fsmoke", "AAAAAA".into());
        let out = gate.check(&fp("fsmoke"), "AAAAAA", "AAAAAA", "BBBBBB".into());
        assert_eq!(out, ConfirmOutcome::Approved);
        // Replaying the consumed code re-challenges instead of executing.
        let out = gate.check(&fp("fsmoke"), "AAAAAA", "AAAAAA", "CCCCCC".into());
        assert_eq!(out, ConfirmOutcome::Challenge { nonce: "CCCCCC".into() });
    }

    #[test]
    fn nonce_from_bytes_maps_into_alphabet() {
        let nonce = nonce_from_bytes(&[0, 30, 31, 61, 200, 255]);
        assert_eq!(nonce.len(), NONCE_LEN);
        for c in nonce.bytes() {
            assert!(NONCE_ALPHABET.contains(&c), "char {c} outside alphabet");
        }
        // No ambiguous glyphs by construction.
        for banned in [b'I', b'L', b'O', b'0', b'1'] {
            assert!(!NONCE_ALPHABET.contains(&banned));
        }
    }
}
