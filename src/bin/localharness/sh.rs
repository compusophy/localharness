//! `localharness sh <file.bl>` — run a bashlite script on the native filesystem
//! with the `lh-*` platform reads wired in. This is localharness running ON
//! bashlite: a script orchestrates the platform — compose sub-scripts with
//! `run`, read the chain with `lh-*` — in ONE local pass, no agent loop, no
//! per-step LLM round. The fractal end of the stack: rustlite builds cartridges,
//! the WASI runtime runs compiled CLIs, bashlite scripts the tools.

use std::sync::Arc;

use async_trait::async_trait;
use localharness::bashlite::{self, BashHost, Output};
use localharness::encoding::bytes_to_hex_str;
use localharness::filesystem::{Filesystem, NativeFilesystem, RootedFilesystem};
use localharness::wallet;

use crate::load_signer;

/// A native bashlite host: the fs sandbox rooted at the script's directory plus
/// the read-only `lh-*` platform commands (localharnesslite). `run_builtin`
/// tries the platform dispatch first, then falls back to the fs builtins.
struct CliBashHost {
    fs: RootedFilesystem,
    identity: Option<String>,
}

#[async_trait(?Send)]
impl BashHost for CliBashHost {
    fn fs(&self) -> &dyn Filesystem {
        &self.fs
    }
    async fn run_builtin(&mut self, cwd: &str, cmd: &str, args: &[String], stdin: &str) -> Output {
        if let Some(out) = bashlite::platform::dispatch(cmd, args, self.identity.as_deref()).await {
            return out;
        }
        bashlite::builtins::dispatch_in(&self.fs, cwd, cmd, args, stdin).await
    }
}

/// Run `path` (a `.bl` script) and return the process exit code. The bashlite
/// sandbox is rooted at the script's parent directory, so `run sibling.bl`,
/// `ls`, `cat`, `find` resolve there; `lh-*` reads hit the active chain.
/// `as_name` sets the identity for `lh-whoami` / default `lh-balance` — `None`
/// runs with no identity (explicit name/0x-address args still work).
pub(crate) async fn cmd_sh(path: &str, as_name: Option<&str>) -> i32 {
    let p = std::path::Path::new(path);
    let src = match std::fs::read_to_string(p) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sh: {path}: {e}");
            return 1;
        }
    };
    // Root the sandbox at the script's directory (or the cwd if it has none).
    let base = p
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());
    // Identity only when `--as <name>` is given (avoids the ambiguous-key prompt
    // when several identities exist + a script that doesn't need one).
    // `bytes_to_hex_str` already prefixes `0x`.
    let identity = as_name
        .and_then(|n| load_signer(Some(n)).ok())
        .map(|s| bytes_to_hex_str(&wallet::address(&s)));
    let fs = RootedFilesystem::new(Arc::new(NativeFilesystem::new()), base);
    let mut host = CliBashHost { fs, identity };
    match bashlite::run(&mut host, &src).await {
        Ok(r) => {
            print!("{}", r.stdout);
            if !r.stderr.is_empty() {
                eprint!("{}", r.stderr);
            }
            r.exit_code
        }
        Err(e) => {
            // A malformed script / fuel exhaustion / output-cap is a usage error.
            eprintln!("sh: {e}");
            2
        }
    }
}
