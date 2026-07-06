//! Source guard (a11y, feedback #75): the templates must keep the screen-reader
//! live regions — the streaming transcript (`role="log"` + polite aria-live) and
//! a `role="status"` on every async tx/status message slot — and must NOT gain
//! an `aria-busy` that would suppress announcements mid-stream.
//!
//! `src/app` is wasm32-only, so (like `chat_toolset_single_source.rs`) this
//! checks the SOURCE as text on a native `cargo test`. Skips if absent.

use std::path::Path;

#[test]
fn templates_keep_live_regions() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app/templates.rs");
    if !p.exists() {
        eprintln!("skip: {} not present (packaged crate?)", p.display());
        return;
    }
    let src = std::fs::read_to_string(&p).expect("read src/app/templates.rs");

    // The streaming transcript is THE live region for assistant output.
    assert!(
        src.contains(r#"div #transcript .transcript role="log" aria-live="polite""#),
        "#transcript must stay a polite role=log live region (feedback #75)"
    );
    // aria-busy=true suppresses live announcements — never add it to templates.
    assert!(!src.contains("aria-busy="), "no aria-busy in templates (it mutes live regions)");

    // Every async tx/status sink stays a polite live region. One assert per id
    // so a regression names the exact slot it dropped.
    for id in [
        "buy-checkout-msg", "onboard-checkout-msg", "feedback-msg", "buy-msg",
        "credits-msg", "pair-msg", "install-msg", "prompt-msg", "model-msg",
        "x402-price-msg", "publish-app-msg", "tool-allowlist-msg", "identity-msg",
        "seed-msg", "invite-result", "adopt-msg", "reset-confirm-msg", "claim-msg",
        "api-key-msg", "turn-status", "fund-banner", "status",
    ] {
        assert!(
            src.lines().any(|l| l.contains(&format!("#{id} ")) && l.contains(r#"role="status""#)),
            "#{id} must carry role=\"status\" so its tx/status swaps are announced (feedback #75)"
        );
    }

    // The banner-embedded #fund-msg nests inside the #fund-banner live region;
    // exactly ONE of the two #fund-msg declarations is itself role=status.
    let fund_status = src
        .lines()
        .filter(|l| l.contains("#fund-msg ") && l.contains(r#"role="status""#))
        .count();
    assert_eq!(fund_status, 1, "one standalone #fund-msg live region (the banner one is nested)");
}
