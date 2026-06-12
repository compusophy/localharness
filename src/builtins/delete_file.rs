//! `delete_file` — remove a file or directory.
//!
//! Wraps [`Filesystem::delete`], which is recursive for directories.
//! Counted as a write capability; gated by the agent's policy.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::filesystem::SharedFilesystem;
use crate::tools::{Tool, ToolContext};

pub struct DeleteFile {
    fs: SharedFilesystem,
}

impl DeleteFile {
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
impl Tool for DeleteFile {
    fn name(&self) -> &str {
        "delete_file"
    }

    fn description(&self) -> &str {
        "Delete a file or directory at `path`. Directories are removed \
         recursively. Errors if the path does not exist. Irreversible — \
         no trash / undo."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File or directory to delete." }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| Error::other(format!("delete_file args: {e}")))?;
        // Deleting the wallet seed / device key bricks the identity.
        if crate::builtins::is_protected_path(&args.path) {
            return Err(crate::builtins::protected_path_error(&args.path));
        }
        self.fs.delete(&args.path).await?;
        Ok(json!({ "ok": true, "path": args.path }))
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use crate::filesystem::NativeFilesystem;

    #[tokio::test]
    async fn deletes_an_existing_file() {
        let mut p = std::env::temp_dir();
        p.push(format!("delete_file_{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&p, "bye").unwrap();
        let tool = DeleteFile::new(Arc::new(NativeFilesystem::new()));
        let out = tool
            .execute(json!({"path": p.display().to_string()}), None)
            .await
            .unwrap();
        assert_eq!(out["ok"], json!(true));
        assert!(!p.exists(), "file should be gone");
    }

    #[tokio::test]
    async fn errors_on_missing_path() {
        let mut p = std::env::temp_dir();
        p.push(format!("delete_file_missing_{}.txt", uuid::Uuid::new_v4()));
        let tool = DeleteFile::new(Arc::new(NativeFilesystem::new()));
        let res = tool
            .execute(json!({"path": p.display().to_string()}), None)
            .await;
        assert!(res.is_err());
    }
}
