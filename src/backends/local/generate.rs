//! Greedy (argmax) decoding for the local Gemma 3 270M backend.
//!
//! KV-cached decode: the prompt is forwarded ONCE (prefill, populating the
//! per-layer [`KvCache`]), then each step forwards only the single new token —
//! O(seq) per token instead of the v1 full-recompute O(seq²). Greedy argmax,
//! no sampling, no beam search: read the final position's logits, take the
//! `argmax` token, append, repeat — until the model emits the Gemma EOS token
//! (`1`), fabricates the next `"\nUser:"` turn, or `max_new` tokens have been
//! generated. The prompt is tokenized with a leading BOS (`2`) by
//! [`GemmaTokenizer::encode`].
//!
//! Compiles on native and `wasm32-unknown-unknown`. The whole module is gated
//! on `feature = "local"` (see `super`).

use burn::tensor::{backend::Backend, Int, Tensor, TensorData};

use super::gemma::{GemmaModel, KvCache, ROPE_CACHE_LEN};
use super::tokenizer::GemmaTokenizer;

/// Gemma end-of-sequence token id. Generation stops as soon as the model emits
/// this (it is NOT included in the decoded output).
const EOS_ID: i64 = 1;

/// The stop marker: the base model keeps continuing the flat transcript past
/// its own turn by fabricating the next "User:" line. Streaming decodes spot it
/// mid-generation and stop — both to avoid painting the fabricated turn live
/// and to stop burning forward passes on text the connection would cut anyway.
const STOP_MARKER: &str = "\nUser:";

/// Incremental delta planner over successive full decodes of the running
/// continuation. Pure (no tensors) so the emission rules are unit-tested
/// natively:
///
/// * **Holdback** — the last `STOP_MARKER.len()-1` bytes are withheld (floored
///   to a char boundary) so a marker arriving split across tokens is never
///   partially emitted; the tail flushes on [`StreamEmitter::finish`].
/// * **Marker cut** — text at/after `STOP_MARKER` is never emitted; hitting it
///   reports `stop = true` so the decode loop can break early.
/// * **Prefix stability** — SentencePiece byte-fallback pieces can make a
///   decode transiently NON-prefix-stable (a split multibyte char decodes as
///   U+FFFD until completed). An already-emitted prefix cannot be retracted,
///   so on a prefix mismatch the emitter goes quiet instead of emitting
///   garbage; the terminal step still carries the authoritative full text.
struct StreamEmitter {
    /// Exactly what has been emitted so far (prefix-stability witness).
    sent: String,
    stopped: bool,
}

impl StreamEmitter {
    fn new() -> Self {
        Self {
            sent: String::new(),
            stopped: false,
        }
    }

    /// Plan the next delta for the full decode-so-far. `final_flush` releases
    /// the holdback (end of generation). Returns the delta to emit (if any)
    /// and whether the stop marker was hit.
    fn step<'a>(&mut self, full: &'a str, final_flush: bool) -> (Option<&'a str>, bool) {
        if self.stopped {
            return (None, true);
        }
        // Prefix stability: only emit while the new decode extends what was
        // already sent verbatim.
        if full.get(..self.sent.len()) != Some(self.sent.as_str()) {
            return (None, false);
        }
        let (cut, hit) = match full.find(STOP_MARKER) {
            Some(i) => (i, true),
            None => (full.len(), false),
        };
        let end = if hit || final_flush {
            cut
        } else {
            // Hold back a potential partial marker, floored to a char boundary.
            let mut end = full.len().saturating_sub(STOP_MARKER.len() - 1);
            while end > 0 && !full.is_char_boundary(end) {
                end -= 1;
            }
            end.min(cut)
        };
        if hit {
            self.stopped = true;
        }
        let start = self.sent.len();
        if end > start {
            self.sent.push_str(&full[start..end]);
            (Some(&full[start..end]), hit)
        } else {
            (None, hit)
        }
    }
}

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

/// Greedy argmax decode (non-streaming). See [`generate_streamed`] — this is
/// the same loop with the per-delta callback stubbed out.
pub async fn generate<B: Backend>(
    model: &GemmaModel<B>,
    tok: &GemmaTokenizer,
    prompt: &str,
    max_new: usize,
    device: &B::Device,
) -> String {
    generate_streamed(model, tok, prompt, max_new, device, |_| true).await
}

/// Greedy argmax decode with incremental text streaming.
///
/// Tokenizes `prompt` (the tokenizer prepends BOS), prefills the KV cache with
/// one full-prompt forward pass, then autoregressively appends the
/// highest-probability next token — forwarding only that single token per step
/// ([`GemmaModel::forward_cached`]) — stopping at the first EOS (`1`), the
/// first fabricated `"\nUser:"` turn ([`STOP_MARKER`]), or after `max_new` new
/// tokens. Returns the decoded continuation (the prompt tokens are not
/// re-emitted; a trailing EOS is dropped).
///
/// `on_delta` is invoked with each newly-stable slice of decoded continuation
/// text as it generates (see [`StreamEmitter`] for the emission rules — text
/// at/after the stop marker is never emitted). Returning `false` cancels
/// generation after the current token. The concatenation of all deltas equals
/// the returned text up to the stop-marker cut and trailing holdback rules;
/// the caller's terminal step remains the authoritative full text.
///
/// `device` is the Burn device the running token tensor is built on; it must
/// match the device `model` lives on.
pub async fn generate_streamed<B: Backend>(
    model: &GemmaModel<B>,
    tok: &GemmaTokenizer,
    prompt: &str,
    max_new: usize,
    device: &B::Device,
    mut on_delta: impl FnMut(&str) -> bool,
) -> String {
    // Tokenize the prompt. The tokenizer is responsible for the leading BOS.
    let tokens: Vec<i64> = tok.encode(prompt);

    // KV-cached decode state: `pending` is what the next forward consumes —
    // the whole prompt for the prefill pass, then exactly one token per step.
    // `total_len` = cached + pending positions (the RoPE-bound sequence length,
    // identical to v1's `tokens.len()`).
    let mut cache: KvCache<B> = model.new_cache();
    let mut pending: Vec<i64> = tokens;
    let mut total_len = pending.len();

    // The continuation we accumulate and decode at the end. Kept separate from
    // `tokens` so the prompt is never echoed back in the returned string.
    let mut generated: Vec<i64> = Vec::with_capacity(max_new);

    // Incremental emission state (holdback / marker-cut / prefix-stability).
    let mut emitter = StreamEmitter::new();

    // In-browser observability: one console line per generated token so a live
    // run's progress + tokens/sec stay measurable alongside the streamed
    // transcript deltas.
    #[cfg(target_arch = "wasm32")]
    let t_start = js_sys::Date::now();
    #[cfg(target_arch = "wasm32")]
    web_sys::console::log_1(
        &format!("[lh-local] generate: prompt={} tokens, max_new={max_new}", total_len).into(),
    );

    for _ in 0..max_new {
        // Clean stop before the forward pass would index past the RoPE cache:
        // once the sequence (cached + pending) reaches `ROPE_CACHE_LEN`, the
        // per-layer RoPE would index the precomputed cache out of bounds and
        // panic the tab. Degrade an over-length context to a clean stop
        // instead (issue #96).
        if at_context_limit(total_len) {
            break;
        }

        // Build the input tensor [1, seq] from the PENDING tokens only (the
        // prompt on the prefill pass, then one token per step — the cache
        // holds everything earlier). We construct a 1-D Int tensor from the
        // i64 ids then reshape to add the batch dim — portable across
        // backends (the Int element may be i32 on wgpu, but `from_data`
        // converts from the i64 source data).
        let seq = pending.len();
        let input = Tensor::<B, 1, Int>::from_data(TensorData::from(pending.as_slice()), device)
            .reshape([1, seq]);

        // Forward pass -> logits [1, seq, vocab] for the new positions only.
        // argmax over the vocab dim gives the most-likely token id at every
        // new position; we only need the last one (the next token).
        let logits = model.forward_cached(input, &mut cache);
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

        pending = vec![next];
        total_len += 1;
        generated.push(next);

        // Stream the newly-stable slice of decoded text; break early on the
        // fabricated-"User:" marker (the connection cuts there anyway) or when
        // the callback cancels.
        let full = tok.decode(&generated);
        let (delta, hit_marker) = emitter.step(&full, false);
        let keep_going = delta.map(&mut on_delta).unwrap_or(true);

        #[cfg(target_arch = "wasm32")]
        {
            let dt = (js_sys::Date::now() - t_start) / 1000.0;
            let tps = generated.len() as f64 / dt.max(1e-9);
            web_sys::console::log_1(
                &format!(
                    "[lh-local] tok {}/{max_new} ({tps:.2} tok/s) text={:?}",
                    generated.len(),
                    full
                )
                .into(),
            );
        }

        if hit_marker || !keep_going {
            break;
        }
    }

    // Release the holdback: flush whatever stable tail hasn't streamed yet.
    let full = tok.decode(&generated);
    if let (Some(delta), _) = emitter.step(&full, true) {
        on_delta(delta);
    }
    full
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

    /// Drive the emitter over successive decodes; return concat of deltas.
    fn run_emitter(decodes: &[&str], flush: &str) -> (String, bool) {
        let mut e = StreamEmitter::new();
        let mut out = String::new();
        let mut hit = false;
        for d in decodes {
            let (delta, h) = e.step(d, false);
            if let Some(t) = delta {
                out.push_str(t);
            }
            hit |= h;
            if hit {
                return (out, hit);
            }
        }
        if let (Some(t), h) = e.step(flush, true) {
            out.push_str(t);
            hit |= h;
        }
        (out, hit)
    }

    /// Holdback withholds a potential partial marker; the final flush releases
    /// it — concat(deltas) == the full decode when no marker appears.
    #[test]
    fn emitter_streams_all_text_with_final_flush() {
        let (out, hit) = run_emitter(&[" Paris", " Paris is", " Paris is nice."], " Paris is nice.");
        assert_eq!(out, " Paris is nice.");
        assert!(!hit);
    }

    /// A stop marker arriving SPLIT across decodes is never partially emitted,
    /// and text at/after it never streams.
    #[test]
    fn emitter_cuts_at_stop_marker_split_across_tokens() {
        let (out, hit) = run_emitter(
            &["Paris.", "Paris.\nUser", "Paris.\nUser: and"],
            "Paris.\nUser: and",
        );
        assert_eq!(out, "Paris.");
        assert!(hit);
    }

    /// Marker in one decode step: emit up to it, stop.
    #[test]
    fn emitter_stops_on_marker_and_goes_quiet() {
        let mut e = StreamEmitter::new();
        let (d, hit) = e.step("hello there\nUser: hi", false);
        assert_eq!(d, Some("hello there"));
        assert!(hit);
        // Quiet after stop, even on flush.
        assert_eq!(e.step("hello there\nUser: hi more", true), (None, true));
    }

    /// A non-prefix-stable decode (e.g. a byte-fallback U+FFFD resolving into
    /// the real char) goes quiet instead of emitting garbage.
    #[test]
    fn emitter_goes_quiet_on_prefix_instability() {
        let mut e = StreamEmitter::new();
        // 13 ASCII bytes; holdback 5 -> emits the first 8: "hello wo".
        let (d, _) = e.step("hello world!!", false);
        assert_eq!(d, Some("hello wo"));
        // An already-emitted byte changed -> prefix mismatch: quiet forever.
        assert_eq!(e.step("hellO world!! more", false), (None, false));
        assert_eq!(e.step("hellO world!! more", true), (None, false));
    }

    /// NATIVE streaming proof — real weights, real GPU. Ignored by default
    /// (needs the ~536MB checkpoint). Run with:
    ///   GEMMA_DIR=target/gemma-test cargo test --release --features local -- --ignored --nocapture gemma_native_stream
    /// Asserts the concatenated streamed deltas equal the returned continuation
    /// up to the stop-marker cut (the transcript paints exactly what the turn
    /// returns), and prints wall-clock tokens/sec.
    #[tokio::test]
    #[ignore]
    async fn gemma_native_stream() {
        let dir = std::env::var("GEMMA_DIR")
            .expect("set GEMMA_DIR to a folder with model.safetensors + tokenizer.json");
        let weights = std::fs::read(format!("{dir}/model.safetensors")).expect("read weights");
        let tok_bytes =
            std::fs::read(format!("{dir}/tokenizer.json")).expect("read tokenizer.json");

        let device = burn::backend::wgpu::WgpuDevice::default();
        let model = super::super::gemma::GemmaModel::<super::super::LocalBackend>::init(
            super::super::gemma::GemmaConfig::gemma_3_270m(),
            &device,
        );
        let model =
            super::super::weights::load_gemma(model, &weights, &device).expect("load_gemma");
        let tok = super::super::tokenizer::GemmaTokenizer::from_bytes(&tok_bytes)
            .expect("load tokenizer");

        let prompt = std::env::var("GEMMA_PROMPT")
            .unwrap_or_else(|_| "The capital of France is".to_string());
        let max_new: usize = std::env::var("GEMMA_MAX_NEW")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(64);

        let mut streamed = String::new();
        let mut deltas = 0usize;
        let t0 = std::time::Instant::now();
        let out = generate_streamed(&model, &tok, &prompt, max_new, &device, |d| {
            streamed.push_str(d);
            deltas += 1;
            true
        })
        .await;
        let dt = t0.elapsed().as_secs_f64();
        // Token count for tok/s: re-encode the continuation (minus BOS).
        let n_tok = tok.encode(&out).len().saturating_sub(1);
        println!(
            "\n=== GEMMA NATIVE STREAM ===\nprompt: {prompt:?}\noutput: {out:?}\n\
             deltas: {deltas}, tokens: {n_tok}, {dt:.2}s, {:.2} tok/s\n===========================\n",
            n_tok as f64 / dt.max(1e-9)
        );
        let cut = out.find(STOP_MARKER).unwrap_or(out.len());
        assert_eq!(
            streamed,
            &out[..cut],
            "streamed deltas must reproduce the returned continuation up to the stop cut"
        );
        assert!(deltas > 1, "expected incremental deltas, got a single blob");
    }

    /// NATIVE KV-cache parity + speed — real weights, real GPU. Ignored by
    /// default (needs the ~536MB checkpoint). Run with:
    ///   GEMMA_DIR=target/gemma-test GEMMA_MAX_NEW=256 cargo test --release --features local --lib -- --ignored --nocapture gemma_kv_parity
    /// Decodes the same prompt twice — v1-style full recompute every step vs
    /// the KV-cached incremental path — asserts the greedy token sequences
    /// match, and prints honest before/after tokens/sec.
    #[tokio::test]
    #[ignore]
    async fn gemma_kv_parity_and_speed() {
        let dir = std::env::var("GEMMA_DIR")
            .expect("set GEMMA_DIR to a folder with model.safetensors + tokenizer.json");
        let weights = std::fs::read(format!("{dir}/model.safetensors")).expect("read weights");
        let tok_bytes =
            std::fs::read(format!("{dir}/tokenizer.json")).expect("read tokenizer.json");

        let device = burn::backend::wgpu::WgpuDevice::default();
        let model = super::super::gemma::GemmaModel::<super::super::LocalBackend>::init(
            super::super::gemma::GemmaConfig::gemma_3_270m(),
            &device,
        );
        let model =
            super::super::weights::load_gemma(model, &weights, &device).expect("load_gemma");
        let tok = super::super::tokenizer::GemmaTokenizer::from_bytes(&tok_bytes)
            .expect("load tokenizer");

        let prompt = std::env::var("GEMMA_PROMPT")
            .unwrap_or_else(|_| "The capital of France is".to_string());
        let max_new: usize = std::env::var("GEMMA_MAX_NEW")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(256);

        // ── BEFORE: the v1 loop — full recompute of the whole sequence every
        // step (each `forward` builds and drops its own throwaway cache).
        let mut tokens: Vec<i64> = tok.encode(&prompt);
        let prompt_len = tokens.len();
        let mut uncached: Vec<i64> = Vec::new();
        let t0 = std::time::Instant::now();
        for _ in 0..max_new {
            if at_context_limit(tokens.len()) {
                break;
            }
            let seq = tokens.len();
            let input = Tensor::<super::super::LocalBackend, 1, Int>::from_data(
                TensorData::from(tokens.as_slice()),
                &device,
            )
            .reshape([1, seq]);
            let logits = model.forward(input);
            let data = logits.argmax(2).into_data_async().await.expect("read-back");
            let next = data.iter::<i64>().last().expect("nonempty");
            if next == EOS_ID {
                break;
            }
            tokens.push(next);
            uncached.push(next);
        }
        let dt_uncached = t0.elapsed().as_secs_f64();

        // ── AFTER: the shipped KV-cached path (prefill + 1 token/step).
        let mut cache = model.new_cache();
        let mut pending: Vec<i64> = tok.encode(&prompt);
        let mut total_len = pending.len();
        let mut cached: Vec<i64> = Vec::new();
        let t1 = std::time::Instant::now();
        for _ in 0..max_new {
            if at_context_limit(total_len) {
                break;
            }
            let seq = pending.len();
            let input = Tensor::<super::super::LocalBackend, 1, Int>::from_data(
                TensorData::from(pending.as_slice()),
                &device,
            )
            .reshape([1, seq]);
            let logits = model.forward_cached(input, &mut cache);
            let data = logits.argmax(2).into_data_async().await.expect("read-back");
            let next = data.iter::<i64>().last().expect("nonempty");
            if next == EOS_ID {
                break;
            }
            pending = vec![next];
            total_len += 1;
            cached.push(next);
        }
        let dt_cached = t1.elapsed().as_secs_f64();

        println!(
            "\n=== GEMMA KV PARITY + SPEED ===\nprompt: {prompt:?} ({prompt_len} tokens)\n\
             uncached: {} tokens in {dt_uncached:.2}s = {:.2} tok/s\n\
             cached:   {} tokens in {dt_cached:.2}s = {:.2} tok/s ({:.1}x)\n\
             text: {:?}\n===============================\n",
            uncached.len(),
            uncached.len() as f64 / dt_uncached.max(1e-9),
            cached.len(),
            cached.len() as f64 / dt_cached.max(1e-9),
            dt_uncached / dt_cached.max(1e-9),
            tok.decode(&cached),
        );
        assert_eq!(
            uncached, cached,
            "greedy token sequences must match between the uncached and KV-cached paths"
        );
        assert!(!cached.is_empty(), "no tokens generated");
    }

    /// Holdback floors to a char boundary — no slice panic on multibyte tails.
    #[test]
    fn emitter_holdback_respects_char_boundaries() {
        let mut e = StreamEmitter::new();
        // "aééé" = 1 + 3×2 bytes = 7; holdback 5 -> end 2, which is inside the
        // first 'é' -> floored to 1.
        let (d, _) = e.step("aééé", false);
        assert_eq!(d, Some("a"));
        let (d, _) = e.step("aééé", true);
        assert_eq!(d, Some("ééé"));
    }
}
