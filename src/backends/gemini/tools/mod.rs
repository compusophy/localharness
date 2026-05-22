//! Built-in tools the Gemini backend exposes to the model.
//!
//! Each tool implements [`Tool`] and is registered into a
//! [`ToolRunner`] by [`register_builtins`] according to the
//! [`CapabilitiesConfig`]. The runtime declares the tools to Gemini via
//! `FunctionDeclaration`s built from `Tool::input_schema()`.

use std::sync::Arc;

use crate::backends::gemini::api::SharedClient;
use crate::tools::{Tool, ToolRunner};
use crate::types::{BuiltinTool, CapabilitiesConfig};

mod ask_question;
#[cfg(feature = "native")]
mod create_file;
#[cfg(feature = "native")]
mod edit_file;
#[cfg(feature = "native")]
mod find_file;
mod finish;
mod generate_image;
#[cfg(feature = "native")]
mod list_directory;
#[cfg(feature = "native")]
mod run_command;
#[cfg(feature = "native")]
mod search_directory;
mod start_subagent;
#[cfg(feature = "native")]
mod view_file;

pub use ask_question::AskQuestion;
#[cfg(feature = "native")]
pub use create_file::CreateFile;
#[cfg(feature = "native")]
pub use edit_file::EditFile;
#[cfg(feature = "native")]
pub use find_file::FindFile;
pub use finish::{Finish, FINISH_TOOL_NAME};
pub use generate_image::GenerateImage;
#[cfg(feature = "native")]
pub use list_directory::ListDirectory;
#[cfg(feature = "native")]
pub use run_command::RunCommand;
#[cfg(feature = "native")]
pub use search_directory::SearchDirectory;
pub use start_subagent::StartSubagent;
#[cfg(feature = "native")]
pub use view_file::ViewFile;

/// Construction dependencies the built-in tools optionally need.
///
/// * `chat_client` + `chat_model` — used by `start_subagent`.
/// * `image_client` + `image_model` — used by `generate_image`.
pub struct BuiltinDeps {
    pub chat_client: Option<SharedClient>,
    pub chat_model: String,
    pub image_client: Option<SharedClient>,
    pub image_model: String,
}

/// Register the enabled built-in tools into `runner` based on
/// `capabilities.effective_tools()`. Returns the names registered.
pub fn register_builtins(
    runner: &ToolRunner,
    capabilities: &CapabilitiesConfig,
    #[cfg_attr(not(feature = "native"), allow(unused_variables))]
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
            other => instantiate_native_tool(*other),
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
fn instantiate_native_tool(tool: BuiltinTool) -> Option<Arc<dyn Tool>> {
    match tool {
        BuiltinTool::ListDirectory => Some(Arc::new(ListDirectory)),
        BuiltinTool::ViewFile => Some(Arc::new(ViewFile)),
        BuiltinTool::FindFile => Some(Arc::new(FindFile)),
        BuiltinTool::SearchDirectory => Some(Arc::new(SearchDirectory)),
        BuiltinTool::CreateFile => Some(Arc::new(CreateFile)),
        BuiltinTool::EditFile => Some(Arc::new(EditFile)),
        BuiltinTool::RunCommand => Some(Arc::new(RunCommand)),
        _ => None,
    }
}

#[cfg(not(feature = "native"))]
fn instantiate_native_tool(_tool: BuiltinTool) -> Option<Arc<dyn Tool>> {
    None
}
