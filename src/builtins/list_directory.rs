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
            // Hide the seed/device-key files — the agent has no business seeing
            // them, and not surfacing them shrinks the prompt-injection surface.
            .filter(|e| !crate::builtins::PROTECTED_FILES.contains(&e.name.as_str()))
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
        // Use an ISOLATED tempdir, NOT the shared OS temp_dir: listing the global
        // temp dir raced other processes churning files in it (an entry vanished
        // between readdir and its stat → the read errored mid-listing), which flaked
        // this test under CI contention. An owned tempdir has stable contents.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"hi").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let tool = ListDirectory::new(Arc::new(NativeFilesystem::new()));
        let out = tool
            .execute(json!({"path": dir.path().display().to_string()}), None)
            .await
            .unwrap();
        let names: Vec<&str> = out["entries"]
            .as_array()
            .expect("entries should be an array")
            .iter()
            .filter_map(|e| e["name"].as_str())
            .collect();
        assert!(names.contains(&"a.txt"), "should list the file: {names:?}");
        assert!(names.contains(&"sub"), "should list the subdir: {names:?}");
        assert_eq!(out["count"].as_u64(), Some(2));
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
