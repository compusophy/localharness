//! `create_file` — atomically create a new file with content.
//!
//! Refuses to overwrite an existing file (use `edit_file` for that).
//! Atomicity is provided by [`Filesystem::write_atomic`].

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::filesystem::SharedFilesystem;
use crate::tools::{Tool, ToolContext};

/// Hard cap on the content we'll write in one create_file call — mirrors
/// view_file's MAX_FILE_BYTES so untrusted/model-driven input can't exhaust
/// storage.
const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;

pub struct CreateFile {
    fs: SharedFilesystem,
}

impl CreateFile {
    pub fn new(fs: SharedFilesystem) -> Self {
        Self { fs }
    }
}

crate::tool_params! {
    /// ONE table generates both this struct and `input_schema` (see
    /// `crate::tool_params`); the schema byte-identity test is below.
    struct Args: serde {
        path: req_str = "Absolute or relative file path to create.",
        content: req_str = "Full UTF-8 content of the new file.",
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for CreateFile {
    fn name(&self) -> &str {
        "create_file"
    }

    fn description(&self) -> &str {
        "Create a new file with the given content. Fails if the file already exists. \
         Writes atomically via tempfile + rename."
    }

    fn input_schema(&self) -> Value {
        Args::schema()
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| Error::other(format!("create_file args: {e}")))?;

        // Never let a tool create/clobber the seed or device-key path.
        if crate::builtins::is_protected_path(&args.path) {
            return Err(crate::builtins::protected_path_error(&args.path));
        }

        if args.content.len() > MAX_FILE_BYTES {
            return Err(Error::other(format!(
                "content is {} bytes, over the {MAX_FILE_BYTES}-byte create_file cap",
                args.content.len()
            )));
        }

        if self.fs.metadata(&args.path).await?.is_some() {
            return Err(Error::other(format!(
                "create_file refuses to overwrite existing file: {}",
                args.path
            )));
        }

        let bytes = args.content.into_bytes();
        let len = bytes.len() as u64;
        self.fs.write_atomic(&args.path, &bytes).await?;

        Ok(json!({
            "ok": true,
            "path": args.path,
            "bytes": len,
        }))
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
                "path":    { "type": "string", "description": "Absolute or relative file path to create." },
                "content": { "type": "string", "description": "Full UTF-8 content of the new file." }
            },
            "required": ["path", "content"]
        });
        assert_eq!(Args::schema().to_string(), frozen.to_string());
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use crate::filesystem::NativeFilesystem;

    #[tokio::test]
    async fn writes_new_file() {
        let mut p = std::env::temp_dir();
        p.push(format!("create_file_test_{}.txt", uuid::Uuid::new_v4()));
        let tool = CreateFile::new(Arc::new(NativeFilesystem::new()));
        let out = tool
            .execute(
                json!({"path": p.display().to_string(), "content": "hello\n"}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(out["ok"].as_bool(), Some(true));
        assert_eq!(out["bytes"].as_u64(), Some(6));
        let content = std::fs::read_to_string(&p).unwrap();
        assert_eq!(content, "hello\n");
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn refuses_to_overwrite() {
        let mut p = std::env::temp_dir();
        p.push(format!("create_file_overwrite_{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&p, "existing").unwrap();
        let tool = CreateFile::new(Arc::new(NativeFilesystem::new()));
        let res = tool
            .execute(
                json!({"path": p.display().to_string(), "content": "new"}),
                None,
            )
            .await;
        assert!(res.is_err());
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn rejects_oversized_content() {
        let mut p = std::env::temp_dir();
        p.push(format!("create_file_oversized_{}.txt", uuid::Uuid::new_v4()));
        let content = "a".repeat(MAX_FILE_BYTES + 1);
        let tool = CreateFile::new(Arc::new(NativeFilesystem::new()));
        let res = tool
            .execute(json!({"path": p.display().to_string(), "content": content}), None)
            .await;
        assert!(res.is_err());
        assert!(std::fs::metadata(&p).is_err(), "oversized file must not be created");
    }
}
