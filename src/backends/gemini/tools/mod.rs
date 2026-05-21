//! Built-in tools the Gemini backend exposes to the model.
//!
//! Each tool implements [`Tool`] and is registered into a
//! [`ToolRunner`] by [`register_builtins`] according to the
//! [`CapabilitiesConfig`]. The runtime declares the tools to Gemini via
//! `FunctionDeclaration`s built from `Tool::input_schema()`.

use std::sync::Arc;

use crate::tools::{Tool, ToolRunner};
use crate::types::{BuiltinTool, CapabilitiesConfig};

mod find_file;
mod finish;
mod list_directory;
mod search_directory;
mod view_file;

pub use find_file::FindFile;
pub use finish::{Finish, FINISH_TOOL_NAME};
pub use list_directory::ListDirectory;
pub use search_directory::SearchDirectory;
pub use view_file::ViewFile;

/// Register the enabled built-in tools into `runner` based on
/// `capabilities.effective_tools()`.
///
/// Returns the set of tool names that were registered, for diagnostics.
pub fn register_builtins(runner: &ToolRunner, capabilities: &CapabilitiesConfig) -> Vec<String> {
    let enabled = capabilities.effective_tools();
    let mut registered = Vec::new();
    for tool in BuiltinTool::ALL {
        if !enabled.contains(tool) {
            continue;
        }
        let boxed: Option<Arc<dyn Tool>> = match tool {
            BuiltinTool::ListDirectory => Some(Arc::new(ListDirectory)),
            BuiltinTool::ViewFile => Some(Arc::new(ViewFile)),
            BuiltinTool::FindFile => Some(Arc::new(FindFile)),
            BuiltinTool::SearchDirectory => Some(Arc::new(SearchDirectory)),
            BuiltinTool::Finish => Some(Arc::new(Finish)),
            // Phase 3+ tools — not implemented yet.
            BuiltinTool::CreateFile
            | BuiltinTool::EditFile
            | BuiltinTool::RunCommand
            | BuiltinTool::AskQuestion
            | BuiltinTool::StartSubagent
            | BuiltinTool::GenerateImage => None,
        };
        if let Some(t) = boxed {
            let name = t.name().to_string();
            // Don't overwrite a user-registered tool of the same name.
            let existing = runner.names();
            if !existing.iter().any(|n| n == &name) {
                runner.register(t);
                registered.push(name);
            }
        }
    }
    registered
}
