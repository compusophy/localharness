//! Conversation history persistence to OPFS.
//!
//! On mount: read `HISTORY_FILE` from OPFS. If present and non-empty,
//! stash the bytes in `App::pending_history` so the next
//! `start_session` seeds the new agent via
//! `GeminiAgentConfig::with_history_bytes`. We also project the
//! history into a flat user/assistant transcript and paint it into
//! `#transcript` so the user actually sees what was restored.
//!
//! After every successful turn: snapshot the agent's history and
//! atomically rewrite `HISTORY_FILE`. Best-effort — failures log to
//! the console but don't bubble up to the UI.

use maud::html;

use crate::backends::gemini::decode_transcript_bytes;
use crate::filesystem::Filesystem;
use crate::types::TranscriptRole;

use super::dom;
use super::templates;
use super::APP;

const HISTORY_FILE: &str = ".lh_history.json";

/// Load history bytes from OPFS into `App::pending_history`. Called
/// once at mount time. If the bytes parse, paints the prior
/// user/assistant turns into `#transcript` so the user can see what
/// the restored session contains — the agent itself isn't built yet
/// (no key applied) but the model's context will match once they send.
pub(crate) async fn load_into_pending() {
    let fs = super::shared_opfs();
    let bytes = match fs.read(HISTORY_FILE).await {
        Ok(b) if !b.is_empty() => b,
        // Empty or missing — fresh session.
        _ => return,
    };
    // Decrypt at-rest history; legacy plaintext falls through unchanged.
    let bytes = super::encryption::open(&bytes).await.unwrap_or(bytes);

    // Project the bytes into a transcript and paint each entry.
    match decode_transcript_bytes(&bytes) {
        Ok(entries) if !entries.is_empty() => {
            paint_entries(&entries);
            // Scroll so the user sees the most recent turn, not the
            // top of a long prior conversation. Deferred because the
            // restore happens before first layout/font-swap settles.
            dom::scroll_to_bottom_soon("transcript");
        }
        Ok(_) => {
            // Empty transcript — bytes existed but no user-visible content.
        }
        Err(err) => {
            // Corrupt bytes — surface but don't crash. The bytes are
            // still stashed for restore; if the model rejects them at
            // session start the user will see the error there.
            web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(&format!(
                "history decode: {err}"
            )));
        }
    }

    APP.with(|cell| cell.borrow_mut().pending_history = Some(bytes));
}

/// Snapshot the agent's history and persist it. Best-effort; logs but
/// doesn't surface errors.
pub(crate) async fn save_from_agent() {
    let bytes = APP.with(|cell| {
        cell.borrow()
            .agent
            .as_ref()
            .and_then(|a| a.history_bytes().ok().flatten())
    });
    let Some(bytes) = bytes else { return };
    let fs = super::shared_opfs();
    // Encrypt at rest; fall back to plaintext if sealing fails so we
    // never drop a snapshot.
    let data = super::encryption::seal(&bytes).await.unwrap_or(bytes);
    if let Err(err) = fs.write_atomic(HISTORY_FILE, &data).await {
        web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(&format!(
            "history save: {err}"
        )));
    }
}

/// Take any pending restored history out of the App state. The first
/// `start_session` consumes it; subsequent calls return `None`.
pub(crate) fn take_pending() -> Option<Vec<u8>> {
    APP.with(|cell| cell.borrow_mut().pending_history.take())
}

/// Paint a sequence of transcript entries into `#transcript` — tool calls
/// first, then the text turn, per entry (matching live turn order). Does
/// NOT clear `#transcript` first; the caller wipes it when replacing.
/// Shared by [`load_into_pending`] (session restore) and the compact
/// repaint in [`super::chat::run_send`] (collapse the visible scrollback
/// into the post-compaction summary).
pub(crate) fn paint_entries(entries: &[crate::types::TranscriptEntry]) {
    for entry in entries {
        // Render tool calls before the assistant text (they happened
        // during the turn, so showing them first matches the live order).
        for tc in &entry.tool_calls {
            let seg_id = APP.with(|cell| cell.borrow_mut().alloc_id());
            let call = crate::types::ToolCall {
                name: tc.name.clone(),
                id: None,
                args: tc.args.clone(),
                canonical_path: None,
            };
            let mut block = templates::tool_call_block(seg_id, &call).into_string();
            if tc.result.is_some() || tc.error.is_some() {
                // Inject the recorded result inline. The live path targets the
                // empty `#tool-{id}-result` div by id with `swap_inner`, but on
                // replay the whole block is appended at once (the div isn't in
                // the DOM yet), so we splice the result HTML into the rendered
                // string by matching the unique empty result slot. Both the
                // block and the result HTML are maud-escaped, so this is a
                // string splice of two already-safe fragments — no XSS surface.
                let result = crate::types::ToolResult {
                    name: tc.name.clone(),
                    id: None,
                    result: tc.result.clone(),
                    error: tc.error.clone(),
                };
                let result_html = templates::tool_call_result(&result).into_string();
                block = inject_result(&block, seg_id, &result_html);
            }
            // A tool with neither result nor error was in-flight when the
            // session ended; it replays with an empty result slot, matching the
            // live "no result yet" state — nothing to inject.
            dom::append_html("transcript", &block);
        }

        // Render the text turn (skip empty text-only entries that were
        // tool-call-only turns, and the internal auto-continue nudge —
        // it never paints as a bubble live, so replay must not either).
        let is_nudge = matches!(entry.role, TranscriptRole::User)
            && entry.text == super::chat::AUTO_CONTINUE_NUDGE;
        if !entry.text.is_empty() && !is_nudge {
            let turn_id = APP.with(|cell| cell.borrow_mut().alloc_id());
            let role = entry.role.as_str();
            let body = match entry.role {
                TranscriptRole::User => html! { (entry.text) },
                TranscriptRole::Assistant => templates::rendered_markdown(&entry.text),
            };
            let html_str = templates::turn(turn_id, role, body, false).into_string();
            dom::append_html("transcript", &html_str);
        }
    }
}

/// Splice `result_html` into the empty `#tool-{seg_id}-result` slot of a
/// rendered tool-call `block`. The slot is `<div id="tool-N-result"></div>`
/// (maud renders the empty div exactly so); `seg_id` is a `u32`, so the slot
/// string is unique within the block and contains no regex/escape-sensitive
/// characters. Returns the block unchanged if the slot isn't found (defensive
/// — the template shape could change), so a replay never drops the block.
///
/// Pure (no DOM, no APP) so the splice can be unit-tested without a browser.
fn inject_result(block: &str, seg_id: u32, result_html: &str) -> String {
    let slot = format!("id=\"tool-{seg_id}-result\"");
    let empty = format!("{slot}></div>");
    let filled = format!("{slot}>{result_html}</div>");
    block.replace(&empty, &filled)
}

/// Wipe the persisted conversation history (the `clear_context` tool).
/// Writes empty bytes rather than deleting: [`load_into_pending`] treats
/// empty/missing as a fresh session, and `OpfsFilesystem::delete` errors
/// on a missing file. Best-effort — logs but never surfaces to the UI.
pub(crate) async fn clear_persisted() {
    let fs = super::shared_opfs();
    if let Err(err) = fs.write_atomic(HISTORY_FILE, &[]).await {
        web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(&format!(
            "history clear: {err}"
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::inject_result;
    use crate::types::ToolResult;

    /// The exact empty slot maud renders for `div id=(result_id) {}`.
    fn empty_block(seg_id: u32) -> String {
        format!("<details id=\"tool-{seg_id}\"><div id=\"tool-{seg_id}-result\"></div></details>")
    }

    #[test]
    fn injects_result_into_the_matching_slot() {
        let block = empty_block(7);
        let out = inject_result(&block, 7, "<pre>ok</pre>");
        assert_eq!(
            out,
            "<details id=\"tool-7\"><div id=\"tool-7-result\"><pre>ok</pre></div></details>"
        );
    }

    #[test]
    fn leaves_block_untouched_when_slot_absent() {
        // Defensive: a template-shape change must not drop the block.
        let block = "<details id=\"tool-1\"><div>no slot here</div></details>";
        assert_eq!(inject_result(block, 1, "<pre>x</pre>"), block);
        // Wrong seg_id never matches the slot, so it's a no-op too.
        let b = empty_block(2);
        assert_eq!(inject_result(&b, 9, "<pre>x</pre>"), b);
    }

    #[test]
    fn only_the_targeted_seg_id_is_filled() {
        // Two adjacent tool blocks: injecting into one must not touch the other.
        let block = format!("{}{}", empty_block(3), empty_block(4));
        let out = inject_result(&block, 3, "RESULT");
        assert!(out.contains("<div id=\"tool-3-result\">RESULT</div>"));
        // seg 4's slot is left empty.
        assert!(out.contains("<div id=\"tool-4-result\"></div>"));
    }

    /// XSS: a tool RESULT containing markup must reach the DOM ESCAPED, not as
    /// live HTML. `inject_result` only splices already-rendered (maud-escaped)
    /// fragments, so the real guarantee is upstream in `tool_call_result`. This
    /// asserts that end-to-end: a malicious result value renders to escaped
    /// text, and the spliced block carries no live `<script>`/`<img>` tag.
    #[test]
    fn malicious_tool_result_is_escaped_end_to_end() {
        let evil = serde_json::json!({
            "note": "<img src=x onerror=alert(1)>",
            "more": "</pre><script>steal()</script>"
        });
        let result = ToolResult {
            name: "view_file".to_string(),
            id: None,
            result: Some(evil),
            error: None,
        };
        let result_html = super::templates::tool_call_result(&result).into_string();
        // The dangerous markup must be HTML-entity-escaped.
        assert!(result_html.contains("&lt;img"), "img tag not escaped: {result_html}");
        assert!(
            result_html.contains("&lt;script&gt;"),
            "script tag not escaped: {result_html}"
        );
        // And it must NOT contain a live, executable tag.
        assert!(!result_html.contains("<script>"), "live <script> leaked: {result_html}");
        assert!(
            !result_html.contains("<img src=x"),
            "live <img> leaked: {result_html}"
        );

        // Splicing it into a block preserves the escaping (it's a plain string
        // replace of two safe fragments — no re-parsing, no unescaping).
        let block = empty_block(5);
        let spliced = inject_result(&block, 5, &result_html);
        assert!(!spliced.contains("<script>"), "splice leaked live <script>");
        assert!(spliced.contains("&lt;script&gt;"), "splice lost escaping");
    }

    /// A tool ERROR string is also escaped (the error branch of the template).
    #[test]
    fn malicious_tool_error_is_escaped() {
        let result = ToolResult {
            name: "create_file".to_string(),
            id: None,
            result: None,
            error: Some("<svg onload=alert(1)>boom".to_string()),
        };
        let html = super::templates::tool_call_result(&result).into_string();
        assert!(html.contains("&lt;svg"), "svg not escaped: {html}");
        assert!(!html.contains("<svg onload"), "live <svg> leaked: {html}");
    }
}
