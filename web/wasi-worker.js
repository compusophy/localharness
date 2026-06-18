// wasi-worker.js — runs an UNTRUSTED compiled wasm "CLI" program OFF the main
// thread under a WASI-SUBSET host, capturing its stdout/stderr as text.
//
// WHY THIS EXISTS (the extensibility POC, on-chain feedback #6):
//   The feedback asked to "run native CLI tools / compilers / the localharness
//   CLI in the browser sandbox". A full native CLI needs a syscall layer; the
//   honest, bounded version that fits this project's no-iframe + framebuffer
//   ethos is a WASI command runtime: a wasm32-wasi module exporting `_start`,
//   linked against a small `wasi_snapshot_preview1` host that maps stdout/stderr
//   to captured text and stubs the rest. This is NOT a full POSIX/x86 machine —
//   no real filesystem, no network, no threads. See the boundary notes below.
//
//   Like the cartridge runtime, the module runs in THIS Web Worker so a hung /
//   unbounded `_start` only blocks the worker; the main thread's WATCHDOG
//   (display.rs-style) can `worker.terminate()` it. Synchronous wasm is
//   un-preemptable from JS, so containment is the only hang defense.
//
// WHAT'S IMPLEMENTED (the wasi_snapshot_preview1 subset a simple program needs):
//   fd_write            — stdout(1)/stderr(2) -> captured UTF-8; other fds noop-ok
//   fd_read             — stdin: always EOF (0 bytes) — no interactive input
//   proc_exit           — ends the run with the given exit code (thrown + caught)
//   args_get / args_sizes_get        — argv (argv[0]="prog" + caller args)
//   environ_get / environ_sizes_get  — empty environment
//   clock_time_get      — Date.now()-based monotonic-ish nanoseconds
//   random_get          — crypto.getRandomValues
//   fd_close / fd_seek / fd_fdstat_get / fd_fdstat_set_flags / fd_prestat_get /
//   fd_prestat_dir_name / path_open / poll_oneoff / sched_yield — minimal
//     stubs (return a WASI errno) so a module that probes them links + degrades
//     gracefully instead of trapping on a missing import.
//
// WHAT'S NOT (documented boundary — never faked):
//   • No real files: there is no preopened directory; path_open returns NOTCAPABLE.
//   • No network / sockets.
//   • Not an x86 PC or a Linux container (that needs v86/WebVM + multi-MB blobs
//     + iframes, which violate this project's design rules).
//   • stdin is always empty (EOF on the first read).
//
// MESSAGE PROTOCOL (see src/app/cli.rs for the main-thread counterpart):
//   main -> worker:
//     { type: 'run', wasm: ArrayBuffer, args: [string], maxOutput?: number }
//   worker -> main:
//     { type: 'done', exitCode, stdout, stderr, truncated }   ran to completion
//     { type: 'error', detail }                                 instantiate/trap

'use strict';

// WASI errno values (subset). 0 = success; the rest are returned by stubs so a
// probing module sees a defined failure instead of a missing-import trap.
const ERRNO_SUCCESS = 0;
const ERRNO_BADF = 8;       // bad file descriptor
const ERRNO_INVAL = 28;     // invalid argument
const ERRNO_NOSYS = 52;     // function not supported
const ERRNO_NOTCAPABLE = 76;

// Hard cap on captured output per stream (bytes) so a runaway printer can't
// balloon worker memory or the postMessage payload. Mirrors the main-thread
// default; a `run` message may lower it but never raise it past this ceiling.
const MAX_OUTPUT_BYTES = 256 * 1024;

// A throw used to unwind the wasm stack on proc_exit (the only clean way to
// stop a WASI command's `_start` from JS). Caught at the run boundary.
class ProcExit {
  constructor(code) { this.code = code; }
}

// Decode a little-endian u32 / write a little-endian u32 into a DataView.
function makeRun(wasmBuf, args, maxOutput) {
  const cap = Math.min(maxOutput | 0 || MAX_OUTPUT_BYTES, MAX_OUTPUT_BYTES);

  // Captured streams (Uint8Array chunks; concatenated + decoded at the end).
  const out = { 1: [], 2: [] };       // fd -> array of byte chunks
  const outLen = { 1: 0, 2: 0 };
  let truncated = false;

  // argv: argv[0] is the program name, then the caller's args. Each is a
  // NUL-terminated UTF-8 C string in args_get's buffer.
  const argv = ['prog', ...(Array.isArray(args) ? args : []).map((a) => String(a))];
  const enc = new TextEncoder();
  const argvBytes = argv.map((a) => {
    const b = enc.encode(a);
    const z = new Uint8Array(b.length + 1); // + NUL
    z.set(b, 0);
    return z;
  });

  // The instance's memory is bound after instantiation (the module may export
  // its own `memory`, the standard for a wasm32-wasi command).
  let memory = null;
  const dv = () => new DataView(memory.buffer);
  const u8 = () => new Uint8Array(memory.buffer);

  function writeU32(ptr, v) { dv().setUint32(ptr, v >>> 0, true); }
  function writeU64(ptr, vBig) {
    // Split a JS number into lo/hi 32-bit halves (good to 2^53; plenty for
    // clock nanoseconds over a short run + any size we report here).
    const lo = vBig % 0x100000000;
    const hi = Math.floor(vBig / 0x100000000);
    dv().setUint32(ptr, lo >>> 0, true);
    dv().setUint32(ptr + 4, hi >>> 0, true);
  }

  // fd_write(fd, iovs, iovs_len, nwritten) — gather the iovec slices and
  // capture them for fd 1 (stdout) / 2 (stderr). Any other fd is accepted +
  // counted (so a program writing to a fd it opened doesn't trap) but dropped.
  function fd_write(fd, iovsPtr, iovsLen, nwrittenPtr) {
    const view = dv();
    const mem = u8();
    let written = 0;
    const sink = out[fd];
    for (let i = 0; i < iovsLen; i++) {
      const base = view.getUint32(iovsPtr + i * 8, true);
      const len = view.getUint32(iovsPtr + i * 8 + 4, true);
      written += len;
      if (!sink || len === 0) continue;
      let take = len;
      if (outLen[fd] + take > cap) { take = Math.max(0, cap - outLen[fd]); truncated = true; }
      if (take > 0) {
        sink.push(mem.slice(base, base + take));
        outLen[fd] += take;
      }
    }
    writeU32(nwrittenPtr, written);
    return ERRNO_SUCCESS;
  }

  // fd_read — stdin only; always report EOF (0 bytes read). A program that
  // blocks on stdin therefore just sees end-of-input rather than hanging.
  function fd_read(_fd, _iovsPtr, _iovsLen, nreadPtr) {
    writeU32(nreadPtr, 0);
    return ERRNO_SUCCESS;
  }

  function args_sizes_get(argcPtr, bufSizePtr) {
    writeU32(argcPtr, argv.length);
    writeU32(bufSizePtr, argvBytes.reduce((n, b) => n + b.length, 0));
    return ERRNO_SUCCESS;
  }
  function args_get(argvPtr, argvBufPtr) {
    const mem = u8();
    let p = argvBufPtr;
    for (let i = 0; i < argvBytes.length; i++) {
      writeU32(argvPtr + i * 4, p);
      mem.set(argvBytes[i], p);
      p += argvBytes[i].length;
    }
    return ERRNO_SUCCESS;
  }
  // Empty environment.
  function environ_sizes_get(countPtr, bufSizePtr) {
    writeU32(countPtr, 0);
    writeU32(bufSizePtr, 0);
    return ERRNO_SUCCESS;
  }
  function environ_get(_envPtr, _envBufPtr) { return ERRNO_SUCCESS; }

  function clock_time_get(_id, _precision, timePtr) {
    // Nanoseconds. Date.now() is milliseconds; *1e6 to ns (coarse but valid).
    writeU64(timePtr, Math.floor(Date.now() * 1e6));
    return ERRNO_SUCCESS;
  }
  function random_get(bufPtr, bufLen) {
    const slice = u8().subarray(bufPtr, bufPtr + bufLen);
    // crypto is available in workers; chunk to the 65536-byte getRandomValues cap.
    for (let off = 0; off < bufLen; off += 65536) {
      const n = Math.min(65536, bufLen - off);
      crypto.getRandomValues(slice.subarray(off, off + n));
    }
    return ERRNO_SUCCESS;
  }

  function proc_exit(code) { throw new ProcExit(code | 0); }

  // Minimal stubs: link cleanly + return a defined errno (not a trap) so a
  // module that probes the wider WASI surface degrades gracefully.
  const fd_close = () => ERRNO_SUCCESS;
  const fd_seek = () => ERRNO_NOSYS;
  const fd_fdstat_get = (_fd, statPtr) => {
    // Zero the 24-byte fdstat; leave filetype 0 (unknown). Enough for libc's
    // isatty/stdio init probe not to trap.
    const mem = u8();
    for (let i = 0; i < 24; i++) mem[statPtr + i] = 0;
    return ERRNO_SUCCESS;
  };
  const fd_fdstat_set_flags = () => ERRNO_SUCCESS;
  const fd_prestat_get = () => ERRNO_BADF;       // no preopened dirs
  const fd_prestat_dir_name = () => ERRNO_INVAL;
  const path_open = () => ERRNO_NOTCAPABLE;      // no real filesystem
  const poll_oneoff = () => ERRNO_NOSYS;
  const sched_yield = () => ERRNO_SUCCESS;

  const wasi = {
    fd_write, fd_read, proc_exit,
    args_get, args_sizes_get, environ_get, environ_sizes_get,
    clock_time_get, random_get,
    fd_close, fd_seek, fd_fdstat_get, fd_fdstat_set_flags,
    fd_prestat_get, fd_prestat_dir_name, path_open, poll_oneoff, sched_yield,
  };

  return {
    // Instantiate against the WASI host, run `_start`, and return the captured
    // streams + exit code. Throws on a real instantiate/trap failure.
    run() {
      const mod = new WebAssembly.Module(wasmBuf);
      // Supply BOTH common WASI module names so a module compiled against either
      // the snapshot or the older `wasi_unstable` import name links.
      const imports = {
        wasi_snapshot_preview1: wasi,
        wasi_unstable: wasi,
      };
      // If the module IMPORTS its memory (rare for a command), give it one.
      for (const imp of WebAssembly.Module.imports(mod)) {
        if (imp.kind === 'memory') {
          imports[imp.module] = imports[imp.module] || {};
          imports[imp.module][imp.name] = new WebAssembly.Memory({ initial: 2 });
          memory = imports[imp.module][imp.name];
        }
      }
      const instance = new WebAssembly.Instance(mod, imports);
      memory = instance.exports.memory || memory;
      if (!memory) throw new Error('module exports no memory');
      const start = instance.exports._start;
      if (typeof start !== 'function') {
        throw new Error('module exports no _start (not a WASI command)');
      }
      let exitCode = 0;
      try {
        start();
      } catch (e) {
        if (e instanceof ProcExit) exitCode = e.code;
        else throw e; // a real trap — surface it
      }
      const dec = new TextDecoder();
      const join = (fd) => dec.decode(concatChunks(out[fd], outLen[fd]));
      return { exitCode, stdout: join(1), stderr: join(2), truncated };
    },
  };
}

// Concatenate the captured byte chunks for one fd into a single Uint8Array.
function concatChunks(chunks, total) {
  const buf = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) { buf.set(c, off); off += c.length; }
  return buf;
}

// Worker wiring — only when running as an actual Web Worker. Under Node (the
// validation harness `scripts/verify-wasi-cli.mjs` requires this file) `self`/
// `postMessage` don't exist; skip wiring and export the pure runner instead so
// the test can run a wasm CLI through THIS host and assert its stdout.
const IS_WORKER = typeof self !== 'undefined' && typeof self.postMessage === 'function';
if (IS_WORKER) {
  self.onmessage = (e) => {
    const msg = e.data;
    if (!msg || msg.type !== 'run') return;
    try {
      const r = makeRun(msg.wasm, msg.args, msg.maxOutput).run();
      self.postMessage({
        type: 'done',
        exitCode: r.exitCode,
        stdout: r.stdout,
        stderr: r.stderr,
        truncated: r.truncated,
      });
    } catch (err) {
      self.postMessage({
        type: 'error',
        detail: (err && err.message) ? err.message : String(err),
      });
    }
  };
}

// Node-only test surface (NOT used by the worker).
if (typeof module !== 'undefined' && module.exports) {
  module.exports = {
    MAX_OUTPUT_BYTES,
    // Run wasm bytes (Uint8Array/ArrayBuffer) through the WASI-subset host and
    // return { exitCode, stdout, stderr, truncated }. Throws on instantiate/trap.
    runWasi(wasmBytes, args = [], maxOutput = MAX_OUTPUT_BYTES) {
      const buf = wasmBytes instanceof Uint8Array ? wasmBytes : new Uint8Array(wasmBytes);
      return makeRun(buf, args, maxOutput).run();
    },
  };
}
