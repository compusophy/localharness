//! `search_directory` — recursive content search (regex).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use globset::{Glob, GlobMatcher};
use regex::RegexBuilder;
use serde::Deserialize;
use serde_json::{json, Value};
use walkdir::WalkDir;

use crate::error::{Error, Result};
use crate::tools::{Tool, ToolContext};

const MAX_MATCHES: usize = 500;
/// Don't read individual files above this size — they're rarely source
/// code and inflate response time.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

pub struct SearchDirectory;

#[derive(Deserialize)]
struct Args {
    path: String,
    pattern: String,
    #[serde(default)]
    file_glob: Option<String>,
    #[serde(default)]
    case_sensitive: Option<bool>,
}

#[async_trait]
impl Tool for SearchDirectory {
    fn name(&self) -> &str {
        "search_directory"
    }

    fn description(&self) -> &str {
        "Recursively search file contents for a regex pattern. Optionally filter \
         files by a glob (e.g. \"*.rs\"). Returns up to 500 matches as \
         { path, line, text } objects."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path":          { "type": "string", "description": "Directory to search under." },
                "pattern":       { "type": "string", "description": "Regex (RE2-style) matched against each line." },
                "file_glob":     { "type": "string", "description": "Optional glob (e.g. \"*.rs\") to restrict files." },
                "case_sensitive":{ "type": "boolean", "description": "Defaults to false." }
            },
            "required": ["path", "pattern"]
        })
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| Error::other(format!("search_directory args: {e}")))?;
        let regex = RegexBuilder::new(&args.pattern)
            .case_insensitive(!args.case_sensitive.unwrap_or(false))
            .build()
            .map_err(|e| Error::other(format!("invalid regex '{}': {e}", args.pattern)))?;
        let file_matcher: Option<GlobMatcher> = match &args.file_glob {
            Some(g) => Some(
                Glob::new(g)
                    .map_err(|e| Error::other(format!("invalid file_glob '{g}': {e}")))?
                    .compile_matcher(),
            ),
            None => None,
        };
        let root = PathBuf::from(&args.path);

        let result = tokio::task::spawn_blocking(move || {
            let mut matches = Vec::new();
            let mut truncated = false;
            for entry in WalkDir::new(&root)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                if let Some(matcher) = &file_matcher {
                    if !matcher.is_match(entry.file_name()) {
                        continue;
                    }
                }
                let meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if meta.len() > MAX_FILE_BYTES {
                    continue;
                }
                let bytes = match std::fs::read(entry.path()) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let text = match std::str::from_utf8(&bytes) {
                    Ok(s) => s,
                    Err(_) => continue, // skip binary files
                };
                for (i, line) in text.split('\n').enumerate() {
                    if regex.is_match(line) {
                        if matches.len() >= MAX_MATCHES {
                            truncated = true;
                            break;
                        }
                        matches.push(json!({
                            "path": entry.path().display().to_string(),
                            "line": i + 1,
                            "text": line,
                        }));
                    }
                }
                if truncated {
                    break;
                }
            }
            (matches, truncated, root)
        })
        .await
        .map_err(|e| Error::other(format!("search_directory join: {e}")))?;

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
