//! Safetensors → Burn [`GemmaModel`] weight loader.
//!
//! Loads a Hugging Face Gemma 3 270M `model.safetensors` checkpoint (the
//! ungated `unsloth/gemma-3-270m` mirror) from an in-memory `&[u8]` into a
//! freshly-initialised [`GemmaModel`]. Compiles on native AND
//! `wasm32-unknown-unknown` — the load path needs no filesystem (the browser
//! fetches the bytes / reads them out of OPFS and hands us the buffer).
//!
//! ## What the loader has to reconcile
//!
//! The HF checkpoint and Burn's hand-written module disagree on three things,
//! all fixed here at load time (zero runtime cost):
//!
//! 1. **Names.** HF uses `model.layers.N.self_attn.q_proj.weight`,
//!    `…input_layernorm.weight`, `model.embed_tokens.weight`, `model.norm.weight`.
//!    Burn's field tree uses `layers.N.self_attn.q_proj.weight`,
//!    `…input_layernorm.gamma`, `embed.weight`, `norm.gamma`. [`hf_to_burn_path`]
//!    does the rename. Any tied `lm_head.weight` in the checkpoint is DROPPED —
//!    Gemma ties the LM head to the input embedding and the Burn model has no
//!    `lm_head` parameter (its `forward` uses `embed.weight^T`).
//!
//! 2. **Layout + dtype.** HF `Linear` weight is `[out, in]`; Burn's row-major
//!    `Linear` is `[in, out]` → transpose. The checkpoint is bf16; the wgpu
//!    backend's float element is f32 → every float tensor is cast to f32.
//!
//! 3. **RMSNorm convention.** Burn's `RmsNorm` is plain `x/rms · γ` with γ
//!    init-to-ones; Gemma applies `x/rms · (1 + w)`. So **+1.0 is added to every
//!    loaded RMSNorm γ** (the four block norms, the two QK-norms, and the final
//!    norm).
//!
//! 4. **RoPE pairing.** Burn's [`RotaryEncoding`] rotates *interleaved* channel
//!    pairs `(2j, 2j+1)`; HF Gemma rotates *split-half* pairs `(j, j+d/2)`. The
//!    two use the same angle set but assign each angle to a different physical
//!    channel pair, so loading HF q/k projections verbatim into Burn's rope
//!    produces garbage. We bake the fixed permutation
//!    `perm[2j]=j, perm[2j+1]=j+d/2` into the OUTPUT channels of `q_proj`/`k_proj`
//!    (per head) and into the `q_norm`/`k_norm` γ vectors (which are applied
//!    per-channel BEFORE rope), so Burn's interleaved op reproduces HF exactly.
//!    `v_proj`/`o_proj` are left untouched (V is never rotated). Because q and k
//!    receive the SAME permutation, attention scores are identical to HF.

// `Rc` is the smart pointer `TensorSnapshot` stores its lazy `data_fn` in.
// Available on both native and wasm32 at the language level.
use std::rc::Rc;

use burn::module::ParamId;
use burn::tensor::backend::Backend;
use burn::tensor::{DType, Shape, TensorData};

use burn_store::{
    ModuleAdapter, ModuleSnapshot, ModuleStore, SafetensorsStore, TensorSnapshot,
    TensorSnapshotError,
};

use super::gemma::{GemmaConfig, GemmaModel};

/// Number of loadable `Param`s a correctly-mapped Gemma 3 270M checkpoint fills:
/// `embed (1) + final norm (1) + 18 * per-layer (13)` where per-layer is
/// 4 block norms + (q/k/v/o)_proj (4) + (q/k)_norm (2) + (gate/up/down)_proj (3).
/// Asserted after `apply` so a mis-named / unmapped tensor fails loudly instead
/// of silently leaving a parameter at its random init.
const EXPECTED_PARAMS: usize = 2 + 18 * 13;

/// Map a Hugging Face Gemma3 tensor name to the corresponding Burn module path
/// (dot notation). Returns `None` to DROP a tensor (e.g. a tied `lm_head.weight`
/// that some exports ship — the Burn model has no such parameter).
fn hf_to_burn_path(name: &str) -> Option<String> {
    // Drop a tied LM head if the checkpoint shipped one (Gemma ties it; the Burn
    // model has no lm_head Param). Match before stripping `model.`.
    if name == "lm_head.weight" || name == "model.lm_head.weight" {
        return None;
    }

    // Most Gemma3 keys are under the `model.` prefix; tolerate its absence.
    let n = name.strip_prefix("model.").unwrap_or(name);

    if n == "embed_tokens.weight" {
        return Some("embed.weight".to_string());
    }
    if n == "norm.weight" {
        return Some("norm.gamma".to_string());
    }

    // layers.{i}.<tail>
    let rest = n.strip_prefix("layers.")?;
    let (idx, tail) = rest.split_once('.')?;
    let mapped_tail = match tail {
        "input_layernorm.weight" => "input_layernorm.gamma",
        "post_attention_layernorm.weight" => "post_attention_layernorm.gamma",
        "pre_feedforward_layernorm.weight" => "pre_feedforward_layernorm.gamma",
        "post_feedforward_layernorm.weight" => "post_feedforward_layernorm.gamma",
        "self_attn.q_proj.weight" => "self_attn.q_proj.weight",
        "self_attn.k_proj.weight" => "self_attn.k_proj.weight",
        "self_attn.v_proj.weight" => "self_attn.v_proj.weight",
        "self_attn.o_proj.weight" => "self_attn.o_proj.weight",
        "self_attn.q_norm.weight" => "self_attn.q_norm.gamma",
        "self_attn.k_norm.weight" => "self_attn.k_norm.gamma",
        "mlp.gate_proj.weight" => "mlp.gate_proj.weight",
        "mlp.up_proj.weight" => "mlp.up_proj.weight",
        "mlp.down_proj.weight" => "mlp.down_proj.weight",
        _ => return None,
    };
    Some(format!("layers.{idx}.{mapped_tail}"))
}

/// The interleave permutation that maps HF split-half RoPE order to Burn's
/// interleaved order, for a head of `head_dim` channels:
/// `perm[2j] = j`, `perm[2j+1] = j + head_dim/2`.
///
/// `perm[burn_pos] = hf_src_channel`, so to reorder an HF-ordered head into Burn
/// order we gather: `out[burn_pos] = hf[perm[burn_pos]]`.
fn rope_perm(head_dim: usize) -> Vec<usize> {
    let half = head_dim / 2;
    let mut perm = vec![0usize; head_dim];
    for j in 0..half {
        perm[2 * j] = j;
        perm[2 * j + 1] = j + half;
    }
    perm
}

/// Reorder, in place, the per-head `head_dim` channel groups along the OUTPUT
/// axis of a `[out, in]` weight whose `out = num_heads * head_dim`. Used to bake
/// the RoPE interleave permutation into q_proj/k_proj.
///
/// `data` is row-major `[out, in]`; row `r` belongs to head `r / head_dim` and
/// to within-head channel `r % head_dim`. We permute the within-head channel.
fn permute_proj_rows(data: &[f32], out: usize, in_dim: usize, head_dim: usize, perm: &[usize]) -> Vec<f32> {
    let mut result = vec![0f32; data.len()];
    let num_heads = out / head_dim;
    for h in 0..num_heads {
        for (burn_ch, &hf_ch) in perm.iter().enumerate() {
            let dst_row = h * head_dim + burn_ch;
            let src_row = h * head_dim + hf_ch;
            let dst = dst_row * in_dim;
            let src = src_row * in_dim;
            result[dst..dst + in_dim].copy_from_slice(&data[src..src + in_dim]);
        }
    }
    result
}

/// Reorder a length-`head_dim` per-channel vector (q_norm / k_norm γ) by `perm`.
fn permute_vec(data: &[f32], perm: &[usize]) -> Vec<f32> {
    perm.iter().map(|&src| data[src]).collect()
}

/// What value transform a given Burn path needs. Decided up front from the path
/// so the lazy closure only captures small `Copy`/`Vec` data.
enum Transform {
    /// Cast to f32 only (embeddings, *_proj that aren't q/k, etc.).
    F32Only,
    /// Transpose `[out, in] -> [in, out]` + cast to f32 (Linear weights).
    Transpose,
    /// Add 1.0 + cast to f32 (RMSNorm γ — block norms + final norm).
    RmsAddOne,
    /// q/k projection: cast to f32, transpose, then RoPE-permute output rows.
    QkProj,
    /// q/k norm γ: add 1.0, cast to f32, then RoPE-permute the channel vector.
    QkNorm,
}

/// Decide the value transform for a snapshot from its Burn path + module type.
fn transform_for(path: &[String], module_type: Option<&str>) -> Transform {
    let last = path.last().map(String::as_str).unwrap_or("");
    let is_qk_proj =
        matches!(last, "weight") && path.iter().any(|p| p == "q_proj" || p == "k_proj");
    let is_qk_norm =
        matches!(last, "gamma") && path.iter().any(|p| p == "q_norm" || p == "k_norm");

    if is_qk_norm {
        return Transform::QkNorm;
    }
    if is_qk_proj {
        return Transform::QkProj;
    }
    match module_type {
        Some("Struct:RmsNorm") => Transform::RmsAddOne,
        Some("Struct:Linear") if last == "weight" => Transform::Transpose,
        _ => Transform::F32Only,
    }
}

/// Value-only adapter, keyed on the BURN path + Burn module type (the Applier
/// supplies the real container_stack before calling `adapt`, so `module_type()`
/// is correct). It performs every numeric transform the checkpoint needs:
/// f32 cast, Linear transpose, RMSNorm +1, and the RoPE interleave permutation
/// on q/k proj + q/k norm. Names are already remapped before `apply`, so this
/// adapter never renames.
#[derive(Clone, Copy)]
struct GemmaValueAdapter {
    head_dim: usize,
}

impl ModuleAdapter for GemmaValueAdapter {
    fn adapt(&self, s: &TensorSnapshot) -> TensorSnapshot {
        let path = s.path_stack.clone().unwrap_or_default();
        let container = s.container_stack.clone().unwrap_or_default();
        let id = s.tensor_id.unwrap_or_default();
        let src = s.clone_data_fn();
        let head_dim = self.head_dim;

        let transform = transform_for(&path, s.module_type().as_deref());

        // Shape of the HF source snapshot (pre-transform).
        let src_dims: Vec<usize> = s.shape.iter().copied().collect();

        match transform {
            Transform::F32Only => {
                let shape = s.shape.clone();
                let data_fn: Rc<dyn Fn() -> Result<TensorData, TensorSnapshotError>> =
                    Rc::new(move || {
                        let d = src()?.convert::<f32>();
                        Ok(d)
                    });
                TensorSnapshot::from_closure(data_fn, DType::F32, shape, path, container, id)
            }

            Transform::RmsAddOne => {
                let shape = s.shape.clone();
                let dims = src_dims.clone();
                let data_fn: Rc<dyn Fn() -> Result<TensorData, TensorSnapshotError>> =
                    Rc::new(move || {
                        let mut v = to_f32_vec(&src()?)?;
                        for x in &mut v {
                            *x += 1.0;
                        }
                        Ok(TensorData::new(v, dims.clone()))
                    });
                TensorSnapshot::from_closure(data_fn, DType::F32, shape, path, container, id)
            }

            Transform::Transpose => {
                let (out, in_dim) = (src_dims[0], src_dims[1]);
                let new_shape: Shape = [in_dim, out].into();
                let data_fn: Rc<dyn Fn() -> Result<TensorData, TensorSnapshotError>> =
                    Rc::new(move || {
                        let v = to_f32_vec(&src()?)?; // [out, in]
                        Ok(TensorData::new(transpose(&v, out, in_dim), [in_dim, out]))
                    });
                TensorSnapshot::from_closure(data_fn, DType::F32, new_shape, path, container, id)
            }

            Transform::QkProj => {
                // HF [out, in] -> RoPE-permute output rows -> transpose -> [in, out].
                let (out, in_dim) = (src_dims[0], src_dims[1]);
                let new_shape: Shape = [in_dim, out].into();
                let data_fn: Rc<dyn Fn() -> Result<TensorData, TensorSnapshotError>> =
                    Rc::new(move || {
                        let v = to_f32_vec(&src()?)?; // [out, in]
                        let perm = rope_perm(head_dim);
                        let permuted = permute_proj_rows(&v, out, in_dim, head_dim, &perm);
                        Ok(TensorData::new(transpose(&permuted, out, in_dim), [in_dim, out]))
                    });
                TensorSnapshot::from_closure(data_fn, DType::F32, new_shape, path, container, id)
            }

            Transform::QkNorm => {
                // RMSNorm γ over head_dim: +1, f32, then RoPE-permute the vector.
                let shape = s.shape.clone();
                let data_fn: Rc<dyn Fn() -> Result<TensorData, TensorSnapshotError>> =
                    Rc::new(move || {
                        let mut v = to_f32_vec(&src()?)?;
                        for x in &mut v {
                            *x += 1.0;
                        }
                        let perm = rope_perm(head_dim);
                        let permuted = permute_vec(&v, &perm);
                        let len = permuted.len();
                        Ok(TensorData::new(permuted, [len]))
                    });
                TensorSnapshot::from_closure(data_fn, DType::F32, shape, path, container, id)
            }
        }
    }

    fn clone_box(&self) -> Box<dyn ModuleAdapter> {
        Box::new(*self)
    }
}

/// Materialise a snapshot's data as an f32 `Vec` regardless of source dtype.
fn to_f32_vec(d: &TensorData) -> Result<Vec<f32>, TensorSnapshotError> {
    d.clone()
        .convert::<f32>()
        .to_vec::<f32>()
        .map_err(|e| TensorSnapshotError::DataError(format!("{e:?}")))
}

/// Row-major transpose of an `[r, c]` matrix into `[c, r]`.
fn transpose(v: &[f32], r: usize, c: usize) -> Vec<f32> {
    let mut t = vec![0f32; v.len()];
    for i in 0..r {
        for j in 0..c {
            t[j * r + i] = v[i * c + j];
        }
    }
    t
}

/// Load a Gemma 3 270M `model.safetensors` checkpoint (bf16, in memory) into a
/// freshly-`init`ed [`GemmaModel`], returning the populated model.
///
/// Handles HF→Burn renaming, Linear transpose, bf16→f32 casting, the Gemma
/// `(1 + w)` RMSNorm convention, the tied-embedding LM head (no separate tensor),
/// and the RoPE interleave permutation on q/k. Errors (as a `String`) on any
/// apply error, missing target tensor, or if fewer than every expected parameter
/// was filled.
pub fn load_gemma<B: Backend>(
    model: GemmaModel<B>,
    safetensors_bytes: &[u8],
    _device: &B::Device,
) -> Result<GemmaModel<B>, String> {
    // 1) Parse the safetensors bytes in memory (no filesystem; wasm-safe — the
    //    safetensors deserialiser needs only `alloc`).
    let mut store = SafetensorsStore::from_bytes(Some(safetensors_bytes.to_vec()));
    let snaps = store
        .get_all_snapshots()
        .map_err(|e| format!("safetensors parse failed: {e}"))?;

    // 2) Rebuild each snapshot under its Burn module path (dropping any tied
    //    lm_head). The value transforms run later, inside the adapter, with the
    //    real Burn container context.
    let head_dim = GemmaConfig::gemma_3_270m().head_dim;
    let mut remapped: Vec<TensorSnapshot> = Vec::with_capacity(snaps.len());
    for s in snaps.values() {
        let hf = s.full_path();
        let Some(burn_path) = hf_to_burn_path(&hf) else {
            continue; // dropped (e.g. tied lm_head)
        };
        let parts: Vec<String> = burn_path.split('.').map(String::from).collect();
        remapped.push(TensorSnapshot::from_closure(
            s.clone_data_fn(),
            s.dtype,
            s.shape.clone(),
            parts,
            s.container_stack.clone().unwrap_or_default(),
            s.tensor_id.unwrap_or_else(ParamId::new),
        ));
    }

    // 3) Apply with the value adapter (f32 cast / transpose / +1 / RoPE perm).
    //    We drive `model.apply` directly (rather than `store.load_from`) so we
    //    can feed pre-remapped snapshots; that bypasses the store's
    //    validate/allow_partial gates, so we inspect the result ourselves.
    let mut model = model;
    let result = model.apply(
        remapped,
        None,
        Some(Box::new(GemmaValueAdapter { head_dim })),
        false,
    );

    if !result.errors.is_empty() {
        return Err(format!("load_gemma: apply errors: {:?}", result.errors));
    }
    if !result.missing.is_empty() {
        let names: Vec<&String> = result.missing.iter().map(|(p, _)| p).collect();
        return Err(format!("load_gemma: missing tensors: {names:?}"));
    }
    if result.applied.len() != EXPECTED_PARAMS {
        return Err(format!(
            "load_gemma: applied {} params, expected {EXPECTED_PARAMS} (a checkpoint tensor was \
             likely mis-named or unmapped, leaving a parameter at random init)",
            result.applied.len()
        ));
    }

    Ok(model)
}
