//! Gemma 3 270M, implemented in Burn.
//!
//! Architecture verified against the OFFICIAL `google/gemma-3-270m`
//! `config.json` (sourced via the ungated `unsloth/gemma-3-270m` mirror, which
//! re-uploads Google's weights+config unchanged). See [`GemmaConfig`] for the
//! exact constants.
//!
//! **STATUS — honest scope.** The model is written and COMPILES (native +
//! wasm32). It is **NOT yet forward-pass-validated against reference logits** —
//! that is the next milestone and needs (a) the weights + tokenizer wired in and
//! (b) a convention check that only a reference comparison can settle:
//!   1. **RoPE pairing.** Burn's [`RotaryEncoding`] and HF Gemma may pair
//!      rotation dimensions differently (interleaved `(i, i+1)` vs split
//!      `(i, i+d/2)`). If they differ, the loaded q/k projections need a
//!      permutation. This is THE classic "compiles but outputs garbage" trap.
//!
//! **Sliding-window attention IS implemented** (it was gap 2 of the original
//! STATUS note): per `config.json`, 15 of the 18 layers are `sliding_attention`
//! (`layer_types`; every 6th — indices 5/11/17 — is `full_attention`) with
//! `"sliding_window": 512`. The mask semantics mirror HF `transformers`
//! `masking_utils.py` (`sliding_window_causal_mask_function`): a query at
//! absolute position `q` attends keys `k` with `k <= q && k > q - 512` — the
//! last 512 positions including itself ([`attn_blocked`]). Sliding layers also
//! TRIM their KV cache to the last `window - 1` entries ([`kept_cache_len`]) —
//! older keys can never be attended again, so per-layer KV memory is capped.
//! RoPE stays ABSOLUTE throughout: keys are rotated at their absolute positions
//! when first cached, and trimming only drops tensor rows — no re-rotation.
//! Gemma-specific details already baked in: GQA (4 q-heads / 1 kv-head),
//! per-head QK-norm (Gemma 3, replacing Gemma 2's attention softcapping),
//! dual-θ RoPE (1e6 global / 1e4 sliding), 4 norms per layer, GeGLU MLP, tied
//! embeddings, and embedding scaling by √hidden. RMSNorm uses Burn's plain
//! `x/rms·γ`; Gemma's `(1+weight)` convention is applied by the weight loader
//! (add 1.0 to each γ at load), keeping this code convention-free.

use burn::module::Module;
use burn::nn::{
    Embedding, EmbeddingConfig, Linear, LinearConfig, RmsNorm, RmsNormConfig, RotaryEncoding,
    RotaryEncodingConfig,
};
use burn::tensor::{activation, backend::Backend, Int, Tensor, TensorData};

/// Cap on the precomputed RoPE frequency cache. The model's true context is
/// `max_position_embeddings` (32768), but a full 32768×256×2 f32 cache is ~64MB
/// — too heavy to build eagerly in a browser tab. 4096 is plenty for the
/// in-tab fast-path / first native validation; raise when long-context lands.
///
/// `pub(crate)` so the decoder ([`super::generate`]) can bound its sequence
/// length against the SAME constant: `forward` indexes the RoPE cache by
/// position, so a sequence reaching `ROPE_CACHE_LEN` would index past the cache
/// and panic the tab. The generation loop guards on this for a clean stop.
pub(crate) const ROPE_CACHE_LEN: usize = 4096;

/// Verified Gemma 3 270M hyperparameters (`google/gemma-3-270m/config.json`).
#[derive(Clone, Debug)]
pub struct GemmaConfig {
    pub vocab_size: usize,              // 262144
    pub hidden_size: usize,            // 640
    pub intermediate_size: usize,      // 2048
    pub num_layers: usize,             // 18
    pub num_heads: usize,              // 4  (query heads)
    pub num_kv_heads: usize,           // 1  (GQA — key/value heads)
    pub head_dim: usize,               // 256 (note: 4*256=1024 != hidden 640)
    pub rope_theta: f32,               // 1_000_000 — full-attention layers
    pub rope_local_base_freq: f32,     // 10_000   — sliding-window layers
    pub sliding_window: usize,         // 512
    pub sliding_window_pattern: usize, // 6 — every 6th layer is full attention
    pub query_pre_attn_scalar: f64,    // 256 → attention scores scaled by 1/√256
    pub rms_norm_eps: f64,             // 1e-6
}

impl GemmaConfig {
    /// The canonical Gemma 3 270M configuration.
    pub fn gemma_3_270m() -> Self {
        Self {
            vocab_size: 262_144,
            hidden_size: 640,
            intermediate_size: 2048,
            num_layers: 18,
            num_heads: 4,
            num_kv_heads: 1,
            head_dim: 256,
            rope_theta: 1_000_000.0,
            rope_local_base_freq: 10_000.0,
            sliding_window: 512,
            sliding_window_pattern: 6,
            query_pre_attn_scalar: 256.0,
            rms_norm_eps: 1e-6,
        }
    }

    /// Whether layer `layer` (0-indexed) uses full (global) attention. Gemma 3
    /// makes every `sliding_window_pattern`-th layer global — for 270M that's
    /// layers 5, 11, 17 (the `full_attention` entries in `layer_types`). All
    /// others use sliding-window attention with `sliding_window` span.
    pub fn is_full_attention(&self, layer: usize) -> bool {
        (layer + 1) % self.sliding_window_pattern == 0
    }

    /// The attention window for layer `layer`: `None` on `full_attention`
    /// layers (global causal), `Some(sliding_window)` on `sliding_attention`
    /// layers (causal AND within the last `sliding_window` positions).
    pub fn layer_window(&self, layer: usize) -> Option<usize> {
        if self.is_full_attention(layer) {
            None
        } else {
            Some(self.sliding_window)
        }
    }

    /// Attention score scale: `1/√query_pre_attn_scalar` (Gemma scales queries,
    /// not by `1/√head_dim`). Equal here only because the scalar == head_dim.
    fn attn_scale(&self) -> f64 {
        1.0 / self.query_pre_attn_scalar.sqrt()
    }
}

/// Whether the key at absolute position `k_abs` is MASKED for the query at
/// absolute position `q_abs`. This is THE mask predicate — the tensor masks in
/// [`GemmaModel::forward_cached`] are built from it, so its unit tests cover
/// the shipped math. Semantics match HF `transformers` `masking_utils.py`:
/// causal (`k <= q`) on every layer, AND-ed with the sliding overlay
/// (`k > q - window`) on sliding layers — a query sees exactly the last
/// `window` positions including itself. `k_abs + w <= q_abs` is the
/// underflow-safe form of `k_abs <= q_abs - w`.
pub(crate) fn attn_blocked(q_abs: usize, k_abs: usize, window: Option<usize>) -> bool {
    k_abs > q_abs || window.is_some_and(|w| k_abs + w <= q_abs)
}

/// How many cached positions a layer must RETAIN after having processed
/// `processed` total positions. Global layers keep everything; a sliding layer
/// keeps the last `window - 1` (the next query at position `processed` attends
/// `(processed - window, processed]` — `window - 1` cached keys plus itself),
/// so its KV cache never grows past `window - 1` entries.
pub(crate) fn kept_cache_len(processed: usize, window: Option<usize>) -> usize {
    match window {
        Some(w) => processed.min(w - 1),
        None => processed,
    }
}

/// Whether an additive mask is needed at all for a forward of `seq` new
/// positions over `kv_len` total keys. `seq == 1` skips the causal mask (all
/// keys are past); a sliding layer additionally needs `kv_len <= window` (with
/// the [`kept_cache_len`] trim that always holds on 1-token decode steps, so
/// decode stays mask-free on every layer).
pub(crate) fn needs_mask(seq: usize, kv_len: usize, window: Option<usize>) -> bool {
    seq > 1 || window.is_some_and(|w| kv_len > w)
}

/// Keep the last `keep` rows of a `[batch, heads, seq, head_dim]` tensor along
/// the sequence dim (the sliding-layer KV-cache trim).
fn trim_to_last<B: Backend>(t: Tensor<B, 4>, keep: usize) -> Tensor<B, 4> {
    let [b, h, s, d] = t.dims();
    t.slice([0..b, 0..h, s - keep..s, 0..d])
}

/// Gated MLP (GeGLU): `down(gelu(gate(x)) ⊙ up(x))`. `gelu` is the tanh
/// approximation (`gelu_pytorch_tanh` in the config).
#[derive(Module, Debug)]
pub struct Mlp<B: Backend> {
    gate_proj: Linear<B>,
    up_proj: Linear<B>,
    down_proj: Linear<B>,
}

impl<B: Backend> Mlp<B> {
    fn init(cfg: &GemmaConfig, device: &B::Device) -> Self {
        let lin = |i, o| LinearConfig::new(i, o).with_bias(false).init(device);
        Self {
            gate_proj: lin(cfg.hidden_size, cfg.intermediate_size),
            up_proj: lin(cfg.hidden_size, cfg.intermediate_size),
            down_proj: lin(cfg.intermediate_size, cfg.hidden_size),
        }
    }

    fn forward(&self, x: Tensor<B, 3>) -> Tensor<B, 3> {
        let gate = activation::gelu(self.gate_proj.forward(x.clone()));
        let up = self.up_proj.forward(x);
        self.down_proj.forward(gate * up)
    }
}

/// Grouped-query attention with per-head QK-norm and RoPE. The chosen RoPE
/// (global vs sliding) and the additive causal mask are passed in by the
/// decoder layer so the per-layer θ selection lives in one place.
#[derive(Module, Debug)]
pub struct Attention<B: Backend> {
    q_proj: Linear<B>,
    k_proj: Linear<B>,
    v_proj: Linear<B>,
    o_proj: Linear<B>,
    q_norm: RmsNorm<B>,
    k_norm: RmsNorm<B>,
}

impl<B: Backend> Attention<B> {
    fn init(cfg: &GemmaConfig, device: &B::Device) -> Self {
        let q_out = cfg.num_heads * cfg.head_dim;
        let kv_out = cfg.num_kv_heads * cfg.head_dim;
        let lin = |i, o| LinearConfig::new(i, o).with_bias(false).init(device);
        let norm = || RmsNormConfig::new(cfg.head_dim).with_epsilon(cfg.rms_norm_eps).init(device);
        Self {
            q_proj: lin(cfg.hidden_size, q_out),
            k_proj: lin(cfg.hidden_size, kv_out),
            v_proj: lin(cfg.hidden_size, kv_out),
            o_proj: lin(q_out, cfg.hidden_size),
            q_norm: norm(),
            k_norm: norm(),
        }
    }

    /// Attention over the layer's KV cache. `x` holds the `seq` NEW positions
    /// starting at absolute position `offset` (`=` the processed count); the
    /// new keys/values are RoPE'd at their ABSOLUTE positions, appended to
    /// `cache`, and the queries attend over the whole cached tensor. `mask` is
    /// the additive mask over `[seq, cached+seq]` (causal, AND within-window on
    /// sliding layers) — `None` when nothing would be masked (see
    /// [`needs_mask`]). `window` is `Some(w)` on sliding layers: after
    /// attending, the stored cache is TRIMMED to its last `w - 1` rows
    /// ([`kept_cache_len`]) — those keys keep the absolute-position RoPE they
    /// were written with, so trimming never touches position math.
    #[allow(clippy::too_many_arguments)]
    fn forward(
        &self,
        x: Tensor<B, 3>,
        cfg: &GemmaConfig,
        rope: &RotaryEncoding<B>,
        mask: Option<Tensor<B, 4>>,
        cache: &mut Option<(Tensor<B, 4>, Tensor<B, 4>)>,
        offset: usize,
        window: Option<usize>,
    ) -> Tensor<B, 3> {
        let [batch, seq, _] = x.dims();
        let (h, kv, hd) = (cfg.num_heads, cfg.num_kv_heads, cfg.head_dim);

        // Project then split into heads: [b, s, h*hd] -> [b, h, s, hd].
        let q = self.q_proj.forward(x.clone()).reshape([batch, seq, h, hd]);
        let k = self.k_proj.forward(x.clone()).reshape([batch, seq, kv, hd]);
        let v = self.v_proj.forward(x).reshape([batch, seq, kv, hd]);

        // QK-norm (Gemma 3): RMSNorm over the head_dim before RoPE.
        let q = self.q_norm.forward(q).swap_dims(1, 2); // [b, h, s, hd]
        let k = self.k_norm.forward(k).swap_dims(1, 2); // [b, kv, s, hd]
        let v = v.swap_dims(1, 2);

        // RoPE on q and k at their ABSOLUTE positions (offset..offset+seq).
        let q = rope.apply(q, offset);
        let k = rope.apply(k, offset);

        // Append the new k/v to the layer cache and attend over ALL of it.
        let (k_all, v_all) = match cache.take() {
            Some((ck, cv)) => (Tensor::cat(vec![ck, k], 2), Tensor::cat(vec![cv, v], 2)),
            None => (k, v),
        };
        // Store the cache, trimmed on sliding layers: keys older than the
        // window can never be attended by any FUTURE query, so only the last
        // `window - 1` rows are kept. The trimmed rows retain their absolute-
        // position RoPE — no recomputation.
        let kv_len = k_all.dims()[2];
        let keep = kept_cache_len(offset + seq, window);
        *cache = Some(if keep < kv_len {
            (
                trim_to_last(k_all.clone(), keep),
                trim_to_last(v_all.clone(), keep),
            )
        } else {
            (k_all.clone(), v_all.clone())
        });

        // GQA: repeat the kv heads up to the query-head count.
        let k_all = k_all.repeat_dim(1, h / kv);
        let v_all = v_all.repeat_dim(1, h / kv);

        // Scaled dot-product attention with the additive causal mask.
        let mut scores = q
            .matmul(k_all.swap_dims(2, 3))
            .mul_scalar(cfg.attn_scale());
        if let Some(mask) = mask {
            scores = scores + mask;
        }
        let probs = activation::softmax(scores, 3);
        let ctx = probs.matmul(v_all); // [b, h, s, hd]

        // Merge heads -> [b, s, h*hd] and project out.
        let ctx = ctx.swap_dims(1, 2).reshape([batch, seq, h * hd]);
        self.o_proj.forward(ctx)
    }
}

/// One Gemma 3 decoder layer. Note the FOUR norms: Gemma 3 sandwiches both the
/// attention block and the MLP block with a pre- and a post-norm.
#[derive(Module, Debug)]
pub struct DecoderLayer<B: Backend> {
    input_layernorm: RmsNorm<B>,
    self_attn: Attention<B>,
    post_attention_layernorm: RmsNorm<B>,
    pre_feedforward_layernorm: RmsNorm<B>,
    mlp: Mlp<B>,
    post_feedforward_layernorm: RmsNorm<B>,
}

impl<B: Backend> DecoderLayer<B> {
    fn init(cfg: &GemmaConfig, device: &B::Device) -> Self {
        let norm = || {
            RmsNormConfig::new(cfg.hidden_size)
                .with_epsilon(cfg.rms_norm_eps)
                .init(device)
        };
        Self {
            input_layernorm: norm(),
            self_attn: Attention::init(cfg, device),
            post_attention_layernorm: norm(),
            pre_feedforward_layernorm: norm(),
            mlp: Mlp::init(cfg, device),
            post_feedforward_layernorm: norm(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn forward(
        &self,
        x: Tensor<B, 3>,
        cfg: &GemmaConfig,
        rope: &RotaryEncoding<B>,
        mask: Option<Tensor<B, 4>>,
        cache: &mut Option<(Tensor<B, 4>, Tensor<B, 4>)>,
        offset: usize,
        window: Option<usize>,
    ) -> Tensor<B, 3> {
        // h = x + post_attn_norm(attn(input_norm(x)))
        let normed = self.input_layernorm.forward(x.clone());
        let attn = self
            .self_attn
            .forward(normed, cfg, rope, mask, cache, offset, window);
        let h = x + self.post_attention_layernorm.forward(attn);
        // out = h + post_ff_norm(mlp(pre_ff_norm(h)))
        let normed = self.pre_feedforward_layernorm.forward(h.clone());
        let ff = self.mlp.forward(normed);
        h + self.post_feedforward_layernorm.forward(ff)
    }
}

/// The full Gemma 3 270M causal LM. Embeddings are tied: the output logits are
/// computed against the (transposed) input embedding matrix — Gemma has no
/// separate `lm_head` weight.
#[derive(Module, Debug)]
pub struct GemmaModel<B: Backend> {
    embed: Embedding<B>,
    layers: Vec<DecoderLayer<B>>,
    norm: RmsNorm<B>,
    rope_global: RotaryEncoding<B>,
    rope_local: RotaryEncoding<B>,
    /// Non-persistent: architecture constants, not checkpoint parameters.
    #[module(skip)]
    config: GemmaConfig,
}

impl<B: Backend> GemmaModel<B> {
    /// Build a fresh model with random-initialised parameters (the weight loader
    /// overwrites these from the checkpoint). Constructing it asserts every
    /// shape in the verified config lines up.
    pub fn init(cfg: GemmaConfig, device: &B::Device) -> Self {
        let layers = (0..cfg.num_layers)
            .map(|_| DecoderLayer::init(&cfg, device))
            .collect();
        let rope = |theta| {
            RotaryEncodingConfig::new(ROPE_CACHE_LEN, cfg.head_dim)
                .with_theta(theta)
                .init(device)
        };
        Self {
            embed: EmbeddingConfig::new(cfg.vocab_size, cfg.hidden_size).init(device),
            layers,
            norm: RmsNormConfig::new(cfg.hidden_size)
                .with_epsilon(cfg.rms_norm_eps)
                .init(device),
            rope_global: rope(cfg.rope_theta),
            rope_local: rope(cfg.rope_local_base_freq),
            config: cfg,
        }
    }

    /// Forward pass: token ids `[batch, seq]` -> logits `[batch, seq, vocab]`.
    /// Positions are absolute `0..seq` — the uncached (full-recompute) path,
    /// implemented as [`Self::forward_cached`] over a throwaway cache so there
    /// is exactly ONE attention codepath.
    pub fn forward(&self, tokens: Tensor<B, 2, Int>) -> Tensor<B, 3> {
        let mut cache = self.new_cache();
        self.forward_cached(tokens, &mut cache)
    }

    /// An empty per-layer KV cache sized for this model.
    pub fn new_cache(&self) -> KvCache<B> {
        KvCache {
            layers: vec![None; self.config.num_layers],
            len: 0,
        }
    }

    /// Incremental forward pass: `tokens` `[batch, seq]` are the NEW positions
    /// `cache.len()..cache.len()+seq`; their keys/values append to `cache` and
    /// only the new positions' logits `[batch, seq, vocab]` are computed. Call
    /// once with the full prompt (prefill), then once per generated token —
    /// each decode step is O(seq_total) instead of the O(seq_total²) full
    /// recompute.
    ///
    /// Per-layer masking (same semantics as [`Self::forward`], which delegates
    /// here): `full_attention` layers get the plain causal mask; the
    /// `sliding_attention` layers get causal AND within the last
    /// `sliding_window` positions ([`attn_blocked`]), and their KV caches are
    /// trimmed to the last `sliding_window - 1` entries ([`kept_cache_len`]).
    /// All positions are ABSOLUTE (`offset + i`) — the trim drops rows, never
    /// re-indexes RoPE.
    pub fn forward_cached(&self, tokens: Tensor<B, 2, Int>, cache: &mut KvCache<B>) -> Tensor<B, 3> {
        let cfg = &self.config;
        let [batch, seq] = tokens.dims();
        let device = tokens.device();
        let offset = cache.len;

        // Embed and scale by √hidden (Gemma's input normaliser).
        let scale = (cfg.hidden_size as f64).sqrt();
        let mut x = self.embed.forward(tokens).mul_scalar(scale);

        // Additive masks [1, 1, seq, kv_len], built from THE predicate
        // ([`attn_blocked`]) over absolute positions; -inf where blocked.
        // Broadcast over batch and heads. Two variants per forward: global
        // (causal only, kv_len = offset+seq) and sliding (causal AND
        // within-window, kv_len = trimmed cache + seq — sliding layers all
        // share one cache length by construction). `None` when nothing would
        // be masked ([`needs_mask`]) — in particular every 1-token decode
        // step, on BOTH layer kinds (the sliding trim keeps kv_len <= window).
        let build_mask = |window: Option<usize>| -> Option<Tensor<B, 4>> {
            let kv_len = kept_cache_len(offset, window) + seq;
            needs_mask(seq, kv_len, window).then(|| {
                let kv_start = offset + seq - kv_len; // abs position of key col 0
                let mut rows = vec![0f32; seq * kv_len];
                for i in 0..seq {
                    for j in 0..kv_len {
                        if attn_blocked(offset + i, kv_start + j, window) {
                            rows[i * kv_len + j] = f32::NEG_INFINITY;
                        }
                    }
                }
                Tensor::<B, 1>::from_data(TensorData::from(rows.as_slice()), &device)
                    .reshape([1, 1, seq, kv_len])
            })
        };
        let mask_global = build_mask(None);
        let mask_sliding = build_mask(Some(cfg.sliding_window));
        let _ = batch;

        for (i, layer) in self.layers.iter().enumerate() {
            let (rope, mask, window) = if cfg.is_full_attention(i) {
                (&self.rope_global, mask_global.clone(), None)
            } else {
                (
                    &self.rope_local,
                    mask_sliding.clone(),
                    Some(cfg.sliding_window),
                )
            };
            x = layer.forward(x, cfg, rope, mask, &mut cache.layers[i], offset, window);
        }
        cache.len += seq;
        let x = self.norm.forward(x);

        // Tied LM head: logits = hidden · embedᵀ.  [b,s,d] · [d,vocab].
        let embed_t = self.embed.weight.val().transpose(); // [hidden, vocab]
        x.matmul(embed_t.unsqueeze::<3>())
    }
}

/// Per-layer key/value cache for incremental decode. One `(k, v)` pair per
/// decoder layer, each `[batch, kv_heads, cached_seq, head_dim]`. `len` is the
/// number of positions PROCESSED (the next token's absolute position) — the
/// physical `cached_seq` equals it only on `full_attention` layers; sliding
/// layers are trimmed to at most `sliding_window - 1` rows
/// ([`kept_cache_len`]). Build via [`GemmaModel::new_cache`]; feed to
/// [`GemmaModel::forward_cached`].
///
/// Memory: 1 kv-head × 256 dims × 2 tensors × 4 bytes = 2KB per token per
/// layer. Past 511 tokens only the 3 global layers keep growing (~6KB/token);
/// the 15 sliding layers are capped at ~15.7MB total.
pub struct KvCache<B: Backend> {
    layers: Vec<Option<(Tensor<B, 4>, Tensor<B, 4>)>>,
    len: usize,
}

impl<B: Backend> KvCache<B> {
    /// Number of positions processed so far (see the type doc — sliding
    /// layers physically retain fewer).
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when nothing has been cached yet.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sliding mask matches the HF `transformers` truth table
    /// (`masking_utils.py` docstring, `sliding_window=3`, 5 positions):
    /// allowed = `kv_idx <= q_idx && kv_idx > q_idx - sliding_window`.
    #[test]
    fn sliding_mask_matches_hf_truth_table() {
        #[rustfmt::skip]
        let allowed = [
            [true,  false, false, false, false],
            [true,  true,  false, false, false],
            [true,  true,  true,  false, false],
            [false, true,  true,  true,  false],
            [false, false, true,  true,  true ],
        ];
        for (q, row) in allowed.iter().enumerate() {
            for (k, &want) in row.iter().enumerate() {
                assert_eq!(!attn_blocked(q, k, Some(3)), want, "q={q} k={k}");
            }
        }
    }

    /// Global layers: plain causal — blocked iff the key is in the future.
    #[test]
    fn global_mask_is_plain_causal() {
        for q in 0..8 {
            for k in 0..8 {
                assert_eq!(attn_blocked(q, k, None), k > q, "q={q} k={k}");
            }
        }
    }

    /// THE regression guard: below the window the sliding mask is IDENTICAL to
    /// the causal mask — windowing must be a no-op for short contexts, so a
    /// sub-512-token decode is bit-for-bit what it was before windowing landed.
    #[test]
    fn windowing_is_noop_below_the_window() {
        let w = GemmaConfig::gemma_3_270m().sliding_window;
        for q in 0..w {
            for k in 0..w {
                assert_eq!(attn_blocked(q, k, Some(w)), attn_blocked(q, k, None));
            }
        }
        // ...and the first position past the window is where they diverge.
        assert!(attn_blocked(w, 0, Some(w)));
        assert!(!attn_blocked(w, 0, None));
        // A query attends exactly `w` keys: (q-w, q].
        let q = 3 * w + 7;
        let attended = (0..=q).filter(|&k| !attn_blocked(q, k, Some(w))).count();
        assert_eq!(attended, w);
        assert!(!attn_blocked(q, q, Some(w)), "self is always attended");
        assert!(!attn_blocked(q, q + 1 - w, Some(w)), "oldest in-window key");
        assert!(attn_blocked(q, q - w, Some(w)), "first out-of-window key");
    }

    /// The layer pattern matches `config.json` `layer_types`: 18 layers,
    /// `full_attention` at 5/11/17, `sliding_attention` (window 512) elsewhere.
    #[test]
    fn layer_pattern_matches_config_layer_types() {
        let cfg = GemmaConfig::gemma_3_270m();
        let full: Vec<usize> = (0..cfg.num_layers)
            .filter(|&l| cfg.is_full_attention(l))
            .collect();
        assert_eq!(full, [5, 11, 17]);
        for l in 0..cfg.num_layers {
            let want = if full.contains(&l) { None } else { Some(512) };
            assert_eq!(cfg.layer_window(l), want, "layer {l}");
        }
    }

    /// Cache-trim math: sliding layers cap at `window - 1` retained rows, and
    /// with that trim a 1-token decode step NEVER needs a mask (kv_len <= w),
    /// while an untrimmed-length step past the window would.
    #[test]
    fn cache_trim_caps_and_keeps_decode_mask_free() {
        let w = 512;
        assert_eq!(kept_cache_len(0, Some(w)), 0);
        assert_eq!(kept_cache_len(w - 1, Some(w)), w - 1);
        assert_eq!(kept_cache_len(w, Some(w)), w - 1); // capped
        assert_eq!(kept_cache_len(10_000, Some(w)), w - 1);
        assert_eq!(kept_cache_len(10_000, None), 10_000); // global: keep all
        for offset in [0, 1, w - 1, w, w + 1, 4 * w] {
            assert!(!needs_mask(1, kept_cache_len(offset, Some(w)) + 1, Some(w)));
            assert!(!needs_mask(1, offset + 1, None));
        }
        assert!(needs_mask(1, w + 1, Some(w))); // untrimmed past-window step would
        assert!(needs_mask(2, 2, None)); // any multi-token prefill masks
        assert!(needs_mask(2, 2, Some(w)));
    }

    /// Trim consistency: across any (offset, seq) schedule, the concatenated
    /// kv tensor (`trimmed cache + seq`) always holds at least the rows the
    /// post-store trim keeps (`trim_to_last` can't underflow), and the mask
    /// builder's assumed cache length matches what the previous call stored.
    #[test]
    fn trim_and_mask_builder_agree_on_cache_length() {
        let w = 512;
        for window in [Some(w), None] {
            let mut cached = 0usize; // physical rows actually stored
            let mut offset = 0usize; // processed positions
            for seq in [600usize, 1, 1, 1, 300, 1] {
                // forward_cached's build_mask assumes this cache length:
                assert_eq!(cached, kept_cache_len(offset, window), "pre @{offset}");
                let kv_len = cached + seq;
                let keep = kept_cache_len(offset + seq, window);
                assert!(keep <= kv_len, "trim would underflow @{offset}+{seq}");
                cached = keep; // what Attention::forward stores
                offset += seq;
            }
        }
    }
}
