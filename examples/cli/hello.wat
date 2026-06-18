;; hello.wat — a tiny WASI command for the in-browser WASI-subset sandbox.
;;
;; A demonstrable end-to-end CLI: it exports `_start` (the WASI command entry),
;; prints a fixed greeting AND its argv to stdout via `fd_write`, then calls
;; `proc_exit(0)`. Hand-authored WAT so the example needs no Rust/clang wasi-sdk
;; toolchain to (re)build — `wat2wasm examples/cli/hello.wat -o examples/cli/hello.wasm`.
;;
;; Any real wasm32-wasi command (`clang --target=wasm32-wasi`, `rustc --target
;; wasm32-wasi`, TinyGo, etc.) exporting `_start` runs under the SAME host; this
;; file is just the committed proof so the path works with zero external setup.
(module
  ;; The WASI host import we use: fd_write(fd, iovs, iovs_len, nwritten) -> errno.
  (import "wasi_snapshot_preview1" "fd_write"
    (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
  ;; args_sizes_get(argc_ptr, buf_size_ptr) + args_get(argv_ptr, argv_buf_ptr).
  (import "wasi_snapshot_preview1" "args_sizes_get"
    (func $args_sizes_get (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "args_get"
    (func $args_get (param i32 i32) (result i32)))

  (memory (export "memory") 1)

  ;; The greeting string, laid out at offset 100.
  (data (i32.const 100) "hello from wasm cli\0a")          ;; 20 bytes incl. \n
  ;; A label before we dump argv.
  (data (i32.const 200) "argv:\0a")                         ;; 6 bytes

  ;; Print a [ptr,len] region to stdout (fd 1) using one iovec at scratch 8..16
  ;; and nwritten at scratch 0.
  (func $print (param $ptr i32) (param $len i32)
    (i32.store (i32.const 8) (local.get $ptr))    ;; iov.base
    (i32.store (i32.const 12) (local.get $len))   ;; iov.len
    (drop (call $fd_write
      (i32.const 1)        ;; fd = stdout
      (i32.const 8)        ;; iovs ptr
      (i32.const 1)        ;; iovs len
      (i32.const 0)))      ;; nwritten ptr
  )

  (func (export "_start")
    (local $argc i32)
    (local $i i32)
    (local $argv_ptr i32)
    (local $str i32)
    (local $slen i32)
    ;; greeting
    (call $print (i32.const 100) (i32.const 20))
    ;; "argv:\n"
    (call $print (i32.const 200) (i32.const 6))

    ;; args_sizes_get -> argc at 300, buf size at 304
    (drop (call $args_sizes_get (i32.const 300) (i32.const 304)))
    (local.set $argc (i32.load (i32.const 300)))
    ;; args_get -> argv pointer array at 320, arg bytes buffer at 400
    (drop (call $args_get (i32.const 320) (i32.const 400)))

    ;; for each arg: print the NUL-terminated string, then a newline
    (local.set $i (i32.const 0))
    (block $done
      (loop $loop
        (br_if $done (i32.ge_u (local.get $i) (local.get $argc)))
        ;; argv_ptr = *(320 + i*4)
        (local.set $argv_ptr
          (i32.load (i32.add (i32.const 320) (i32.mul (local.get $i) (i32.const 4)))))
        ;; compute strlen
        (local.set $str (local.get $argv_ptr))
        (local.set $slen (i32.const 0))
        (block $slen_done
          (loop $slen_loop
            (br_if $slen_done
              (i32.eqz (i32.load8_u (i32.add (local.get $str) (local.get $slen)))))
            (local.set $slen (i32.add (local.get $slen) (i32.const 1)))
            (br $slen_loop)))
        (call $print (local.get $argv_ptr) (local.get $slen))
        ;; newline: reuse byte at offset 119 ('\n' from the greeting's trailing \n)
        (call $print (i32.const 119) (i32.const 1))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)))

    (call $proc_exit (i32.const 0))
  )
)
