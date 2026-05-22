//! Native OS filesystem implementation of [`Filesystem`].
//!
//! Wraps `tokio::fs` for the async surface and uses `spawn_blocking`
//! around `walkdir` / `tempfile` so synchronous traversal and atomic
//! writes don't block the async runtime.

use std::io;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tempfile::NamedTempFile;
use walkdir::WalkDir;

use super::{DirEntry, EntryKind, Filesystem, Metadata, WalkEntry};
use crate::error::{Error, Result};

/// Filesystem backed by the host operating system.
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeFilesystem;

impl NativeFilesystem {
    pub fn new() -> Self {
        Self
    }
}

fn classify(meta: &std::fs::Metadata, file_type: std::fs::FileType) -> EntryKind {
    if file_type.is_symlink() {
        EntryKind::Symlink
    } else if meta.is_dir() {
        EntryKind::Directory
    } else if meta.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}

fn classify_meta_only(meta: &std::fs::Metadata) -> EntryKind {
    let ft = meta.file_type();
    if ft.is_symlink() {
        EntryKind::Symlink
    } else if meta.is_dir() {
        EntryKind::Directory
    } else if meta.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}

#[async_trait]
impl Filesystem for NativeFilesystem {
    async fn read(&self, path: &str) -> Result<Vec<u8>> {
        let p = PathBuf::from(path);
        tokio::fs::read(&p)
            .await
            .map_err(|e| Error::other(format!("read({}): {e}", p.display())))
    }

    async fn write_atomic(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let target = PathBuf::from(path);
        let parent: Option<PathBuf> = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf);
        let owned = bytes.to_vec();

        tokio::task::spawn_blocking(move || -> Result<()> {
            if let Some(p) = &parent {
                std::fs::create_dir_all(p)
                    .map_err(|e| Error::other(format!("create_dir_all({}): {e}", p.display())))?;
            }
            let dir = parent.as_deref().unwrap_or(Path::new("."));
            let mut tmp = NamedTempFile::new_in(dir)
                .map_err(|e| Error::other(format!("tempfile in {}: {e}", dir.display())))?;
            tmp.write_all(&owned)
                .map_err(|e| Error::other(format!("write: {e}")))?;
            tmp.persist(&target)
                .map_err(|e| Error::other(format!("rename to {}: {e}", target.display())))?;
            Ok(())
        })
        .await
        .map_err(|e| Error::other(format!("write_atomic join: {e}")))?
    }

    async fn metadata(&self, path: &str) -> Result<Option<Metadata>> {
        let p = PathBuf::from(path);
        match tokio::fs::metadata(&p).await {
            Ok(meta) => {
                let kind = classify_meta_only(&meta);
                Ok(Some(Metadata {
                    kind,
                    size: meta.len(),
                }))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::other(format!("metadata({}): {e}", p.display()))),
        }
    }

    async fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        let p = PathBuf::from(path);
        let mut read = tokio::fs::read_dir(&p)
            .await
            .map_err(|e| Error::other(format!("read_dir({}): {e}", p.display())))?;
        let mut entries: Vec<DirEntry> = Vec::new();
        while let Some(entry) = read
            .next_entry()
            .await
            .map_err(|e| Error::other(format!("next_entry: {e}")))?
        {
            let meta = entry
                .metadata()
                .await
                .map_err(|e| Error::other(format!("metadata: {e}")))?;
            let ft = meta.file_type();
            let kind = classify(&meta, ft);
            let size = if matches!(kind, EntryKind::File) {
                Some(meta.len())
            } else {
                None
            };
            entries.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                kind,
                size,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    async fn walk(&self, path: &str, max_depth: Option<usize>) -> Result<Vec<WalkEntry>> {
        let root = PathBuf::from(path);
        let result = tokio::task::spawn_blocking(move || -> Vec<WalkEntry> {
            let mut walker = WalkDir::new(&root).follow_links(false);
            if let Some(d) = max_depth {
                walker = walker.max_depth(d);
            }
            let mut out = Vec::new();
            for entry in walker.into_iter().filter_map(|e| e.ok()) {
                let ft = entry.file_type();
                let kind = if ft.is_symlink() {
                    EntryKind::Symlink
                } else if ft.is_dir() {
                    EntryKind::Directory
                } else if ft.is_file() {
                    EntryKind::File
                } else {
                    EntryKind::Other
                };
                let size = if matches!(kind, EntryKind::File) {
                    entry.metadata().ok().map(|m| m.len())
                } else {
                    None
                };
                out.push(WalkEntry {
                    path: entry.path().display().to_string(),
                    kind,
                    size,
                });
            }
            out
        })
        .await
        .map_err(|e| Error::other(format!("walk join: {e}")))?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("lh_nfs_{label}_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn touch(dir: &Path, rel: &str, content: &[u8]) {
        let mut p = dir.to_path_buf();
        for part in rel.split('/') {
            p.push(part);
        }
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
    }

    #[tokio::test]
    async fn metadata_returns_none_for_missing_path() {
        let fs = NativeFilesystem::new();
        let res = fs
            .metadata("/definitely/does/not/exist/lh-nfs-test-zzz")
            .await
            .unwrap();
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn metadata_reports_size_and_kind_for_file() {
        let dir = unique_dir("meta");
        touch(&dir, "x.txt", b"abcdef");
        let fs = NativeFilesystem::new();
        let meta = fs
            .metadata(&dir.join("x.txt").display().to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(meta.kind, EntryKind::File);
        assert_eq!(meta.size, 6);

        let dir_meta = fs
            .metadata(&dir.display().to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(dir_meta.kind, EntryKind::Directory);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn read_returns_full_bytes() {
        let dir = unique_dir("read");
        touch(&dir, "blob.bin", &[0u8, 1, 2, 3, 255]);
        let fs = NativeFilesystem::new();
        let bytes = fs
            .read(&dir.join("blob.bin").display().to_string())
            .await
            .unwrap();
        assert_eq!(bytes, vec![0, 1, 2, 3, 255]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn write_atomic_creates_parent_dirs_and_replaces() {
        let dir = unique_dir("write");
        let target = dir.join("a/b/c.txt");
        let fs = NativeFilesystem::new();
        fs.write_atomic(&target.display().to_string(), b"first")
            .await
            .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"first");

        // Overwrites the existing file.
        fs.write_atomic(&target.display().to_string(), b"second")
            .await
            .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"second");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn read_dir_sorts_by_name() {
        let dir = unique_dir("sort");
        touch(&dir, "c", b"");
        touch(&dir, "a", b"");
        touch(&dir, "b", b"");
        let fs = NativeFilesystem::new();
        let entries = fs.read_dir(&dir.display().to_string()).await.unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn read_dir_carries_size_for_files_only() {
        let dir = unique_dir("size");
        touch(&dir, "file.txt", b"hello");
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        let fs = NativeFilesystem::new();
        let entries = fs.read_dir(&dir.display().to_string()).await.unwrap();
        let file = entries.iter().find(|e| e.name == "file.txt").unwrap();
        let inner = entries.iter().find(|e| e.name == "inner").unwrap();
        assert_eq!(file.size, Some(5));
        assert_eq!(file.kind, EntryKind::File);
        assert_eq!(inner.size, None);
        assert_eq!(inner.kind, EntryKind::Directory);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn walk_with_max_depth_caps_recursion() {
        let dir = unique_dir("walk");
        touch(&dir, "top.txt", b"");
        touch(&dir, "a/mid.txt", b"");
        touch(&dir, "a/b/deep.txt", b"");
        let fs = NativeFilesystem::new();

        let all = fs
            .walk(&dir.display().to_string(), Some(2))
            .await
            .unwrap();
        // depth 0 = root dir; depth 1 = top.txt + a; depth 2 = a/mid.txt + a/b.
        // a/b/deep.txt (depth 3) excluded.
        let deep_visible = all.iter().any(|e| e.path.ends_with("deep.txt"));
        assert!(!deep_visible, "max_depth=2 should hide depth-3 entries");

        let unbounded = fs.walk(&dir.display().to_string(), None).await.unwrap();
        assert!(unbounded.iter().any(|e| e.path.ends_with("deep.txt")));
        std::fs::remove_dir_all(&dir).ok();
    }
}
