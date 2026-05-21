//! Context-window compaction.
//!
//! When the running token count for a turn exceeds
//! `CapabilitiesConfig::compaction_threshold`, we trim history. The
//! strategy:
//!
//! 1. Keep the system instruction (it lives outside `history`).
//! 2. Keep the most-recent `KEEP_RECENT_TURNS` user/model turn pairs
//!    verbatim — function-call/response pairs are kept together.
//! 3. Ask Gemini to summarize everything before the keep-window into a
//!    single short paragraph.
//! 4. Replace that prefix with one synthetic user-role turn containing
//!    the summary, tagged so future readers know it's a compaction
//!    artifact.
//!
//! If summarization fails (network error, missing client) we fall back
//! to dropping the oldest turns until we're under the keep-window. The
//! agent never errors out of a turn because of a compaction failure —
//! the dispatch loop logs at WARN and continues.

use parking_lot::Mutex;
use tracing::{debug, warn};

use crate::backends::gemini::api::SharedClient;
use crate::backends::gemini::wire::{
    Content, ContentRole, FinishReason, GenerateContentRequest, Part,
};
use crate::error::Result;

/// Tag prepended to a compaction summary so the model (and humans
/// inspecting history) can tell what they're looking at.
pub const COMPACTION_TAG: &str = "[compacted prior context]";

/// How many recent user/model turn pairs we always keep verbatim. The
/// model needs immediate context — a hard ceiling that's not too
/// stingy (don't break a multi-step tool use) and not too generous
/// (don't defeat the point of compaction).
const KEEP_RECENT_TURNS: usize = 6;

/// Pre-summary check: if history has fewer than this many entries
/// past the keep-window, compaction is a no-op (nothing to gain).
const MIN_HISTORY_TO_COMPACT: usize = 8;

/// Model used for summarization. Cheap + fast is right here — the
/// summary doesn't need to be brilliant, just faithful.
const SUMMARY_PROMPT: &str = "Summarize the conversation below in 200 words or less. \
    Preserve key facts, decisions, file paths, and any user requests. Drop greetings, \
    chit-chat, and redundant tool output. Output only the summary; no preamble.";

/// Try to compact `history` in place. Returns `true` if anything
/// changed. Safe to call from inside the agent loop — never errors out,
/// only logs.
pub async fn try_compact(
    history: &Mutex<Vec<Content>>,
    client: &SharedClient,
    model: &str,
) -> bool {
    let snapshot = history.lock().clone();
    let total = snapshot.len();
    if total < MIN_HISTORY_TO_COMPACT {
        debug!(total, "compaction: history too short, skipping");
        return false;
    }

    // Find a split point that respects function-call/response pairs.
    let split = pick_split(&snapshot, KEEP_RECENT_TURNS);
    if split == 0 {
        debug!("compaction: nothing to summarize before the keep-window");
        return false;
    }

    let (to_summarize, to_keep) = snapshot.split_at(split);
    debug!(
        to_summarize = to_summarize.len(),
        to_keep = to_keep.len(),
        "compaction: attempting summary"
    );

    let summary = match summarize(client, model, to_summarize).await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "compaction: summarization failed; falling back to drop-oldest");
            return drop_oldest_fallback(history, split);
        }
    };

    if summary.trim().is_empty() {
        warn!("compaction: summarization returned empty text; falling back to drop-oldest");
        return drop_oldest_fallback(history, split);
    }

    // Install the summary as a single synthetic user turn at the head.
    let synthetic = Content {
        role: ContentRole::User,
        parts: vec![Part::Text {
            text: format!("{COMPACTION_TAG}\n{summary}"),
        }],
    };
    let mut hist = history.lock();
    if hist.len() != total {
        // Another turn raced us. Bail rather than corrupt the new state.
        warn!("compaction: history changed under us; aborting install");
        return false;
    }
    let kept: Vec<Content> = hist.split_off(split);
    hist.clear();
    hist.push(synthetic);
    hist.extend(kept);
    debug!(new_len = hist.len(), "compaction: installed summary");
    true
}

/// Pick an index `i` such that history[..i] is summarized and
/// history[i..] is kept. Honors:
///
/// * `KEEP_RECENT_TURNS` (a turn = one user+model pair, so 2 entries),
/// * function-call/response pairing (a Model turn carrying a
///   functionCall must be followed by the next User turn carrying the
///   matching functionResponse — never split between them).
fn pick_split(history: &[Content], keep_pairs: usize) -> usize {
    let keep_entries = keep_pairs * 2;
    if history.len() <= keep_entries {
        return 0;
    }
    let mut split = history.len() - keep_entries;

    // Walk forward off any boundary that would orphan a functionCall
    // from its functionResponse, or split a User->Model pair mid-turn.
    while split < history.len() {
        let prev = split.checked_sub(1).and_then(|i| history.get(i));
        let here = &history[split];

        let prev_carries_calls = prev.is_some_and(turn_has_function_call);
        let here_is_response = matches!(here.role, ContentRole::User) && turn_is_function_response(here);

        let crosses_call_response = prev_carries_calls && here_is_response;
        let mid_turn = matches!(here.role, ContentRole::Model);

        if crosses_call_response || mid_turn {
            split += 1;
            continue;
        }
        break;
    }

    split.min(history.len())
}

fn turn_has_function_call(c: &Content) -> bool {
    c.parts.iter().any(|p| matches!(p, Part::FunctionCall { .. }))
}

fn turn_is_function_response(c: &Content) -> bool {
    c.parts
        .iter()
        .all(|p| matches!(p, Part::FunctionResponse { .. }))
        && !c.parts.is_empty()
}

async fn summarize(client: &SharedClient, model: &str, history: &[Content]) -> Result<String> {
    use futures_util::stream::StreamExt;

    // Render the to-summarize slice as a readable transcript. We feed
    // it as the user message of a one-shot request — no system
    // instruction, no tools, no history.
    let transcript = render_transcript(history);
    let req = GenerateContentRequest {
        contents: vec![Content {
            role: ContentRole::User,
            parts: vec![Part::Text {
                text: format!("{SUMMARY_PROMPT}\n\n---\n{transcript}"),
            }],
        }],
        ..Default::default()
    };

    let mut stream = client.stream_generate(model, &req).await?;
    let mut out = String::new();
    let mut finish: Option<FinishReason> = None;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        for cand in chunk.candidates {
            if let Some(content) = cand.content {
                for part in content.parts {
                    if let Part::Text { text } = part {
                        out.push_str(&text);
                    }
                }
            }
            if let Some(r) = cand.finish_reason {
                finish = Some(r);
            }
        }
    }
    if !matches!(finish, Some(FinishReason::Stop) | None) {
        warn!(?finish, "compaction summary finished abnormally");
    }
    Ok(out)
}

fn render_transcript(history: &[Content]) -> String {
    let mut out = String::with_capacity(history.len() * 64);
    for entry in history {
        let role = match entry.role {
            ContentRole::User => "USER",
            ContentRole::Model => "MODEL",
        };
        out.push_str("## ");
        out.push_str(role);
        out.push('\n');
        for part in &entry.parts {
            match part {
                Part::Text { text } => out.push_str(text),
                Part::Thought {
                    text: Some(t), ..
                } => {
                    out.push_str("[thought] ");
                    out.push_str(t);
                }
                Part::FunctionCall { function_call } => {
                    out.push_str("[tool_call ");
                    out.push_str(&function_call.name);
                    out.push_str("] ");
                    out.push_str(&function_call.args.to_string());
                }
                Part::FunctionResponse { function_response } => {
                    out.push_str("[tool_result ");
                    out.push_str(&function_response.name);
                    out.push_str("] ");
                    let body = function_response.response.to_string();
                    // Truncate huge tool results — the summarizer
                    // doesn't need every byte.
                    if body.len() > 512 {
                        out.push_str(&body[..512]);
                        out.push_str("…[truncated]");
                    } else {
                        out.push_str(&body);
                    }
                }
                Part::InlineData { inline_data } => {
                    out.push_str("[inline_data ");
                    out.push_str(&inline_data.mime_type);
                    out.push(']');
                }
                _ => {}
            }
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// Last-resort fallback when summarization isn't available. Just drops
/// the `split` oldest entries. Crude but always correct.
fn drop_oldest_fallback(history: &Mutex<Vec<Content>>, split: usize) -> bool {
    let mut hist = history.lock();
    if split >= hist.len() {
        return false;
    }
    let kept: Vec<Content> = hist.split_off(split);
    hist.clear();
    hist.push(Content {
        role: ContentRole::User,
        parts: vec![Part::Text {
            text: format!("{COMPACTION_TAG}\n[prior turns dropped]"),
        }],
    });
    hist.extend(kept);
    debug!(new_len = hist.len(), "compaction: drop-oldest fallback applied");
    true
}

/// Decide whether to attempt compaction based on the running token
/// count. `threshold` of `None` disables compaction entirely.
pub fn should_compact(total_tokens: Option<i32>, threshold: Option<u32>) -> bool {
    match (total_tokens, threshold) {
        (Some(t), Some(th)) => t as u32 > th,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::gemini::wire::{FunctionCall, FunctionResponse};
    use serde_json::json;

    fn user_text(s: &str) -> Content {
        Content {
            role: ContentRole::User,
            parts: vec![Part::Text { text: s.into() }],
        }
    }
    fn model_text(s: &str) -> Content {
        Content {
            role: ContentRole::Model,
            parts: vec![Part::Text { text: s.into() }],
        }
    }
    fn model_call(name: &str) -> Content {
        Content {
            role: ContentRole::Model,
            parts: vec![Part::FunctionCall {
                function_call: FunctionCall {
                    name: name.into(),
                    args: json!({}),
                },
            }],
        }
    }
    fn user_response(name: &str) -> Content {
        Content {
            role: ContentRole::User,
            parts: vec![Part::FunctionResponse {
                function_response: FunctionResponse {
                    name: name.into(),
                    response: json!({"ok": true}),
                },
            }],
        }
    }

    #[test]
    fn pick_split_below_keep_window() {
        let h = vec![user_text("u1"), model_text("m1")];
        assert_eq!(pick_split(&h, 6), 0);
    }

    #[test]
    fn pick_split_respects_keep_window() {
        // 10 user/model pairs = 20 entries. Keep last 6 pairs = 12
        // entries. Expected split: 20 - 12 = 8.
        let h: Vec<Content> = (0..10)
            .flat_map(|i| {
                vec![
                    user_text(&format!("u{i}")),
                    model_text(&format!("m{i}")),
                ]
            })
            .collect();
        assert_eq!(h.len(), 20);
        assert_eq!(pick_split(&h, 6), 8);
    }

    #[test]
    fn pick_split_does_not_orphan_function_response() {
        // [..., model_call(view_file), user_response(view_file), model_text(end)]
        // If the natural split lands between model_call and user_response,
        // pick_split should walk forward past the response.
        let mut h: Vec<Content> = (0..20).map(|i| user_text(&format!("u{i}"))).collect();
        // Replace the boundary entries to create a tool pair right at split.
        let pair_index = 14; // some index past min keep window
        h[pair_index] = model_call("view_file");
        h[pair_index + 1] = user_response("view_file");

        let split = pick_split(&h, 6);
        // Split index should NOT land between the call and the response.
        assert_ne!(split, pair_index + 1, "split must not orphan response");
    }

    #[test]
    fn should_compact_only_when_over_threshold() {
        assert!(!should_compact(None, Some(1000)));
        assert!(!should_compact(Some(500), None));
        assert!(!should_compact(Some(500), Some(1000)));
        assert!(should_compact(Some(1500), Some(1000)));
    }
}
