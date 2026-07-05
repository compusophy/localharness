//! Pure decision core for the seed-pull apex round-trip legs (native-tested;
//! the `turn_flow` hoisting pattern). The wasm plumbing lives in
//! `src/app/seed_pull.rs` + the mount routing in `src/app/mod.rs`; THIS module
//! owns the two decisions that used to cause a visible face repaint for every
//! pure visitor:
//!
//! - **Return leg** ([`import_action`] / [`should_repaint`]): only an actual
//!   sealed-seed payload (`?seed_import=1#s=…`) goes through the
//!   import-interstitial + repaint path. A `?seed_import=none` bounce (the
//!   common pure-visitor case) or a payload-less `1` must NOT — the mount
//!   scrubs the URL and falls through to the single normal paint.
//! - **Apex bounce** ([`none_bounce`]): when the apex has nothing to hand
//!   over, it goes BACK in history (bfcache restores the visitor's
//!   already-painted face with zero repaint; a cache miss reloads the clean
//!   URL) instead of a forward `?seed_import=none` navigation. The forward
//!   nav survives only as the fallback for a tab with no history to go back
//!   to (a hand-typed export URL).

/// What the tenant mount should do with a `?seed_import=…` return leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportAction {
    /// `?seed_import=1` with a sealed payload: paint the import interstitial,
    /// import the seed into this origin's OPFS, then repaint (the owner
    /// adoption/upgrade path — unchanged).
    ImportAndRepaint,
    /// Nothing usable came back (`none`, unknown mode, or `1` without a
    /// payload): scrub the URL + ephemeral key and fall through to the ONE
    /// normal tenant paint. No interstitial, no extra repaint.
    ScrubOnly,
}

/// Classify a return leg. `mode` = the `seed_import` query value (`None` when
/// the param is absent → not a return leg at all → `None`); `has_payload` =
/// a `#s=<ct>` fragment is present.
pub fn import_action(mode: Option<&str>, has_payload: bool) -> Option<ImportAction> {
    let mode = mode?;
    if mode == "1" && has_payload {
        Some(ImportAction::ImportAndRepaint)
    } else {
        Some(ImportAction::ScrubOnly)
    }
}

/// `true` iff this return leg carries an actual seed and must run the
/// import + repaint path. Everything else leaves the paint flow alone.
pub fn should_repaint(mode: Option<&str>, has_payload: bool) -> bool {
    import_action(mode, has_payload) == Some(ImportAction::ImportAndRepaint)
}

/// How the apex should bounce when it has NO seed to hand over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoneBounce {
    /// `history.back()` — restores the subdomain's already-painted face from
    /// bfcache (zero repaint) or reloads the clean URL on a cache miss. The
    /// kick always navigated in-tab, so the subdomain is the previous entry.
    Back,
    /// Forward-navigate to `?seed_import=none` — only when there is nothing
    /// behind us (hand-typed export URL in a fresh tab).
    ForwardNone,
}

/// Decide the no-seed bounce from the tab's history length.
pub fn none_bounce(history_len: u32) -> NoneBounce {
    if history_len > 1 {
        NoneBounce::Back
    } else {
        NoneBounce::ForwardNone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_param_is_not_a_return_leg() {
        assert_eq!(import_action(None, false), None);
        assert_eq!(import_action(None, true), None);
        assert!(!should_repaint(None, true));
    }

    #[test]
    fn sealed_payload_runs_the_import_repaint_path() {
        assert_eq!(import_action(Some("1"), true), Some(ImportAction::ImportAndRepaint));
        assert!(should_repaint(Some("1"), true));
    }

    #[test]
    fn none_bounce_scrubs_without_repaint() {
        assert_eq!(import_action(Some("none"), false), Some(ImportAction::ScrubOnly));
        assert!(!should_repaint(Some("none"), false));
        // a stray fragment on a `none` bounce still must not repaint
        assert!(!should_repaint(Some("none"), true));
    }

    #[test]
    fn payloadless_or_junk_modes_scrub_without_repaint() {
        assert_eq!(import_action(Some("1"), false), Some(ImportAction::ScrubOnly));
        assert!(!should_repaint(Some("1"), false));
        assert!(!should_repaint(Some("2"), true));
        assert!(!should_repaint(Some(""), true));
    }

    #[test]
    fn apex_goes_back_when_history_allows() {
        assert_eq!(none_bounce(2), NoneBounce::Back);
        assert_eq!(none_bounce(10), NoneBounce::Back);
        assert_eq!(none_bounce(1), NoneBounce::ForwardNone);
        assert_eq!(none_bounce(0), NoneBounce::ForwardNone);
    }
}
