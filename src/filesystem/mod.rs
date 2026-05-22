//! Filesystem abstraction for the built-in fs tools.
//!
//! The 6 fs-shaped builtins (`list_directory`, `view_file`, `find_file`,
//! `search_directory`, `create_file`, `edit_file`) call through this
//! trait so the same tool code can target a native OS filesystem
//! ([`NativeFilesystem`]) or, eventually, OPFS in a browser tab. The
//! native impl is gated behind the `native` cargo feature.
//!
//! Paths are UTF-8 strings on the wire (Gemini sends `path` as JSON
//! string) and are passed through verbatim — the impl decides how to
//! resolve them. On native that's `std::path::PathBuf::from(&str)`; on
//! a future OPFS impl it'll be the OPFS path syntax.
//!
//! ## Implementing a custom backend
//!
//! Implement [`Filesystem`] for your type, then hand it to the agent
//! via `GeminiAgentConfig::with_filesystem` — the runtime will register
//! the 6 fs builtins on top of it:
//!
//! ```no_run
//! use std::sync::Arc;
//! use async_trait::async_trait;
//! use localharness::filesystem::{
//!     DirEntry, EntryKind, Filesystem, Metadata, WalkEntry,
//! };
//! use localharness::{GeminiAgentConfig, Result};
//!
//! #[derive(Debug)]
//! struct MyFs;
//!
//! #[async_trait]
//! impl Filesystem for MyFs {
//!     async fn read(&self, _path: &str) -> Result<Vec<u8>> {
//!         Ok(Vec::new())
//!     }
//!     async fn write_atomic(&self, _path: &str, _bytes: &[u8]) -> Result<()> {
//!         Ok(())
//!     }
//!     async fn metadata(&self, _path: &str) -> Result<Option<Metadata>> {
//!         Ok(None)
//!     }
//!     async fn read_dir(&self, _path: &str) -> Result<Vec<DirEntry>> {
//!         Ok(Vec::new())
//!     }
//!     async fn walk(
//!         &self,
//!         _path: &str,
//!         _max_depth: Option<usize>,
//!     ) -> Result<Vec<WalkEntry>> {
//!         Ok(Vec::new())
//!     }
//! }
//!
//! // `Arc<MyFs>` unsize-coerces to `Arc<dyn Filesystem>` automatically.
//! let _cfg = GeminiAgentConfig::new("api-key").with_filesystem(Arc::new(MyFs));
//! ```

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::Result;
use crate::runtime::MaybeSendSync;

#[cfg(feature = "native")]
pub mod native;

#[cfg(feature = "native")]
pub use native::NativeFilesystem;

#[cfg(target_arch = "wasm32")]
pub mod opfs;

#[cfg(target_arch = "wasm32")]
pub use opfs::OpfsFilesystem;

/// What a directory entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

impl EntryKind {
    /// Stable lowercase string the tools surface to the model.
    pub fn as_str(&self) -> &'static str {
        match self {
            EntryKind::File => "file",
            EntryKind::Directory => "directory",
            EntryKind::Symlink => "symlink",
            EntryKind::Other => "other",
        }
    }
}

/// One immediate child of a directory.
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// File name only — no path components.
    pub name: String,
    pub kind: EntryKind,
    /// File size in bytes; `None` for non-files or when unknown.
    pub size: Option<u64>,
}

/// One entry produced by a recursive walk.
#[derive(Debug, Clone)]
pub struct WalkEntry {
    /// Full path joined with the walk root.
    pub path: String,
    pub kind: EntryKind,
    pub size: Option<u64>,
}

/// Lightweight stat result.
#[derive(Debug, Clone)]
pub struct Metadata {
    pub kind: EntryKind,
    pub size: u64,
}

/// The operations the built-in fs tools need from a filesystem.
///
/// Implementations are responsible for normalising errors into
/// [`crate::error::Error`] with a useful message.
///
/// The `Debug` supertrait lets `GeminiBackendConfig` (which now stores
/// an optional `Filesystem`) derive `Debug`; impl authors typically
/// satisfy it with `#[derive(Debug)]` on a marker struct.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait Filesystem: MaybeSendSync + std::fmt::Debug {
    /// Read the full contents of a file as bytes.
    ///
    /// Errors if `path` does not exist, is a directory, or is otherwise
    /// unreadable.
    async fn read(&self, path: &str) -> Result<Vec<u8>>;

    /// Atomically write `bytes` to `path`, creating any missing parent
    /// directories. Replaces any existing file at the destination.
    ///
    /// **Atomicity contract:** a concurrent reader observes either the
    /// full pre-write contents or the full post-write contents — never
    /// a partial / torn write. A crash mid-write must not leave a
    /// partially-written file at `path`. The native impl satisfies this
    /// via tempfile + rename in the destination directory; an OPFS impl
    /// would use the OPFS sync access handle's `truncate + write +
    /// flush` sequence.
    async fn write_atomic(&self, path: &str, bytes: &[u8]) -> Result<()>;

    /// Return metadata for `path`, or `None` if the path does not exist.
    /// Other I/O errors (e.g. permission denied) propagate as `Err`.
    async fn metadata(&self, path: &str) -> Result<Option<Metadata>>;

    /// List the immediate children of `path`, sorted by name.
    ///
    /// Errors if `path` does not exist or is not a directory.
    async fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>>;

    /// Recursively walk `path`, returning every entry under it (files
    /// and directories). Symlinks are not followed. If `max_depth` is
    /// `Some(d)`, recursion is limited to depth `d` (the root itself is
    /// depth 0). Implementations may return entries in any order.
    async fn walk(&self, path: &str, max_depth: Option<usize>) -> Result<Vec<WalkEntry>>;
}

/// Type alias for a shared filesystem handle.
pub type SharedFilesystem = Arc<dyn Filesystem>;

/// Last path component of `p` (after the final `/` or `\`).
///
/// Used by `find_file` and `search_directory` to apply glob filters
/// against just the file name, independent of the directory chain
/// produced by [`Filesystem::walk`].
pub(crate) fn file_name(p: &str) -> &str {
    match p.rfind(['/', '\\']) {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

#[cfg(test)]
mod tests {
    use super::file_name;

    #[test]
    fn file_name_no_separator() {
        assert_eq!(file_name("foo.rs"), "foo.rs");
    }

    #[test]
    fn file_name_unix_separator() {
        assert_eq!(file_name("a/b/c.rs"), "c.rs");
    }

    #[test]
    fn file_name_windows_separator() {
        assert_eq!(file_name(r"a\b\c.rs"), "c.rs");
    }

    #[test]
    fn file_name_mixed_separators() {
        assert_eq!(file_name(r"C:\proj/sub\file.rs"), "file.rs");
    }

    #[test]
    fn file_name_empty_string() {
        assert_eq!(file_name(""), "");
    }

    #[test]
    fn file_name_trailing_separator_returns_empty() {
        // A path ending in a separator has no file component.
        assert_eq!(file_name("a/b/"), "");
    }
}
