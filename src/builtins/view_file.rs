//! `view_file` — read a text file with optional line range.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::filesystem::SharedFilesystem;
use crate::tools::{Tool, ToolContext};

/// Soft cap on how much we'll send to the model in one call. Past this
/// the tool truncates and the model is told there's more.
const MAX_BYTES_RETURNED: usize = 256 * 1024;

/// Hard cap on the on-disk file size we'll read into memory at all. The
/// tool reads the whole file before slicing to a line range, so without
/// this a model pointing at a multi-GB file (or an unbounded pseudo-file)
/// could exhaust memory. 16 MiB is far above any real source file.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

pub struct ViewFile {
    fs: SharedFilesystem,
}

impl ViewFile {
    pub fn new(fs: SharedFilesystem) -> Self {
        Self { fs }
    }
}

#[derive(Deserialize)]
struct Args {
    path: String,
    #[serde(default)]
    start_line: Option<u32>,
    #[serde(default)]
    end_line: Option<u32>,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for ViewFile {
    fn name(&self) -> &str {
        "view_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a text file. Optionally limit to a 1-indexed inclusive \
         line range via start_line / end_line. Large outputs are truncated."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path":       { "type": "string", "description": "Absolute or relative file path." },
                "start_line": { "type": "integer", "minimum": 1, "description": "1-indexed first line to return." },
                "end_line":   { "type": "integer", "minimum": 1, "description": "1-indexed last line to return (inclusive)." }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| Error::other(format!("view_file args: {e}")))?;

        // Never let a tool read the wallet seed / device key into the transcript.
        if crate::builtins::is_protected_path(&args.path) {
            return Err(crate::builtins::protected_path_error(&args.path));
        }

        // Refuse to read a huge file into memory. (metadata() may be None
        // on backends that don't implement it — then we fall through to the
        // read, same as before.)
        if let Some(meta) = self.fs.metadata(&args.path).await? {
            if meta.size > MAX_FILE_BYTES {
                return Err(Error::other(format!(
                    "file is {} bytes, over the {MAX_FILE_BYTES}-byte view cap; \
                     pass start_line/end_line to read a range",
                    meta.size
                )));
            }
        }

        let bytes = self.fs.read(&args.path).await?;

        // UTF-8 lossy so we don't error on the occasional binary nibble.
        let full = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = full.split_inclusive('\n').collect();
        let total_lines = lines.len() as u32;

        let (start, end) = match (args.start_line, args.end_line) {
            (Some(s), Some(e)) => (s.max(1), e.min(total_lines).max(1)),
            (Some(s), None) => (s.max(1), total_lines),
            (None, Some(e)) => (1, e.min(total_lines).max(1)),
            (None, None) => (1, total_lines),
        };
        if start > end {
            return Err(Error::other(format!(
                "start_line ({start}) > end_line ({end})"
            )));
        }

        let slice = lines
            .iter()
            .skip((start - 1) as usize)
            .take((end - start + 1) as usize)
            .copied()
            .collect::<String>();

        let (content, truncated) = if slice.len() > MAX_BYTES_RETURNED {
            // Truncate on a UTF-8 char boundary near the cap.
            let mut cut = MAX_BYTES_RETURNED;
            while !slice.is_char_boundary(cut) && cut > 0 {
                cut -= 1;
            }
            (slice[..cut].to_string(), true)
        } else {
            (slice, false)
        };

        Ok(json!({
            "path": args.path,
            "total_lines": total_lines,
            "start_line": start,
            "end_line": end,
            "truncated": truncated,
            "content": content,
        }))
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use crate::filesystem::NativeFilesystem;
    use std::path::PathBuf;

    #[tokio::test]
    async fn rejects_inverted_line_range() {
        let tmp = tempfile_path("view_file_inverted.txt");
        tokio::fs::write(&tmp, b"a\nb\nc\nd\n").await.unwrap();
        let tool = ViewFile::new(Arc::new(NativeFilesystem::new()));
        let res = tool
            .execute(
                json!({"path": tmp.display().to_string(), "start_line": 3, "end_line": 2}),
                None,
            )
            .await;
        assert!(res.is_err(), "start_line > end_line should error");
        let _ = std::fs::remove_file(tmp);
    }

    #[tokio::test]
    async fn reads_existing_file_with_range() {
        let tmp = tempfile_path("view_file_test.txt");
        tokio::fs::write(&tmp, b"alpha\nbeta\ngamma\ndelta\n")
            .await
            .unwrap();
        let tool = ViewFile::new(Arc::new(NativeFilesystem::new()));
        let out = tool
            .execute(
                json!({"path": tmp.display().to_string(), "start_line": 2, "end_line": 3}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(out["content"].as_str().unwrap(), "beta\ngamma\n");
        assert_eq!(out["start_line"].as_u64(), Some(2));
        assert_eq!(out["end_line"].as_u64(), Some(3));
        assert_eq!(out["total_lines"].as_u64(), Some(4));
    }

    fn tempfile_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(name);
        let _ = std::fs::remove_file(&p);
        p
    }
}
