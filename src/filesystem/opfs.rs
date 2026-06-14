//! Browser OPFS-backed implementation of [`Filesystem`].
//!
//! Uses the Origin Private File System exposed via
//! `navigator.storage.getDirectory()`. Each browser tab/origin gets its
//! own private root directory.
//!
//! ## Atomicity
//!
//! `write_atomic` relies on OPFS's `FileSystemWritableFileStream`
//! semantics: writes are buffered to a swap file and the original is
//! atomically replaced on `close()`. A page reload mid-write leaves
//! the original file intact.
//!
//! ## Path handling
//!
//! OPFS has no native path syntax — only handles. This impl splits
//! incoming paths on `/`, drops empty components, and resolves each
//! component as a directory (or final file). Leading slashes are
//! ignored; OPFS-rooted paths and relative paths are equivalent.

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use js_sys::{Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    File, FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemGetDirectoryOptions,
    FileSystemGetFileOptions, FileSystemHandle, FileSystemHandleKind, FileSystemRemoveOptions,
    FileSystemWritableFileStream,
};

use super::{DirEntry, EntryKind, Filesystem, Metadata, WalkEntry};
use crate::error::{Error, Result};

/// Hard cap on entries a single `walk` collects. find_file/search_directory
/// cap their own RESULTS, but they collect the whole walk first, so without
/// this a walk over a huge tree would exhaust memory. Mirrors
/// `NativeFilesystem::MAX_WALK_ENTRIES`; 200k is far beyond any real workspace.
const MAX_WALK_ENTRIES: usize = 200_000;

/// Filesystem backed by the browser's Origin Private File System.
///
/// Cheap to clone: holds an `Rc` to the OPFS root handle once acquired.
#[derive(Debug, Clone, Default)]
pub struct OpfsFilesystem {
    // Cache the OPFS root after first acquisition; getDirectory()
    // returns the same logical handle every time but each call is async.
    root: Rc<RefCell<Option<FileSystemDirectoryHandle>>>,
}

impl OpfsFilesystem {
    pub fn new() -> Self {
        Self::default()
    }

    async fn root_handle(&self) -> Result<FileSystemDirectoryHandle> {
        if let Some(h) = self.root.borrow().as_ref() {
            return Ok(h.clone());
        }
        let window = web_sys::window().ok_or_else(|| Error::other("no window: not in a browser"))?;
        let storage = window.navigator().storage();
        let promise = storage.get_directory();
        let val = JsFuture::from(promise)
            .await
            .map_err(|e| Error::other(format!("getDirectory: {}", js_err(&e))))?;
        let handle: FileSystemDirectoryHandle = val
            .dyn_into()
            .map_err(|_| Error::other("getDirectory: not a FileSystemDirectoryHandle"))?;
        *self.root.borrow_mut() = Some(handle.clone());
        Ok(handle)
    }

    /// Walk a path's parent components, returning the deepest directory
    /// handle and the final segment (`None` if the path resolves to the
    /// root itself).
    async fn resolve_parent(
        &self,
        path: &str,
        create_dirs: bool,
    ) -> Result<(FileSystemDirectoryHandle, Option<String>)> {
        let parts = split_path(path);
        if parts.is_empty() {
            return Ok((self.root_handle().await?, None));
        }
        let mut dir = self.root_handle().await?;
        for component in &parts[..parts.len() - 1] {
            dir = get_subdir(&dir, component, create_dirs).await?;
        }
        Ok((dir, Some(parts.last().unwrap().clone())))
    }

    /// Resolve a path to the directory handle it names (errors if the
    /// path doesn't exist or names a file).
    async fn resolve_dir(&self, path: &str) -> Result<FileSystemDirectoryHandle> {
        let parts = split_path(path);
        let mut dir = self.root_handle().await?;
        for component in &parts {
            dir = get_subdir(&dir, component, false).await?;
        }
        Ok(dir)
    }
}

#[async_trait(?Send)]
impl Filesystem for OpfsFilesystem {
    async fn read(&self, path: &str) -> Result<Vec<u8>> {
        let (parent, name) = self.resolve_parent(path, false).await?;
        let name = name.ok_or_else(|| Error::other(format!("read({path}): path is empty")))?;
        let file_handle = get_file(&parent, &name, false).await?;
        let file_val = JsFuture::from(file_handle.get_file())
            .await
            .map_err(|e| Error::other(format!("getFile({path}): {}", js_err(&e))))?;
        let file: File = file_val
            .dyn_into()
            .map_err(|_| Error::other(format!("getFile({path}): not a File")))?;
        let buf = JsFuture::from(file.array_buffer())
            .await
            .map_err(|e| Error::other(format!("arrayBuffer({path}): {}", js_err(&e))))?;
        let array = Uint8Array::new(&buf);
        Ok(array.to_vec())
    }

    async fn write_atomic(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let (parent, name) = self.resolve_parent(path, true).await?;
        let name =
            name.ok_or_else(|| Error::other(format!("write_atomic({path}): path is empty")))?;
        let file_handle = get_file(&parent, &name, true).await?;
        let writable_val = JsFuture::from(file_handle.create_writable())
            .await
            .map_err(|e| Error::other(format!("createWritable({path}): {}", js_err(&e))))?;
        let writable: FileSystemWritableFileStream = writable_val
            .dyn_into()
            .map_err(|_| Error::other("createWritable: not a writable stream"))?;
        let array = Uint8Array::from(bytes);
        let write_promise = writable
            .write_with_buffer_source(&array)
            .map_err(|e| Error::other(format!("write({path}): {}", js_err(&e))))?;
        JsFuture::from(write_promise)
            .await
            .map_err(|e| Error::other(format!("write({path}): {}", js_err(&e))))?;
        JsFuture::from(writable.close())
            .await
            .map_err(|e| Error::other(format!("close({path}): {}", js_err(&e))))?;
        Ok(())
    }

    async fn metadata(&self, path: &str) -> Result<Option<Metadata>> {
        let (parent, name) = self.resolve_parent(path, false).await?;
        let Some(name) = name else {
            // Path resolved to OPFS root — it's a directory.
            return Ok(Some(Metadata {
                kind: EntryKind::Directory,
                size: 0,
            }));
        };
        // Try file first, then directory.
        match get_file(&parent, &name, false).await {
            Ok(fh) => {
                let file_val = JsFuture::from(fh.get_file())
                    .await
                    .map_err(|e| Error::other(format!("getFile({path}): {}", js_err(&e))))?;
                let file: File = file_val
                    .dyn_into()
                    .map_err(|_| Error::other(format!("getFile({path}): not a File")))?;
                Ok(Some(Metadata {
                    kind: EntryKind::File,
                    size: file.size() as u64,
                }))
            }
            Err(_) => match get_subdir(&parent, &name, false).await {
                Ok(_) => Ok(Some(Metadata {
                    kind: EntryKind::Directory,
                    size: 0,
                })),
                Err(_) => Ok(None),
            },
        }
    }

    async fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        let dir = self.resolve_dir(path).await?;
        let mut entries = collect_entries(&dir).await?;
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    async fn walk(&self, path: &str, max_depth: Option<usize>) -> Result<Vec<WalkEntry>> {
        let root = self.resolve_dir(path).await?;
        let mut out = Vec::new();
        // Root entry itself at depth 0.
        out.push(WalkEntry {
            path: path.trim_end_matches('/').to_string(),
            kind: EntryKind::Directory,
            size: None,
        });
        walk_dir(&root, path, 1, max_depth, &mut out).await?;
        Ok(out)
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let (parent, name) = self.resolve_parent(path, false).await?;
        let name =
            name.ok_or_else(|| Error::other(format!("delete({path}): cannot delete OPFS root")))?;
        let opts = FileSystemRemoveOptions::new();
        opts.set_recursive(true);
        let promise = parent.remove_entry_with_options(&name, &opts);
        JsFuture::from(promise)
            .await
            .map_err(|e| Error::other(format!("removeEntry({path}): {}", js_err(&e))))?;
        Ok(())
    }
}

fn split_path(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .map(|s| s.to_string())
        .collect()
}

async fn get_subdir(
    parent: &FileSystemDirectoryHandle,
    name: &str,
    create: bool,
) -> Result<FileSystemDirectoryHandle> {
    let opts = FileSystemGetDirectoryOptions::new();
    opts.set_create(create);
    let promise = parent.get_directory_handle_with_options(name, &opts);
    let val = JsFuture::from(promise)
        .await
        .map_err(|e| Error::other(format!("getDirectoryHandle({name}): {}", js_err(&e))))?;
    val.dyn_into()
        .map_err(|_| Error::other(format!("getDirectoryHandle({name}): wrong type")))
}

async fn get_file(
    parent: &FileSystemDirectoryHandle,
    name: &str,
    create: bool,
) -> Result<FileSystemFileHandle> {
    let opts = FileSystemGetFileOptions::new();
    opts.set_create(create);
    let promise = parent.get_file_handle_with_options(name, &opts);
    let val = JsFuture::from(promise)
        .await
        .map_err(|e| Error::other(format!("getFileHandle({name}): {}", js_err(&e))))?;
    val.dyn_into()
        .map_err(|_| Error::other(format!("getFileHandle({name}): wrong type")))
}

/// Iterate a directory's entries via the JS async iterator protocol.
async fn collect_entries(dir: &FileSystemDirectoryHandle) -> Result<Vec<DirEntry>> {
    let iter_method =
        Reflect::get(dir, &JsValue::from_str("entries")).map_err(|_| Error::other("entries"))?;
    let iter_fn = iter_method
        .dyn_ref::<js_sys::Function>()
        .ok_or_else(|| Error::other("entries() not callable"))?;
    let iterator = iter_fn
        .call0(dir)
        .map_err(|e| Error::other(format!("entries(): {}", js_err(&e))))?;
    let next_fn = Reflect::get(&iterator, &JsValue::from_str("next"))
        .map_err(|_| Error::other("iterator.next"))?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| Error::other("iterator.next not a function"))?;

    let mut out = Vec::new();
    loop {
        let promise = next_fn
            .call0(&iterator)
            .map_err(|e| Error::other(format!("iterator.next: {}", js_err(&e))))?;
        let result = JsFuture::from(js_sys::Promise::from(promise))
            .await
            .map_err(|e| Error::other(format!("iterator await: {}", js_err(&e))))?;
        let done = Reflect::get(&result, &JsValue::from_str("done"))
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if done {
            break;
        }
        let value = Reflect::get(&result, &JsValue::from_str("value"))
            .map_err(|_| Error::other("iterator value"))?;
        // value is [name, handle] tuple (a 2-element array).
        let pair: js_sys::Array = value
            .dyn_into()
            .map_err(|_| Error::other("entry value not an array"))?;
        let name = pair
            .get(0)
            .as_string()
            .ok_or_else(|| Error::other("entry[0] not a string"))?;
        let handle_val = pair.get(1);
        let handle: FileSystemHandle = handle_val
            .dyn_into()
            .map_err(|_| Error::other("entry[1] not a FileSystemHandle"))?;
        let (kind, size) = match handle.kind() {
            FileSystemHandleKind::File => {
                let fh: FileSystemFileHandle = handle.unchecked_into();
                let file_val = JsFuture::from(fh.get_file())
                    .await
                    .map_err(|e| Error::other(format!("getFile: {}", js_err(&e))))?;
                let file: File = file_val
                    .dyn_into()
                    .map_err(|_| Error::other("getFile: not a File"))?;
                (EntryKind::File, Some(file.size() as u64))
            }
            FileSystemHandleKind::Directory => (EntryKind::Directory, None),
            _ => (EntryKind::Other, None),
        };
        out.push(DirEntry { name, kind, size });
    }
    Ok(out)
}

/// Recursive depth-first walk. `depth` is the depth of `dir` relative
/// to the walk root (root itself is depth 0).
async fn walk_dir(
    dir: &FileSystemDirectoryHandle,
    prefix: &str,
    depth: usize,
    max_depth: Option<usize>,
    out: &mut Vec<WalkEntry>,
) -> Result<()> {
    if let Some(d) = max_depth {
        if depth > d {
            return Ok(());
        }
    }
    let entries = collect_entries(dir).await?;
    for entry in entries {
        // Stop once the global cap is hit (an over-large tree must not
        // exhaust memory) — matches NativeFilesystem's MAX_WALK_ENTRIES.
        if out.len() >= MAX_WALK_ENTRIES {
            return Ok(());
        }
        let path = if prefix.is_empty() || prefix == "/" {
            entry.name.clone()
        } else {
            format!("{}/{}", prefix.trim_end_matches('/'), entry.name)
        };
        match entry.kind {
            EntryKind::File => {
                out.push(WalkEntry {
                    path,
                    kind: EntryKind::File,
                    size: entry.size,
                });
            }
            EntryKind::Directory => {
                out.push(WalkEntry {
                    path: path.clone(),
                    kind: EntryKind::Directory,
                    size: None,
                });
                let sub = get_subdir(dir, &entry.name, false).await?;
                // Recursive async — use Box::pin to avoid infinite future size.
                Box::pin(walk_dir(&sub, &path, depth + 1, max_depth, out)).await?;
            }
            _ => {
                out.push(WalkEntry {
                    path,
                    kind: entry.kind,
                    size: entry.size,
                });
            }
        }
    }
    Ok(())
}

/// Best-effort stringify of a JsValue error.
fn js_err(e: &JsValue) -> String {
    if let Some(s) = e.as_string() {
        return s;
    }
    if let Ok(name) = Reflect::get(e, &JsValue::from_str("name")) {
        if let Ok(msg) = Reflect::get(e, &JsValue::from_str("message")) {
            return format!(
                "{}: {}",
                name.as_string().unwrap_or_default(),
                msg.as_string().unwrap_or_default()
            );
        }
    }
    // Fall back to the Object.prototype.toString form.
    let obj: Object = e.clone().unchecked_into();
    obj.to_string().as_string().unwrap_or_else(|| "<js error>".into())
}
