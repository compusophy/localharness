//! The evaluator: walk a parsed script, expand words, run pipelines through the
//! [`BashHost`], and accumulate stdout. TOTAL + BOUNDED — every loop iteration
//! and command spends one unit of FUEL, so a `while true` can't hang the tab
//! (it returns a clear "fuel exhausted" error instead).
//!
//! State the evaluator owns (NOT the host): variables, the current directory
//! (`cd` mutates this), and `$?` (last exit code). The host owns the
//! filesystem and the builtin set.

use std::collections::HashMap;

use super::ast::{ChainOp, Command, Stmt, Word, WordPart};
use super::builtins;
use super::host::{BashHost, Output};
use super::BashError;

/// Default fuel budget — the max number of commands + loop iterations a script
/// may execute. Generous for real chores (hundreds of fs ops) yet a hard stop
/// for runaway loops. Tunable via [`Evaluator::with_fuel`].
pub const DEFAULT_FUEL: u64 = 10_000;

/// Hard cap on accumulated stdout (bytes). A script that prints unboundedly
/// (e.g. `while true; do echo x; done` — though fuel catches that first) can't
/// blow up memory; output past this is an error so the result is never silently
/// truncated into something misleading.
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;

/// The accumulated result of running a whole script.
#[derive(Debug, Clone, Default)]
pub struct ScriptResult {
    /// Everything written to stdout (commands' stdout, in order).
    pub stdout: String,
    /// Everything written to stderr (commands' stderr).
    pub stderr: String,
    /// The exit code of the last command run (`$?` at end of script).
    pub exit_code: i32,
}

/// A bashlite interpreter over a [`BashHost`]. One-shot: build it, call
/// [`Evaluator::run`].
pub struct Evaluator<'h, H: BashHost + ?Sized> {
    host: &'h mut H,
    vars: HashMap<String, String>,
    cwd: String,
    last_status: i32,
    fuel: u64,
    stdout: String,
    stderr: String,
}

/// Internal control-flow signal threaded out of statement execution. (v1 has no
/// `break`/`continue`/`return` keywords yet — see the v2 note in `mod.rs` — so
/// the only non-normal flow is a hard error, carried by `Result`.)
type Flow = Result<(), BashError>;

impl<'h, H: BashHost + ?Sized> Evaluator<'h, H> {
    /// Build an evaluator over `host`, starting at sandbox root `/` with the
    /// default fuel budget.
    pub fn new(host: &'h mut H) -> Self {
        Self {
            host,
            vars: HashMap::new(),
            cwd: "/".to_string(),
            last_status: 0,
            fuel: DEFAULT_FUEL,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    /// Override the fuel budget (commands + loop iterations).
    pub fn with_fuel(mut self, fuel: u64) -> Self {
        self.fuel = fuel;
        self
    }

    /// Run a parsed script to completion, returning accumulated output.
    pub async fn run(mut self, body: &[Stmt]) -> Result<ScriptResult, BashError> {
        self.exec_block(body).await?;
        Ok(ScriptResult { stdout: self.stdout, stderr: self.stderr, exit_code: self.last_status })
    }

    /// Spend one unit of fuel; error if exhausted.
    fn burn(&mut self) -> Flow {
        match self.fuel.checked_sub(1) {
            Some(f) => {
                self.fuel = f;
                Ok(())
            }
            None => Err(BashError::fuel()),
        }
    }

    fn push_stdout(&mut self, s: &str) -> Flow {
        if self.stdout.len() + s.len() > MAX_OUTPUT_BYTES {
            return Err(BashError::other(format!(
                "output exceeded {} bytes",
                MAX_OUTPUT_BYTES
            )));
        }
        self.stdout.push_str(s);
        Ok(())
    }

    fn push_stderr(&mut self, s: &str) -> Flow {
        if self.stderr.len() + s.len() > MAX_OUTPUT_BYTES {
            return Err(BashError::other(format!(
                "output exceeded {} bytes",
                MAX_OUTPUT_BYTES
            )));
        }
        self.stderr.push_str(s);
        Ok(())
    }

    async fn exec_block(&mut self, body: &[Stmt]) -> Flow {
        for stmt in body {
            self.exec_stmt(stmt).await?;
        }
        Ok(())
    }

    // Boxed recursion: `exec_stmt` -> `exec_block` -> `exec_stmt` and command
    // substitution all recurse, so the async future must be heap-allocated to
    // have a finite size. `Pin<Box<...>>` is the standard async-recursion shim.
    fn exec_stmt<'s>(&'s mut self, stmt: &'s Stmt) -> super::BoxFut<'s, Flow> {
        Box::pin(async move {
            self.burn()?;
            match stmt {
                Stmt::Assign { name, value } => {
                    let v = self.expand_word(value).await?;
                    self.vars.insert(name.clone(), v);
                    self.last_status = 0;
                }
                Stmt::Pipeline(cmds) => {
                    let out = self.run_pipeline(cmds).await?;
                    self.push_stdout(&out.stdout)?;
                    if !out.stderr.is_empty() {
                        self.push_stderr(&out.stderr)?;
                    }
                    self.last_status = out.code;
                }
                Stmt::AndOr { pipelines, ops } => {
                    // Run the first pipeline, then chain: `&&` runs the next only
                    // on success, `||` only on failure. A skipped pipeline leaves
                    // the running code unchanged and we CONTINUE to the next op
                    // (so `a && b || c` runs `c` when `a` failed — not a `break`).
                    let first = self.run_pipeline(&pipelines[0]).await?;
                    self.push_stdout(&first.stdout)?;
                    if !first.stderr.is_empty() {
                        self.push_stderr(&first.stderr)?;
                    }
                    let mut code = first.code;
                    for (op, pipe) in ops.iter().zip(&pipelines[1..]) {
                        let run_next = match op {
                            ChainOp::And => code == 0,
                            ChainOp::Or => code != 0,
                        };
                        if !run_next {
                            continue;
                        }
                        self.burn()?;
                        let out = self.run_pipeline(pipe).await?;
                        self.push_stdout(&out.stdout)?;
                        if !out.stderr.is_empty() {
                            self.push_stderr(&out.stderr)?;
                        }
                        code = out.code;
                    }
                    self.last_status = code;
                }
                Stmt::If { arms, otherwise } => {
                    let mut ran = false;
                    for (cond, body) in arms {
                        if self.eval_condition(cond).await? {
                            self.exec_block(body).await?;
                            ran = true;
                            break;
                        }
                    }
                    if !ran {
                        if let Some(body) = otherwise {
                            self.exec_block(body).await?;
                        } else {
                            self.last_status = 0;
                        }
                    }
                }
                Stmt::For { var, items, body } => {
                    // Expand all item words first (each may interpolate / substitute),
                    // then FIELD-SPLIT each on whitespace so `for f in $(find …)` /
                    // `for f in $(ls)` iterates one value per line — the fan-out
                    // pattern that lets a loop `run` each discovered script. (Splitting
                    // is for-items only; command args / assignments stay one-word. A
                    // quoted multi-word literal like `for x in "a b"` therefore still
                    // splits — a known v1 simplification, rare in practice.)
                    let mut values = Vec::new();
                    for w in items {
                        let expanded = self.expand_word(w).await?;
                        values.extend(expanded.split_whitespace().map(String::from));
                    }
                    for v in values {
                        self.burn()?;
                        self.vars.insert(var.clone(), v);
                        self.exec_block(body).await?;
                    }
                }
                Stmt::While { cond, body } => {
                    while self.eval_condition(cond).await? {
                        self.burn()?;
                        self.exec_block(body).await?;
                    }
                }
            }
            Ok(())
        })
    }

    /// Run a condition block (the `if`/`while` test) and report whether the LAST
    /// statement exited 0. Output from the condition IS emitted (matching the
    /// shell — a command in a condition still prints), but typically it's a
    /// `[ ... ]` test which prints nothing.
    async fn eval_condition(&mut self, cond: &[Stmt]) -> Result<bool, BashError> {
        self.exec_block(cond).await?;
        Ok(self.last_status == 0)
    }

    /// Run a pipeline: feed each command's stdout as the next command's stdin.
    /// The pipeline's result is the LAST command's output + exit code.
    async fn run_pipeline(&mut self, cmds: &[Command]) -> Result<Output, BashError> {
        let mut stdin = String::new();
        let mut last = Output::default();
        let mut accumulated_stderr = String::new();
        for (i, cmd) in cmds.iter().enumerate() {
            self.burn()?;
            let out = self.run_command(cmd, &stdin).await?;
            accumulated_stderr.push_str(&out.stderr);
            if i + 1 < cmds.len() {
                // Pipe stdout forward; the intermediate command's stderr still
                // surfaces, its stdout does not (it's consumed by the pipe).
                stdin = out.stdout;
            } else {
                last = out;
            }
        }
        // Fold every stage's stderr into the final result so a failing
        // mid-pipeline command isn't silent.
        last.stderr = accumulated_stderr;
        Ok(last)
    }

    /// Run a single command: expand its name + args, handle the `cd` special
    /// (it mutates evaluator state, not the fs), else dispatch to the host.
    async fn run_command(&mut self, cmd: &Command, stdin: &str) -> Result<Output, BashError> {
        let name = self.expand_word(&cmd.name).await?;
        let mut args = Vec::with_capacity(cmd.args.len());
        for a in &cmd.args {
            args.push(self.expand_word(a).await?);
        }
        // `cd` is evaluator-local — it changes `cwd`, which every later builtin
        // resolves against. Empty target → root (no $HOME concept in the sandbox).
        if name == "cd" {
            let target = args.first().map(String::as_str).unwrap_or("/");
            let next = builtins::resolve(&self.cwd, target);
            // Verify the target exists and is a directory before moving there.
            match self.host.fs().metadata(&next).await {
                Ok(Some(m)) if m.kind == crate::filesystem::EntryKind::Directory => {
                    self.cwd = next;
                    return Ok(Output::ok(""));
                }
                Ok(Some(_)) => return Ok(Output::err(format!("cd: {target}: not a directory"), 1)),
                Ok(None) => {
                    // Allow `cd /` even on a backend with no explicit root entry.
                    if next == "/" {
                        self.cwd = next;
                        return Ok(Output::ok(""));
                    }
                    return Ok(Output::err(format!("cd: {target}: no such file or directory"), 1));
                }
                Err(e) => return Ok(Output::err(format!("cd: {target}: {e}"), 1)),
            }
        }
        // `pwd` — print the current directory.
        if name == "pwd" {
            return Ok(Output::ok(format!("{}\n", self.cwd)));
        }
        // `run <file.bl>` — FRACTAL composition: execute another bashlite script
        // in a nested evaluator (shared fs + fuel, isolated vars/cwd), its stdout
        // becoming this command's stdout. Bounded by the SHARED fuel budget, so
        // script-runs-script recursion (a.bl runs b.bl runs a.bl) can't hang —
        // it exhausts fuel and stops. This is the primitive that makes a script a
        // composition of scripts, the same way host::compose nests cartridges.
        if name == "run" || name == "." || name == "source" {
            let Some(path_arg) = args.first() else {
                return Ok(Output::err(format!("{name}: missing script path"), 2));
            };
            let path = builtins::resolve(&self.cwd, path_arg);
            let src = match self.host.fs().read(&path).await {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(e) => return Ok(Output::err(format!("{name}: {path_arg}: {e}"), 1)),
            };
            return self.run_nested(&src, path_arg).await;
        }
        let cwd = self.cwd.clone();
        Ok(self.host.run_builtin(&cwd, &name, &args, stdin).await)
    }

    /// Execute a sub-script `src` in a NESTED evaluator that shares this one's
    /// host + fuel but starts with fresh vars (subshell isolation) at the current
    /// cwd. Returns its captured stdout/stderr/exit as one [`Output`]. A
    /// lex/parse failure in the sub-script is a NONZERO exit (the parent script
    /// continues), NOT a fatal `BashError`; fuel/output-cap errors DO propagate
    /// (they're global limits). Backs the `run`/`source` fractal builtin.
    async fn run_nested(&mut self, src: &str, label: &str) -> Result<Output, BashError> {
        let body = match super::lexer::lex(src).and_then(|t| super::parser::parse(&t)) {
            Ok(b) => b,
            Err(e) => return Ok(Output::err(format!("run: {label}: {e}"), 2)),
        };
        let mut sub = Evaluator {
            host: self.host,
            vars: HashMap::new(),
            cwd: self.cwd.clone(),
            last_status: 0,
            fuel: self.fuel,
            stdout: String::new(),
            stderr: String::new(),
        };
        let flow = sub.exec_block(&body).await;
        // Reclaim the fuel the sub-script spent BEFORE propagating any fatal
        // limit error, so the global budget stays accurate across compositions.
        self.fuel = sub.fuel;
        flow?;
        Ok(Output { stdout: sub.stdout, stderr: sub.stderr, code: sub.last_status })
    }

    /// Expand a word into its final string: concatenate literal/variable/
    /// command-substitution segments. No field splitting in v1 (one word → one
    /// string), which keeps behaviour predictable and total.
    fn expand_word<'s>(&'s mut self, word: &'s Word) -> super::BoxFut<'s, Result<String, BashError>> {
        Box::pin(async move {
            let mut out = String::new();
            for part in word {
                match part {
                    WordPart::Lit(s) => out.push_str(s),
                    WordPart::Var(name) => {
                        // `$?` is the last exit status; otherwise a plain var
                        // ("" if unset, like the shell with nounset off).
                        if name == "?" {
                            out.push_str(&self.last_status.to_string());
                        } else {
                            out.push_str(self.vars.get(name).map(String::as_str).unwrap_or(""));
                        }
                    }
                    WordPart::Subst(src) => {
                        let captured = self.run_substitution(src).await?;
                        out.push_str(&captured);
                    }
                }
            }
            Ok(out)
        })
    }

    /// Run a `$(...)` substitution: lex + parse the inner source, run it in a
    /// NESTED evaluator that shares this one's vars/cwd/fuel, and return its
    /// stdout with trailing newlines trimmed (shell semantics). The nested run
    /// spends from the SAME fuel budget so substitution can't be a fuel loophole.
    async fn run_substitution(&mut self, src: &str) -> Result<String, BashError> {
        let tokens = super::lexer::lex(src)?;
        let body = super::parser::parse(&tokens)?;
        // Share state by value-copy in / out: a substitution sees current vars +
        // cwd but its own assignments do NOT leak back (subshell semantics),
        // EXCEPT fuel, which must be consumed globally.
        let mut sub = Evaluator {
            host: self.host,
            vars: self.vars.clone(),
            cwd: self.cwd.clone(),
            last_status: self.last_status,
            fuel: self.fuel,
            stdout: String::new(),
            stderr: String::new(),
        };
        sub.exec_block(&body).await?;
        // Reclaim the fuel the subshell spent + propagate its last status.
        self.fuel = sub.fuel;
        self.last_status = sub.last_status;
        // Move the captured streams out and drop `sub` so it releases its borrow of
        // `self.host` before we take `&mut self` to fold stderr up.
        let sub_stderr = std::mem::take(&mut sub.stderr);
        let mut captured = std::mem::take(&mut sub.stdout);
        drop(sub);
        // Fold subshell stderr up so errors aren't lost.
        self.push_stderr(&sub_stderr)?;
        while captured.ends_with('\n') {
            captured.pop();
        }
        Ok(captured)
    }
}
