//! Parity guard for the DEFAULT cartridge framebuffer (telemetry #73).
//!
//! The default lives in Rust (`rustlite::loader::DEFAULT_FB_*`, which
//! `app::display` re-uses) and is HAND-PORTED into the Web Worker as
//! `FB_W_DEFAULT`/`FB_H_DEFAULT`. If those drift, a cartridge is sized for one
//! surface and drawn on another.
//!
//! This is the bug's second face. The first was PROSE: tool descriptions, docs,
//! and worker comments all claimed 320x240 long after the code moved to
//! 512x512. Agents believe their tool descriptions — one laid a UI out for the
//! wrong surface, drew its buttons off-screen, and got no diagnostic because
//! every primitive silently clips. So this also pins that no stale 320x240
//! claim creeps back into the model-facing text.

use std::path::Path;

fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// Parse `pub const <name>: i32 = <n>;` out of the loader source. `loader` is
/// `pub(crate)`, so this reads the SOURCE rather than widening the SDK's public
/// API for a test — the same idiom as `seed_pull_boot_parity`.
fn rust_default(name: &str) -> i32 {
    let src = read("src/rustlite/loader.rs");
    let needle = format!("pub const {name}: i32 = ");
    let start = src.find(&needle).unwrap_or_else(|| panic!("{needle} not found")) + needle.len();
    let rest = &src[start..];
    let end = rest.find(';').expect("terminating semicolon");
    rest[..end].trim().parse().expect("an integer default")
}

#[test]
fn worker_default_dims_match_rust() {
    let worker = read("web/cartridge-worker.js");
    for (name, value) in [
        ("FB_W_DEFAULT", rust_default("DEFAULT_FB_W")),
        ("FB_H_DEFAULT", rust_default("DEFAULT_FB_H")),
    ] {
        let decl = format!("const {name} = {value};");
        assert!(
            worker.contains(&decl),
            "web/cartridge-worker.js must declare `{decl}` to match \
             rustlite::loader::{name} — the worker sizes the real framebuffer"
        );
    }
}

/// The model reads these strings and lays its cartridge out from them.
#[test]
fn no_stale_320x240_claim_in_model_facing_text() {
    for file in [
        "src/builtins/run_cartridge.rs",
        "src/builtins/render_html.rs",
        "web/cartridge-worker.js",
        "web/llms.txt",
    ] {
        let text = read(file);
        assert!(
            !text.contains("320x240") && !text.contains("320×240"),
            "{file} still claims a 320x240 framebuffer; the default is {}x{}",
            rust_default("DEFAULT_FB_W"),
            rust_default("DEFAULT_FB_H"),
        );
    }
}
