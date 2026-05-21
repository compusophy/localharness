//! `run_command` — execute a shell command with timeout + output cap.
//!
//! Runs through the platform shell (`cmd /C` on Windows, `sh -c`
//! elsewhere). Bounded stdout/stderr (each capped at 256 KiB), kill on
//! timeout, exit code surfaced verbatim.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
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

pub struct RunCommand;

#[derive(Deserialize)]
struct Args {
    command: String,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    timeout_sec: Option<u64>,
}

#[async_trait]
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
        json!({
            "type": "object",
            "properties": {
                "command":     { "type": "string", "description": "Shell command line." },
                "working_dir": { "type": "string", "description": "Optional CWD for the command." },
                "timeout_sec": { "type": "integer", "minimum": 1, "maximum": 600, "description": "Timeout in seconds (default 30, max 600)." }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| Error::other(format!("run_command args: {e}")))?;
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

        let mut child = cmd
            .spawn()
            .map_err(|e| Error::other(format!("spawn: {e}")))?;
        let mut stdout = child.stdout.take().expect("stdout pipe present");
        let mut stderr = child.stderr.take().expect("stderr pipe present");

        let stdout_handle = tokio::spawn(async move { read_capped(&mut stdout).await });
        let stderr_handle = tokio::spawn(async move { read_capped(&mut stderr).await });

        let wait = child.wait();
        let result = timeout(timeout_dur, wait).await;

        let (exit_code, timed_out) = match result {
            Ok(Ok(status)) => (status.code(), false),
            Ok(Err(e)) => {
                warn!(?e, "child wait failed");
                (None, false)
            }
            Err(_) => {
                if let Err(e) = child.start_kill() {
                    warn!(?e, "kill after timeout failed");
                }
                let _ = child.wait().await;
                (None, true)
            }
        };

        let stdout = stdout_handle.await.unwrap_or_default();
        let stderr = stderr_handle.await.unwrap_or_default();

        Ok(json!({
            "exit_code": exit_code,
            "timed_out": timed_out,
            "stdout": String::from_utf8_lossy(&stdout.0).into_owned(),
            "stderr": String::from_utf8_lossy(&stderr.0).into_owned(),
            "stdout_truncated": stdout.1,
            "stderr_truncated": stderr.1,
        }))
    }
}

/// Read a stream into a bounded buffer. Returns `(bytes, truncated)`.
async fn read_capped(reader: &mut (impl tokio::io::AsyncRead + Unpin)) -> (Vec<u8>, bool) {
    let mut buf = Vec::with_capacity(8 * 1024);
    let mut scratch = [0u8; 8 * 1024];
    let mut truncated = false;
    loop {
        match reader.read(&mut scratch).await {
            Ok(0) => break,
            Ok(n) => {
                let remaining = OUTPUT_CAP.saturating_sub(buf.len());
                if remaining == 0 {
                    truncated = true;
                    // Drain the rest so the child can exit cleanly.
                    while let Ok(n) = reader.read(&mut scratch).await {
                        if n == 0 {
                            break;
                        }
                    }
                    break;
                }
                let take = remaining.min(n);
                buf.extend_from_slice(&scratch[..take]);
                if take < n {
                    truncated = true;
                }
            }
            Err(_) => break,
        }
    }
    (buf, truncated)
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
