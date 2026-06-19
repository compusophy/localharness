//! `localharness sh <file.bl>` — run a bashlite script on the native filesystem
//! with the `lh-*` platform commands wired in. This is localharness running ON
//! bashlite: a script orchestrates the platform — compose sub-scripts with
//! `run`, read the chain with `lh-*`, MOVE value with `lh-send` — in one local
//! pass, no agent loop, no per-step LLM round. The fractal end of the stack:
//! rustlite builds cartridges, the WASI runtime runs compiled CLIs, bashlite
//! scripts the tools.
//!
//! Value-moving commands ride the **dry-run-manifest** gate: the script first
//! runs DRY (nothing sent), emitting a one-line plan per move; without
//! `--confirm` it prints the plan and stops; with `--confirm` it shows the plan
//! then runs LIVE. Read-only / composition scripts (no moves) just run.

use std::sync::Arc;

use async_trait::async_trait;
use k256::ecdsa::SigningKey;
use localharness::bashlite::platform::WriteEnv;
use localharness::bashlite::{self, BashHost, Output};
use localharness::encoding::bytes_to_hex_str;
use localharness::filesystem::{Filesystem, NativeFilesystem, RootedFilesystem};
use localharness::{registry, wallet};

use crate::{load_signer, load_sponsor};

/// A native bashlite host: the fs sandbox rooted at the script's directory, the
/// read-only `lh-*` reads, and the value-moving `lh-*` writes behind the
/// dry-run-manifest gate. `run_builtin` tries reads, then writes, then fs.
struct CliBashHost {
    fs: RootedFilesystem,
    identity: Option<String>,
    signer: Option<SigningKey>,
    sponsor: SigningKey,
    fee_token: String,
    dry_run: bool,
    /// One line per value move encountered (the confirm manifest).
    manifest: Vec<String>,
}

#[async_trait(?Send)]
impl BashHost for CliBashHost {
    fn fs(&self) -> &dyn Filesystem {
        &self.fs
    }
    async fn run_builtin(&mut self, cwd: &str, cmd: &str, args: &[String], stdin: &str) -> Output {
        // 1. read-only lh-* (no identity needed beyond the default subject).
        if let Some(out) = bashlite::platform::dispatch(cmd, args, self.identity.as_deref()).await {
            return out;
        }
        // 2. value-moving lh-* — needs an identity, rides the dry-run gate.
        if bashlite::platform::is_write_command(cmd) {
            let Some(signer) = self.signer.as_ref() else {
                return Output::err(format!("{cmd}: needs an identity — run with --as <name>"), 2);
            };
            let env = WriteEnv { signer, sponsor: &self.sponsor, fee_token: &self.fee_token };
            if let Some((out, plan)) =
                bashlite::platform::dispatch_write(cmd, args, &env, self.dry_run).await
            {
                if !plan.is_empty() {
                    self.manifest.push(plan);
                }
                return out;
            }
        }
        // 3. fs builtins.
        bashlite::builtins::dispatch_in(&self.fs, cwd, cmd, args, stdin).await
    }
}

/// Run `path` once with the given `dry_run` flag; returns the script result and
/// the value-move manifest it produced.
async fn run_pass(
    src: &str,
    base: &str,
    identity: Option<String>,
    signer: Option<SigningKey>,
    sponsor: SigningKey,
    fee_token: String,
    dry_run: bool,
) -> Result<(bashlite::ScriptResult, Vec<String>), String> {
    let fs = RootedFilesystem::new(Arc::new(NativeFilesystem::new()), base.to_string());
    let mut host = CliBashHost {
        fs,
        identity,
        signer,
        sponsor,
        fee_token,
        dry_run,
        manifest: Vec::new(),
    };
    let res = bashlite::run(&mut host, src).await.map_err(|e| e.to_string())?;
    Ok((res, host.manifest))
}

/// Run `path` (a `.bl` script) and return the process exit code. `as_name` sets
/// the identity; `confirm` authorizes value moves (see the dry-run-manifest gate).
pub(crate) async fn cmd_sh(path: &str, as_name: Option<&str>, confirm: bool) -> i32 {
    let p = std::path::Path::new(path);
    let src = match std::fs::read_to_string(p) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sh: {path}: {e}");
            return 1;
        }
    };
    let base = p
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());

    let signer = as_name.and_then(|n| load_signer(Some(n)).ok());
    // `bytes_to_hex_str` already prefixes `0x`.
    let identity = signer.as_ref().map(|s| bytes_to_hex_str(&wallet::address(s)));
    let sponsor = match load_sponsor() {
        Ok(s) => s,
        Err(code) => return code,
    };
    let fee_token = registry::ALPHA_USD_ADDRESS().to_string();

    // Pass 1 — DRY-RUN: collect the value-move manifest; send nothing.
    let (dry, manifest) = match run_pass(
        &src,
        &base,
        identity.clone(),
        signer.clone(),
        sponsor.clone(),
        fee_token.clone(),
        true,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            eprintln!("sh: {e}");
            return 2;
        }
    };

    // No value moves → a pure read/compose script: its dry output IS the result.
    if manifest.is_empty() {
        print!("{}", dry.stdout);
        if !dry.stderr.is_empty() {
            eprint!("{}", dry.stderr);
        }
        return dry.exit_code;
    }

    // Value-moving: show the plan.
    print!("{}", dry.stdout);
    if !dry.stderr.is_empty() {
        eprint!("{}", dry.stderr);
    }
    eprintln!("\nthis script moves value ({} action(s)):", manifest.len());
    for m in &manifest {
        eprintln!("  - {m}");
    }
    if !confirm {
        eprintln!("nothing sent. re-run with --confirm to execute.");
        return 0;
    }

    // Pass 2 — LIVE: re-run for real (reads are idempotent; writes execute).
    eprintln!("--confirm given — executing…");
    match run_pass(&src, &base, identity, signer, sponsor, fee_token, false).await {
        Ok((live, _)) => {
            print!("{}", live.stdout);
            if !live.stderr.is_empty() {
                eprint!("{}", live.stderr);
            }
            live.exit_code
        }
        Err(e) => {
            eprintln!("sh: {e}");
            2
        }
    }
}
