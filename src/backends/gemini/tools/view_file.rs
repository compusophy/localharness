//! `view_file` — read a text file with optional line range.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::tools::{Tool, ToolContext};

/// Soft cap on how much we'll send to the model in one call. Past this
/// the tool truncates and the model is told there's more.
const MAX_BYTES_RETURNED: usize = 256 * 1024;

pub struct ViewFile;

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
        let path: PathBuf = PathBuf::from(&args.path);

        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| Error::other(format!("read({}): {e}", path.display())))?;

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
            "path": path.display().to_string(),
            "total_lines": total_lines,
            "start_line": start,
            "end_line": end,
            "truncated": truncated,
            "content": content,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_existing_file_with_range() {
        let tmp = tempfile_path("view_file_test.txt");
        tokio::fs::write(&tmp, b"alpha\nbeta\ngamma\ndelta\n")
            .await
            .unwrap();
        let tool = ViewFile;
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
