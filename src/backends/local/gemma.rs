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
//! (b) two convention checks that only a reference comparison can settle:
//!   1. **RoPE pairing.** Burn's [`RotaryEncoding`] and HF Gemma may pair
//!      rotation dimensions differently (interleaved `(i, i+1)` vs split
//!      `(i, i+d/2)`). If they differ, the loaded q/k projections need a
//!      permutation. This is THE classic "compiles but outputs garbage" trap.
//!   2. **Sliding-window mask.** Only the causal mask is applied here; the
//!      512-token sliding window is a no-op for prompts shorter than 512 (fine
//!      for first validation) but must be added for long contexts.
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
use burn::tensor::{activation, backend::Backend, Int, Tensor};

/// Cap on the precomputed RoPE frequency cache. The model's true context is
/// `max_position_embeddings` (32768), but a full 32768×256×2 f32 cache is ~64MB
/// — too heavy to build eagerly in a browser tab. 4096 is plenty for the
/// in-tab fast-path / first native validation; raise when long-context lands.
const ROPE_CACHE_LEN: usize = 4096;

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

    /// Attention score scale: `1/√query_pre_attn_scalar` (Gemma scales queries,
    /// not by `1/√head_dim`). Equal here only because the scalar == head_dim.
    fn attn_scale(&self) -> f64 {
        1.0 / self.query_pre_attn_scalar.sqrt()
    }
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

    fn forward(
        &self,
        x: Tensor<B, 3>,
        cfg: &GemmaConfig,
        rope: &RotaryEncoding<B>,
        mask: Tensor<B, 4>,
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

        // RoPE on q and k (applied over the last dim = head_dim).
        let q = rope.forward(q);
        let k = rope.forward(k);

        // GQA: repeat the kv heads up to the query-head count.
        let k = k.repeat_dim(1, h / kv);
        let v = v.repeat_dim(1, h / kv);

        // Scaled dot-product attention with the additive causal mask.
        let scores = q
            .matmul(k.swap_dims(2, 3))
            .mul_scalar(cfg.attn_scale())
            + mask;
        let probs = activation::softmax(scores, 3);
        let ctx = probs.matmul(v); // [b, h, s, hd]

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

    fn forward(
        &self,
        x: Tensor<B, 3>,
        cfg: &GemmaConfig,
        rope: &RotaryEncoding<B>,
        mask: Tensor<B, 4>,
    ) -> Tensor<B, 3> {
        // h = x + post_attn_norm(attn(input_norm(x)))
        let normed = self.input_layernorm.forward(x.clone());
        let attn = self.self_attn.forward(normed, cfg, rope, mask);
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
    pub fn forward(&self, tokens: Tensor<B, 2, Int>) -> Tensor<B, 3> {
        let cfg = &self.config;
        let [batch, seq] = tokens.dims();
        let device = tokens.device();

        // Embed and scale by √hidden (Gemma's input normaliser).
        let scale = (cfg.hidden_size as f64).sqrt();
        let mut x = self.embed.forward(tokens).mul_scalar(scale);

        // Additive causal mask [1, 1, seq, seq]: 0 on/below the diagonal,
        // -inf above (future positions). Broadcasts over batch and heads.
        let q_idx = Tensor::<B, 1, Int>::arange(0..seq as i64, &device).reshape([seq, 1]);
        let k_idx = Tensor::<B, 1, Int>::arange(0..seq as i64, &device).reshape([1, seq]);
        let future = k_idx.greater(q_idx); // bool [seq, seq], true where k>q
        let mask = Tensor::<B, 2>::zeros([seq, seq], &device)
            .mask_fill(future, f32::NEG_INFINITY)
            .reshape([1, 1, seq, seq]);
        let _ = batch;

        for (i, layer) in self.layers.iter().enumerate() {
            let rope = if cfg.is_full_attention(i) {
                &self.rope_global
            } else {
                &self.rope_local
            };
            x = layer.forward(x, cfg, rope, mask.clone());
        }
        let x = self.norm.forward(x);

        // Tied LM head: logits = hidden · embedᵀ.  [b,s,d] · [d,vocab].
        let embed_t = self.embed.weight.val().transpose(); // [hidden, vocab]
        x.matmul(embed_t.unsqueeze::<3>())
    }
}
