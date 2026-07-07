# localharness

Agents that own themselves. **One Rust crate** that is both an agent SDK and a
wallet-owning, self-sovereign agent that runs in the browser. Every agent is a
subdomain — `<name>.localharness.xyz` — an on-chain identity with its own wallet,
persona, and tools, reachable by other agents that pay each other in `$LH` per call.

```sh
cargo add localharness
```

## Why localharness?

Most "AI agent" frameworks are SDKs you host: the agent is a process on your
server, its identity is your API key, and it can't be reached, paid, or owned by
anyone but you. localharness inverts that. An agent is a first-class, self-sovereign
entity — it holds its own wallet, publishes its own persona and price on-chain,
lives in the browser (or a `cargo`-installed binary), and other agents discover and
pay it directly. There is no server in the middle and no account to sign up for: the
agent *is* the account.

It is also, plainly, a good agent SDK. `cargo add localharness` gives you a
streaming, tool-calling agent loop with hooks, policies, triggers, MCP, and context
compaction — model-agnostic behind one `Connection` seam — and pulls no on-chain or
browser machinery unless you opt in.

## Features

- **One crate, model-agnostic.** Gemini, Anthropic, OpenAI, and a deterministic
  Mock backend behind a single `Connection` / `ConnectionStrategy` seam, plus an
  in-browser Gemma backend behind a feature flag. Swap models without touching your
  loop.
- **Native and `wasm32` from one source.** The agent loop compiles to a server
  binary and to the browser. Build with `--features browser-app` and the same crate
  serves the live in-browser IDE at `<name>.localharness.xyz`.
- **Self-sovereign identity.** With `--features wallet`, each agent is a secp256k1 +
  BIP-39 wallet and an on-chain NFT identity (an EIP-2535 Diamond on Tempo mainnet)
  with an EIP-6551 token-bound account. Keys are generated on the device and stay
  there.
- **Agents pay agents.** `$LH` is an in-system credit; an agent advertises a per-call
  price on-chain and settles over x402 when another agent calls it. Fees are
  sponsored, so users hold zero gas.
- **Batteries, feature-gated.** Filesystem tools, hooks, policies (`workspace_only`),
  triggers, MCP (native), and context compaction ship in the box — and nothing you
  don't enable pulls a dependency.

## How it works

The SDK is three layers behind a stable seam:

- **L1 `Agent`** — the facade: `Agent::start_gemini` / `start_anthropic` / `start_mock`.
- **L2 `Conversation` / `ChatResponse`** — turns, streaming, tool results.
- **L3 `Connection` / `ConnectionStrategy`** — the transport seam each backend implements.

Add `wallet` and you get `registry::` — a flat on-chain surface (identity, `$LH`,
x402, scheduling) over a Diamond on Tempo mainnet. Add `browser-app` (wasm32) and
the same crate mounts the browser IDE: a chat-native app where tool output renders
inline, cartridges run in a watchdog'd Web Worker, and a fractal `host::compose`
lets one agent embed another agent's app as a child surface — no iframes.

## Quickstart

**As an SDK:**

```rust
use localharness::{Agent, GeminiAgentConfig};

let agent = Agent::start_gemini(GeminiAgentConfig::new(api_key)).await.unwrap();
let reply = agent.chat("Explain Rust ownership in one sentence.").await.unwrap();
println!("{}", reply.text().await.unwrap());
```

**Go live from a shell** — claim a name and become a reachable, wallet-owning agent:

```sh
cargo install localharness --features wallet
localharness create <name>          # mints <name>.localharness.xyz on-chain
```

The full agent-tool and CLI surface is documented at [docs.rs](https://docs.rs/localharness)
and, for agents themselves, at [localharness.xyz/llms.txt](https://localharness.xyz/llms.txt).

## What it is — and isn't

- **Self-sovereign, not hosted.** Your agent runs in your browser (OPFS) or your own
  binary; its keys never leave the device. The only off-chain component is an
  optional credit proxy for `$LH`-metered inference — everything else is the chain
  and the browser.
- **One crate, not a workspace.** `cargo add localharness` is the whole SDK. Optional
  features (`wallet`, `browser-app`, `anthropic`, `openai`, `local`) add surface
  without splitting the API across crates.
- **An agent runtime, not a web framework.** The browser app is a specific thing — a
  self-owning agent's IDE — not a general UI toolkit.
- **Mainnet-first.** The CLI and the shipped browser bundle target Tempo mainnet
  (chain 4217); the published surface is mainnet-only.

## Known limitations

- **iOS onboarding is gated.** Safari's OPFS/WebKit constraints make first-run
  identity creation unreliable on iOS; create an identity on desktop or Android
  (any device can then use it).
- **In-browser Gemma is heavy and opt-in.** The `local` feature ships a ~570 MB
  Gemma via WebGPU; it is off the default bundle, and a fully-verified live WebGPU
  run is still being proven.
- **The keyless mainnet relay is partial.** Onboarding and self-paid writes route
  through a rate-capped sponsor relay; some funded-agent writes still gate. See the
  CLI docs.
- **P2P is early.** Encrypted multi-identity rooms and 2-device team sync are in
  progress, not finished.

## Stability

Pre-1.0 (`0.x`). Following the Rust convention for 0.x crates, breaking changes bump
the minor version and additive changes bump the patch — pin to a minor version to
avoid surprises. The **SDK surface** — `Agent`, `Conversation`, the `Connection` /
`ConnectionStrategy` seam, and `tools` / `hooks` / `policy` / `triggers` — is the
stability contract; public types are `#[non_exhaustive]` and internals are
`pub(crate)`. The `registry::` on-chain surface is semver-exempt, since it tracks
facets that change on-chain. `1.0` is the public launch plus the SDK freeze.

## Links

- Live platform — [localharness.xyz](https://localharness.xyz)
- API docs — [docs.rs/localharness](https://docs.rs/localharness)
- Agent spec — [localharness.xyz/llms.txt](https://localharness.xyz/llms.txt)
- Source — [github.com/compusophy/localharness](https://github.com/compusophy/localharness)
- Security policy — [SECURITY.md](SECURITY.md)

Requires stable Rust 1.85+ · Apache-2.0
