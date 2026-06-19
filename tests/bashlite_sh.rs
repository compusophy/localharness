//! Integration: `localharness sh` runs a bashlite script end-to-end over a
//! RootedFilesystem — the CLI host wiring + `run` composition + `&&`/`||` + the
//! dry-run path (no value moves → runs normally). NO network, fully
//! deterministic. Guards the integration the in-crate unit tests can't reach
//! (they exercise the evaluator, not the built binary + fs rooting).
//!
//! Gated on `wallet` because the `localharness` bin (and thus
//! `CARGO_BIN_EXE_localharness`) only builds with its required features.

#![cfg(feature = "wallet")]

use std::process::Command;

#[test]
fn sh_composes_scripts_over_the_rooted_fs() {
    // A unique temp dir (no tempfile dep — it's an optional feature).
    let dir = std::env::temp_dir().join(format!("lh-sh-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    std::fs::write(
        dir.join("main.bl"),
        // write+cat over the rooted fs · `run` a sibling script · `&&` on success
        // · `||` fallback on an unknown command.
        "write out.txt seed\n\
         run child.bl\n\
         cat out.txt && echo done\n\
         frobnicate || echo recovered\n",
    )
    .unwrap();
    std::fs::write(dir.join("child.bl"), "echo from-child\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_localharness"))
        .args(["sh", dir.join("main.bl").to_str().unwrap()])
        .output()
        .expect("run `localharness sh`");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        out.status.success(),
        "non-zero exit {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status.code()
    );
    assert!(stdout.contains("from-child"), "run-composition missing:\n{stdout}");
    assert!(stdout.contains("seed"), "write+cat over rooted fs missing:\n{stdout}");
    assert!(stdout.contains("done"), "&& chaining missing:\n{stdout}");
    assert!(stdout.contains("recovered"), "|| fallback missing:\n{stdout}");
}
