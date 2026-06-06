//! Local in-browser model backend — Gemma 3 270M via Burn's wgpu/WebGPU
//! backend. Runs fully in the tab: no proxy, no `$LH`, no API key.
//!
//! **Status: scaffolding.** This module currently only proves the Burn/wgpu
//! tensor stack COMPILES on every target the crate builds for (native AND
//! `wasm32-unknown-unknown`) — the WebGPU-via-Burn feasibility gate for the
//! whole local-inference approach, validated before porting Gemma. The Gemma
//! transformer now lives in [`gemma`] (written + compiling, not yet
//! forward-pass-validated). The weight loader, tokenizer, and the
//! `ConnectionStrategy` / `Connection` impls land in the next phases.

/// Gemma 3 270M model architecture in Burn, verified against the official
/// `google/gemma-3-270m` config. Compiles native + wasm32; the forward pass is
/// not yet validated against reference logits — see the module docs.
pub mod gemma;

/// Safetensors → Burn `GemmaModel` weight loader (HF→Burn rename, transpose,
/// bf16→f32, RMSNorm `(1+w)`, RoPE interleave permutation). In-memory bytes,
/// no filesystem — wasm-safe.
pub mod weights;

/// Gemma tokenizer (HF `tokenizers` crate, `unstable_wasm`) loaded from raw
/// `tokenizer.json` bytes.
pub mod tokenizer;

/// Greedy (argmax) text generation over a loaded `GemmaModel` — no KV cache in
/// v1, recompute-per-step; identical on native + wasm32.
pub mod generate;

/// The `Connection` / `ConnectionStrategy` seam wiring the loaded Gemma engine
/// into the agent loop — mirrors the Anthropic backend. Weights are read from
/// OPFS; `send()` runs `generate()` and emits a text `Step`.
pub mod connection;

/// Best-effort textual tool-call parser: extracts the philschmid `tool_code`
/// markdown fence (`name(arg=val)`) from generated text into `(name, json
/// args)`, no `eval`. Drives the connection's parse-then-dispatch tool loop.
pub mod tool_parse;

/// The Burn backend the local model runs on: `wgpu` — WebGPU on `wasm32`,
/// Vulkan/Metal/DX12 on native. Named here so the rest of the backend is
/// written once against `LocalBackend` and the concrete device is chosen at
/// the edges.
pub type LocalBackend = burn::backend::Wgpu;

/// Compile-feasibility smoke. References the Burn tensor type on the wgpu
/// backend so `cargo check` is forced to build the entire Burn/wgpu/cubecl
/// dependency tree for the current target. Not wired into the agent loop —
/// it exists solely so the wasm32 build proves WebGPU-via-Burn compiles
/// before we invest in the Gemma port. Returns the monomorphised type name.
pub fn burn_feasibility_smoke() -> &'static str {
    std::any::type_name::<burn::tensor::Tensor<LocalBackend, 1>>()
}
