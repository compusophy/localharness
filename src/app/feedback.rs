//! Feedback: the write-only `[feedback]` modal + its OPFS mirror. Submissions
//! flow OFF-CHAIN through `telemetry::report_feedback` → proxy `/api/telemetry`
//! → a GitHub issue in the private telemetry repo (the task list). The old
//! on-chain `FeedbackFacet.submitFeedback` path (and its `lh_feedback_onchain`
//! opt-in) is REMOVED — don't reintroduce a sponsored on-chain feedback write.

use wasm_bindgen::prelude::*;

use super::dom;

/// Validate + rate-limit the feedback textarea, mirror it to OPFS, and file it
/// off-chain via telemetry.
pub(crate) fn feedback_submit() {
    let Some(textarea) = dom::textarea_by_id("feedback-text") else {
        return;
    };
    let text = textarea.value().trim().to_string();
    if text.is_empty() {
        return; // silent no-op per [[feedback-no-explanatory-validation]]
    }

    // Client-side rate limit: one submission per 60 seconds.
    thread_local! {
        static LAST_FEEDBACK_MS: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
    }
    let now = js_sys::Date::now();
    let elapsed = LAST_FEEDBACK_MS.with(|c| now - c.get());
    if elapsed < 60_000.0 {
        let remaining = ((60_000.0 - elapsed) / 1000.0).ceil() as u32;
        dom::swap_inner(
            "feedback-msg",
            &dom::msg_span(dom::Msg::Muted, &format!("wait {remaining}s")),
        );
        return;
    }
    LAST_FEEDBACK_MS.with(|c| c.set(now));

    dom::swap_inner("feedback-msg", &dom::msg_span(dom::Msg::Muted, "sending…"));
    wasm_bindgen_futures::spawn_local(async move {
        // Mirror to local OPFS first so the user always has a copy even if the
        // network leg fails. Best-effort — log and continue on error.
        if let Err(err) = append_feedback_local(&text).await {
            web_sys::console::warn_1(&JsValue::from_str(&format!("feedback local copy: {err}")));
        }
        let agent = super::tenant::current_name().unwrap_or_else(|| "apex".to_string());
        // The rich off-chain report is THE record — full device/settings/
        // conversation context. This is a deliberate user action, so it always
        // sends (independent of the auto-telemetry toggle).
        super::telemetry::report_feedback(agent, text.clone()).await;
        dom::swap_inner("feedback-msg", &dom::msg_span(dom::Msg::Accent, "✓ sent"));
        // Clear the textarea — leaving the sent text in place made a second
        // SUBMIT click double-file the same note.
        if let Some(textarea) = dom::textarea_by_id("feedback-text") {
            textarea.set_value("");
        }
    });
}

/// Append a feedback entry to `.lh_feedback.txt` in this origin's OPFS as a
/// local-first mirror (the canonical record is the telemetry issue). One line
/// per entry: `ISO-timestamp\tTEXT\n`.
async fn append_feedback_local(text: &str) -> Result<(), String> {
    let fs = super::shared_opfs();
    let existing = fs.read(".lh_feedback.txt").await.unwrap_or_default();
    let now = js_sys::Date::new_0().to_iso_string().as_string().unwrap_or_default();
    let entry = format!("{now}\t{text}\n");
    let mut combined = existing;
    combined.extend_from_slice(entry.as_bytes());
    fs.write_atomic(".lh_feedback.txt", &combined)
        .await
        .map_err(|e| format!("{e}"))
}
