//! Gemma 3 tokenizer for the local in-browser model backend.
//!
//! Thin wrapper over HuggingFace's `tokenizers` crate, loaded from raw
//! `tokenizer.json` bytes (`include_bytes!`, an OPFS read, or a CDN fetch —
//! no filesystem or network dependency in here). Compiles on BOTH native and
//! `wasm32-unknown-unknown`: the crate is pulled with
//! `default-features = false, features = ["unstable_wasm"]`, which swaps the C
//! `onig` regex for pure-Rust `fancy-regex` and points `getrandom` at the
//! browser WebCrypto backend — the candle-wasm-examples recipe.
//!
//! ## BOS handling
//!
//! Gemma special tokens: `pad = 0`, `eos = 1`, `bos = 2`, `unk = 3`; vocab
//! 262144. The model expects a single leading `<bos>` (id 2). Gemma's
//! `tokenizer.json` ships a `TemplateProcessing` post-processor that *also*
//! prepends `<bos>` when `encode(text, add_special_tokens = true)` is used — so
//! using that path AND manually prepending would yield a doubled BOS and
//! corrupt the first-token statistics.
//!
//! To make the contract (`encode` prepends BOS=2) unambiguous regardless of
//! whether the loaded json carries that post-processor, this wrapper encodes
//! with `add_special_tokens = false` (no auto-specials) and prepends exactly
//! one BOS by hand. Result: precisely one `<bos>` at the front, always.
//!
//! ## Type bridge
//!
//! The model's `forward` takes `Tensor<B, 2, Int>` (i64-shaped token ids); the
//! `tokenizers` crate speaks `u32`. `encode` returns `Vec<i64>` and `decode`
//! takes `&[i64]`, converting at the boundary. Negative ids (none should ever
//! occur from the model's argmax over a 262144 vocab) are dropped on decode.

use tokenizers::Tokenizer;

/// Gemma `<pad>` token id.
pub const GEMMA_PAD: i64 = 0;
/// Gemma `<eos>` token id (greedy generation stops here).
pub const GEMMA_EOS: i64 = 1;
/// Gemma `<bos>` token id (prepended by [`GemmaTokenizer::encode`]).
pub const GEMMA_BOS: i64 = 2;
/// Gemma `<unk>` token id.
pub const GEMMA_UNK: i64 = 3;

/// A loaded Gemma 3 tokenizer. Construct via [`load`].
pub struct GemmaTokenizer {
    inner: Tokenizer,
}

/// Load a [`GemmaTokenizer`] from raw `tokenizer.json` bytes.
///
/// `bytes` is the full HuggingFace fast-tokenizer JSON (Gemma's is ~33 MB).
/// No filesystem or network is touched — feed it `include_bytes!` output, an
/// `OpfsFilesystem::read` result, or a CDN `fetch`. wasm-clean.
pub fn load(bytes: &[u8]) -> Result<GemmaTokenizer, String> {
    let inner = Tokenizer::from_bytes(bytes)
        .map_err(|e| format!("GemmaTokenizer: parse tokenizer.json: {e}"))?;
    Ok(GemmaTokenizer { inner })
}

impl GemmaTokenizer {
    /// Load from raw `tokenizer.json` bytes — alias for the free [`load`] fn,
    /// for call sites that prefer the associated-function form.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        load(bytes)
    }

    /// Encode `text` into token ids with a single leading `<bos>` (id 2).
    ///
    /// Encodes with `add_special_tokens = false` (so the json's own
    /// `TemplateProcessing` BOS does NOT also fire) and prepends exactly one
    /// `GEMMA_BOS`. Gemma adds BOS but not EOS, matching this. On any internal
    /// tokenizer error the result is just `[<bos>]` — a benign empty prompt —
    /// so encode never panics and never produces a malformed sequence.
    pub fn encode(&self, text: &str) -> Vec<i64> {
        match self.inner.encode(text, /* add_special_tokens = */ false) {
            Ok(enc) => {
                let ids = enc.get_ids();
                let mut out = Vec::with_capacity(ids.len() + 1);
                out.push(GEMMA_BOS);
                out.extend(ids.iter().map(|&id| id as i64));
                out
            }
            Err(_) => vec![GEMMA_BOS],
        }
    }

    /// Decode token ids back into text, skipping special tokens (BOS/EOS/pad).
    ///
    /// Negative ids (never expected from the model's argmax) are filtered out
    /// before handing the `u32` slice to the tokenizer. Returns an empty
    /// string on any internal decode error rather than panicking.
    pub fn decode(&self, ids: &[i64]) -> String {
        let u32_ids: Vec<u32> = ids
            .iter()
            .filter(|&&id| id >= 0)
            .map(|&id| id as u32)
            .collect();
        self.inner
            .decode(&u32_ids, /* skip_special_tokens = */ true)
            .unwrap_or_default()
    }

    /// Decode token ids WITHOUT stripping special tokens (debugging /
    /// round-trip inspection). Same id-bridging + error handling as [`decode`].
    ///
    /// [`decode`]: GemmaTokenizer::decode
    pub fn decode_raw(&self, ids: &[i64]) -> String {
        let u32_ids: Vec<u32> = ids
            .iter()
            .filter(|&&id| id >= 0)
            .map(|&id| id as u32)
            .collect();
        self.inner
            .decode(&u32_ids, /* skip_special_tokens = */ false)
            .unwrap_or_default()
    }
}
