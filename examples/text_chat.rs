//! Minimal end-to-end example against the Gemini backend.
//!
//! Run:
//!
//! ```sh
//! export GEMINI_API_KEY="..."
//! cargo run --example text_chat -- "Write a haiku about Rust."
//! ```
//!
//! Streams text tokens to stdout as they arrive.

use std::env;

use futures_util::StreamExt;
use localharness::{Agent, GeminiAgentConfig, Result};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "localharness=info".into()),
        )
        .compact()
        .init();

    let api_key = env::var("GEMINI_API_KEY")
        .expect("set GEMINI_API_KEY before running this example");

    let prompt: String = env::args()
        .skip(1)
        .collect::<Vec<_>>()
        .join(" ");
    let prompt = if prompt.is_empty() {
        "Tell me a fun fact about the Rust programming language.".to_string()
    } else {
        prompt
    };

    let cfg = GeminiAgentConfig::new(api_key)
        .with_system_instructions("You are concise. Three sentences max.");

    let agent = Agent::start_gemini(cfg).await?;
    let response = agent.chat(prompt).await?;

    let mut tokens = response.text_stream();
    while let Some(chunk) = tokens.next().await {
        let text = chunk?;
        print!("{}", text);
        use std::io::Write as _;
        std::io::stdout().flush().ok();
    }
    println!();

    if let Some(usage) = agent.conversation().last_turn_usage() {
        eprintln!(
            "\n[usage] prompt={:?} candidates={:?} total={:?}",
            usage.prompt_token_count, usage.candidates_token_count, usage.total_token_count
        );
    }

    agent.shutdown().await?;
    Ok(())
}
