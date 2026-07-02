//! `rename_file` — rename or move a file / directory.
//!
//! Wraps [`Filesystem::rename`]. Native backend uses an atomic
//! `std::fs::rename`; OPFS falls back to read + write + delete via
//! the default trait impl.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::filesystem::{EntryKind, SharedFilesystem};
use crate::tools::{Tool, ToolContext};

pub struct RenameFile {
    fs: SharedFilesystem,
}

impl RenameFile {
    pub fn new(fs: SharedFilesystem) -> Self {
        Self { fs }
    }
}

crate::tool_params! {
    /// ONE table generates both this struct and `input_schema` (see
    /// `crate::tool_params`); the schema byte-identity test is below.
    struct Args: serde {
        from: req_str = "Current path.",
        to: req_str = "New path.",
    }
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
        Args::schema()
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
        // Renaming a DIRECTORY relocates everything under it, so renaming an
        // ancestor of the seed moves `.lh_wallet` even though the from/to
        // basename checks above pass — the same identity brick. Walk `from`
        // when it's a directory and refuse if it CONTAINS a protected file.
        // (Best-effort, like the existence check below.) (I8)
        if matches!(self.fs.metadata(&args.from).await, Ok(Some(m)) if m.kind == EntryKind::Directory) {
            if let Ok(entries) = self.fs.walk(&args.from, None).await {
                if let Some(hit) = entries
                    .iter()
                    .find(|e| crate::builtins::is_protected_path(&e.path))
                {
                    return Err(crate::builtins::protected_path_error(&hit.path));
                }
            }
        }
        // Refuse to SILENTLY clobber an existing destination — native rename
        // overwrites by platform default, which is irreversible data loss and
        // rename_file is not confirm-gated. Best-effort existence check (a
        // transient metadata error falls through to the rename, as before); the
        // caller deletes `to` first to intentionally overwrite.
        if matches!(self.fs.metadata(&args.to).await, Ok(Some(_))) {
            return Err(Error::other(format!(
                "destination '{}' already exists — delete it first to overwrite",
                args.to
            )));
        }
        self.fs.rename(&args.from, &args.to).await?;
        Ok(json!({ "ok": true, "from": args.from, "to": args.to }))
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
                "from": { "type": "string", "description": "Current path." },
                "to":   { "type": "string", "description": "New path." }
            },
            "required": ["from", "to"]
        });
        assert_eq!(Args::schema().to_string(), frozen.to_string());
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

    #[tokio::test]
    async fn refuses_to_clobber_existing_destination() {
        // Native rename overwrites by platform default — silent, irreversible
        // data loss, and rename_file isn't confirm-gated. It must refuse instead.
        let dir = std::env::temp_dir();
        let from = dir.join(format!("rn_from_{}.txt", uuid::Uuid::new_v4()));
        let to = dir.join(format!("rn_to_{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&from, "SOURCE").unwrap();
        std::fs::write(&to, "IMPORTANT").unwrap();
        let tool = RenameFile::new(Arc::new(NativeFilesystem::new()));
        let res = tool
            .execute(
                json!({"from": from.display().to_string(), "to": to.display().to_string()}),
                None,
            )
            .await;
        assert!(res.is_err(), "must refuse to clobber an existing destination");
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "IMPORTANT", "dest untouched");
        assert_eq!(std::fs::read_to_string(&from).unwrap(), "SOURCE", "source untouched");
        let _ = std::fs::remove_file(from);
        let _ = std::fs::remove_file(to);
    }

    /// Renaming a DIRECTORY that contains a protected identity file relocates
    /// the seed — the from/to basename guards don't see nested contents, so it
    /// must be refused. (I8)
    #[tokio::test]
    async fn refuses_to_rename_a_dir_containing_the_seed() {
        let base = std::env::temp_dir().join(format!("rn_dir_{}", uuid::Uuid::new_v4()));
        let from = base.join("from");
        std::fs::create_dir_all(&from).unwrap();
        let seed = from.join(".lh_wallet");
        std::fs::write(&seed, b"SECRET SEED PHRASE").unwrap();
        let to = base.join("to");
        let tool = RenameFile::new(Arc::new(NativeFilesystem::new()));
        let res = tool
            .execute(
                json!({"from": from.display().to_string(), "to": to.display().to_string()}),
                None,
            )
            .await;
        assert!(res.is_err(), "must refuse to rename a dir holding the seed");
        assert!(seed.exists(), "seed must stay put after the refused rename");
        assert!(!to.exists(), "destination must not be created");
        std::fs::remove_dir_all(&base).ok();
    }
}
