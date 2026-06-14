//! Stdio transport for an MCP server.
//!
//! Spawns the server as a subprocess and frames newline-delimited JSON
//! over stdin/stdout. A single reader task forwards every line to the
//! caller via an `mpsc` channel; the caller writes by acquiring a
//! `tokio::sync::Mutex` over the child's stdin handle.
//!
//! Server stderr is captured into `tracing::debug!` so MCP server
//! crashes don't disappear silently.

use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::error::{Error, Result};

/// Bound for the inbound channel from the reader task to McpClient.
/// 64 is plenty: each entry is one JSON-RPC message. Backpressure here
/// just slows the reader if the client is overwhelmed.
const INBOUND_CAPACITY: usize = 64;

pub struct StdioTransport {
    stdin: Arc<Mutex<ChildStdin>>,
    pub(crate) inbound: Mutex<mpsc::Receiver<String>>,
    pub(crate) reader: JoinHandle<()>,
    pub(crate) stderr_logger: Option<JoinHandle<()>>,
    child: Mutex<Option<Child>>,
}

impl StdioTransport {
    pub async fn spawn(command: &str, args: &[String]) -> Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| Error::other(format!("mcp spawn '{command}': {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::other("mcp child has no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::other("mcp child has no stdout"))?;

        let (tx, rx) = mpsc::channel::<String>(INBOUND_CAPACITY);
        let reader = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        debug!("mcp transport: stdout EOF");
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line.trim_end_matches(['\r', '\n']);
                        if trimmed.is_empty() {
                            continue;
                        }
                        if tx.send(trimmed.to_string()).await.is_err() {
                            debug!("mcp transport: receiver dropped");
                            break;
                        }
                    }
                    Err(e) => {
                        warn!(?e, "mcp transport: stdout read failed");
                        break;
                    }
                }
            }
        });

        let stderr_logger = child.stderr.take().map(spawn_stderr_logger);

        Ok(Self {
            stdin: Arc::new(Mutex::new(stdin)),
            inbound: Mutex::new(rx),
            reader,
            stderr_logger,
            child: Mutex::new(Some(child)),
        })
    }

    pub async fn send(&self, payload: &str) -> Result<()> {
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| Error::other(format!("mcp write: {e}")))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| Error::other(format!("mcp write nl: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| Error::other(format!("mcp flush: {e}")))?;
        Ok(())
    }

    pub async fn shutdown(&self) {
        // Drop stdin so the server sees EOF.
        {
            let mut stdin = self.stdin.lock().await;
            let _ = stdin.shutdown().await;
        }
        // Abort tasks.
        self.reader.abort();
        if let Some(h) = &self.stderr_logger {
            h.abort();
        }
        // Wait briefly for the child to exit; then kill if needed.
        let mut guard = self.child.lock().await;
        if let Some(mut child) = guard.take() {
            let wait = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                child.wait(),
            )
            .await;
            if wait.is_err() {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }
    }
}

fn spawn_stderr_logger(mut stderr: tokio::process::ChildStderr) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf = Vec::with_capacity(4096);
        let mut scratch = [0u8; 4096];
        loop {
            match stderr.read(&mut scratch).await {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&scratch[..n]);
                    drain_complete_lines(&mut buf, emit_stderr_line);
                }
                Err(_) => break,
            }
        }
        // A server that dies (or logs without a trailing newline) leaves a
        // final unterminated fragment in `buf` at EOF — flush it so the last
        // line of a crash trace isn't silently swallowed.
        flush_remainder(&mut buf, emit_stderr_line);
    })
}

/// Drain every `\n`-terminated line out of `buf` (the terminator stripped),
/// handing each to `emit`. Leaves any trailing partial line in `buf` for the
/// next read. Pure (no I/O) so the framing is unit-testable.
fn drain_complete_lines(buf: &mut Vec<u8>, mut emit: impl FnMut(&[u8])) {
    while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
        let line: Vec<u8> = buf.drain(..=pos).collect();
        emit(&line[..line.len() - 1]);
    }
}

/// Emit the final unterminated fragment (if any) at EOF, then clear `buf`.
fn flush_remainder(buf: &mut Vec<u8>, mut emit: impl FnMut(&[u8])) {
    if !buf.is_empty() {
        emit(buf);
        buf.clear();
    }
}

/// Log one captured stderr line (newline already stripped), trimming a lone
/// trailing `\r` so CRLF servers don't leave artifacts.
fn emit_stderr_line(line: &[u8]) {
    let line = String::from_utf8_lossy(line);
    debug!(target: "localharness::mcp", "{}", line.trim_end_matches('\r'));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect emitted lines as owned strings (matching the production
    /// `\r`-trim) so we can assert on framing.
    fn collect(buf: &mut Vec<u8>) -> (Vec<String>, Vec<String>) {
        let mut complete = Vec::new();
        drain_complete_lines(buf, |l| {
            complete.push(String::from_utf8_lossy(l).trim_end_matches('\r').to_string())
        });
        let mut remainder = Vec::new();
        flush_remainder(buf, |l| {
            remainder.push(String::from_utf8_lossy(l).trim_end_matches('\r').to_string())
        });
        (complete, remainder)
    }

    #[test]
    fn drains_complete_lines_and_keeps_partial() {
        let mut buf = b"one\ntwo\npartial".to_vec();
        let mut complete = Vec::new();
        drain_complete_lines(&mut buf, |l| {
            complete.push(String::from_utf8_lossy(l).into_owned())
        });
        assert_eq!(complete, vec!["one".to_string(), "two".to_string()]);
        // The unterminated tail stays for the next read.
        assert_eq!(buf, b"partial");
    }

    #[test]
    fn flush_emits_final_unterminated_line() {
        // The EOF case the fix targets: a crash trace's last line with no
        // trailing newline must still be emitted.
        let mut buf = b"boom: panic at the end".to_vec();
        let (complete, remainder) = collect(&mut buf);
        assert!(complete.is_empty());
        assert_eq!(remainder, vec!["boom: panic at the end".to_string()]);
        assert!(buf.is_empty());
    }

    #[test]
    fn flush_is_noop_on_clean_eof() {
        // A buffer fully consumed by complete lines flushes nothing.
        let mut buf = b"only\n".to_vec();
        let (complete, remainder) = collect(&mut buf);
        assert_eq!(complete, vec!["only".to_string()]);
        assert!(remainder.is_empty());
    }

    #[test]
    fn trims_trailing_cr_on_crlf_lines() {
        let mut buf = b"crlf\r\nlast\r".to_vec();
        let (complete, remainder) = collect(&mut buf);
        assert_eq!(complete, vec!["crlf".to_string()]);
        // The final fragment had a lone CR that the emitter trims.
        assert_eq!(remainder, vec!["last".to_string()]);
    }
}
