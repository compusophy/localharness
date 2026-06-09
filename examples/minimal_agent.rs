//! The "hello world" of the SDK: build an agent, run a turn, print the answer.
//!
//! This is the smallest possible end-to-end use of the agent loop. It uses the
//! built-in offline `MockConnection` instead of a real LLM, so it runs with
//! **NO API key and NO network** — completely deterministic. The mock replays a
//! fixed script of model "turns" you write yourself; the Nth `agent.chat(...)`
//! consumes the Nth scripted turn. Everything else (the `Agent` facade, the
//! `Conversation`/`ChatResponse` streaming, shutdown) is the exact same code a
//! real Gemini or Claude backend drives — only the model is swapped out.
//!
//! To talk to a real model instead, swap `Agent::start_mock(...)` for
//! `Agent::start_gemini(GeminiAgentConfig::new(api_key)...)` — same `.chat()`,
//! same `.text()`. See `basic_agent.rs` for that live path.
//!
//! Run (no key, no network):
//!
//! ```sh
//! cargo run --example minimal_agent
//! ```

use localharness::{Agent, MockAgentConfig, MockConnection};

#[tokio::main]
async fn main() -> localharness::Result<()> {
    // 1. Script the model. `MockConnection::builder()` returns a fluent builder;
    //    each `.turn(...)` appends ONE scripted model turn. Here the first turn
    //    streams a single line of text, the second a different line. A real
    //    backend would generate these; the mock just replays them verbatim.
    let backend = MockConnection::builder()
        .turn(|t| t.text("Hello from localharness — this is a scripted reply."))
        .turn(|t| t.text("And here is the second turn, replayed in order."))
        .build();

    // 2. Start the agent on the mock backend. `MockAgentConfig::new(backend)` is
    //    the offline parallel of `GeminiAgentConfig::new(key)`; `start_mock`
    //    wires up the same conversation + streaming machinery the live backends
    //    use. No `.await`-able network call happens — it's all in-process.
    let agent = Agent::start_mock(MockAgentConfig::new(backend)).await?;

    // 3. Run a turn. `chat` sends a prompt and returns a `ChatResponse`, a lazy
    //    multi-cursor stream over the turn's chunks. `.text()` drains it to the
    //    full concatenated reply. (The mock ignores the prompt text — its reply
    //    is whatever the matching scripted turn said.)
    let first = agent.chat("anything — the mock ignores the prompt").await?;
    println!("turn 1: {}", first.text().await?);

    // 4. Each subsequent `chat` advances to the next scripted turn.
    let second = agent.chat("again").await?;
    println!("turn 2: {}", second.text().await?);

    // 5. Clean teardown stops the agent's background tasks. Always do this (or
    //    drop the agent) so spawned tasks don't outlive what you intended.
    agent.shutdown().await?;
    Ok(())
}
