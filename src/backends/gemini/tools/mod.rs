//! Built-in tools the Gemini backend exposes to the model.
//!
//! Each tool implements [`Tool`] and is registered into a
//! [`ToolRunner`] by [`register_builtins`] according to the
//! [`CapabilitiesConfig`]. The runtime declares the tools to Gemini via
//! `FunctionDeclaration`s built from `Tool::input_schema()`.

use std::sync::Arc;

use crate::backends::gemini::api::SharedClient;
use crate::filesystem::SharedFilesystem;
use crate::tools::{Tool, ToolRunner};
use crate::types::{BuiltinTool, CapabilitiesConfig};

mod ask_question;
mod call_agent;
mod compile_rustlite;
mod create_file;
mod delete_file;
mod edit_file;
mod find_file;
mod finish;
mod generate_image;
mod list_directory;
mod rename_file;
mod run_cartridge;
#[cfg(feature = "native")]
mod run_command;
mod search_directory;
mod start_subagent;
mod view_file;

pub use ask_question::AskQuestion;
pub use create_file::CreateFile;
pub use delete_file::DeleteFile;
pub use edit_file::EditFile;
pub use find_file::FindFile;
pub use finish::{Finish, FINISH_TOOL_NAME};
pub use generate_image::GenerateImage;
pub use list_directory::ListDirectory;
pub use rename_file::RenameFile;
#[cfg(feature = "native")]
pub use run_command::RunCommand;
pub use search_directory::SearchDirectory;
pub use start_subagent::StartSubagent;
pub use view_file::ViewFile;

/// Construction dependencies the built-in tools optionally need.
///
/// * `chat_client` + `chat_model` — used by `start_subagent`.
/// * `image_client` + `image_model` — used by `generate_image`.
/// * `fs` — used by the 6 filesystem builtins (list_directory, view_file,
///   find_file, search_directory, create_file, edit_file). If `None`,
///   those builtins are skipped.
pub struct BuiltinDeps {
    pub chat_client: Option<SharedClient>,
    pub chat_model: String,
    pub image_client: Option<SharedClient>,
    pub image_model: String,
    pub fs: Option<SharedFilesystem>,
}

/// Construct an `Arc<dyn Tool>` of `$ty` if a filesystem is present in
/// `$deps.fs`. The fs-shaped builtins all share the same constructor
/// shape (`T::new(SharedFilesystem)`), so the macro keeps the match arm
/// for each one to a single line.
macro_rules! fs_tool {
    ($deps:expr, $ty:ident) => {
        $deps
            .fs
            .as_ref()
            .map(|fs| Arc::new($ty::new(fs.clone())) as Arc<dyn Tool>)
    };
}

/// Register the enabled built-in tools into `runner` based on
/// `capabilities.effective_tools()`. Returns the names registered.
pub fn register_builtins(
    runner: &ToolRunner,
    capabilities: &CapabilitiesConfig,
    deps: &BuiltinDeps,
) -> Vec<String> {
    let enabled = capabilities.effective_tools();
    let mut registered = Vec::new();
    for tool in BuiltinTool::ALL {
        if !enabled.contains(tool) {
            continue;
        }
        let boxed: Option<Arc<dyn Tool>> = match tool {
            BuiltinTool::Finish => Some(Arc::new(Finish)),
            BuiltinTool::AskQuestion => Some(Arc::new(AskQuestion)),
            BuiltinTool::GenerateImage => deps.image_client.as_ref().map(|c| {
                Arc::new(GenerateImage::new(c.clone(), deps.image_model.clone())) as Arc<dyn Tool>
            }),
            BuiltinTool::StartSubagent => deps.chat_client.as_ref().map(|c| {
                Arc::new(StartSubagent::new(c.clone(), deps.chat_model.clone())) as Arc<dyn Tool>
            }),
            BuiltinTool::ListDirectory => fs_tool!(deps, ListDirectory),
            BuiltinTool::ViewFile => fs_tool!(deps, ViewFile),
            BuiltinTool::FindFile => fs_tool!(deps, FindFile),
            BuiltinTool::SearchDirectory => fs_tool!(deps, SearchDirectory),
            BuiltinTool::CreateFile => fs_tool!(deps, CreateFile),
            BuiltinTool::EditFile => fs_tool!(deps, EditFile),
            BuiltinTool::DeleteFile => fs_tool!(deps, DeleteFile),
            BuiltinTool::RenameFile => fs_tool!(deps, RenameFile),
            BuiltinTool::CallAgent => Some(Arc::new(call_agent::CallAgent) as Arc<dyn Tool>),
            BuiltinTool::CompileRustlite => Some(Arc::new(compile_rustlite::CompileRustlite) as Arc<dyn Tool>),
            BuiltinTool::RunCartridge => Some(Arc::new(run_cartridge::RunCartridge) as Arc<dyn Tool>),
            BuiltinTool::RunCommand => instantiate_run_command(),
        };
        if let Some(t) = boxed {
            let name = t.name().to_string();
            let existing = runner.names();
            if !existing.iter().any(|n| n == &name) {
                runner.register(t);
                registered.push(name);
            }
        }
    }
    registered
}

#[cfg(feature = "native")]
fn instantiate_run_command() -> Option<Arc<dyn Tool>> {
    Some(Arc::new(RunCommand))
}

#[cfg(not(feature = "native"))]
fn instantiate_run_command() -> Option<Arc<dyn Tool>> {
    None
}
