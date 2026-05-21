//! `list_directory` — list immediate children of a directory.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::tools::{Tool, ToolContext};

pub struct ListDirectory;

#[derive(Deserialize)]
struct Args {
    path: String,
}

#[async_trait]
impl Tool for ListDirectory {
    fn name(&self) -> &str {
        "list_directory"
    }

    fn description(&self) -> &str {
        "List the immediate children of a directory. Returns each entry's name, kind \
         (\"file\" | \"directory\" | \"symlink\" | \"other\"), and size in bytes for files."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute or relative directory path." }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| Error::other(format!("list_directory args: {e}")))?;
        let path: PathBuf = PathBuf::from(&args.path);

        let read = tokio::fs::read_dir(&path)
            .await
            .map_err(|e| Error::other(format!("read_dir({}): {e}", path.display())))?;
        let mut entries = Vec::new();
        let mut iter = read;
        while let Some(entry) = iter
            .next_entry()
            .await
            .map_err(|e| Error::other(format!("next_entry: {e}")))?
        {
            let meta = entry
                .metadata()
                .await
                .map_err(|e| Error::other(format!("metadata: {e}")))?;
            let kind = if meta.file_type().is_symlink() {
                "symlink"
            } else if meta.is_dir() {
                "directory"
            } else if meta.is_file() {
                "file"
            } else {
                "other"
            };
            let mut entry_obj = json!({
                "name": entry.file_name().to_string_lossy(),
                "kind": kind,
            });
            if meta.is_file() {
                entry_obj["size"] = json!(meta.len());
            }
            entries.push(entry_obj);
        }
        // Stable order so the model sees the same listing across calls.
        entries.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
        });

        Ok(json!({
            "path": path.display().to_string(),
            "entries": entries,
            "count": entries.len(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lists_known_directory() {
        let tmp = std::env::temp_dir();
        let tool = ListDirectory;
        let out = tool
            .execute(json!({"path": tmp.display().to_string()}), None)
            .await
            .unwrap();
        assert!(out["entries"].is_array(), "entries should be an array");
        // Count is a non-negative integer.
        assert!(out["count"].as_u64().is_some());
    }

    #[tokio::test]
    async fn errors_on_missing_directory() {
        let tool = ListDirectory;
        let res = tool
            .execute(json!({"path": "/definitely/does/not/exist/abc123"}), None)
            .await;
        assert!(res.is_err());
    }
}
