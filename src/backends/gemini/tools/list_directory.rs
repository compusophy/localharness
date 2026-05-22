//! `list_directory` — list immediate children of a directory.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::filesystem::SharedFilesystem;
use crate::tools::{Tool, ToolContext};

pub struct ListDirectory {
    fs: SharedFilesystem,
}

impl ListDirectory {
    pub fn new(fs: SharedFilesystem) -> Self {
        Self { fs }
    }
}

#[derive(Deserialize)]
struct Args {
    path: String,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
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
        let entries = self.fs.read_dir(&args.path).await?;

        let entry_values: Vec<Value> = entries
            .into_iter()
            .map(|e| {
                let mut obj = json!({
                    "name": e.name,
                    "kind": e.kind.as_str(),
                });
                if let Some(size) = e.size {
                    obj["size"] = json!(size);
                }
                obj
            })
            .collect();

        let count = entry_values.len();
        Ok(json!({
            "path": args.path,
            "entries": entry_values,
            "count": count,
        }))
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use crate::filesystem::NativeFilesystem;

    #[tokio::test]
    async fn lists_known_directory() {
        let tmp = std::env::temp_dir();
        let tool = ListDirectory::new(Arc::new(NativeFilesystem::new()));
        let out = tool
            .execute(json!({"path": tmp.display().to_string()}), None)
            .await
            .unwrap();
        assert!(out["entries"].is_array(), "entries should be an array");
        assert!(out["count"].as_u64().is_some());
    }

    #[tokio::test]
    async fn errors_on_missing_directory() {
        let tool = ListDirectory::new(Arc::new(NativeFilesystem::new()));
        let res = tool
            .execute(json!({"path": "/definitely/does/not/exist/abc123"}), None)
            .await;
        assert!(res.is_err());
    }
}
