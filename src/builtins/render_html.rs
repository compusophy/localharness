//! `render_html` — render an HTML document onto the DISPLAY.
//!
//! The DISPLAY is a pixel framebuffer, not a browser engine, so this is a
//! *snapshot* renderer: it lays out a block-level subset of the HTML
//! (headings, paragraphs, lists) as monochrome text. It cannot run scripts
//! or apply CSS. For an interactive/animated app, use `run_cartridge`
//! instead. This pairs with `create_file` so the agent can write an
//! `index.html` and immediately show it to the user.

use std::sync::Arc;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::Result;
use crate::tools::{Tool, ToolContext};

pub struct RenderHtml;

crate::tool_params! {
    /// ONE table generates both this struct and `input_schema` (see
    /// `crate::tool_params`); the schema byte-identity test is below.
    /// Lenient mode reproduces the historical `.get().and_then(as_str)
    /// .unwrap_or("")` extraction exactly — validation stays in the body.
    struct Args: lenient {
        source: req_str = "the HTML document to render",
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for RenderHtml {
    fn name(&self) -> &str {
        "render_html"
    }

    fn description(&self) -> &str {
        "Render an HTML document onto the visual display (a 512x512 pixel \
         framebuffer the user sees). This is a snapshot text renderer, NOT \
         a browser: it shows block-level text (h1-h6, p, ul/li, blockquote, \
         br) laid out and word-wrapped in a bitmap font, monochrome. It does \
         NOT run JavaScript, apply CSS, or load images — headings just get a \
         bigger font. Pass the full HTML source as `source`. For an \
         interactive or animated app, use `run_cartridge` instead. Tip: pair \
         with create_file to save the same HTML as `index.html` if you want \
         it to persist."
    }

    fn input_schema(&self) -> Value {
        Args::schema()
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let Args { source } = Args::lenient(&args);
        if source.is_empty() {
            return Ok(json!({ "error": "source is required" }));
        }

        #[cfg(all(target_arch = "wasm32", feature = "browser-app"))]
        {
            match crate::app::display::render_html(&source) {
                Ok(()) => Ok(json!({ "status": "rendered on display" })),
                Err(err) => Ok(json!({
                    "error": "render failed",
                    "detail": format!("{err:?}")
                })),
            }
        }

        #[cfg(not(all(target_arch = "wasm32", feature = "browser-app")))]
        {
            Ok(json!({
                "rendered": false,
                "note": "the visual display requires the browser app"
            }))
        }
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
                "source": {
                    "type": "string",
                    "description": "the HTML document to render"
                }
            },
            "required": ["source"]
        });
        assert_eq!(Args::schema().to_string(), frozen.to_string());
    }
}
