//! Source guards: `web/opfs-worker.js` (the WebKit OPFS write broker) must
//! stay in parity with `src/filesystem/opfs.rs` — same path semantics, the
//! safety invariants that make it the iOS fix, and the wiring both sides
//! assume. Mirrors the `tests/seed_pull_boot_parity.rs` pattern: the worker is
//! hand-written JS (irreducible worker glue), so a test pins the load-bearing
//! lines instead of trusting review to catch drift.

use std::fs;

fn worker_src() -> String {
    fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/web/opfs-worker.js"))
        .expect("web/opfs-worker.js must exist — filesystem/opfs.rs spawns it by URL")
}

fn opfs_rs() -> String {
    fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/filesystem/opfs.rs"))
        .expect("src/filesystem/opfs.rs")
}

/// The broker uses the worker-only sync-access API (iOS 15.2+), truncates
/// before writing at 0 (write_atomic replaces, never appends), flushes, and
/// closes in `finally` (an open handle holds an exclusive lock; iOS kills
/// background workers — a leaked handle strands the file locked).
#[test]
fn worker_write_sequence_is_safe() {
    let src = worker_src();
    assert!(src.contains("createSyncAccessHandle"));
    assert!(src.contains("truncate(0)"));
    assert!(src.contains("{ at: 0 }"));
    assert!(src.contains("flush()"));
    assert!(src.contains("finally"));
    assert!(src.contains("close()"));
}

/// Path semantics mirror `split_path`: split on '/', drop empty segments and
/// ".", create parent dirs on demand — so a path writes to the SAME file no
/// matter which side performs the write.
#[test]
fn worker_path_semantics_mirror_split_path() {
    let src = worker_src();
    assert!(src.contains("split('/')"));
    assert!(src.contains("s !== '' && s !== '.'"));
    assert!(src.contains("{ create: true }"));
    // and the Rust side still defines the canonical rule this mirrors
    let rs = opfs_rs();
    assert!(rs.contains("fn split_path"));
    assert!(rs.contains("!s.is_empty() && *s != \".\""));
}

/// Ops run strictly sequentially (promise-chain tail): sync handles take an
/// exclusive per-file lock, so interleaved handlers would contend.
#[test]
fn worker_serializes_ops() {
    let src = worker_src();
    assert!(src.contains("let tail = Promise.resolve()"));
    assert!(src.contains("tail = tail.then("));
}

/// The Rust client spawns the worker by this exact URL, and an engine without
/// sync-access must answer `unsupported` (the Rust side latches + falls back).
#[test]
fn wiring_matches_both_sides() {
    let src = worker_src();
    assert!(src.contains("unsupported"));
    let rs = opfs_rs();
    assert!(rs.contains("Worker::new(\"/opfs-worker.js\")"));
    assert!(rs.contains("LH_FORCE_WORKER_FS"));
    assert!(rs.contains("BrokerWrite::Unsupported"));
    // vercel serves it revalidated (stale broker + fresh wasm = protocol skew)
    let vercel = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/vercel.json"))
        .expect("vercel.json");
    assert!(vercel.contains("/opfs-worker.js"));
}
