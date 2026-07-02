//! host_audio: Web Audio (AudioContext) cartridge sound.
//!
//! The audio analog of host_display's framebuffer: integer-only host fns a
//! rustlite cartridge calls, no DOM. One AudioContext per tab (browsers cap
//! context count) lives in a thread_local, lazily created + resumed on the
//! first call (an AudioContext is silent until a user gesture — and a
//! cartridge only runs after the user opened it, so the first tone resumes
//! it). Voices are osc/buffer -> per-voice gain -> shared master gain ->
//! destination, and auto-free on `onended` so the handle table can't grow
//! unbounded. Mirrors `mod net`'s poll/fire-and-forget style + handle table.
//! The worker implements `host_audio` itself and FORWARDS each op here (only
//! the main thread has an AudioContext) via `worker::handle_audio`.

use std::cell::RefCell;

use js_sys::{Function, Reflect};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{AudioContext, GainNode, OscillatorType};

/// Cap concurrent voices so a runaway cartridge can't spawn thousands of
/// nodes; the oldest live voice is stopped first (mirrors host_net's
/// MAX_INBOX bound).
const MAX_VOICES: usize = 64;

thread_local! {
    /// One shared AudioContext + master gain per tab, created lazily on
    /// the first audio host call.
    static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
}

struct Engine {
    ctx: AudioContext,
    master: GainNode,
    /// Live voices by handle index; a stopped voice becomes `None` so
    /// handles never alias (same scheme as host_net's socket table).
    voices: Vec<Option<Voice>>,
}

struct Voice {
    /// The scheduled source node (oscillator or buffer source) as a
    /// `JsValue`, so `stop` can call `.stop()` on it early regardless of
    /// the concrete type.
    node: JsValue,
    /// Keeps the `onended` closure alive for the voice's lifetime.
    _onended: Closure<dyn FnMut()>,
}

/// Get-or-create the shared engine, resuming the context (a no-op if
/// already running). Returns `None` only if the browser has no
/// AudioContext or node creation fails.
fn with_engine<R>(f: impl FnOnce(&mut Engine) -> R) -> Option<R> {
    ENGINE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let ctx = AudioContext::new().ok()?;
            let master = ctx.create_gain().ok()?;
            master.gain().set_value(0.3);
            let _ = master.connect_with_audio_node(&ctx.destination());
            *slot = Some(Engine { ctx, master, voices: Vec::new() });
        }
        let eng = slot.as_mut()?;
        let _ = eng.ctx.resume();
        Some(f(eng))
    })
}

/// Insert a voice, capping the table at `MAX_VOICES`; returns its handle.
/// The oldest live voice is stopped if we're at the cap.
fn push_voice(eng: &mut Engine, voice: Voice) -> i32 {
    let live = eng.voices.iter().filter(|v| v.is_some()).count();
    if live >= MAX_VOICES {
        if let Some(slot) = eng.voices.iter_mut().find(|s| s.is_some()) {
            if let Some(old) = slot.take() {
                stop_node(&old.node);
            }
        }
    }
    if let Some(i) = eng.voices.iter().position(|s| s.is_none()) {
        eng.voices[i] = Some(voice);
        i as i32
    } else {
        eng.voices.push(Some(voice));
        (eng.voices.len() - 1) as i32
    }
}

/// Call `.stop()` on an oscillator/buffer-source `JsValue`, ignoring
/// errors (the node may already have ended).
fn stop_node(node: &JsValue) {
    if let Ok(f) = Reflect::get(node, &JsValue::from_str("stop")) {
        if let Ok(f) = f.dyn_into::<Function>() {
            let _ = f.call0(node);
        }
    }
}

fn osc_type(wave: i32) -> OscillatorType {
    match wave {
        1 => OscillatorType::Square,
        2 => OscillatorType::Sawtooth,
        3 => OscillatorType::Triangle,
        _ => OscillatorType::Sine,
    }
}

/// Schedule a tone `delay_ms` in the future for `dur_ms`. Shared by
/// `tone` (delay 0) and `tone_at`. Returns a voice handle or -1.
/// `pub(super)` so the cartridge-worker bridge can play tones forwarded
/// from the worker (an AudioContext can't run in a worker, so audio host
/// calls round-trip to the main thread).
pub(crate) fn play_tone(freq: i32, dur_ms: i32, wave: i32, delay_ms: i32) -> i32 {
    with_engine(|eng| {
        let osc = match eng.ctx.create_oscillator() {
            Ok(o) => o,
            Err(_) => return -1,
        };
        let gain = match eng.ctx.create_gain() {
            Ok(g) => g,
            Err(_) => return -1,
        };
        osc.set_type(osc_type(wave));
        osc.frequency().set_value(freq.max(1) as f32);

        let t0 = eng.ctx.current_time() + (delay_ms.max(0) as f64) / 1000.0;
        let dur = (dur_ms.max(1) as f64) / 1000.0;
        // 4ms attack / release so notes don't click.
        let g = gain.gain();
        let _ = g.set_value_at_time(0.0, t0);
        let _ = g.linear_ramp_to_value_at_time(1.0, t0 + 0.004);
        let _ = g.set_value_at_time(1.0, (t0 + dur - 0.004).max(t0 + 0.004));
        let _ = g.linear_ramp_to_value_at_time(0.0, t0 + dur);

        let _ = osc.connect_with_audio_node(&gain);
        let _ = gain.connect_with_audio_node(&eng.master);
        let _ = osc.start_with_when(t0);
        let _ = osc.stop_with_when(t0 + dur);

        let node: JsValue = osc.clone().into();
        let onended = Closure::<dyn FnMut()>::new(move || {});
        osc.set_onended(Some(onended.as_ref().unchecked_ref()));
        push_voice(eng, Voice { node, _onended: onended })
    })
    .unwrap_or(-1)
}

/// White-noise burst for `dur_ms`. Extracted so the cartridge-worker bridge
/// can play `host_audio::noise` forwarded from the worker. Returns a voice
/// handle or -1.
pub(crate) fn play_noise(dur_ms: i32) -> i32 {
    with_engine(|eng| {
        let sr = eng.ctx.sample_rate();
        let frames = sr as u32; // 1s of noise (truncated by duration)
        let buf = match eng.ctx.create_buffer(1, frames, sr) {
            Ok(b) => b,
            Err(_) => return -1,
        };
        let mut data = vec![0f32; frames as usize];
        // Cheap LCG white noise (getrandom not needed for audio).
        let mut s: u32 = 0x2545_F491;
        for x in data.iter_mut() {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *x = ((s >> 8) as f32 / 8_388_608.0) - 1.0;
        }
        if buf.copy_to_channel(&data, 0).is_err() {
            return -1;
        }
        let src = match eng.ctx.create_buffer_source() {
            Ok(s) => s,
            Err(_) => return -1,
        };
        src.set_buffer(Some(&buf));
        let gain = match eng.ctx.create_gain() {
            Ok(g) => g,
            Err(_) => return -1,
        };
        let t0 = eng.ctx.current_time();
        let dur = (dur_ms.max(1) as f64) / 1000.0;
        let g = gain.gain();
        let _ = g.set_value_at_time(0.8, t0);
        let _ = g.linear_ramp_to_value_at_time(0.0, t0 + dur);
        let _ = src.connect_with_audio_node(&gain);
        let _ = gain.connect_with_audio_node(&eng.master);
        let _ = src.start_with_when(t0);
        // stop_with_when/set_onended live on the AudioScheduledSourceNode
        // base class in current web-sys; the same-named methods directly on
        // AudioBufferSourceNode are deprecated duplicates.
        let scheduled: &web_sys::AudioScheduledSourceNode = src.as_ref();
        let _ = scheduled.stop_with_when(t0 + dur);
        let node: JsValue = src.clone().into();
        let onended = Closure::<dyn FnMut()>::new(move || {});
        scheduled.set_onended(Some(onended.as_ref().unchecked_ref()));
        push_voice(eng, Voice { node, _onended: onended })
    })
    .unwrap_or(-1)
}

/// Stop one voice by handle, or all voices when `handle < 0`. Extracted so
/// the cartridge-worker bridge can forward `host_audio::stop`.
pub(crate) fn stop_handle(handle: i32) {
    ENGINE.with(|cell| {
        if let Some(eng) = cell.borrow_mut().as_mut() {
            if handle < 0 {
                for slot in eng.voices.iter_mut() {
                    if let Some(v) = slot.take() {
                        stop_node(&v.node);
                    }
                }
            } else if let Some(slot) = eng.voices.get_mut(handle as usize) {
                if let Some(v) = slot.take() {
                    stop_node(&v.node);
                }
            }
        }
    });
}

/// Set the master gain (`pct` 0..=100). Extracted so the cartridge-worker
/// bridge can forward `host_audio::set_volume`.
pub(crate) fn set_master_volume(pct: i32) {
    with_engine(|eng| {
        eng.master.gain().set_value((pct.clamp(0, 100) as f32) / 100.0);
    });
}

// NOTE: the in-thread `build_host_audio` import builder was removed with the
// in-thread cartridge runtime (issue #77). The cartridge runs in a Web Worker
// now, which implements `host_audio` itself and FORWARDS each op to the main
// thread (only the main thread has an AudioContext); the worker bridge calls
// `play_tone`/`play_noise`/`stop_handle`/`set_master_volume`/`stop_all` here.

/// Stop every scheduled voice + suspend the context (called on cartridge
/// swap / `display::stop`) so a swap never leaves a drone playing.
pub(crate) fn stop_all() {
    ENGINE.with(|cell| {
        if let Some(eng) = cell.borrow_mut().as_mut() {
            for slot in eng.voices.iter_mut() {
                if let Some(v) = slot.take() {
                    stop_node(&v.node);
                }
            }
            let _ = eng.ctx.suspend();
        }
    });
}
