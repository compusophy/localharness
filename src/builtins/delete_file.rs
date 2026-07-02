//! `delete_file` — remove a file or directory.
//!
//! Wraps [`Filesystem::delete`], which is recursive for directories.
//! Counted as a write capability; gated by the agent's policy.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::filesystem::{EntryKind, SharedFilesystem};
use crate::tools::{Tool, ToolContext};

pub struct DeleteFile {
    fs: SharedFilesystem,
}

impl DeleteFile {
    pub fn new(fs: SharedFilesystem) -> Self {
        Self { fs }
    }
}

crate::tool_params! {
    /// ONE table generates both this struct and `input_schema` (see
    /// `crate::tool_params`); the schema byte-identity test is below.
    struct Args: serde {
        path: req_str = "File or directory to delete.",
    }
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
        Args::schema()
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| Error::other(format!("delete_file args: {e}")))?;
        // Deleting the wallet seed / device key bricks the identity.
        if crate::builtins::is_protected_path(&args.path) {
            return Err(crate::builtins::protected_path_error(&args.path));
        }
        // A directory delete is RECURSIVE (native: remove_dir_all), so deleting
        // `.` or any ancestor of the seed would wipe `.lh_wallet` even though a
        // direct `delete_file(".lh_wallet")` is refused above. Walk the target
        // and refuse if it CONTAINS a protected file. (Best-effort: a metadata/
        // walk error falls through to the delete, as the bare delete did before.)
        if matches!(self.fs.metadata(&args.path).await, Ok(Some(m)) if m.kind == EntryKind::Directory) {
            if let Ok(entries) = self.fs.walk(&args.path, None).await {
                if let Some(hit) = entries
                    .iter()
                    .find(|e| crate::builtins::is_protected_path(&e.path))
                {
                    return Err(crate::builtins::protected_path_error(&hit.path));
                }
            }
        }
        self.fs.delete(&args.path).await?;
        Ok(json!({ "ok": true, "path": args.path }))
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
                "path": { "type": "string", "description": "File or directory to delete." }
            },
            "required": ["path"]
        });
        assert_eq!(Args::schema().to_string(), frozen.to_string());
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

    /// A RECURSIVE directory delete must refuse if the tree contains a
    /// protected identity file — `delete_file(dir)` would otherwise wipe a
    /// nested `.lh_wallet` and brick the identity, sidestepping the per-path
    /// guard. (I8)
    #[tokio::test]
    async fn refuses_to_recursively_delete_a_dir_containing_the_seed() {
        let dir = std::env::temp_dir().join(format!("delete_dir_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        let seed = dir.join("nested").join(".lh_wallet");
        std::fs::write(&seed, b"SECRET SEED PHRASE").unwrap();
        let tool = DeleteFile::new(Arc::new(NativeFilesystem::new()));
        let res = tool
            .execute(json!({"path": dir.display().to_string()}), None)
            .await;
        assert!(res.is_err(), "recursive delete over a nested seed must refuse");
        assert!(seed.exists(), "seed must survive the refused recursive delete");
        std::fs::remove_dir_all(&dir).ok();
    }
}
