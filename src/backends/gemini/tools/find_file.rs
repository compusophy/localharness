//! `find_file` — recursive file-name search.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use globset::{Glob, GlobMatcher};
use serde::Deserialize;
use serde_json::{json, Value};
use walkdir::WalkDir;

use crate::error::{Error, Result};
use crate::tools::{Tool, ToolContext};

/// Cap on results to prevent unbounded output for shallow patterns.
const MAX_RESULTS: usize = 1000;

pub struct FindFile;

#[derive(Deserialize)]
struct Args {
    path: String,
    pattern: String,
    #[serde(default)]
    max_depth: Option<usize>,
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
        json!({
            "type": "object",
            "properties": {
                "path":      { "type": "string", "description": "Directory to search under." },
                "pattern":   { "type": "string", "description": "Glob pattern matched against file names." },
                "max_depth": { "type": "integer", "minimum": 1, "description": "Optional recursion depth cap." }
            },
            "required": ["path", "pattern"]
        })
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| Error::other(format!("find_file args: {e}")))?;
        let matcher: GlobMatcher = Glob::new(&args.pattern)
            .map_err(|e| Error::other(format!("invalid glob '{}': {e}", args.pattern)))?
            .compile_matcher();
        let root = PathBuf::from(&args.path);
        let max_depth = args.max_depth;

        // walkdir is sync; run on the blocking pool so we don't park
        // an async worker.
        let result = tokio::task::spawn_blocking(move || {
            let mut walker = WalkDir::new(&root).follow_links(false);
            if let Some(d) = max_depth {
                walker = walker.max_depth(d);
            }
            let mut matches = Vec::new();
            let mut truncated = false;
            for entry in walker.into_iter().filter_map(|e| e.ok()) {
                if !entry.file_type().is_file() {
                    continue;
                }
                if !matcher.is_match(entry.file_name()) {
                    continue;
                }
                if matches.len() >= MAX_RESULTS {
                    truncated = true;
                    break;
                }
                matches.push(entry.path().display().to_string());
            }
            (matches, truncated, root)
        })
        .await
        .map_err(|e| Error::other(format!("find_file join: {e}")))?;

        let (matches, truncated, root) = result;
        Ok(json!({
            "root": root.display().to_string(),
            "pattern": args.pattern,
            "matches": matches,
            "count": matches.len(),
            "truncated": truncated,
        }))
    }
}
