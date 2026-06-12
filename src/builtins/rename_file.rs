//! `rename_file` — rename or move a file / directory.
//!
//! Wraps [`Filesystem::rename`]. Native backend uses an atomic
//! `std::fs::rename`; OPFS falls back to read + write + delete via
//! the default trait impl.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::filesystem::SharedFilesystem;
use crate::tools::{Tool, ToolContext};

pub struct RenameFile {
    fs: SharedFilesystem,
}

impl RenameFile {
    pub fn new(fs: SharedFilesystem) -> Self {
        Self { fs }
    }
}

#[derive(Deserialize)]
struct Args {
    from: String,
    to: String,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for RenameFile {
    fn name(&self) -> &str {
        "rename_file"
    }

    fn description(&self) -> &str {
        "Rename or move a file from `from` to `to`. On native, atomic \
         when both paths are on the same filesystem. On OPFS, performs \
         read + write + delete (not atomic but safe — original is only \
         removed after the new path lands)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "from": { "type": "string", "description": "Current path." },
                "to":   { "type": "string", "description": "New path." }
            },
            "required": ["from", "to"]
        })
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| Error::other(format!("rename_file args: {e}")))?;
        if args.from == args.to {
            return Err(Error::other("from and to are identical"));
        }
        // Renaming the seed/device key away (or clobbering one) bricks identity.
        if crate::builtins::is_protected_path(&args.from) {
            return Err(crate::builtins::protected_path_error(&args.from));
        }
        if crate::builtins::is_protected_path(&args.to) {
            return Err(crate::builtins::protected_path_error(&args.to));
        }
        self.fs.rename(&args.from, &args.to).await?;
        Ok(json!({ "ok": true, "from": args.from, "to": args.to }))
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use crate::filesystem::NativeFilesystem;

    #[tokio::test]
    async fn renames_a_file() {
        let dir = std::env::temp_dir();
        let from = dir.join(format!("rename_from_{}.txt", uuid::Uuid::new_v4()));
        let to = dir.join(format!("rename_to_{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&from, "hello").unwrap();
        let tool = RenameFile::new(Arc::new(NativeFilesystem::new()));
        let out = tool
            .execute(
                json!({"from": from.display().to_string(), "to": to.display().to_string()}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(out["ok"], json!(true));
        assert!(!from.exists());
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "hello");
        let _ = std::fs::remove_file(to);
    }

    #[tokio::test]
    async fn rejects_identical_paths() {
        let tool = RenameFile::new(Arc::new(NativeFilesystem::new()));
        let res = tool
            .execute(json!({"from": "x.txt", "to": "x.txt"}), None)
            .await;
        assert!(res.is_err());
    }
}
