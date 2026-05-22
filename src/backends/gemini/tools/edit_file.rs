//! `edit_file` — replace a string in a file with another string.
//!
//! Matches Python's `edit_file` semantics: `old_string` must appear
//! exactly once. Atomic write via tempfile + rename.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tempfile::NamedTempFile;

use crate::error::{Error, Result};
use crate::tools::{Tool, ToolContext};

pub struct EditFile;

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
        let path = PathBuf::from(&args.path);

        if args.old_string.is_empty() {
            return Err(Error::other("old_string must not be empty"));
        }

        let original = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| Error::other(format!("read({}): {e}", path.display())))?;

        let count = original.matches(&args.old_string).count();
        if count == 0 {
            return Err(Error::other(format!(
                "old_string not found in {}",
                path.display()
            )));
        }
        if count > 1 && !args.replace_all {
            return Err(Error::other(format!(
                "old_string found {count} times in {} (need exactly 1, or set replace_all=true)",
                path.display()
            )));
        }

        let updated = if args.replace_all {
            original.replace(&args.old_string, &args.new_string)
        } else {
            // Replace exactly once.
            original.replacen(&args.old_string, &args.new_string, 1)
        };
        let replacements = count;
        let bytes = updated.into_bytes();
        let path_for_task = path.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let parent = path_for_task
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::Path::new("."));
            let mut tmp = NamedTempFile::new_in(parent)
                .map_err(|e| Error::other(format!("tempfile: {e}")))?;
            tmp.write_all(&bytes)
                .map_err(|e| Error::other(format!("write: {e}")))?;
            tmp.persist(&path_for_task)
                .map_err(|e| Error::other(format!("rename: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| Error::other(format!("edit_file join: {e}")))??;

        Ok(json!({
            "ok": true,
            "path": path.display().to_string(),
            "replacements": replacements,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn replaces_once() {
        let mut p = std::env::temp_dir();
        p.push(format!("edit_file_test_{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&p, "hello world\n").unwrap();
        let tool = EditFile;
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
        let tool = EditFile;
        let res = tool
            .execute(
                json!({"path": p.display().to_string(), "old_string": "a", "new_string": "x"}),
                None,
            )
            .await;
        assert!(res.is_err());
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn replaces_all_when_flag_set() {
        let mut p = std::env::temp_dir();
        p.push(format!("edit_file_all_{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&p, "a b a").unwrap();
        let tool = EditFile;
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
