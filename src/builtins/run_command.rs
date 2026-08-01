//! `run_command` — execute a shell command with timeout + output cap.
//!
//! Runs through the platform shell (`cmd /C` on Windows, `sh -c`
//! elsewhere). Bounded stdout/stderr (each capped at 256 KiB), kill on
//! timeout, exit code surfaced verbatim.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::warn;

use crate::error::{Error, Result};
use crate::tools::{Tool, ToolContext};

const OUTPUT_CAP: usize = 256 * 1024;
const DEFAULT_TIMEOUT_SEC: u64 = 30;
const MAX_TIMEOUT_SEC: u64 = 600;
/// How long to wait for a pipe to reach EOF after the child is gone before
/// giving up on further capture (a surviving grandchild can hold the write
/// end open forever — the drain must never hang the agent loop).
const DRAIN_TIMEOUT_SEC: u64 = 2;
/// Bound on reaping a killed child: SIGKILL can't be blocked, but a process
/// in uninterruptible sleep (D-state qemu/NFS) can still refuse to die.
const REAP_TIMEOUT_SEC: u64 = 5;

/// Live process-group leaders (unix). `process_group(0)` moves children out
/// of the terminal's foreground group, so the terminal's own SIGINT no longer
/// reaches them — a CLI Ctrl-C handler calls [`kill_live_process_groups`] to
/// do what the terminal used to.
#[cfg(unix)]
static LIVE_GROUPS: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

/// Kill every process group `run_command` still has live (unix; no-op
/// elsewhere). For the CLI's Ctrl-C path.
pub async fn kill_live_process_groups() {
    #[cfg(unix)]
    {
        let pids: Vec<u32> = std::mem::take(&mut *LIVE_GROUPS.lock().unwrap());
        for pid in pids {
            kill_group(Some(pid)).await;
        }
    }
}

pub struct RunCommand;

crate::tool_params! {
    /// ONE table generates both this struct and `input_schema` (see
    /// `crate::tool_params`); the schema byte-identity test is below.
    struct Args: serde {
        command: req_str = "Shell command line.",
        working_dir: opt_str = "Optional CWD for the command.",
        timeout_sec: opt_u64 min 1 max 600 = "Timeout in seconds (default 30, max 600).",
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for RunCommand {
    fn name(&self) -> &str {
        "run_command"
    }

    fn description(&self) -> &str {
        "Execute a shell command. Returns { stdout, stderr, exit_code, timed_out }. \
         Each stream is capped at 256 KiB; default timeout 30 s, max 600 s. \
         Use sparingly — gate with a policy."
    }

    fn input_schema(&self) -> Value {
        Args::schema()
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| Error::bad_args("run_command", format!("run_command args: {e}")))?;
        let timeout_dur = Duration::from_secs(
            args.timeout_sec
                .unwrap_or(DEFAULT_TIMEOUT_SEC)
                .min(MAX_TIMEOUT_SEC),
        );

        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", &args.command]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", &args.command]);
            c
        };
        if let Some(dir) = &args.working_dir {
            cmd.current_dir(dir);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Unix: run the shell as its OWN process-group leader so a timeout
        // kill can reach the whole tree. `start_kill` alone kills `sh -c` and
        // leaves grandchildren (a foregrounded qemu, a spawned server) alive
        // AND holding the pipe write-ends — which then blocked the unbounded
        // drains below forever and froze the whole agent loop (TB-15
        // qemu-alpine-ssh: 900s burned on one call).
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd
            .spawn()
            // Stays `Error::other`: a spawn failure is an OS error but neither
            // an fs op (`Fs` would lie) nor `Io` (its "io: " prefix changes text).
            .map_err(|e| Error::other(format!("spawn: {e}")))?;
        let mut stdout = child.stdout.take().expect("stdout pipe present");
        let mut stderr = child.stderr.take().expect("stderr pipe present");

        // Readers append into SHARED state after every read so a bounded
        // drain-abort keeps everything captured so far — an aborted task-local
        // buffer would silently report a successful command as "no output"
        // (the model-facing-lie class).
        let cap_out: Capture = Arc::new(std::sync::Mutex::new((Vec::new(), false)));
        let cap_err: Capture = Arc::new(std::sync::Mutex::new((Vec::new(), false)));
        let stdout_handle = tokio::spawn({
            let cap = cap_out.clone();
            async move { read_capped_into(&mut stdout, cap).await }
        });
        let stderr_handle = tokio::spawn({
            let cap = cap_err.clone();
            async move { read_capped_into(&mut stderr, cap).await }
        });

        // Registered so a CLI Ctrl-C can group-kill it (see LIVE_GROUPS).
        // Underscore: only the unix blocks read it.
        let _child_pid = child.id();
        #[cfg(unix)]
        if let Some(pid) = _child_pid {
            LIVE_GROUPS.lock().unwrap().push(pid);
        }

        let wait = child.wait();
        let result = timeout(timeout_dur, wait).await;

        let (exit_code, timed_out) = match result {
            Ok(Ok(status)) => (status.code(), false),
            Ok(Err(e)) => {
                warn!(?e, "child wait failed");
                // Group-kill BEFORE reaping: `child.id()` is still Some until a
                // successful wait, so the leader is live-or-zombie when
                // signalled — no PID-reuse window.
                kill_group(child.id()).await;
                let _ = child.start_kill();
                (None, false)
            }
            Err(_) => {
                kill_group(child.id()).await;
                if let Err(e) = child.start_kill() {
                    warn!(?e, "kill after timeout failed");
                }
                let _ = timeout(Duration::from_secs(REAP_TIMEOUT_SEC), child.wait()).await;
                (None, true)
            }
        };

        #[cfg(unix)]
        if let Some(pid) = _child_pid {
            LIVE_GROUPS.lock().unwrap().retain(|p| *p != pid);
        }

        // BOUNDED drains: any surviving pipe-holder (a daemonized grandchild
        // after a normal exit, a kill-race survivor after a timeout) would
        // otherwise block these awaits until an EOF that never comes.
        let out_gave_up = drain_bounded(stdout_handle).await;
        let err_gave_up = drain_bounded(stderr_handle).await;
        let (stdout, stdout_truncated) = std::mem::take(&mut *cap_out.lock().unwrap());
        let (stderr, stderr_truncated) = std::mem::take(&mut *cap_err.lock().unwrap());

        Ok(json!({
            "exit_code": exit_code,
            "timed_out": timed_out,
            "stdout": String::from_utf8_lossy(&stdout).into_owned(),
            "stderr": String::from_utf8_lossy(&stderr).into_owned(),
            "stdout_truncated": stdout_truncated,
            "stderr_truncated": stderr_truncated,
            // Distinct from *_truncated (the 256 KiB cap): capture stopped
            // early because something still holds the pipe open. Output above
            // is everything received before the drain gave up.
            "capture_timed_out": out_gave_up || err_gave_up,
        }))
    }
}

/// Kill the child's whole process GROUP (unix; no-op elsewhere). With
/// `process_group(0)` the child is its own group leader (pgid == pid), so
/// `kill -9 -<pid>` signals every descendant — shelling out to `kill` keeps
/// this libc-free. Only called on timeout/wait-failure paths: a NORMAL exit
/// must leave deliberately-backgrounded servers running.
#[cfg(unix)]
async fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        let _ = Command::new("kill")
            .args(["-9", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
}
#[cfg(not(unix))]
async fn kill_group(_pid: Option<u32>) {}

/// Shared capture state: `(bytes so far, hit the 256 KiB cap)`. The reader
/// appends after EVERY read so nothing is lost if the drain gives up.
type Capture = Arc<std::sync::Mutex<(Vec<u8>, bool)>>;

/// Await a pipe-drain task for at most [`DRAIN_TIMEOUT_SEC`], then abort it.
/// Returns whether the drain gave up (bytes live in the shared `Capture`).
async fn drain_bounded(handle: tokio::task::JoinHandle<()>) -> bool {
    let abort = handle.abort_handle();
    match timeout(Duration::from_secs(DRAIN_TIMEOUT_SEC), handle).await {
        Ok(_) => false,
        Err(_) => {
            abort.abort();
            warn!("pipe drain timed out — a survivor still holds the write end; keeping captured bytes");
            true
        }
    }
}

/// Read a stream into the shared bounded capture.
async fn read_capped_into(reader: &mut (impl tokio::io::AsyncRead + Unpin), cap: Capture) {
    let mut scratch = [0u8; 8 * 1024];
    loop {
        match reader.read(&mut scratch).await {
            Ok(0) => break,
            Ok(n) => {
                // Tight lock scope — a MutexGuard held across an await would
                // make the future !Send (and the generator analysis flags it
                // even past an explicit drop).
                let cap_hit = {
                    let mut guard = cap.lock().unwrap();
                    let remaining = OUTPUT_CAP.saturating_sub(guard.0.len());
                    if remaining == 0 {
                        guard.1 = true;
                        true
                    } else {
                        let take = remaining.min(n);
                        guard.0.extend_from_slice(&scratch[..take]);
                        if take < n {
                            guard.1 = true;
                        }
                        false
                    }
                };
                if cap_hit {
                    // Drain the rest so the child can exit cleanly.
                    while let Ok(n) = reader.read(&mut scratch).await {
                        if n == 0 {
                            break;
                        }
                    }
                    break;
                }
            }
            Err(_) => break,
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
                "command":     { "type": "string", "description": "Shell command line." },
                "working_dir": { "type": "string", "description": "Optional CWD for the command." },
                "timeout_sec": { "type": "integer", "minimum": 1, "maximum": 600, "description": "Timeout in seconds (default 30, max 600)." }
            },
            "required": ["command"]
        });
        assert_eq!(Args::schema().to_string(), frozen.to_string());
    }

    /// Parse parity with the replaced `#[derive(Deserialize)]` struct: the
    /// `Option` fields default to `None` on missing (serde's built-in Option
    /// handling — the old `#[serde(default)]` was redundant), and a missing
    /// `command` errors naming the field.
    #[test]
    fn serde_parse_matches_the_old_derive() {
        let a: Args = serde_json::from_value(json!({"command": "echo hi"})).unwrap();
        assert_eq!((a.command.as_str(), a.working_dir, a.timeout_sec), ("echo hi", None, None));
        let a: Args =
            serde_json::from_value(json!({"command": "ls", "timeout_sec": 5, "working_dir": "/tmp"}))
                .unwrap();
        assert_eq!((a.timeout_sec, a.working_dir.as_deref()), (Some(5), Some("/tmp")));
        assert!(serde_json::from_value::<Args>(json!({})).is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_simple_echo() {
        let tool = RunCommand;
        let cmd = if cfg!(windows) {
            "echo hello"
        } else {
            "printf 'hello'"
        };
        let out = tool.execute(json!({"command": cmd}), None).await.unwrap();
        let stdout = out["stdout"].as_str().unwrap();
        assert!(stdout.contains("hello"), "stdout was: {stdout:?}");
        assert_eq!(out["exit_code"].as_i64(), Some(0));
        assert_eq!(out["timed_out"].as_bool(), Some(false));
    }

    #[tokio::test]
    async fn surfaces_nonzero_exit_code() {
        let tool = RunCommand;
        let cmd = if cfg!(windows) { "exit /B 7" } else { "exit 7" };
        let out = tool.execute(json!({"command": cmd}), None).await.unwrap();
        assert_eq!(out["exit_code"].as_i64(), Some(7));
    }

    /// H1 regression: `sh` exits instantly but a backgrounded grandchild
    /// holds the pipe — capture must give up within the drain bound WITHOUT
    /// losing the bytes already read (the TB-15 qemu freeze + byte-loss pair).
    #[cfg(unix)]
    #[tokio::test]
    async fn background_grandchild_neither_hangs_nor_loses_output() {
        let tool = RunCommand;
        let t0 = std::time::Instant::now();
        let out = tool
            .execute(json!({"command": "sleep 30 & echo hi"}), None)
            .await
            .unwrap();
        assert!(t0.elapsed() < Duration::from_secs(10), "drain must be bounded");
        assert!(out["stdout"].as_str().unwrap().contains("hi"), "bytes kept: {out:?}");
        assert_eq!(out["exit_code"].as_i64(), Some(0));
        assert_eq!(out["capture_timed_out"].as_bool(), Some(true));
    }

    #[tokio::test]
    async fn enforces_timeout() {
        let tool = RunCommand;
        // Sleep 5s but timeout at 1s.
        let cmd = if cfg!(windows) {
            // `timeout` cmd isn't reliable from non-interactive shells; use ping.
            "ping -n 5 127.0.0.1 >NUL"
        } else {
            "sleep 5"
        };
        let out = tool
            .execute(json!({"command": cmd, "timeout_sec": 1}), None)
            .await
            .unwrap();
        assert_eq!(out["timed_out"].as_bool(), Some(true));
    }
}
