//! `search_directory` — recursive content search (regex).

use std::sync::Arc;

use async_trait::async_trait;
use globset::{Glob, GlobMatcher};
use regex::RegexBuilder;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::filesystem::{file_name, EntryKind, SharedFilesystem};
use crate::tools::{Tool, ToolContext};

const MAX_MATCHES: usize = 500;
/// Don't read individual files above this size — they're rarely source
/// code and inflate response time.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

pub struct SearchDirectory {
    fs: SharedFilesystem,
}

impl SearchDirectory {
    pub fn new(fs: SharedFilesystem) -> Self {
        Self { fs }
    }
}

#[derive(Deserialize)]
struct Args {
    path: String,
    pattern: String,
    #[serde(default)]
    file_glob: Option<String>,
    #[serde(default)]
    case_sensitive: Option<bool>,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
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

        let entries = self.fs.walk(&args.path, None).await?;

        let mut matches: Vec<Value> = Vec::new();
        let mut truncated = false;
        'outer: for entry in entries {
            if !matches!(entry.kind, EntryKind::File) {
                continue;
            }
            // Never surface the seed/device-key contents in search results.
            if crate::builtins::is_protected_path(&entry.path) {
                continue;
            }
            let name = file_name(&entry.path);
            if let Some(matcher) = &file_matcher {
                if !matcher.is_match(name) {
                    continue;
                }
            }
            if let Some(sz) = entry.size {
                if sz > MAX_FILE_BYTES {
                    continue;
                }
            }
            let bytes = match self.fs.read(&entry.path).await {
                Ok(b) => b,
                Err(_) => continue,
            };
            let text = match std::str::from_utf8(&bytes) {
                Ok(s) => s,
                Err(_) => continue, // skip binary files
            };
            for (i, raw_line) in text.split('\n').enumerate() {
                // Strip a trailing CR so `$`-anchored patterns match on CRLF
                // files (common on Windows) and the surfaced `text` doesn't carry
                // a stray `\r`. Splitting on `\n` alone left every CRLF line
                // ending in `\r`, so `alpha$` silently missed `alpha\r\n`.
                let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
                if regex.is_match(line) {
                    if matches.len() >= MAX_MATCHES {
                        truncated = true;
                        break 'outer;
                    }
                    matches.push(json!({
                        "path": entry.path,
                        "line": i + 1,
                        "text": line,
                    }));
                }
            }
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

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use crate::filesystem::NativeFilesystem;
    use std::path::PathBuf;

    fn unique_dir(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("lh_search_dir_{label}_{}", uuid::Uuid::new_v4()));
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
    async fn crlf_lines_match_dollar_anchor_without_cr_leak() {
        // A Windows-authored (CRLF) file. Splitting on `\n` alone left each line
        // ending in `\r`, so `alpha$` silently missed it and the surfaced text
        // leaked the `\r`. The trailing-CR strip fixes both.
        let root = unique_dir("crlf");
        write(&root, "win.txt", "alpha\r\nbeta\r\n");
        let tool = SearchDirectory::new(Arc::new(NativeFilesystem::new()));
        let out = tool
            .execute(
                json!({ "path": root.display().to_string(), "pattern": "alpha$" }),
                None,
            )
            .await
            .unwrap();
        assert_eq!(out["count"].as_u64(), Some(1), "alpha$ must match a CRLF line: {out}");
        assert_eq!(
            out["matches"][0]["text"],
            json!("alpha"),
            "surfaced text must not carry the trailing CR",
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn finds_regex_matches_with_line_numbers() {
        let root = unique_dir("regex");
        write(
            &root,
            "src/lib.rs",
            "fn one() {}\nfn two() {}\nstruct Foo;\n",
        );
        write(&root, "src/other.rs", "fn three() {}\n");
        write(&root, "README.md", "fn not_code() {}\n");

        let tool = SearchDirectory::new(Arc::new(NativeFilesystem::new()));
        let out = tool
            .execute(
                json!({
                    "path": root.display().to_string(),
                    "pattern": r"^fn ",
                }),
                None,
            )
            .await
            .unwrap();

        // Three lines start with `fn `, one in each of the three files.
        assert_eq!(out["count"].as_u64(), Some(4), "got {}", out);
        let matches = out["matches"].as_array().unwrap();
        // Every match carries a 1-indexed line number.
        for m in matches {
            assert!(m["line"].as_u64().unwrap() >= 1);
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn file_glob_restricts_search() {
        let root = unique_dir("glob");
        write(&root, "a.rs", "needle\n");
        write(&root, "b.md", "needle\n");
        write(&root, "sub/c.rs", "needle\n");

        let tool = SearchDirectory::new(Arc::new(NativeFilesystem::new()));
        let out = tool
            .execute(
                json!({
                    "path": root.display().to_string(),
                    "pattern": "needle",
                    "file_glob": "*.rs",
                }),
                None,
            )
            .await
            .unwrap();
        assert_eq!(out["count"].as_u64(), Some(2), "got {}", out);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn case_insensitive_by_default() {
        let root = unique_dir("case");
        write(&root, "a.txt", "NEEDLE\n");

        let tool = SearchDirectory::new(Arc::new(NativeFilesystem::new()));
        let insens = tool
            .execute(
                json!({"path": root.display().to_string(), "pattern": "needle"}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(insens["count"].as_u64(), Some(1));

        let sens = tool
            .execute(
                json!({
                    "path": root.display().to_string(),
                    "pattern": "needle",
                    "case_sensitive": true,
                }),
                None,
            )
            .await
            .unwrap();
        assert_eq!(sens["count"].as_u64(), Some(0));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn rejects_invalid_regex() {
        let root = unique_dir("badre");
        let tool = SearchDirectory::new(Arc::new(NativeFilesystem::new()));
        let res = tool
            .execute(
                json!({
                    "path": root.display().to_string(),
                    "pattern": "(",
                }),
                None,
            )
            .await;
        assert!(res.is_err());
        std::fs::remove_dir_all(&root).ok();
    }
}
