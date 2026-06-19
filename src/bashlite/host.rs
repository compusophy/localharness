//! The host seam: [`BashHost`] is everything the pure evaluator needs from the
//! outside world. Eval never touches a real filesystem or clock directly — it
//! goes through this trait, so the lexer/parser/eval run unchanged under
//! `cargo test` with an in-memory host and in the browser over OPFS.
//!
//! v1 builtins are FS-ONLY (read/create/search) — no value-moving / `lh-*`
//! platform commands (deferred to v2, see `design/bashlite.md`). A builtin
//! receives its already-expanded `args` plus piped `stdin`, and returns an
//! [`Output`] (stdout text + exit code). Builtins must be TOTAL: report errors
//! as a nonzero exit + stderr text, never panic.

use crate::filesystem::Filesystem;

/// The result of running one command: captured stdout/stderr text and an exit
/// code (0 = success). Mirrors the `{ exit_code, stdout, stderr }` shape the
/// `execute_script` tool returns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

impl Output {
    /// A successful run with `stdout` and exit 0.
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self { stdout: stdout.into(), stderr: String::new(), code: 0 }
    }
    /// A failed run: `stderr` text and exit code `code` (defaults to 1 if 0).
    pub fn err(stderr: impl Into<String>, code: i32) -> Self {
        Self { stdout: String::new(), stderr: stderr.into(), code: if code == 0 { 1 } else { code } }
    }
}

/// The capabilities bashlite eval needs from its environment.
///
/// `async_trait(?Send)` on EVERY target: the bashlite evaluator's recursion
/// futures are boxed-local (non-`Send`), and the interpreter is inherently
/// single-threaded — it's awaited directly (CLI/tests) or run on the browser's
/// single-threaded executor, never spawned across threads. A `Send` bound here
/// would force `H: Send` through the whole evaluator for no benefit.
#[async_trait::async_trait(?Send)]
pub trait BashHost {
    /// The sandbox filesystem the fs builtins operate over (OPFS in-browser,
    /// Native on the CLI, an in-memory map in tests).
    fn fs(&self) -> &dyn Filesystem;

    /// Run a command by name with already-expanded `args`, the caller's current
    /// directory `cwd`, and piped `stdin`. The default impl dispatches the v1 fs
    /// builtins ([`crate::bashlite::builtins`]). A host OVERRIDES this to add
    /// commands — e.g. the `lh-*` platform reads ([`crate::bashlite::platform`]) —
    /// delegating to `dispatch_in` for anything it doesn't own. This is the
    /// extension seam: the evaluator routes every non-control command through it.
    async fn run_builtin(&mut self, cwd: &str, cmd: &str, args: &[String], stdin: &str) -> Output {
        crate::bashlite::builtins::dispatch_in(self.fs(), cwd, cmd, args, stdin).await
    }
}
