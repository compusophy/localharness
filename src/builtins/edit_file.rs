//! `edit_file` — replace a string in a file with another string.
//!
//! Matches Python's `edit_file` semantics: `old_string` must appear
//! exactly once unless `replace_all=true`. Atomicity is provided by
//! [`Filesystem::write_atomic`].

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::filesystem::SharedFilesystem;
use crate::tools::{Tool, ToolContext};

/// Hard cap on both the file we'll read into memory and the post-replace
/// result we'll write — mirrors view_file's MAX_FILE_BYTES so a model can't
/// OOM on a huge file or grow one unboundedly via replace_all.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

pub struct EditFile {
    fs: SharedFilesystem,
}

impl EditFile {
    pub fn new(fs: SharedFilesystem) -> Self {
        Self { fs }
    }
}

#[derive(Deserialize)]
struct Args {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for EditFile {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Replace `old_string` with `new_string` in a file. By default `old_string` \
         must appear exactly once (the tool fails otherwise). Set replace_all=true \
         to replace every occurrence. Writes atomically via tempfile + rename."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path":        { "type": "string", "description": "File to edit." },
                "old_string":  { "type": "string", "description": "Substring to replace; must be unique unless replace_all=true." },
                "new_string":  { "type": "string", "description": "Replacement." },
                "replace_all": { "type": "boolean", "description": "Replace every occurrence; default false." }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| Error::other(format!("edit_file args: {e}")))?;

        if args.old_string.is_empty() {
            return Err(Error::other("old_string must not be empty"));
        }
        // Editing the seed/device key would corrupt the identity (and reads it).
        if crate::builtins::is_protected_path(&args.path) {
            return Err(crate::builtins::protected_path_error(&args.path));
        }

        if let Some(meta) = self.fs.metadata(&args.path).await? {
            if meta.size > MAX_FILE_BYTES {
                return Err(Error::other(format!(
                    "file is {} bytes, over the {MAX_FILE_BYTES}-byte edit cap — too large to edit",
                    meta.size
                )));
            }
        }

        let bytes = self.fs.read(&args.path).await?;
        let original = String::from_utf8(bytes)
            .map_err(|e| Error::other(format!("read({}): not valid UTF-8: {e}", args.path)))?;

        let count = original.matches(&args.old_string).count();
        if count == 0 {
            return Err(Error::other(format!(
                "old_string not found in {}",
                args.path
            )));
        }
        if count > 1 && !args.replace_all {
            return Err(Error::other(format!(
                "old_string found {count} times in {} (need exactly 1, or set replace_all=true)",
                args.path
            )));
        }

        let updated = if args.replace_all {
            original.replace(&args.old_string, &args.new_string)
        } else {
            original.replacen(&args.old_string, &args.new_string, 1)
        };
        if updated.len() as u64 > MAX_FILE_BYTES {
            return Err(Error::other(format!(
                "edit would produce {} bytes, over the {MAX_FILE_BYTES}-byte cap",
                updated.len()
            )));
        }
        let replacements = count;
        self.fs
            .write_atomic(&args.path, updated.as_bytes())
            .await?;

        Ok(json!({
            "ok": true,
            "path": args.path,
            "replacements": replacements,
        }))
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use crate::filesystem::NativeFilesystem;

    #[tokio::test]
    async fn rejects_empty_old_string() {
        let mut p = std::env::temp_dir();
        p.push(format!("edit_file_empty_{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&p, "anything").unwrap();
        let tool = EditFile::new(Arc::new(NativeFilesystem::new()));
        let res = tool
            .execute(
                json!({"path": p.display().to_string(), "old_string": "", "new_string": "x"}),
                None,
            )
            .await;
        assert!(res.is_err(), "empty old_string should error");
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn errors_when_old_string_absent() {
        let mut p = std::env::temp_dir();
        p.push(format!("edit_file_absent_{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&p, "hello world\n").unwrap();
        let tool = EditFile::new(Arc::new(NativeFilesystem::new()));
        let res = tool
            .execute(
                json!({
                    "path": p.display().to_string(),
                    "old_string": "no_such_text",
                    "new_string": "x",
                }),
                None,
            )
            .await;
        assert!(res.is_err(), "missing old_string should error");
        // File unchanged.
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello world\n");
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn replaces_once() {
        let mut p = std::env::temp_dir();
        p.push(format!("edit_file_test_{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&p, "hello world\n").unwrap();
        let tool = EditFile::new(Arc::new(NativeFilesystem::new()));
        let out = tool
            .execute(
                json!({
                    "path": p.display().to_string(),
                    "old_string": "world",
                    "new_string": "Rust"
                }),
                None,
            )
            .await
            .unwrap();
        assert_eq!(out["replacements"].as_u64(), Some(1));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello Rust\n");
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn errors_on_multiple_matches() {
        let mut p = std::env::temp_dir();
        p.push(format!("edit_file_dup_{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&p, "a b a").unwrap();
        let tool = EditFile::new(Arc::new(NativeFilesystem::new()));
        let res = tool
            .execute(
                json!({"path": p.display().to_string(), "old_string": "a", "new_string": "x"}),
                None,
            )
            .await;
        assert!(res.is_err());
        // File must be unchanged on validation failure — no partial edit.
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "a b a");
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn rejects_output_growth_past_cap() {
        let mut p = std::env::temp_dir();
        p.push(format!("edit_file_grow_{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&p, "x").unwrap();
        let tool = EditFile::new(Arc::new(NativeFilesystem::new()));
        let res = tool
            .execute(
                json!({
                    "path": p.display().to_string(),
                    "old_string": "x",
                    "new_string": "a".repeat((MAX_FILE_BYTES as usize) + 1),
                    "replace_all": true
                }),
                None,
            )
            .await;
        assert!(res.is_err(), "output over cap should error");
        // File must be unchanged — no oversized write.
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "x");
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn replaces_all_when_flag_set() {
        let mut p = std::env::temp_dir();
        p.push(format!("edit_file_all_{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&p, "a b a").unwrap();
        let tool = EditFile::new(Arc::new(NativeFilesystem::new()));
        let out = tool
            .execute(
                json!({
                    "path": p.display().to_string(),
                    "old_string": "a",
                    "new_string": "x",
                    "replace_all": true
                }),
                None,
            )
            .await
            .unwrap();
        assert_eq!(out["replacements"].as_u64(), Some(2));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "x b x");
        let _ = std::fs::remove_file(p);
    }
}
