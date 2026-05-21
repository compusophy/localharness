//! `create_file` — atomically create a new file with content.
//!
//! Writes via a tempfile in the same directory, then renames into place
//! — so a crash mid-write never leaves a half-written file. Refuses to
//! overwrite an existing file (use `edit_file` for that).

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tempfile::NamedTempFile;

use crate::error::{Error, Result};
use crate::tools::{Tool, ToolContext};

pub struct CreateFile;

#[derive(Deserialize)]
struct Args {
    path: String,
    content: String,
}

#[async_trait]
impl Tool for CreateFile {
    fn name(&self) -> &str {
        "create_file"
    }

    fn description(&self) -> &str {
        "Create a new file with the given content. Fails if the file already exists. \
         Writes atomically via tempfile + rename."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path":    { "type": "string", "description": "Absolute or relative file path to create." },
                "content": { "type": "string", "description": "Full UTF-8 content of the new file." }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| Error::other(format!("create_file args: {e}")))?;
        let path = PathBuf::from(&args.path);

        if path.exists() {
            return Err(Error::other(format!(
                "create_file refuses to overwrite existing file: {}",
                path.display()
            )));
        }

        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf());
        let bytes = args.content.into_bytes();
        let path_for_task = path.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            if let Some(p) = &parent {
                std::fs::create_dir_all(p)
                    .map_err(|e| Error::other(format!("create_dir_all: {e}")))?;
            }
            let dir = parent.as_deref().unwrap_or(std::path::Path::new("."));
            let mut tmp = NamedTempFile::new_in(dir)
                .map_err(|e| Error::other(format!("tempfile: {e}")))?;
            tmp.write_all(&bytes)
                .map_err(|e| Error::other(format!("write: {e}")))?;
            tmp.persist(&path_for_task)
                .map_err(|e| Error::other(format!("rename: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| Error::other(format!("create_file join: {e}")))??;

        Ok(json!({
            "ok": true,
            "path": path.display().to_string(),
            "bytes": std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_new_file() {
        let mut p = std::env::temp_dir();
        p.push(format!("create_file_test_{}.txt", uuid::Uuid::new_v4()));
        let tool = CreateFile;
        let out = tool
            .execute(
                json!({"path": p.display().to_string(), "content": "hello\n"}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(out["ok"].as_bool(), Some(true));
        let content = std::fs::read_to_string(&p).unwrap();
        assert_eq!(content, "hello\n");
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn refuses_to_overwrite() {
        let mut p = std::env::temp_dir();
        p.push(format!("create_file_overwrite_{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&p, "existing").unwrap();
        let tool = CreateFile;
        let res = tool
            .execute(
                json!({"path": p.display().to_string(), "content": "new"}),
                None,
            )
            .await;
        assert!(res.is_err());
        let _ = std::fs::remove_file(p);
    }
}
