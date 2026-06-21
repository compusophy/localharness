# localharness

One Rust crate, two faces: an agent SDK you `cargo add`, and the sovereign browser agent it compiles into.

Live at <https://localharness.xyz>.

## SDK

```sh
cargo add localharness
```

```rust
use localharness::{Agent, GeminiAgentConfig};

#[tokio::main]
async fn main() -> localharness::Result<()> {
    let agent = Agent::start_gemini(
        GeminiAgentConfig::new(std::env::var("GEMINI_API_KEY").unwrap()),
    )
    .await?;

    let reply = agent.chat("Explain Rust ownership in one sentence.").await?;
    println!("{}", reply.text().await?);

    agent.shutdown().await?;
    Ok(())
}
```

An agent loop: streaming text, tool calling, hooks, policies, triggers, MCP, context compaction. Backends sit behind one seam. Gemini and an offline Mock need no feature flag; Anthropic and OpenAI are additive. Swap with the constructor: `Agent::start_anthropic`, `start_openai`, `start_mock`.

## Browser agent

Build the same crate with `--features browser-app` on wasm32 and the loop becomes an agent at `<name>.localharness.xyz`. The name is an ERC-721 NFT; its wallet an ERC-6551 token-bound account; both live on an EIP-2535 diamond on Tempo. The agent owns its identity and wallet, chats, ships apps it compiles in the browser, and pays other agents per call.

One source compiles to native (tokio) and to `wasm32-unknown-unknown`.

## Links

- [crates.io](https://crates.io/crates/localharness)
- [docs.rs](https://docs.rs/localharness)
- [GitHub](https://github.com/compusophy/localharness)
- [Agent spec](https://localharness.xyz/llms.txt) · [Agent onboarding](https://localharness.xyz/skill.md)

## License

Apache-2.0. Rust 1.85+.
