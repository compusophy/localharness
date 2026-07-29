//! Execution receipts — a signed-hash record that a specific computation
//! happened: *this* content-addressed module, *this* export, *these* args,
//! *this* result. The primitive nobody else in the agent field has (Buzz signs
//! messages, ERC-8004 signs opinions, x402 signs payments): a receipt is
//! **checkable without trust**, because rustlite cartridges are deterministic
//! wasm — anyone can recompile/re-execute and compare hashes. A sybil cannot
//! manufacture one without actually running the code.
//!
//! v1 ships the CANONICAL ENCODING + hash + the natively-verifiable half:
//! **build receipts** (source → wasm binding; the CLI `receipt` command), with
//! the `call` record ready for the browser's `compose::call` wiring — native
//! targets deliberately have no wasm executor (untrusted execution is
//! browser-only, by design), so execution receipts are emitted where execution
//! happens. Deferred, explicitly: browser-side emission + a re-execution
//! verifier tool.
//!
//! Feature `wallet` (keccak). The preimage layout is VERSIONED — any change to
//! it bumps `RECEIPT_V` and keeps the old layout decodable, or every previously
//! pinned hash silently stops matching.

use crate::registry::keccak32;

/// Preimage layout version. Bump on ANY change to [`Receipt::preimage`].
pub const RECEIPT_V: u8 = 1;

/// Outcome of the recorded call, mirroring `compose::call_status` semantics.
/// For a build receipt (no call) this field is absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallStatus {
    /// The export ran to completion and returned `result`.
    Ok = 0,
    /// The export trapped (contained by the host; see `src/compose.rs`).
    Trapped = 1,
    /// The export was refused (missing, arity too wide, budget).
    Refused = 2,
}

/// The executed-call half of a receipt: which export ran, with what integer
/// args (the whole rustlite ABI is integer-only), and what came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRecord {
    /// Exported function name.
    pub export: String,
    /// Arguments as passed (rustlite ABI: at most `compose::MAX_CALL_ARGS`).
    pub args: Vec<i32>,
    /// The export's own return value; `None` for a unit export.
    pub result: Option<i32>,
    /// How the call ended.
    pub status: CallStatus,
    /// Reserved: fuel consumed. Rustlite emits no fuel checks today, so this
    /// is always 0 in v1 — the field exists so adding metering later does not
    /// bump the layout.
    pub fuel: u64,
}

/// A receipt binding source → module → (optionally) one executed call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// keccak256 of the rustlite SOURCE bytes, exactly as compiled.
    pub source_keccak: [u8; 32],
    /// keccak256 of the emitted wasm bytes — the same content address the
    /// compose layer keys libraries by.
    pub module_keccak: [u8; 32],
    /// The deterministic producer, e.g. `rustlite 0.74.0` — recompiling with
    /// the same version MUST reproduce `module_keccak` byte-identically.
    pub compiler: String,
    /// The executed call, when this receipt records an execution (browser
    /// emission); `None` = a build receipt (the CLI's natively-checkable kind).
    pub call: Option<CallRecord>,
}

impl Receipt {
    /// A build receipt for `source` and the wasm it compiled to.
    pub fn build(source: &str, wasm: &[u8]) -> Self {
        Receipt {
            source_keccak: keccak32(source.as_bytes()),
            module_keccak: keccak32(wasm),
            compiler: compiler_tag(),
            call: None,
        }
    }

    /// The canonical preimage. Deterministic and self-delimiting: version byte,
    /// then fixed-width hashes, then length-prefixed compiler tag, then either
    /// a 0x00 (no call) or 0x01 followed by the call record with fixed-width
    /// integer encodings. No JSON, no maps — nothing whose byte order can
    /// drift between implementations.
    pub fn preimage(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(96 + self.compiler.len() + 32);
        out.push(RECEIPT_V);
        out.extend_from_slice(&self.source_keccak);
        out.extend_from_slice(&self.module_keccak);
        out.extend_from_slice(&(self.compiler.len() as u32).to_be_bytes());
        out.extend_from_slice(self.compiler.as_bytes());
        match &self.call {
            None => out.push(0x00),
            Some(c) => {
                out.push(0x01);
                out.extend_from_slice(&(c.export.len() as u32).to_be_bytes());
                out.extend_from_slice(c.export.as_bytes());
                out.push(c.args.len() as u8);
                for a in &c.args {
                    out.extend_from_slice(&a.to_be_bytes());
                }
                match c.result {
                    None => out.push(0x00),
                    Some(r) => {
                        out.push(0x01);
                        out.extend_from_slice(&r.to_be_bytes());
                    }
                }
                out.push(c.status as u8);
                out.extend_from_slice(&c.fuel.to_be_bytes());
            }
        }
        out
    }

    /// keccak256 of the preimage — THE receipt hash: what gets pinned in a
    /// regression corpus, anchored in a Merkle root, or disputed.
    pub fn hash(&self) -> [u8; 32] {
        keccak32(&self.preimage())
    }

    /// Human/JSON form (hex hashes). The JSON is a VIEW — the hash commits to
    /// [`Receipt::preimage`], never to this serialization.
    pub fn to_json(&self) -> serde_json::Value {
        let hex = |b: &[u8; 32]| format!("0x{}", crate::encoding::bytes_to_hex(b));
        let mut v = serde_json::json!({
            "v": RECEIPT_V,
            "source_keccak": hex(&self.source_keccak),
            "module_keccak": hex(&self.module_keccak),
            "compiler": self.compiler,
            "receipt": hex(&self.hash()),
        });
        if let Some(c) = &self.call {
            v["call"] = serde_json::json!({
                "export": c.export,
                "args": c.args,
                "result": c.result,
                "status": match c.status {
                    CallStatus::Ok => "ok",
                    CallStatus::Trapped => "trapped",
                    CallStatus::Refused => "refused",
                },
                "fuel": c.fuel,
            });
        }
        v
    }
}

/// The deterministic-producer tag stamped into every receipt.
pub fn compiler_tag() -> String {
    format!("rustlite {}", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::{CallRecord, CallStatus, Receipt, RECEIPT_V};

    fn sample_call() -> CallRecord {
        CallRecord {
            export: "step".into(),
            args: vec![7, -3],
            result: Some(42),
            status: CallStatus::Ok,
            fuel: 0,
        }
    }

    /// Same inputs → same hash; any field change → different hash. This is
    /// the property the whole primitive rests on.
    #[test]
    fn hash_is_deterministic_and_binding() {
        let a = Receipt::build("fn render() {}", b"\0asm-bytes");
        let b = Receipt::build("fn render() {}", b"\0asm-bytes");
        assert_eq!(a.hash(), b.hash());
        // Different source binds differently even with identical wasm.
        let c = Receipt::build("fn render() { }", b"\0asm-bytes");
        assert_ne!(a.hash(), c.hash());
        // Attaching a call changes the hash.
        let mut d = a.clone();
        d.call = Some(sample_call());
        assert_ne!(a.hash(), d.hash());
        // …and every call field is binding.
        let mut e = d.clone();
        e.call.as_mut().unwrap().result = Some(43);
        assert_ne!(d.hash(), e.hash());
        let mut f = d.clone();
        f.call.as_mut().unwrap().status = CallStatus::Trapped;
        assert_ne!(d.hash(), f.hash());
    }

    /// The preimage is self-delimiting: a crafted compiler tag cannot collide
    /// with the call-record region (length prefixes, not separators).
    #[test]
    fn preimage_is_length_prefixed_not_delimited() {
        let a = Receipt {
            source_keccak: [1; 32],
            module_keccak: [2; 32],
            compiler: "x\u{0}\u{1}".into(),
            call: None,
        };
        let mut b = a.clone();
        b.compiler = "x".into();
        // If the encoding used separators instead of lengths these could
        // collide; with length prefixes they must not.
        assert_ne!(a.preimage(), b.preimage());
        assert_eq!(a.preimage()[0], RECEIPT_V);
    }

    /// Golden hash: pins the v1 preimage layout. If this test breaks, you
    /// changed the canonical encoding — bump RECEIPT_V instead of updating
    /// the constant, or every previously pinned receipt silently unpins.
    #[test]
    fn golden_v1_layout() {
        let r = Receipt {
            source_keccak: [0xAA; 32],
            module_keccak: [0xBB; 32],
            compiler: "rustlite test".into(),
            call: Some(sample_call()),
        };
        let pre = r.preimage();
        assert_eq!(pre[0], 1); // version byte
        assert_eq!(&pre[1..33], &[0xAA; 32]);
        assert_eq!(&pre[33..65], &[0xBB; 32]);
        // 4-byte BE length then the tag.
        assert_eq!(&pre[65..69], &13u32.to_be_bytes());
        assert_eq!(&pre[69..82], b"rustlite test");
        assert_eq!(pre[82], 0x01); // call present
        // export len + "step" + argc + 2 args + result flag + result + status + fuel
        assert_eq!(&pre[83..87], &4u32.to_be_bytes());
        assert_eq!(&pre[87..91], b"step");
        assert_eq!(pre[91], 2); // argc
        assert_eq!(&pre[92..96], &7i32.to_be_bytes());
        assert_eq!(&pre[96..100], &(-3i32).to_be_bytes());
        assert_eq!(pre[100], 0x01); // result present
        assert_eq!(&pre[101..105], &42i32.to_be_bytes());
        assert_eq!(pre[105], 0); // status ok
        assert_eq!(&pre[106..114], &0u64.to_be_bytes());
        assert_eq!(pre.len(), 114);
    }

    /// JSON is a view: it carries the receipt hash but the hash never depends
    /// on the JSON (field order / formatting can't unpin a receipt).
    #[test]
    fn json_view_carries_the_hash() {
        let r = Receipt::build("fn render() {}", b"wasm");
        let j = r.to_json();
        assert_eq!(j["v"], 1);
        assert!(j["receipt"].as_str().unwrap().starts_with("0x"));
        assert_eq!(j["call"], serde_json::Value::Null);
        let hex = j["receipt"].as_str().unwrap().to_string();
        assert_eq!(hex, r.to_json()["receipt"].as_str().unwrap());
    }
}
