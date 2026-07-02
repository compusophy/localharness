//! `find_file` — recursive file-name search.

use std::sync::Arc;

use async_trait::async_trait;
use globset::{Glob, GlobMatcher};
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::filesystem::{file_name, EntryKind, SharedFilesystem};
use crate::tools::{Tool, ToolContext};

/// Cap on results to prevent unbounded output for shallow patterns.
const MAX_RESULTS: usize = 1000;

pub struct FindFile {
    fs: SharedFilesystem,
}

impl FindFile {
    pub fn new(fs: SharedFilesystem) -> Self {
        Self { fs }
    }
}

crate::tool_params! {
    /// ONE table generates both this struct and `input_schema` (see
    /// `crate::tool_params`); the schema byte-identity test is below.
    /// `max_depth` was `Option<usize>` — `opt_u32` (the schema already said
    /// `minimum: 1`) covers every sane depth; the walk call converts.
    struct Args: serde {
        path: req_str = "Directory to search under.",
        pattern: req_str = "Glob pattern matched against file names.",
        max_depth: opt_u32 min 1 = "Optional recursion depth cap.",
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for FindFile {
    fn name(&self) -> &str {
        "find_file"
    }

    fn description(&self) -> &str {
        "Recursively search for files whose name matches a glob pattern \
         (e.g. \"*.rs\", \"test_*.py\"). Returns up to 1000 matches."
    }

    fn input_schema(&self) -> Value {
        Args::schema()
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| Error::other(format!("find_file args: {e}")))?;
        let matcher: GlobMatcher = Glob::new(&args.pattern)
            .map_err(|e| Error::other(format!("invalid glob '{}': {e}", args.pattern)))?
            .compile_matcher();

        let entries = self
            .fs
            .walk(&args.path, args.max_depth.map(|d| d as usize))
            .await?;

        let mut matches: Vec<String> = Vec::new();
        let mut truncated = false;
        for entry in entries {
            if !matches!(entry.kind, EntryKind::File) {
                continue;
            }
            // Don't surface the protected identity files in name matches.
            if crate::builtins::is_protected_path(&entry.path) {
                continue;
            }
            let name = file_name(&entry.path);
            if !matcher.is_match(name) {
                continue;
            }
            if matches.len() >= MAX_RESULTS {
                truncated = true;
                break;
            }
            matches.push(entry.path);
        }

        let count = matches.len();
        Ok(json!({
            "root": args.path,
            "pattern": args.pattern,
            "matches": matches,
            "count": count,
            "truncated": truncated,
        }))
    }
}

#[cfg(test)]
mod schema_tests {
    use super::Args;
    use serde_json::json;

    /// BYTE-IDENTITY: the macro-generated schema must serialize byte-for-byte
    /// equal to the hand-written literal it replaced (frozen verbatim here) —
    /// the wire shape is model-behavior-load-bearing.
    #[test]
    fn schema_is_byte_identical_to_the_frozen_original() {
        let frozen = json!({
            "type": "object",
            "properties": {
                "path":      { "type": "string", "description": "Directory to search under." },
                "pattern":   { "type": "string", "description": "Glob pattern matched against file names." },
                "max_depth": { "type": "integer", "minimum": 1, "description": "Optional recursion depth cap." }
            },
            "required": ["path", "pattern"]
        });
        assert_eq!(Args::schema().to_string(), frozen.to_string());
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use crate::filesystem::NativeFilesystem;
    use std::path::PathBuf;

    fn unique_dir(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("lh_find_file_{label}_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write(dir: &std::path::Path, rel: &str, content: &str) {
        let mut p = dir.to_path_buf();
        for part in rel.split('/') {
            p.push(part);
        }
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
    }

    #[tokio::test]
    async fn matches_glob_recursively() {
        let root = unique_dir("recursive");
        write(&root, "a.rs", "");
        write(&root, "b.py", "");
        write(&root, "sub/c.rs", "");
        write(&root, "sub/d.txt", "");
        write(&root, "sub/deeper/e.rs", "");

        let tool = FindFile::new(Arc::new(NativeFilesystem::new()));
        let out = tool
            .execute(
                json!({"path": root.display().to_string(), "pattern": "*.rs"}),
                None,
            )
            .await
            .unwrap();
        let count = out["count"].as_u64().unwrap();
        assert_eq!(count, 3, "expected three .rs files, got {}", out);
        assert_eq!(out["truncated"].as_bool(), Some(false));

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn max_depth_caps_recursion() {
        let root = unique_dir("depth");
        write(&root, "top.rs", "");
        write(&root, "sub/mid.rs", "");
        write(&root, "sub/deeper/bottom.rs", "");

        let tool = FindFile::new(Arc::new(NativeFilesystem::new()));
        let out = tool
            .execute(
                json!({
                    "path": root.display().to_string(),
                    "pattern": "*.rs",
                    "max_depth": 2,
                }),
                None,
            )
            .await
            .unwrap();
        // walkdir depth: root=0, top.rs=1, sub/mid.rs=2; deeper/bottom.rs=3 excluded.
        assert_eq!(out["count"].as_u64(), Some(2), "got {}", out);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn missing_root_returns_empty_silently() {
        // walkdir filter_map(|e| e.ok()) swallows the not-found error, so
        // find_file on a missing root returns 0 matches rather than Err.
        // This is the pre-M3a behavior and we lock it in here.
        let tool = FindFile::new(Arc::new(NativeFilesystem::new()));
        let out = tool
            .execute(
                json!({"path": "/definitely/missing/lh-find-test-zzz", "pattern": "*.rs"}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(out["count"].as_u64(), Some(0));
        assert_eq!(out["truncated"].as_bool(), Some(false));
    }

    #[tokio::test]
    async fn rejects_invalid_glob() {
        let root = unique_dir("badglob");
        let tool = FindFile::new(Arc::new(NativeFilesystem::new()));
        let res = tool
            .execute(
                json!({"path": root.display().to_string(), "pattern": "[abc"}),
                None,
            )
            .await;
        assert!(res.is_err());
        std::fs::remove_dir_all(&root).ok();
    }
}
