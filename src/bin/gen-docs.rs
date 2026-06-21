//! `gen-docs` — the doc-integrity generator.
//!
//! Fills every `<!-- GEN:<key> -->...<!-- /GEN:<key> -->` block in the managed
//! docs (`web/skill.md`, `web/llms.txt`) with the freshly-rendered
//! block from [`localharness::docs_manifest`] — the single source of truth for
//! the drift-prone facts (chain addresses, version, pricing, tool list, CLI
//! list).
//!
//! Modes:
//!   `cargo run --bin gen-docs`            — REWRITE the docs in place (default).
//!   `cargo run --bin gen-docs -- --check` — render in-memory, diff vs the
//!                                            files, print any drift, exit 1 if
//!                                            ANY block is stale (else exit 0).
//!
//! IDEMPOTENT: running it twice is a no-op. The `--check` mode is the gate the
//! release scripts run in pre-flight, so a version bump cannot ship stale docs.
//!
//! Requires `--features wallet` (the manifest reads `registry::chain`).

// gen-docs is a NATIVE-only dev tool: it writes the managed docs into the repo via
// std::fs and is never part of the wasm bundle. `cargo check --target wasm32` still
// builds every bin, and `docs_manifest` is wasm-excluded (it carries the testnet
// MODERATO strings stripped from the prod bundle), so provide a wasm no-op main and
// gate the real tool to native.
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::process::ExitCode;
#[cfg(not(target_arch = "wasm32"))]
use localharness::docs_manifest;

/// The managed docs, relative to the crate root. The top-level `README.md` is
/// deliberately NOT here — it is hand-written and minimal (a README is not the
/// generated docs); `web/llms.txt` is the full generated agent spec.
#[cfg(not(target_arch = "wasm32"))]
const MANAGED_DOCS: &[&str] = &["web/skill.md", "web/llms.txt"];

#[cfg(not(target_arch = "wasm32"))]
fn crate_root() -> PathBuf {
    // CARGO_MANIFEST_DIR points at the crate root both for `cargo run` and for
    // a `cargo install`ed binary's build — the same anchor the lib tests use.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> ExitCode {
    let check = std::env::args().any(|a| a == "--check");
    let root = crate_root();

    let mut any_drift = false;
    let mut any_error = false;

    for rel in MANAGED_DOCS {
        let path = root.join(rel);
        let doc = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("gen-docs: cannot read {}: {e}", path.display());
                any_error = true;
                continue;
            }
        };

        let (filled, report) = docs_manifest::fill(&doc);

        if check {
            if report.drifted() {
                any_drift = true;
                for key in &report.changed {
                    println!("DRIFT  {rel}: GEN:{key} is stale");
                }
            } else {
                println!("ok     {rel} ({} block(s) fresh)", report.fresh.len());
            }
        } else if report.drifted() {
            if let Err(e) = write_doc(&path, &filled) {
                eprintln!("gen-docs: cannot write {}: {e}", path.display());
                any_error = true;
                continue;
            }
            println!(
                "updated {rel}: {}",
                report
                    .changed
                    .iter()
                    .map(|k| format!("GEN:{k}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        } else {
            println!("ok      {rel} ({} block(s) already fresh)", report.fresh.len());
        }
    }

    if any_error {
        return ExitCode::FAILURE;
    }
    if check && any_drift {
        eprintln!("\ndoc drift detected — run `cargo run --bin gen-docs` to regenerate.");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Write the doc back, preserving its line endings sanely (we write with `\n`;
/// the docs are LF in the repo).
#[cfg(not(target_arch = "wasm32"))]
fn write_doc(path: &Path, content: &str) -> std::io::Result<()> {
    std::fs::write(path, content)
}
