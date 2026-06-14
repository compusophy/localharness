//! Greedy (argmax) decoding for the local Gemma 3 270M backend.
//!
//! `v1` deliberately keeps this as simple as the architecture allows: no KV
//! cache, no sampling, no beam search. Each step re-runs the FULL forward pass
//! over the running token sequence, reads the logits for the final position,
//! takes the `argmax` token, appends it, and repeats — until the model emits
//! the Gemma EOS token (`1`) or `max_new` tokens have been generated. The
//! prompt is tokenized with a leading BOS (`2`) by [`GemmaTokenizer::encode`].
//!
//! Recomputing the whole sequence every step is O(n²) and obviously slower than
//! a cached decode, but it is correct, allocation-light, and — critically —
//! identical on native and `wasm32`, which is all `v1` needs. A KV cache is a
//! later optimisation that does not change this public surface.
//!
//! Compiles on native and `wasm32-unknown-unknown`. The whole module is gated
//! on `feature = "local"` (see `super`).

use burn::tensor::{backend::Backend, Int, Tensor, TensorData};

use super::gemma::{GemmaModel, ROPE_CACHE_LEN};
use super::tokenizer::GemmaTokenizer;

/// Gemma end-of-sequence token id. Generation stops as soon as the model emits
/// this (it is NOT included in the decoded output).
const EOS_ID: i64 = 1;

/// Whether a forward pass over `seq` tokens would index past the RoPE cache.
///
/// `forward` (and the per-layer `RotaryEncoding`) index the precomputed cache by
/// absolute position `0..seq`, so the largest valid sequence length is exactly
/// `ROPE_CACHE_LEN` (positions `0..ROPE_CACHE_LEN`). A sequence of `ROPE_CACHE_LEN`
/// or more tokens would read position `ROPE_CACHE_LEN` and panic the tab; the
/// decoder stops cleanly instead. Pure so the bound is unit-tested without the
/// (heavy) Burn backend.
fn at_context_limit(seq: usize) -> bool {
    seq >= ROPE_CACHE_LEN
}

/// Greedy argmax decode.
///
/// Tokenizes `prompt` (the tokenizer prepends BOS), then autoregressively
/// appends the highest-probability next token — recomputing the full forward
/// pass each step (no KV cache in v1) — stopping at the first EOS (`1`) or after
/// `max_new` new tokens. Returns the decoded continuation (the prompt tokens are
/// not re-emitted; a trailing EOS is dropped).
///
/// `device` is the Burn device the running token tensor is built on; it must
/// match the device `model` lives on.
pub async fn generate<B: Backend>(
    model: &GemmaModel<B>,
    tok: &GemmaTokenizer,
    prompt: &str,
    max_new: usize,
    device: &B::Device,
) -> String {
    // Tokenize the prompt. The tokenizer is responsible for the leading BOS.
    let mut tokens: Vec<i64> = tok.encode(prompt);

    // The continuation we accumulate and decode at the end. Kept separate from
    // `tokens` so the prompt is never echoed back in the returned string.
    let mut generated: Vec<i64> = Vec::with_capacity(max_new);

    for _ in 0..max_new {
        // Clean stop before the forward pass would index past the RoPE cache.
        // v1 has no KV cache, so `tokens` (prompt + continuation) grows every
        // step; once it reaches `ROPE_CACHE_LEN`, `forward` would index the
        // precomputed RoPE cache out of bounds and panic the tab. Degrade an
        // over-length context to a clean stop instead (issue #96).
        if at_context_limit(tokens.len()) {
            break;
        }

        // Build the input tensor [1, seq] from the running token sequence. We
        // construct a 1-D Int tensor from the i64 ids then reshape to add the
        // batch dim — portable across backends (the Int element may be i32 on
        // wgpu, but `from_data` converts from the i64 source data).
        let seq = tokens.len();
        let input = Tensor::<B, 1, Int>::from_data(TensorData::from(tokens.as_slice()), device)
            .reshape([1, seq]);

        // Forward pass -> logits [1, seq, vocab]. argmax over the vocab dim
        // gives the most-likely token id at every position; we only need the
        // last position's prediction (the next token).
        let logits = model.forward(input);
        let argmax = logits.argmax(2); // [1, seq, 1]

        // Read the argmax ids back to the host. Use the ASYNC read-back: the
        // sync `into_data()`/`try_read_sync()` PANICS on wasm32 (a WebGPU buffer
        // read can't block the browser event loop). `into_data_async().await`
        // is correct on both targets — it resolves immediately on native and
        // yields to the GPU read on wasm. It returns a `Result`; a read-back
        // failure ends generation cleanly rather than panicking in the tab.
        let data = match argmax.into_data_async().await {
            Ok(d) => d,
            Err(_) => break,
        };
        // `iter::<i64>()` converts from whatever integer dtype the backend uses
        // (e.g. i32 on wgpu) into i64.
        let ids: Vec<i64> = data.iter::<i64>().collect();

        // The next-token prediction lives at the final sequence position. The
        // flattened [1, seq, 1] data is `seq` elements long, so the last is the
        // prediction for the token *after* the current sequence.
        let next = match ids.last().copied() {
            Some(id) => id,
            None => break, // empty logits — nothing to do (defensive)
        };

        if next == EOS_ID {
            break;
        }

        tokens.push(next);
        generated.push(next);
    }

    tok.decode(&generated)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The forward pass indexes the RoPE cache by position `0..seq`, so the
    // boundary is exactly `ROPE_CACHE_LEN`: `ROPE_CACHE_LEN - 1` tokens are the
    // last SAFE length (positions `0..ROPE_CACHE_LEN`), and `ROPE_CACHE_LEN`
    // tokens would read position `ROPE_CACHE_LEN` — out of bounds. The pre-fix
    // loop had no such guard and panicked the tab there (issue #96).
    #[test]
    fn context_limit_guards_the_rope_cache_boundary() {
        assert!(!at_context_limit(0));
        assert!(!at_context_limit(ROPE_CACHE_LEN - 1)); // last safe length
        assert!(at_context_limit(ROPE_CACHE_LEN)); // first out-of-bounds length
        assert!(at_context_limit(ROPE_CACHE_LEN + 1));
    }
}
