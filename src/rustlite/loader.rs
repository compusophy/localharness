//! Cartridge loader — instantiate compiled wasm bytes in the browser.
//!
//! Takes the output of `codegen::emit`, instantiates it via
//! `WebAssembly.instantiate`, wires up host imports, and provides
//! a `call` method to invoke exported functions.
//!
//! wasm32-only — on native targets this module compiles but all
//! methods return errors (no browser WebAssembly API).

use crate::rustlite::CompileError;

pub struct Cartridge {
    #[cfg(target_arch = "wasm32")]
    instance: wasm_bindgen::JsValue,
    #[cfg(target_arch = "wasm32")]
    memory: wasm_bindgen::JsValue,
    #[cfg(not(target_arch = "wasm32"))]
    _phantom: (),
}

impl Cartridge {
    /// Instantiate compiled wasm bytes into a runnable cartridge.
    pub async fn load(wasm_bytes: &[u8]) -> Result<Self, CompileError> {
        #[cfg(target_arch = "wasm32")]
        {
            load_wasm(wasm_bytes).await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = wasm_bytes;
            Err(CompileError::new("cartridge loading requires a browser environment"))
        }
    }

    /// List all exported function names.
    pub fn exports(&self) -> Vec<String> {
        #[cfg(target_arch = "wasm32")]
        {
            list_exports(&self.instance)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Vec::new()
        }
    }

    /// Call an exported function with i32 arguments, returns i32.
    pub fn call_i32(&self, name: &str, args: &[i32]) -> Result<i32, CompileError> {
        #[cfg(target_arch = "wasm32")]
        {
            call_export_i32(&self.instance, name, args)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (name, args);
            Err(CompileError::new("cartridge execution requires a browser environment"))
        }
    }

    /// Read a string from cartridge memory at the given pointer.
    /// Expects length-prefixed layout: 4 bytes LE length, then UTF-8 payload.
    pub fn read_string(&self, ptr: i32) -> Result<String, CompileError> {
        #[cfg(target_arch = "wasm32")]
        {
            read_string_from_memory(&self.memory, ptr)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ptr;
            Err(CompileError::new("requires browser environment"))
        }
    }
}

#[cfg(target_arch = "wasm32")]
async fn load_wasm(wasm_bytes: &[u8]) -> Result<Cartridge, CompileError> {
    use js_sys::{Reflect, WebAssembly};
    use wasm_bindgen::JsValue;
    use wasm_bindgen_futures::JsFuture;

    let imports = build_host_imports()?;

    let promise = WebAssembly::instantiate_buffer(wasm_bytes, &imports);
    let result = JsFuture::from(promise)
        .await
        .map_err(|e| CompileError::new(format!("instantiate failed: {e:?}")))?;

    let instance = Reflect::get(&result, &JsValue::from_str("instance"))
        .map_err(|e| CompileError::new(format!("no instance: {e:?}")))?;

    let exports = Reflect::get(&instance, &JsValue::from_str("exports"))
        .map_err(|e| CompileError::new(format!("no exports: {e:?}")))?;
    let memory = Reflect::get(&exports, &JsValue::from_str("memory"))
        .unwrap_or(JsValue::NULL);

    Ok(Cartridge { instance, memory })
}

#[cfg(target_arch = "wasm32")]
fn build_host_imports() -> Result<js_sys::Object, CompileError> {
    use js_sys::{Object, Reflect};
    use wasm_bindgen::prelude::*;

    let imports = Object::new();

    // host_log module — ambient, always available
    let host_log = Object::new();
    let log_info = Closure::<dyn Fn(i32)>::new(|_ptr: i32| {
        web_sys::console::log_1(&wasm_bindgen::JsValue::from_str("[cartridge] log"));
    });
    let _ = Reflect::set(&host_log, &JsValue::from_str("info"), log_info.as_ref());
    let _ = Reflect::set(&host_log, &JsValue::from_str("warn"), log_info.as_ref());
    let _ = Reflect::set(&host_log, &JsValue::from_str("error"), log_info.as_ref());
    let _ = Reflect::set(&host_log, &JsValue::from_str("debug"), log_info.as_ref());
    log_info.forget();
    let _ = Reflect::set(&imports, &JsValue::from_str("host_log"), &host_log);

    // host_time module — ambient
    let host_time = Object::new();
    let now_fn = Closure::<dyn Fn() -> f64>::new(|| {
        js_sys::Date::now()
    });
    let _ = Reflect::set(&host_time, &JsValue::from_str("now_unix_ms"), now_fn.as_ref());
    let _ = Reflect::set(&host_time, &JsValue::from_str("monotonic_ms"), now_fn.as_ref());
    now_fn.forget();
    let _ = Reflect::set(&imports, &JsValue::from_str("host_time"), &host_time);

    // host_abort module — ambient
    let host_abort = Object::new();
    let panic_fn = Closure::<dyn Fn(i32)>::new(|_ptr: i32| {
        web_sys::console::error_1(&wasm_bindgen::JsValue::from_str("[cartridge] panic"));
    });
    let _ = Reflect::set(&host_abort, &JsValue::from_str("panic"), panic_fn.as_ref());
    panic_fn.forget();

    let fuel_fn = Closure::<dyn Fn() -> f64>::new(|| 1_000_000.0);
    let _ = Reflect::set(&host_abort, &JsValue::from_str("fuel_remaining"), fuel_fn.as_ref());
    fuel_fn.forget();

    let mem_fn = Closure::<dyn Fn() -> i32>::new(|| 0);
    let _ = Reflect::set(&host_abort, &JsValue::from_str("memory_bytes"), mem_fn.as_ref());
    mem_fn.forget();
    let _ = Reflect::set(&imports, &JsValue::from_str("host_abort"), &host_abort);

    Ok(imports)
}

#[cfg(target_arch = "wasm32")]
fn list_exports(instance: &wasm_bindgen::JsValue) -> Vec<String> {
    use js_sys::Reflect;
    use wasm_bindgen::JsValue;

    let exports = match Reflect::get(instance, &JsValue::from_str("exports")) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let keys = js_sys::Object::keys(&js_sys::Object::from(exports));
    let mut names = Vec::new();
    for i in 0..keys.length() {
        if let Some(key) = keys.get(i).as_string() {
            if key != "memory" {
                names.push(key);
            }
        }
    }
    names
}

#[cfg(target_arch = "wasm32")]
fn call_export_i32(
    instance: &wasm_bindgen::JsValue,
    name: &str,
    args: &[i32],
) -> Result<i32, CompileError> {
    use js_sys::Reflect;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::JsValue;

    let exports = Reflect::get(instance, &JsValue::from_str("exports"))
        .map_err(|_| CompileError::new("no exports"))?;
    let func = Reflect::get(&exports, &JsValue::from_str(name))
        .map_err(|_| CompileError::new(format!("export '{name}' not found")))?;
    let func: js_sys::Function = func
        .dyn_into()
        .map_err(|_| CompileError::new(format!("'{name}' is not a function")))?;

    let js_args = js_sys::Array::new();
    for &arg in args {
        js_args.push(&JsValue::from(arg));
    }

    let result = func
        .apply(&JsValue::NULL, &js_args)
        .map_err(|e| CompileError::new(format!("call failed: {e:?}")))?;

    result
        .as_f64()
        .map(|v| v as i32)
        .ok_or_else(|| CompileError::new("function did not return a number"))
}

#[cfg(target_arch = "wasm32")]
fn read_string_from_memory(
    memory: &wasm_bindgen::JsValue,
    ptr: i32,
) -> Result<String, CompileError> {
    use js_sys::Reflect;
    use wasm_bindgen::JsValue;

    let buffer = Reflect::get(memory, &JsValue::from_str("buffer"))
        .map_err(|_| CompileError::new("no memory buffer"))?;
    let array = js_sys::Uint8Array::new(&buffer);

    let ptr = ptr as u32;
    let mut len_bytes = [0u8; 4];
    for i in 0..4 {
        len_bytes[i] = array.get_index(ptr + i as u32) as u8;
    }
    let len = u32::from_le_bytes(len_bytes);
    if len > 65536 {
        return Err(CompileError::new(format!("string too long: {len}")));
    }

    let mut bytes = vec![0u8; len as usize];
    for i in 0..len {
        bytes[i as usize] = array.get_index(ptr + 4 + i) as u8;
    }

    String::from_utf8(bytes)
        .map_err(|e| CompileError::new(format!("invalid utf-8: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cartridge_exports_list() {
        // On native, load returns an error — just verify the API compiles
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(Cartridge::load(&[]));
        assert!(result.is_err());
    }
}
