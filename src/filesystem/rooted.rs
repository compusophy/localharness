//! [`RootedFilesystem`] — confine a [`Filesystem`] to a sub-tree.
//!
//! The bashlite sandbox addresses files as `/`-rooted paths; the native CLI
//! wants those to resolve UNDER a real base directory (the script's dir / the
//! working dir), not the OS root. This wrapper maps every sandbox path `"/x"` to
//! `"<base>/x"` on the way in and strips the base back off `walk` results on the
//! way out, so the inner backend (Native/OPFS) sees real paths while the caller
//! sees only the sandbox. Separator-insensitive (Windows `walkdir` hands back
//! `\`-paths), so it's cross-platform.

use async_trait::async_trait;

use super::{DirEntry, Filesystem, Metadata, SharedFilesystem, WalkEntry};
use crate::error::Result;

/// A [`Filesystem`] confined to `base` within `inner`. Sandbox paths are
/// `/`-rooted; they resolve under `base` in the inner backend.
#[derive(Debug, Clone)]
pub struct RootedFilesystem {
    inner: SharedFilesystem,
    /// The base directory in the INNER backend's address space (no trailing `/`).
    base: String,
}

impl RootedFilesystem {
    /// Confine `inner` to `base` (e.g. an absolute OS dir over `NativeFilesystem`).
    pub fn new(inner: SharedFilesystem, base: impl Into<String>) -> Self {
        let base = base.into().replace('\\', "/");
        let base = base.trim_end_matches('/').to_string();
        Self { inner, base }
    }

    /// Sandbox path `"/x"` → inner path `"<base>/x"`. `"/"` → the base itself.
    ///
    /// Self-defending — RootedFilesystem IS the sandbox boundary, so it does not
    /// trust the caller to hand it a normalized path (M6): `\` is treated as a
    /// separator (else a `..\` component slips past bashlite's `/`-only collapse
    /// and climbs above `base` once `std::path` interprets the `\` on Windows),
    /// and every `..` is collapsed RELATIVE TO THE SANDBOX ROOT — an empty stack
    /// never pops below itself, so the result is structurally confined to `base`
    /// regardless of what the caller passes (default-deny escape).
    fn real(&self, p: &str) -> String {
        let p = p.replace('\\', "/");
        let mut stack: Vec<&str> = Vec::new();
        for comp in p.split('/') {
            match comp {
                "" | "." => {}
                // A `..` at the sandbox root is a no-op — it can never climb
                // above `base`.
                ".." => {
                    stack.pop();
                }
                c => stack.push(c),
            }
        }
        if stack.is_empty() {
            return self.base.clone();
        }
        format!("{}/{}", self.base, stack.join("/"))
    }

    /// Inner path → sandbox path: strip the base prefix (separator-insensitive),
    /// always returning a `/`-rooted path.
    fn virt(&self, real: &str) -> String {
        let r = real.replace('\\', "/");
        match r.strip_prefix(&self.base) {
            None | Some("") => "/".to_string(),
            Some(rest) if rest.starts_with('/') => rest.to_string(),
            Some(rest) => format!("/{rest}"),
        }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Filesystem for RootedFilesystem {
    async fn read(&self, path: &str) -> Result<Vec<u8>> {
        self.inner.read(&self.real(path)).await
    }
    async fn write_atomic(&self, path: &str, bytes: &[u8]) -> Result<()> {
        self.inner.write_atomic(&self.real(path), bytes).await
    }
    async fn metadata(&self, path: &str) -> Result<Option<Metadata>> {
        self.inner.metadata(&self.real(path)).await
    }
    async fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        // DirEntry carries only names (no paths), so no translation needed.
        self.inner.read_dir(&self.real(path)).await
    }
    async fn walk(&self, path: &str, max_depth: Option<usize>) -> Result<Vec<WalkEntry>> {
        let entries = self.inner.walk(&self.real(path), max_depth).await?;
        Ok(entries
            .into_iter()
            .map(|e| WalkEntry { path: self.virt(&e.path), kind: e.kind, size: e.size })
            .collect())
    }
    async fn delete(&self, path: &str) -> Result<()> {
        self.inner.delete(&self.real(path)).await
    }
    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        self.inner.rename(&self.real(from), &self.real(to)).await
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use crate::filesystem::NativeFilesystem;
    use std::sync::Arc;

    #[tokio::test]
    async fn confines_reads_writes_and_walk_to_base() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_string_lossy().to_string();
        let fs = RootedFilesystem::new(Arc::new(NativeFilesystem::new()), base.clone());

        // A sandbox-absolute write lands UNDER base, not at the OS root.
        fs.write_atomic("/sub/a.txt", b"hello").await.unwrap();
        assert_eq!(fs.read("/sub/a.txt").await.unwrap(), b"hello");
        // The real file exists under base.
        assert!(std::path::Path::new(&base).join("sub").join("a.txt").exists());

        // walk results come back as SANDBOX paths (base stripped, `/`-rooted) —
        // never leaking the OS base / drive letter.
        fs.write_atomic("/sub/b.txt", b"x").await.unwrap();
        let paths: Vec<String> =
            fs.walk("/sub", None).await.unwrap().into_iter().map(|e| e.path).collect();
        assert!(paths.iter().all(|p| p.starts_with("/sub") && !p.contains(':')), "{paths:?}");
        assert!(paths.iter().any(|p| p == "/sub/a.txt"));
        assert!(paths.iter().any(|p| p == "/sub/b.txt"));
    }

    #[test]
    fn real_and_virt_round_trip() {
        let fs = RootedFilesystem::new(Arc::new(NativeFilesystem::new()), "/tmp/base/");
        assert_eq!(fs.real("/"), "/tmp/base");
        assert_eq!(fs.real("/x/y"), "/tmp/base/x/y");
        assert_eq!(fs.virt("/tmp/base/x/y"), "/x/y");
        assert_eq!(fs.virt("/tmp/base"), "/");
        // Windows-style backslash results from the inner backend still strip.
        assert_eq!(fs.virt(r"\tmp\base\x"), "/x");
    }

    /// M6: a path component carrying backslashes (or `..`) must never escape
    /// `base` — `\` is normalized to a separator and `..` collapses relative to
    /// the sandbox root, so std::path can't climb above the sandbox on Windows.
    #[test]
    fn dotdot_and_backslash_cannot_escape_base() {
        let fs = RootedFilesystem::new(Arc::new(NativeFilesystem::new()), "/tmp/base/");
        // The exact M6 escape vector: backslash `..` segments stay confined.
        assert_eq!(fs.real(r"..\..\..\..\secret.key"), "/tmp/base/secret.key");
        // Forward-slash `..` is likewise collapsed to the root, never above it.
        assert_eq!(fs.real("/../../etc/passwd"), "/tmp/base/etc/passwd");
        // Mixed separators normalize uniformly under base.
        assert_eq!(fs.real(r"/sub\nested/file"), "/tmp/base/sub/nested/file");
        // Every result stays prefixed by base.
        for p in [r"..\..\x", "/../../y", r"a\..\..\b", "////"] {
            assert!(fs.real(p).starts_with("/tmp/base"), "escaped: {p} -> {}", fs.real(p));
        }
    }
}
