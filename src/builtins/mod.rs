//! The crate-wide built-in tool registry — backend-NEUTRAL.
//!
//! Each tool implements [`Tool`] and is registered into a [`ToolRunner`] by
//! [`register_builtins`](crate::builtins::register_builtins) according to the
//! [`CapabilitiesConfig`]. EVERY backend (Gemini, Anthropic, local — and the
//! mock when an Agent injects a runner) registers from here; only the two
//! Gemini-client-coupled tools (`start_subagent`, `generate_image`) skip when
//! no client is supplied in [`BuiltinDeps`](crate::builtins::BuiltinDeps).
//!
//! Lived at `backends/gemini/tools/` until 0.29.x (the Gemini backend was
//! written first); a re-export shim remains there so old paths compile.
//!
//! SCHEMA CONSTRAINT (load-bearing): every tool's `input_schema()` must use a
//! single `type` (no `["string","null"]` unions) and none of
//! `additionalProperties`/`$schema`/`$ref`/`oneOf`/`anyOf`/`allOf` — Gemini
//! rejects union-type schemas with a 400 that bricks ALL chat, and Anthropic
//! rejects them too. Guarded by `builtin_tool_schemas_have_no_union_types`
//! below (plus the Anthropic-side declaration lint in
//! `backends/anthropic/mod.rs`).

use std::sync::Arc;

use crate::backends::gemini::api::SharedClient;
use crate::filesystem::SharedFilesystem;
use crate::tools::{Tool, ToolRunner};
use crate::types::{BuiltinTool, CapabilitiesConfig};

mod ask_question;
mod call_agent;
mod compile_rustlite;
mod create_file;
mod current_time;
mod delete_file;
mod edit_file;
mod find_file;
mod finish;
mod generate_image;
mod list_directory;
mod rename_file;
mod run_cartridge;
mod render_html;
mod configure_agent;
#[cfg(feature = "native")]
mod run_command;
mod search_directory;
mod start_subagent;
mod view_file;

pub use ask_question::AskQuestion;
pub use call_agent::NO_SESSION_ERR;
pub use create_file::CreateFile;
pub use delete_file::DeleteFile;
pub use edit_file::EditFile;
pub use find_file::FindFile;
pub use finish::{Finish, FINISH_TOOL_NAME};
pub use generate_image::GenerateImage;
pub use list_directory::ListDirectory;
pub use rename_file::RenameFile;
#[cfg(feature = "native")]
pub use run_command::RunCommand;
pub use search_directory::SearchDirectory;
pub use start_subagent::StartSubagent;
pub use view_file::ViewFile;

/// Identity / secret files the AGENT'S filesystem tools must never read,
/// modify, destroy, or surface. `.lh_wallet` is the BIP-39 seed — the private
/// key the whole identity derives from; a prompt injection that got the model
/// to `view_file`/`search_directory` it could exfiltrate the seed into the
/// transcript, and `delete_file`/`rename_file` on it bricks the identity (the
/// reset-brick failure mode). The device key is likewise secret; the owner
/// hints have no legitimate agent use. Matched on the final path component
/// (path-independent). Mirrors `filesystem::EXEMPT_FILES` by intent — both
/// protect identity material — but is a SEPARATE list: that one governs
/// at-rest encryption (keep plaintext), this one governs agent tool ACCESS.
pub(crate) const PROTECTED_FILES: &[&str] =
    &[".lh_wallet", ".lh_device_key", ".lh_owner", ".lh_linked_owner"];

/// True iff `path`'s final component is a protected identity/secret file that
/// the agent's filesystem tools must refuse to touch. Splits on both `/` and
/// `\` so a Windows-style path can't slip a protected basename past the check.
pub(crate) fn is_protected_path(path: &str) -> bool {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    PROTECTED_FILES.contains(&base)
}

/// The error a filesystem builtin returns when asked to touch a protected
/// identity file. Phrased so the model understands it's a hard policy, not a
/// transient failure, and stops retrying.
pub(crate) fn protected_path_error(path: &str) -> crate::error::Error {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    crate::error::Error::other(format!(
        "refused: '{base}' is a protected identity/secret file (the wallet seed \
         or device key) — agent filesystem tools cannot read, modify, or delete it"
    ))
}

/// Construction dependencies the built-in tools optionally need.
///
/// * `chat_client` + `chat_model` — used by `start_subagent` (the model the
///   spawned subagent runs against).
/// * `image_client` + `image_model` — used by `generate_image`.
/// * `fs` — used by the 8 filesystem builtins (list_directory, view_file,
///   find_file, search_directory, create_file, edit_file, delete_file,
///   rename_file). If `None`, those builtins are skipped. ALSO handed to
///   `start_subagent` so the spawned subagent's reduced fs builtins operate
///   over the SAME store the parent uses.
pub struct BuiltinDeps {
    pub chat_client: Option<SharedClient>,
    pub chat_model: String,
    pub image_client: Option<SharedClient>,
    pub image_model: String,
    pub fs: Option<SharedFilesystem>,
}

/// Construct an `Arc<dyn Tool>` of `$ty` if a filesystem is present in
/// `$deps.fs`. The fs-shaped builtins all share the same constructor
/// shape (`T::new(SharedFilesystem)`), so the macro keeps the match arm
/// for each one to a single line.
macro_rules! fs_tool {
    ($deps:expr, $ty:ident) => {
        $deps
            .fs
            .as_ref()
            .map(|fs| Arc::new($ty::new(fs.clone())) as Arc<dyn Tool>)
    };
}

/// Register the enabled built-in tools into `runner` based on
/// `capabilities.effective_tools()`. Returns the names registered.
pub fn register_builtins(
    runner: &ToolRunner,
    capabilities: &CapabilitiesConfig,
    deps: &BuiltinDeps,
) -> Vec<String> {
    let enabled = capabilities.effective_tools();
    let mut registered = Vec::new();
    for tool in BuiltinTool::ALL {
        if !enabled.contains(tool) {
            continue;
        }
        let boxed: Option<Arc<dyn Tool>> = match tool {
            BuiltinTool::Finish => Some(Arc::new(Finish)),
            BuiltinTool::AskQuestion => Some(Arc::new(AskQuestion)),
            BuiltinTool::GenerateImage => deps.image_client.as_ref().map(|c| {
                Arc::new(GenerateImage::new(c.clone(), deps.image_model.clone())) as Arc<dyn Tool>
            }),
            BuiltinTool::StartSubagent => deps.chat_client.as_ref().map(|c| {
                // The subagent is TOOL-BEARING: hand it the SAME filesystem the
                // parent's fs builtins write to, so it can do real work over the
                // shared OPFS (it gets a REDUCED allowlist — fs builtins + finish,
                // never nested subagents / value-moving tools; see start_subagent.rs).
                Arc::new(StartSubagent::with_filesystem(
                    c.clone(),
                    deps.chat_model.clone(),
                    deps.fs.clone(),
                )) as Arc<dyn Tool>
            }),
            BuiltinTool::ListDirectory => fs_tool!(deps, ListDirectory),
            BuiltinTool::ViewFile => fs_tool!(deps, ViewFile),
            BuiltinTool::FindFile => fs_tool!(deps, FindFile),
            BuiltinTool::SearchDirectory => fs_tool!(deps, SearchDirectory),
            BuiltinTool::CreateFile => fs_tool!(deps, CreateFile),
            BuiltinTool::EditFile => fs_tool!(deps, EditFile),
            BuiltinTool::DeleteFile => fs_tool!(deps, DeleteFile),
            BuiltinTool::RenameFile => fs_tool!(deps, RenameFile),
            BuiltinTool::CallAgent => Some(Arc::new(call_agent::CallAgent) as Arc<dyn Tool>),
            BuiltinTool::CompileRustlite => Some(Arc::new(compile_rustlite::CompileRustlite) as Arc<dyn Tool>),
            BuiltinTool::RunCartridge => Some(Arc::new(run_cartridge::RunCartridge) as Arc<dyn Tool>),
            BuiltinTool::RenderHtml => Some(Arc::new(render_html::RenderHtml) as Arc<dyn Tool>),
            BuiltinTool::ConfigureAgent => Some(Arc::new(configure_agent::ConfigureAgent) as Arc<dyn Tool>),
            BuiltinTool::CurrentTime => Some(Arc::new(current_time::CurrentTime) as Arc<dyn Tool>),
            BuiltinTool::RunCommand => instantiate_run_command(),
        };
        if let Some(t) = boxed {
            let name = t.name().to_string();
            let existing = runner.names();
            if !existing.iter().any(|n| n == &name) {
                runner.register(t);
                registered.push(name);
            }
        }
    }
    registered
}

#[cfg(feature = "native")]
fn instantiate_run_command() -> Option<Arc<dyn Tool>> {
    Some(Arc::new(RunCommand))
}

#[cfg(not(feature = "native"))]
fn instantiate_run_command() -> Option<Arc<dyn Tool>> {
    None
}

/// The ONE structured compile-failure report every rustlite-compiling tool
/// returns (`compile_rustlite`, `run_cartridge` — and the shape
/// `create_and_publish_app` folds into its error string). Fields:
///
/// * `error`    — always `"compilation failed"` (the dispatch discriminant).
/// * `code`     — the stable `LHxxxx` label (`null` for an uncoded internal error).
/// * `detail`   — the compiler message verbatim (code-prefixed, with the raw
///   `[start..end]` byte span).
/// * `location` — `"line N, col M"` resolved against `source` (`null` if spanless).
/// * `snippet`  — the offending source line with a caret marker underneath
///   (`null` if spanless), so the agent fixes the exact spot instead of
///   hunting byte offsets.
/// * `hint`     — the per-code fix hint from [`crate::error_codes`], falling
///   back to the generic fix-and-recompile loop discipline.
pub(crate) fn compile_failure_report(
    err: &crate::rustlite::CompileError,
    source: &str,
) -> serde_json::Value {
    let hint = err
        .code
        .and_then(crate::error_codes::lookup)
        .map(|e| e.hint)
        .unwrap_or(
            "Fix the issue at the reported source location and recompile. Common \
             causes: a feature rustlite lacks (traits, generics, references, \
             Vec/String building, Option/Result) or a wrong host fn name/arity. \
             Do NOT run_cartridge or publish until this compiles clean.",
        );
    serde_json::json!({
        "error": "compilation failed",
        "code": err.code.map(crate::error_codes::fmt_label),
        "detail": err.to_string(),
        "location": err.location(source),
        "snippet": err.span.and_then(|s| crate::rustlite::render_snippet(source, s)),
        "hint": hint,
    })
}

#[cfg(test)]
mod compile_failure_report_tests {
    /// The structured report carries everything an agent needs to self-fix:
    /// the stable code, the verbatim message, a line/col locator, the
    /// offending line with a caret, and the registry fix hint — for every
    /// stage of the pipeline (this exercises a typecheck failure on line 3).
    #[test]
    fn report_is_fully_populated_for_a_spanned_error() {
        let src = "fn frame(t: i32) {\n  host::display::clear(0);\n  let x = true + 1;\n  host::display::present();\n}";
        let err = crate::rustlite::compile(src).expect_err("must fail");
        let r = super::compile_failure_report(&err, src);
        assert_eq!(r["error"], "compilation failed");
        assert_eq!(r["code"], "LH0204");
        assert!(r["detail"].as_str().unwrap().starts_with("LH0204:"), "{r}");
        assert!(r["location"].as_str().unwrap().starts_with("line 3, col "), "{r}");
        let snippet = r["snippet"].as_str().unwrap();
        assert!(snippet.contains("let x = true + 1;"), "{r}");
        assert!(snippet.lines().last().unwrap().contains('^'), "{r}");
        // the hint is the LH0204 registry hint, not the generic fallback
        assert_eq!(r["hint"].as_str(), crate::error_codes::lookup(204).map(|e| e.hint), "{r}");
    }

    /// A spanless internal error still produces a valid report — null
    /// location/snippet, generic hint — never a panic or an empty object.
    #[test]
    fn report_degrades_gracefully_without_a_span() {
        let err = crate::rustlite::CompileError::new("internal: lowering invariant");
        let r = super::compile_failure_report(&err, "fn frame(t: i32) {}");
        assert_eq!(r["error"], "compilation failed");
        assert!(r["code"].is_null());
        assert_eq!(r["detail"], "internal: lowering invariant");
        assert!(r["location"].is_null());
        assert!(r["snippet"].is_null());
        assert!(r["hint"].as_str().unwrap().contains("recompile"), "{r}");
    }
}

#[cfg(test)]
mod protected_path_tests {
    use super::*;

    #[test]
    fn matches_seed_and_identity_files_by_basename() {
        for p in [
            ".lh_wallet",
            "./.lh_wallet",
            "/data/agent/.lh_wallet",
            "subdir\\.lh_device_key",
            ".lh_owner",
            ".lh_linked_owner",
        ] {
            assert!(is_protected_path(p), "{p} must be protected");
        }
        for p in ["note.txt", "app.rl", ".lh_history.json", "wallet.txt", "lh_wallet"] {
            assert!(!is_protected_path(p), "{p} must NOT be protected");
        }
    }

    #[cfg(feature = "native")]
    #[tokio::test]
    async fn view_and_delete_refuse_the_seed_file() {
        use crate::filesystem::NativeFilesystem;
        use crate::tools::Tool;
        use serde_json::json;
        use std::sync::Arc;

        let dir = std::env::temp_dir().join(format!("lh_protect_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let seed = dir.join(".lh_wallet");
        std::fs::write(&seed, b"SECRET SEED PHRASE").unwrap();
        let seed_str = seed.display().to_string();
        let fs = Arc::new(NativeFilesystem::new());

        // view_file must refuse — the seed never reaches the transcript.
        let v = ViewFile::new(fs.clone())
            .execute(json!({ "path": seed_str }), None)
            .await;
        assert!(v.is_err(), "view_file must refuse the seed");
        assert!(format!("{:?}", v.unwrap_err()).contains("protected"));

        // delete_file must refuse — the seed file survives (no brick).
        let d = DeleteFile::new(fs.clone())
            .execute(json!({ "path": seed_str }), None)
            .await;
        assert!(d.is_err(), "delete_file must refuse the seed");
        assert!(seed.exists(), "seed file must still exist after refused delete");

        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod schema_lint_tests {
    use super::*;
    use crate::filesystem::NativeFilesystem;

    /// Recursively assert no JSON-Schema node uses an ARRAY-valued `type`
    /// (a nullable union like `["string","null"]`). Gemini's function-
    /// declaration schema rejects union types with a 400 Bad Request —
    /// which silently bricked EVERY chat turn when `configure_agent` shipped
    /// with `"type": ["string","null"]`. This test catches that class of bug
    /// locally, in `cargo test`, instead of in production.
    fn assert_single_type(v: &serde_json::Value, tool: &str, path: &str) {
        match v {
            serde_json::Value::Object(map) => {
                if let Some(t) = map.get("type") {
                    assert!(
                        !t.is_array(),
                        "tool `{tool}` schema at `{path}.type` = {t} is an array — \
                         Gemini 400s on union types; use a single `type` string",
                    );
                }
                for (k, val) in map {
                    assert_single_type(val, tool, &format!("{path}.{k}"));
                }
            }
            serde_json::Value::Array(arr) => {
                for (i, val) in arr.iter().enumerate() {
                    assert_single_type(val, tool, &format!("{path}[{i}]"));
                }
            }
            _ => {}
        }
    }

    /// Every builtin tool's `input_schema` (sent verbatim as the wire
    /// `parameters`) must be a Gemini-compatible schema. Covers all tools
    /// constructible without a live API client (i.e. everything except
    /// generate_image / start_subagent, which need a client).
    #[test]
    fn builtin_tool_schemas_have_no_union_types() {
        let fs: SharedFilesystem = Arc::new(NativeFilesystem::new());
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(Finish),
            Arc::new(AskQuestion),
            Arc::new(current_time::CurrentTime),
            Arc::new(configure_agent::ConfigureAgent),
            Arc::new(call_agent::CallAgent),
            Arc::new(compile_rustlite::CompileRustlite),
            Arc::new(run_cartridge::RunCartridge),
            Arc::new(render_html::RenderHtml),
            Arc::new(ListDirectory::new(fs.clone())),
            Arc::new(ViewFile::new(fs.clone())),
            Arc::new(FindFile::new(fs.clone())),
            Arc::new(SearchDirectory::new(fs.clone())),
            Arc::new(CreateFile::new(fs.clone())),
            Arc::new(EditFile::new(fs.clone())),
            Arc::new(DeleteFile::new(fs.clone())),
            Arc::new(RenameFile::new(fs.clone())),
        ];
        for t in &tools {
            assert_single_type(&t.input_schema(), t.name(), "parameters");
        }
    }

    /// The filesystem builtins gate on a SUPPLIED `Filesystem`, not on the
    /// `native` feature — so they register on wasm32 over OPFS just as on
    /// native. Guards against re-introducing a `#[cfg(feature = "native")]`
    /// on the fs tools (only `run_command` is native-only).
    #[test]
    fn fs_builtins_gate_on_filesystem_not_native() {
        use crate::tools::ToolRunner;
        let caps = CapabilitiesConfig::unrestricted();
        let fs_names = ["list_directory", "view_file", "find_file", "search_directory",
            "create_file", "edit_file", "delete_file", "rename_file"];

        let with_fs = BuiltinDeps {
            chat_client: None,
            chat_model: String::new(),
            image_client: None,
            image_model: String::new(),
            fs: Some(Arc::new(NativeFilesystem::new()) as SharedFilesystem),
        };
        let runner = ToolRunner::new();
        let registered = register_builtins(&runner, &caps, &with_fs);
        for t in fs_names {
            assert!(
                registered.iter().any(|n| n == t),
                "`{t}` must register when a filesystem is supplied"
            );
        }

        let no_fs = BuiltinDeps {
            chat_client: None,
            chat_model: String::new(),
            image_client: None,
            image_model: String::new(),
            fs: None,
        };
        let runner2 = ToolRunner::new();
        let registered2 = register_builtins(&runner2, &caps, &no_fs);
        for t in fs_names {
            assert!(
                !registered2.iter().any(|n| n == t),
                "`{t}` must be skipped when no filesystem is supplied"
            );
        }
    }
}
