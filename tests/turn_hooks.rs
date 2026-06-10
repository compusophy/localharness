//! Integration tests: PreTurnHook / PostTurnHook wired through the turn loop.
//!
//! Drives a real [`Agent`] over the offline mock backend (no network, no key,
//! no LLM — the same pattern as `src/backends/mock/`'s own tests) to prove the
//! turn-hook contract end to end:
//!
//! * a registered `PreTurnHook` deny BLOCKS the turn before the model runs —
//!   the failure surfaces to `chat()`/`text()` as an `Err` carrying
//!   `"turn denied by hook: {reason}"`, and the denied prompt leaves no trace
//!   (a follow-up allowed turn replays the FIRST scripted model turn, proving
//!   the deny consumed nothing);
//! * a registered `PostTurnHook` fires after a successful turn's terminal
//!   step with the turn's final text — and never for a denied turn.
//!
//! The wire-level half of the invariant (the denied prompt never enters the
//! backend's HISTORY) is covered natively in
//! `src/backends/gemini/loop.rs::tests::pre_turn_deny_keeps_prompt_out_of_history`.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use localharness::backends::mock::{MockAgentConfig, MockConnection};
use localharness::hooks::{PostTurnHook, PreTurnHook, TurnContext};
use localharness::types::HookResult;
use localharness::{Agent, Content};

/// A `PreTurnHook` that denies any prompt containing the marker `"BLOCKED"`,
/// recording every prompt it inspected.
struct DenyMarked {
    inspected: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl PreTurnHook for DenyMarked {
    fn name(&self) -> &str {
        "test::deny_marked"
    }
    async fn run(&self, _ctx: &TurnContext, prompt: &Content) -> localharness::Result<HookResult> {
        let text = prompt.as_text().unwrap_or_default();
        self.inspected.lock().unwrap().push(text.clone());
        if text.contains("BLOCKED") {
            Ok(HookResult::deny("the prompt is on the blocklist"))
        } else {
            Ok(HookResult::allow())
        }
    }
}

/// A `PostTurnHook` that records every response it observed, in order.
struct RecordingPostTurn {
    seen: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl PostTurnHook for RecordingPostTurn {
    fn name(&self) -> &str {
        "test::recording_post_turn"
    }
    async fn run(&self, _ctx: &TurnContext, response: &str) -> localharness::Result<()> {
        self.seen.lock().unwrap().push(response.to_string());
        Ok(())
    }
}

/// CONTRACT (deny): a `PreTurnHook` deny blocks the turn BEFORE the model
/// runs. The denied `chat` surfaces an `Err` carrying the hook's reason, the
/// post-turn hooks never fire for it, and the conversation is left clean — the
/// follow-up ALLOWED turn replays scripted turn #1, proving the denied prompt
/// neither ran the model nor consumed any conversation state.
#[tokio::test]
async fn pre_turn_deny_blocks_the_turn_and_leaves_history_clean() {
    // ONE scripted model turn. If the denied chat reached the "model", it
    // would consume this turn and the follow-up would get an empty default.
    let backend = MockConnection::builder()
        .turn(|t| t.text("the only scripted answer"))
        .build();
    let agent = Agent::start_mock(MockAgentConfig::new(backend))
        .await
        .expect("mock agent starts");

    let inspected = Arc::new(Mutex::new(Vec::new()));
    let post_seen = Arc::new(Mutex::new(Vec::new()));
    agent.hooks().register_pre_turn(Arc::new(DenyMarked {
        inspected: inspected.clone(),
    }));
    agent.hooks().register_post_turn(Arc::new(RecordingPostTurn {
        seen: post_seen.clone(),
    }));

    // 1. The denied turn: chat() dispatches, but the stream yields an Err
    //    with the deny reason — the uniform turn_error surfacing.
    let denied = agent
        .chat("this prompt is BLOCKED")
        .await
        .expect("send dispatches; the deny surfaces on the stream");
    let err = denied
        .text()
        .await
        .expect_err("a denied turn must surface an Err, not an empty success");
    let msg = err.to_string();
    assert!(
        msg.contains("turn denied by hook: the prompt is on the blocklist"),
        "the Err must carry the deny reason, got: {msg}"
    );

    // The hook DID inspect the denied prompt (the gate ran)...
    assert_eq!(
        inspected.lock().unwrap().as_slice(),
        ["this prompt is BLOCKED"],
        "the pre-turn hook saw the prompt"
    );
    // ...but no post-turn hook fired — the turn never completed.
    assert!(
        post_seen.lock().unwrap().is_empty(),
        "post-turn hooks must NOT fire for a denied turn"
    );

    // 2. The follow-up ALLOWED turn gets scripted turn #1 — the denied
    //    prompt consumed nothing and is invisible to the conversation.
    let reply = agent
        .chat("a clean prompt")
        .await
        .expect("chat starts")
        .text()
        .await
        .expect("the allowed turn completes");
    assert_eq!(
        reply, "the only scripted answer",
        "the denied turn must not have consumed the first scripted model turn"
    );

    agent.shutdown().await.expect("clean shutdown");
}

/// CONTRACT (observe): a registered `PostTurnHook` fires after each
/// successful turn's terminal step, observing the turn's final text — once
/// per turn, in order.
#[tokio::test]
async fn post_turn_hook_fires_after_each_successful_turn() {
    let backend = MockConnection::builder()
        .turn(|t| t.text("first answer"))
        .turn(|t| t.text("second answer"))
        .build();
    let agent = Agent::start_mock(MockAgentConfig::new(backend))
        .await
        .expect("mock agent starts");

    let seen = Arc::new(Mutex::new(Vec::new()));
    agent.hooks().register_post_turn(Arc::new(RecordingPostTurn {
        seen: seen.clone(),
    }));

    let r1 = agent.chat("one").await.unwrap().text().await.unwrap();
    assert_eq!(r1, "first answer");
    let r2 = agent.chat("two").await.unwrap().text().await.unwrap();
    assert_eq!(r2, "second answer");

    // `text()` resolves on the terminal step; the post-turn hooks run right
    // after it, before the turn flips the connection idle — so waiting for
    // idle deterministically orders this assert after the dispatch.
    agent
        .conversation()
        .connection()
        .wait_for_idle()
        .await
        .expect("turn settles");

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["first answer", "second answer"],
        "the post-turn hook observes each completed turn's final text, in order"
    );

    agent.shutdown().await.expect("clean shutdown");
}
