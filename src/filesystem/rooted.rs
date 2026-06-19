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
    fn real(&self, p: &str) -> String {
        if p == "/" || p.is_empty() {
            return self.base.clone();
        }
        let p = if p.starts_with('/') { p.to_string() } else { format!("/{p}") };
        format!("{}{}", self.base, p)
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
}
