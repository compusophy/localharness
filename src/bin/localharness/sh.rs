//! `localharness sh <file.bl>` — run a bashlite script on the native filesystem
//! with the `lh-*` platform commands wired in. This is localharness running ON
//! bashlite: a script orchestrates the platform — compose sub-scripts with
//! `run`, read the chain with `lh-*`, MOVE value with `lh-send` — in one local
//! pass, no agent loop, no per-step LLM round. The fractal end of the stack:
//! rustlite builds cartridges, the WASI runtime runs compiled CLIs, bashlite
//! scripts the tools.
//!
//! Value-moving commands ride the **dry-run-manifest** gate: the script runs
//! DRY exactly ONCE (nothing sent), emitting a one-line plan per move and
//! capturing each move (command + effective args) into the manifest; without
//! `--confirm` it prints the plan and stops; with `--confirm` it executes
//! EXACTLY the captured manifest — it never re-runs the script, so a divergent
//! second pass (time/random/fs-state-dependent control flow) can't swap in a
//! value move the user never approved. Read-only / composition scripts (no
//! moves) just run.

use std::sync::Arc;

use async_trait::async_trait;
use k256::ecdsa::SigningKey;
use localharness::bashlite::platform::WriteEnv;
use localharness::bashlite::{self, BashHost, Output};
use localharness::encoding::bytes_to_hex_str;
use localharness::filesystem::{Filesystem, NativeFilesystem, RootedFilesystem};
use localharness::wallet;

use crate::load_signer;

/// A native bashlite host: the fs sandbox rooted at the script's directory, the
/// read-only `lh-*` reads, and the value-moving `lh-*` writes behind the
/// dry-run-manifest gate. The host runs the script DRY-ONLY: it COLLECTS each
/// value move into [`Self::manifest`] and sends nothing. The approved manifest
/// — not a script re-run — is what executes LIVE (see [`run_source`]), so no
/// cross-pass divergence can substitute an unapproved move. `run_builtin` tries
/// reads, then writes, then fs.
struct CliBashHost {
    fs: RootedFilesystem,
    identity: Option<String>,
    signer: Option<SigningKey>,
    /// Every value move encountered, in order — the confirm manifest. On
    /// `--confirm` EXACTLY these execute (the script is never re-run).
    manifest: Vec<PlannedMove>,
}

/// One value move captured during the dry pass: the command, its EFFECTIVE args
/// (source-file paths already substituted with content), and the one-line plan
/// shown to the user. The live pass dispatches EXACTLY this rather than
/// re-running the script, so the confirmed plan is BINDING — a divergent second
/// pass (clock/random/fs-state) can't swap in a move the user never authorized.
struct PlannedMove {
    cmd: String,
    args: Vec<String>,
    plan: String,
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
        // 2. value-moving / state-changing lh-* — needs an identity, rides the
        //    dry-run gate.
        if bashlite::platform::is_write_command(cmd) {
            let Some(signer) = self.signer.as_ref() else {
                return Output::err(format!("{cmd}: needs an identity — run with --as <name>"), 2);
            };
            // Some writes (lh-publish) take a SOURCE FILE PATH as their 2nd arg;
            // read it via the sandbox fs HERE (the host owns it) and substitute
            // the CONTENT so the publish core stays fs-agnostic.
            let mut effective: Vec<String> = args.to_vec();
            if bashlite::platform::write_reads_source_file(cmd) {
                if let Some(path) = args.get(1) {
                    let resolved = bashlite::resolve_path(cwd, path);
                    match self.fs.read(&resolved).await {
                        Ok(bytes) => {
                            effective[1] = String::from_utf8_lossy(&bytes).into_owned();
                        }
                        Err(e) => {
                            return Output::err(format!("{cmd}: {path}: {e}"), 1);
                        }
                    }
                }
            }
            let env = WriteEnv { signer };
            // DRY-ONLY here: collect the plan and send nothing. The APPROVED
            // manifest (not a re-run of this script) is what executes live, so
            // record the command + effective args alongside the plan.
            if let Some((out, plan)) =
                bashlite::platform::dispatch_write(cmd, &effective, &env, true).await
            {
                if !plan.is_empty() {
                    self.manifest.push(PlannedMove {
                        cmd: cmd.to_string(),
                        args: effective,
                        plan,
                    });
                }
                return out;
            }
        }
        // 3. fs builtins.
        bashlite::builtins::dispatch_in(&self.fs, cwd, cmd, args, stdin).await
    }
}

/// Run `path` once DRY (sends nothing); returns the script result and the
/// value-move manifest it produced. The script is NEVER run live — the approved
/// manifest is dispatched directly (see [`run_source`]) so no cross-pass
/// divergence can inject an unapproved move.
async fn run_pass(
    src: &str,
    base: &str,
    identity: Option<String>,
    signer: Option<SigningKey>,
) -> Result<(bashlite::ScriptResult, Vec<PlannedMove>), String> {
    let fs = RootedFilesystem::new(Arc::new(NativeFilesystem::new()), base.to_string());
    let mut host = CliBashHost {
        fs,
        identity,
        signer,
        manifest: Vec::new(),
    };
    let res = bashlite::run(&mut host, src).await.map_err(|e| e.to_string())?;
    Ok((res, host.manifest))
}

/// Run a `.bl` script FILE and return the process exit code. `as_name` sets the
/// identity; `confirm` authorizes value moves (see the dry-run-manifest gate).
/// The fs sandbox roots at the script's directory.
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
    run_source(&src, &base, as_name, confirm).await
}

/// Run an INLINE script (`sh -c '<source>'`) — the same shell without a file,
/// sandboxed at the current working directory. Handy for one-liners
/// (`localharness sh -c 'lh-balance' --as me`).
pub(crate) async fn cmd_sh_inline(src: &str, as_name: Option<&str>, confirm: bool) -> i32 {
    run_source(src, ".", as_name, confirm).await
}

/// The shared run pipeline: load the identity, run the DRY pass ONCE to
/// collect the value-move manifest, then print (read/compose scripts) or gate on
/// `--confirm` (value-moving scripts). `base` roots the fs sandbox.
async fn run_source(src: &str, base: &str, as_name: Option<&str>, confirm: bool) -> i32 {
    let signer = as_name.and_then(|n| load_signer(Some(n)).ok());
    // `bytes_to_hex_str` already prefixes `0x`.
    let identity = signer.as_ref().map(|s| bytes_to_hex_str(&wallet::address(s)));

    // DRY-RUN (the ONLY script execution): collect the value-move manifest;
    // send nothing. The captured moves ARE what executes on --confirm — the
    // script is never re-run, so a divergent second pass can't inject a move the
    // user didn't approve.
    let (dry, manifest) = match run_pass(src, base, identity, signer.clone()).await {
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
        eprintln!("  - {}", m.plan);
    }
    if !confirm {
        eprintln!("nothing sent. re-run with --confirm to execute.");
        return 0;
    }

    // --confirm given — execute EXACTLY the approved manifest, in order. Bind
    // execution to the plan the user saw: dispatch each captured move LIVE
    // rather than re-running the script (a re-run could diverge and move value
    // the user never authorized).
    eprintln!("--confirm given — executing…");
    let Some(signer) = signer.as_ref() else {
        // A move was only ever recorded with a signer present, so this is
        // unreachable — refuse rather than silently skip if it somehow occurs.
        eprintln!("sh: no identity — run with --as <name>");
        return 2;
    };
    let env = WriteEnv { signer };
    let mut code = 0;
    for m in &manifest {
        match bashlite::platform::dispatch_write(&m.cmd, &m.args, &env, false).await {
            Some((out, _plan)) => {
                print!("{}", out.stdout);
                if !out.stderr.is_empty() {
                    eprint!("{}", out.stderr);
                }
                if out.code != 0 {
                    code = out.code;
                }
            }
            // A manifest entry is always a value-moving command dispatch_write
            // owns; treat the impossible None as an error, never a silent skip.
            None => {
                eprintln!("sh: {}: not a value-moving command", m.cmd);
                code = 2;
            }
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M7 regression: the dry pass must capture each value move STRUCTURALLY
    /// (command + effective args + plan), because `--confirm` executes exactly
    /// that captured manifest — it never re-runs the script. A re-run could
    /// diverge (clock/random/fs-state-dependent control flow) and move value the
    /// user never approved; binding execution to the captured move closes that.
    #[tokio::test]
    async fn dry_pass_captures_structured_moves_not_a_rerun() {
        let k = wallet::generate();
        let fs = RootedFilesystem::new(Arc::new(NativeFilesystem::new()), ".".to_string());
        let mut host = CliBashHost {
            fs,
            identity: Some(bytes_to_hex_str(&wallet::address(&k.signer))),
            signer: Some(k.signer.clone()),
            manifest: Vec::new(),
        };
        // `lh-send` to a 0x ADDRESS is network-free in the dry pass (no resolve).
        let addr = "0x00000000000000000000000000000000000000aa";
        let res = bashlite::run(&mut host, &format!("lh-send {addr} 2.5"))
            .await
            .expect("script runs");
        assert_eq!(res.exit_code, 0, "{:?}", res.stderr);
        // Exactly one move, captured with its command + effective args + plan —
        // enough to re-dispatch it live WITHOUT re-running the script.
        assert_eq!(host.manifest.len(), 1);
        let m = &host.manifest[0];
        assert_eq!(m.cmd, "lh-send");
        assert_eq!(m.args, vec![addr.to_string(), "2.5".to_string()]);
        assert!(m.plan.contains("send 2.5 $LH"), "{}", m.plan);
    }
}
