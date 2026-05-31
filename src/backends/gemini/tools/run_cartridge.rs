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
         the visual display (a 256x144 pixel framebuffer the user sees). \
         The cartridge must export `fn frame(t: i32)` (animated; `t` is \
         elapsed milliseconds, driven every frame) or `fn render()` \
         (one-shot). Start with `use host::display;`. Drawing: \
         clear(rgb), fill_rect(x,y,w,h,rgb), set_pixel(x,y,rgb), \
         draw_char(x,y,codepoint,rgb,scale) (one 5x7 glyph; codepoint is \
         an ASCII code like 65 for 'A'; scale 1..n), \
         draw_number(x,y,value,rgb,scale) (renders a decimal integer), \
         present() (flush to screen — call last). Layout/info: width(), \
         height(). Input (poll each frame): pointer_x(), pointer_y() \
         (cursor in framebuffer coords), pointer_down() (1 while pressed). \
         State across frames (rustlite has no globals): state_get(slot) \
         and state_set(slot,value) give 64 integer slots that persist — \
         use these to hold app state like a calculator's accumulator. \
         Colors are 0xRRGGBB integers (16777215 = white, 0 = black). \
         Fonts cover 0-9, A-Z, space, and + - * / = . ( ). To build a \
         clickable button: fill_rect for the box, draw_char/draw_number \
         for the label, and each frame check if pointer_down() and the \
         pointer is inside the box. Each run is auto-saved to `cartridge.rl` \
         so it shows up in the files panel and survives a reload (re-open it \
         from files to run it again). This is the tool to use whenever the \
         user wants to build, run, or see a visual app — it launches live \
         on the DISPLAY, no reload and no fullscreen takeover. ONLY when the \
         user EXPLICITLY asks to make this subdomain PERMANENTLY BECOME the \
         app (boot straight into it fullscreen on every load, no IDE chrome) \
         should you ALSO save the exact same source to a file named `app.rl` \
         with create_file. Do not write `app.rl` for an ordinary \
         'build/show me an app' request — that opts the user into a \
         fullscreen takeover they didn't ask for, and it won't run until the \
         next page reload anyway."
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
            // Persist the source so the run is visible in the files panel
            // and survives a reload. Best-effort — a write failure must not
            // block the run itself.
            let saved = {
                use crate::filesystem::Filesystem;
                let fs = crate::app::shared_opfs();
                fs.write_atomic("cartridge.rl", source.as_bytes()).await.is_ok()
            };
            match crate::app::display::run_wasm(&wasm_bytes).await {
                Ok(()) => Ok(json!({
                    "status": "running on display",
                    "saved": if saved { "cartridge.rl" } else { "" },
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
