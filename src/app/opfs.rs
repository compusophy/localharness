//! OPFS file-browser panel. Reads through the public [`Filesystem`]
//! trait so the same code would work against any backend. All DOM
//! output goes through maud templates → `swap_inner` on fixed ids;
//! the panel itself never builds a node manually.
//!
//! [`Filesystem`]: crate::filesystem::Filesystem

use crate::filesystem::{EntryKind, Filesystem};

use super::dom;
use super::templates;
use super::APP;

/// Re-render the OPFS panel against the current cwd. Safe to call on
/// every chat turn; if OPFS is unavailable we show an error row
/// instead of panicking.
pub(crate) async fn refresh() {
    let cwd = APP.with(|cell| cell.borrow().opfs_cwd.clone());
    let fs = super::shared_opfs();
    let path = cwd_path(&cwd);

    // Breadcrumb first — it doesn't depend on the read succeeding.
    dom::swap_inner(
        "fs-breadcrumb",
        &templates::opfs_breadcrumb(&cwd).into_string(),
    );

    match fs.read_dir(&path).await {
        Ok(mut entries) => {
            // Directories first, then files, alpha within each group.
            entries.sort_by(|a, b| {
                let a_dir = matches!(a.kind, EntryKind::Directory);
                let b_dir = matches!(b.kind, EntryKind::Directory);
                match (a_dir, b_dir) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.name.cmp(&b.name),
                }
            });
            dom::swap_inner(
                "fs-list",
                &templates::opfs_list(&cwd, &entries).into_string(),
            );
        }
        Err(err) => {
            dom::swap_inner(
                "fs-list",
                &templates::opfs_error(&format!("{err}")).into_string(),
            );
        }
    }
}

/// Navigate into a subdirectory and re-render. `target` is the
/// data-arg the click handler captured — interpreted as a `/`-joined
/// path of segment names from the OPFS root (an empty string means
/// "go to root").
pub(crate) async fn navigate(target: &str) {
    let new_cwd: Vec<String> = if target.is_empty() {
        Vec::new()
    } else {
        target
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    };
    APP.with(|cell| cell.borrow_mut().opfs_cwd = new_cwd);
    close_viewer();
    refresh().await;
}

/// Open the named file (relative to cwd). A `.wasm` file is a display
/// cartridge — hand it to the framebuffer loader. Everything else opens
/// in the text editor.
pub(crate) async fn open_file(name: &str) {
    if name.ends_with(".wasm") {
        display_file(name).await
    } else {
        edit_file(name).await
    }
}

/// Read a `.wasm` file from OPFS and run it as a display cartridge in
/// the framebuffer surface.
pub(crate) async fn display_file(name: &str) {
    let (path, display_path) = resolve_path(name);
    let fs = super::shared_opfs();
    match fs.read(&path).await {
        Ok(bytes) => {
            if let Err(err) = super::display::run_wasm(&bytes).await {
                super::dom::set_status(&format!("display {display_path}: {err:?}"), true);
            }
        }
        Err(err) => {
            super::dom::set_status(&format!("display {display_path}: {err}"), true);
        }
    }
}

/// Open the named file in editor mode. Reads up to 1 MiB (larger than
/// the read-only preview cap) because the user may want to edit longer
/// files. Files larger than that won't load — we surface an error
/// instead of letting an editor silently truncate.
pub(crate) async fn edit_file(name: &str) {
    let (path, display_path) = resolve_path(name);
    let fs = super::shared_opfs();
    const MAX_EDIT: usize = 1024 * 1024;

    match fs.read(&path).await {
        Ok(bytes) if bytes.len() > MAX_EDIT => {
            super::dom::set_status(
                &format!(
                    "{display_path}: too large to edit in-tab ({} bytes > {MAX_EDIT})",
                    bytes.len()
                ),
                true,
            );
        }
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            dom::swap_inner(
                "view-content",
                &templates::opfs_editor(&display_path, name, &text).into_string(),
            );
            set_view_collapsed(false);
            // Focus the textarea so the user can start typing immediately.
            if let Some(ta) = dom::textarea_by_id("fs-editor") {
                let _ = ta.focus();
            }
        }
        Err(err) => {
            super::dom::set_status(&format!("edit {display_path}: {err}"), true);
        }
    }
}

/// Write the current editor contents back to OPFS, then re-render the
/// viewer with the saved text.
pub(crate) async fn save_file(name: &str) {
    let Some(editor) = dom::textarea_by_id("fs-editor") else {
        super::dom::set_status("save: editor textarea missing", true);
        return;
    };
    let contents = editor.value();
    let (path, display_path) = resolve_path(name);
    let fs = super::shared_opfs();
    if let Err(err) = fs.write_atomic(&path, contents.as_bytes()).await {
        super::dom::set_status(&format!("save {display_path}: {err}"), true);
        return;
    }
    super::dom::set_status(&format!("saved {display_path} ({} bytes)", contents.len()), false);
    // Re-render the read-only viewer with the freshly-saved contents,
    // and refresh the panel so size shows the new value.
    open_file(name).await;
    refresh().await;
}

/// Build (resolved-OPFS-path, display-path) from a cwd-relative leaf.
fn resolve_path(name: &str) -> (String, String) {
    let cwd = APP.with(|cell| cell.borrow().opfs_cwd.clone());
    let mut path = cwd_path(&cwd);
    if !path.ends_with('/') {
        path.push('/');
    }
    path.push_str(name);
    let display = if cwd.is_empty() {
        format!("/{name}")
    } else {
        format!("/{}/{name}", cwd.join("/"))
    };
    (path, display)
}

/// Walk the OPFS root and delete every top-level entry. Then refresh
/// the panel back to root. Called from the `opfs-wipe` action.
pub(crate) async fn wipe() {
    let fs = super::shared_opfs();
    let entries = match fs.read_dir("").await {
        Ok(es) => es,
        Err(err) => {
            super::dom::set_status(&format!("wipe: {err}"), true);
            return;
        }
    };
    let mut failed: Vec<String> = Vec::new();
    for entry in entries {
        if let Err(err) = fs.delete(&entry.name).await {
            failed.push(format!("{}: {err}", entry.name));
        }
    }
    APP.with(|cell| cell.borrow_mut().opfs_cwd.clear());
    close_viewer();
    refresh().await;
    if failed.is_empty() {
        super::dom::set_status("OPFS wiped.", false);
    } else {
        super::dom::set_status(
            &format!("OPFS partial wipe — {} entries failed", failed.len()),
            true,
        );
    }
}

pub(crate) fn close_viewer() {
    // Stop any running display cartridge loop before tearing down the
    // surface, so an orphaned rAF tick can't keep blitting.
    super::display::stop();
    // Collapse the view panel and clear its content — opening a file
    // again re-renders fresh.
    dom::swap_inner("view-content", "");
    set_view_collapsed(true);
}

/// Toggle the `view-collapsed` class on `#layout`. CSS hides
/// `.view-panel` when this class is present.
pub(crate) fn set_view_collapsed(collapsed: bool) {
    let Some(layout) = dom::by_id("layout") else { return };
    let cls = layout.class_name();
    let parts: Vec<&str> = cls.split_whitespace().collect();
    let has = parts.contains(&"view-collapsed");
    if has == collapsed {
        return;
    }
    let new_cls = if collapsed {
        if parts.is_empty() {
            "view-collapsed".to_string()
        } else {
            format!("{} view-collapsed", parts.join(" "))
        }
    } else {
        parts.iter().filter(|c| **c != "view-collapsed").copied().collect::<Vec<_>>().join(" ")
    };
    layout.set_class_name(&new_cls);
}

fn cwd_path(cwd: &[String]) -> String {
    if cwd.is_empty() {
        "".to_string()
    } else {
        cwd.join("/")
    }
}
