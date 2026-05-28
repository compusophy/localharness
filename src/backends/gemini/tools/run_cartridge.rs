//! `run_cartridge` — compile rustlite source and run it on the DISPLAY.
//!
//! This is the agent→display loop: the agent writes a rustlite cartridge
//! and this tool compiles it in-browser and hands the wasm to the
//! framebuffer (`crate::app::display`), where it draws live pixels. Unlike
//! `compile_rustlite` (which runs through the headless loader and returns
//! an i32), this puts the cartridge on screen.

use std::sync::Arc;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::Result;
use crate::tools::{Tool, ToolContext};

pub struct RunCartridge;

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for RunCartridge {
    fn name(&self) -> &str {
        "run_cartridge"
    }

    fn description(&self) -> &str {
        "Compile rustlite source into a display cartridge and run it on \
         the visual display (a pixel framebuffer the user sees). The \
         cartridge must export `fn frame(t: i32)` (animated; `t` is \
         elapsed milliseconds, driven each frame) or `fn render()` \
         (one-shot), and draw using the host::display API: \
         `use host::display;` then `display::clear(rgb)`, \
         `display::fill_rect(x, y, w, h, rgb)`, `display::set_pixel(x, y, rgb)`, \
         `display::present()` (flush to screen), `display::width()`, \
         `display::height()`, `display::pointer_x()`, `display::pointer_y()` \
         (cursor in framebuffer coords). Colors are 0xRRGGBB integers \
         (e.g. 16777215 = white, 0 = black). The framebuffer is 256x144. \
         Always call display::present() at the end of frame/render."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "rustlite source code for the cartridge"
                }
            },
            "required": ["source"]
        })
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let source = args.get("source").and_then(|v| v.as_str()).unwrap_or("");
        if source.is_empty() {
            return Ok(json!({ "error": "source is required" }));
        }

        let wasm_bytes = match crate::rustlite::compile(source) {
            Ok(bytes) => bytes,
            Err(err) => {
                return Ok(json!({
                    "error": "compilation failed",
                    "detail": err.to_string()
                }));
            }
        };

        #[cfg(all(target_arch = "wasm32", feature = "browser-app"))]
        {
            match crate::app::display::run_wasm(&wasm_bytes).await {
                Ok(()) => Ok(json!({
                    "status": "running on display",
                    "wasm_size": wasm_bytes.len()
                })),
                Err(err) => Ok(json!({
                    "error": "run failed",
                    "detail": format!("{err:?}"),
                    "wasm_size": wasm_bytes.len()
                })),
            }
        }

        #[cfg(not(all(target_arch = "wasm32", feature = "browser-app")))]
        {
            Ok(json!({
                "compiled": true,
                "wasm_size": wasm_bytes.len(),
                "note": "the visual display requires the browser app"
            }))
        }
    }
}
