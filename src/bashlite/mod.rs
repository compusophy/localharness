//! bashlite — a tiny, deterministic, TOTAL shell that scripts the platform's
//! filesystem in ONE pass (see `design/bashlite.md`).
//!
//! The cost unlock: an agent doing a multi-step fs chore today burns one LLM
//! round (full context + ~70 tool schemas) per step. bashlite collapses the
//! chore into a single `execute_script` tool call — the platform runs the whole
//! script locally and only the final stdout returns to the model.
//!
//! Shape (mirrors `rustlite/`): lexer → parser → eval over a [`BashHost`] trait,
//! so the whole interpreter is native-testable (`cargo test`) and the
//! browser/CLI just supply the host. NO eval, NO process spawning, NO network.
//!
//! ## v1 language subset (this module)
//!
//! - Variables: `x=value`, `x=$(cmd)`; `$x` / `${x}` interpolation; `$?` exit.
//! - Pipes: `a | b | c` (stdout → stdin).
//! - Control flow: `if/elif/else/fi`, `for NAME in …; do … done`,
//!   `while …; do … done`.
//! - Tests: `[ … ]` / `test …` (string `=`/`!=`/`-z`/`-n`, int `-eq`/`-lt`/…).
//! - Command substitution: `$(…)` (nested, subshell vars, shared fuel).
//! - Builtins (fs): `echo`, `cd`, `pwd`, `ls`, `cat`, `grep`, `find`,
//!   `wc`, `mkdir`, `write`/`create` (CREATE-only), `true`/`false`.
//! - **Composition (`run`/`source`/`.`)**: execute another `.bl` script in a
//!   nested evaluator (shared fs + fuel) — a script is a composition of scripts.
//!   FRACTAL: the sub-script can itself `run` more, bounded by the shared fuel.
//! - **Host extension**: a [`BashHost`] adds commands by overriding `run_builtin`
//!   (e.g. the `lh-*` platform reads in [`platform`]) — the evaluator routes every
//!   non-control command through it, so "localharnesslite" is just more commands.
//! - `for NAME in $(…)` FIELD-SPLITS on whitespace (the fan-out pattern).
//! - Quoting: `'single'` literal, `"double"` interpolating, `\` escape.
//! - BOUNDED: a fuel budget caps total commands + loop iterations so a
//!   `while true` (or `run`-recursion) can't hang; output is byte-capped.
//!
//! ## Deferred to v2 (intentionally NOT in v1 — see `design/bashlite.md`)
//!
//! - Value-MOVING `lh-*` commands (`lh-send`, `lh-create`, …) + the
//!   dry-run-manifest confirm flow. (Read-only `lh-*` ship in [`platform`].)
//! - `break` / `continue` / `return`, functions, `&&` / `||` between commands,
//!   here-docs, redirection (`>`/`>>`/`<`), real regex grep, file-arg `wc`,
//!   field splitting of unquoted expansions OUTSIDE `for` items, globbing,
//!   arithmetic `$(( ))`.

/// Token kinds.
pub mod token;
/// Shell-shaped byte lexer.
pub mod lexer;
/// Script AST.
pub mod ast;
/// Recursive-descent parser.
pub mod parser;
/// The [`BashHost`] seam + [`Output`].
pub mod host;
/// FS-only builtin commands over [`crate::filesystem::Filesystem`].
pub mod builtins;
/// Read-only `lh-*` platform commands (localharnesslite). Needs `wallet`.
#[cfg(feature = "wallet")]
pub mod platform;
/// Bounded, total evaluator.
pub mod eval;

pub use eval::{Evaluator, ScriptResult, DEFAULT_FUEL, MAX_OUTPUT_BYTES};
pub use host::{BashHost, Output};

/// A boxed future alias for the evaluator's async recursion. `?Send` on wasm to
/// match the rest of the crate's single-threaded executor.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type BoxFut<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + 'a>>;
#[cfg(target_arch = "wasm32")]
pub(crate) type BoxFut<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + 'a>>;

/// An error from any stage (lex, parse, or a fatal runtime condition like fuel
/// exhaustion). A command FAILING (nonzero exit) is NOT an error — that's a
/// normal [`Output`] with a nonzero code that the script can branch on. Only a
/// malformed script or a runaway guard produces a `BashError`.
#[derive(Debug, Clone, PartialEq)]
pub struct BashError {
    /// What went wrong (`parse`, `fuel`, or `other`).
    pub kind: BashErrorKind,
    /// Human-readable message.
    pub message: String,
}

/// The category of a [`BashError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashErrorKind {
    /// A lex or parse failure (malformed script).
    Parse,
    /// The fuel budget was exhausted (likely a runaway loop).
    Fuel,
    /// A fatal runtime condition (e.g. output cap exceeded).
    Other,
}

impl BashError {
    pub(crate) fn parse(msg: impl Into<String>) -> Self {
        Self { kind: BashErrorKind::Parse, message: msg.into() }
    }
    pub(crate) fn fuel() -> Self {
        Self {
            kind: BashErrorKind::Fuel,
            message: format!(
                "fuel exhausted (>{} commands/iterations) — likely an unbounded loop",
                eval::DEFAULT_FUEL
            ),
        }
    }
    pub(crate) fn other(msg: impl Into<String>) -> Self {
        Self { kind: BashErrorKind::Other, message: msg.into() }
    }
}

impl std::fmt::Display for BashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self.kind {
            BashErrorKind::Parse => "syntax error",
            BashErrorKind::Fuel => "limit",
            BashErrorKind::Other => "error",
        };
        write!(f, "bashlite {label}: {}", self.message)
    }
}

impl std::error::Error for BashError {}

/// Compile + run `source` against `host` with the default fuel budget,
/// returning the accumulated stdout/stderr/exit code.
pub async fn run<H: BashHost + ?Sized>(
    host: &mut H,
    source: &str,
) -> Result<ScriptResult, BashError> {
    run_with_fuel(host, source, eval::DEFAULT_FUEL).await
}

/// Like [`run`] but with an explicit fuel budget (commands + loop iterations).
pub async fn run_with_fuel<H: BashHost + ?Sized>(
    host: &mut H,
    source: &str,
    fuel: u64,
) -> Result<ScriptResult, BashError> {
    let tokens = lexer::lex(source)?;
    let body = parser::parse(&tokens)?;
    Evaluator::new(host).with_fuel(fuel).run(&body).await
}

// ===========================================================================
// Native-testable core: an in-memory filesystem + host, and a thorough suite
// covering the lexer, parser, eval, every builtin, fuel, substitution, pipes.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result as LhResult;
    use crate::filesystem::{
        DirEntry, EntryKind, Filesystem, Metadata, WalkEntry,
    };
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// A trivial in-memory filesystem: a flat path→bytes map. Directories are
    /// implicit (any prefix of a stored file path). Enough to exercise every
    /// builtin without touching the disk.
    #[derive(Debug, Default)]
    struct MemFs {
        files: Mutex<BTreeMap<String, Vec<u8>>>,
    }

    impl MemFs {
        fn with(files: &[(&str, &str)]) -> Self {
            let m = MemFs::default();
            {
                let mut g = m.files.lock().unwrap();
                for (p, c) in files {
                    g.insert(norm(p), c.as_bytes().to_vec());
                }
            }
            m
        }
        /// Does any stored file live under `dir` (making it an implicit dir)?
        fn is_dir(&self, dir: &str) -> bool {
            let dir = norm(dir);
            if dir == "/" {
                return true;
            }
            let prefix = format!("{dir}/");
            self.files.lock().unwrap().keys().any(|k| k.starts_with(&prefix))
        }
    }

    fn norm(p: &str) -> String {
        let mut stack: Vec<&str> = Vec::new();
        for c in p.split('/') {
            match c {
                "" | "." => {}
                ".." => {
                    stack.pop();
                }
                x => stack.push(x),
            }
        }
        if stack.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", stack.join("/"))
        }
    }

    #[async_trait::async_trait]
    impl Filesystem for MemFs {
        async fn read(&self, path: &str) -> LhResult<Vec<u8>> {
            self.files
                .lock()
                .unwrap()
                .get(&norm(path))
                .cloned()
                .ok_or_else(|| crate::error::Error::other(format!("{path}: not found")))
        }
        async fn write_atomic(&self, path: &str, bytes: &[u8]) -> LhResult<()> {
            self.files.lock().unwrap().insert(norm(path), bytes.to_vec());
            Ok(())
        }
        async fn metadata(&self, path: &str) -> LhResult<Option<Metadata>> {
            let p = norm(path);
            if self.files.lock().unwrap().contains_key(&p) {
                return Ok(Some(Metadata { kind: EntryKind::File, size: 0 }));
            }
            if self.is_dir(&p) {
                return Ok(Some(Metadata { kind: EntryKind::Directory, size: 0 }));
            }
            Ok(None)
        }
        async fn read_dir(&self, path: &str) -> LhResult<Vec<DirEntry>> {
            let dir = norm(path);
            let prefix = if dir == "/" { "/".to_string() } else { format!("{dir}/") };
            let mut names: BTreeMap<String, EntryKind> = BTreeMap::new();
            for key in self.files.lock().unwrap().keys() {
                if let Some(rest) = key.strip_prefix(&prefix) {
                    if rest.is_empty() {
                        continue;
                    }
                    match rest.split_once('/') {
                        Some((head, _)) => {
                            names.insert(head.to_string(), EntryKind::Directory);
                        }
                        None => {
                            names.entry(rest.to_string()).or_insert(EntryKind::File);
                        }
                    }
                }
            }
            Ok(names
                .into_iter()
                .map(|(name, kind)| DirEntry { name, kind, size: None })
                .collect())
        }
        async fn walk(&self, path: &str, _max_depth: Option<usize>) -> LhResult<Vec<WalkEntry>> {
            let root = norm(path);
            let prefix = if root == "/" { "/".to_string() } else { format!("{root}/") };
            let mut out = Vec::new();
            let mut seen_dirs: std::collections::BTreeSet<String> = Default::default();
            for key in self.files.lock().unwrap().keys() {
                if root != "/" && !key.starts_with(&prefix) {
                    continue;
                }
                // Emit each intermediate directory once.
                let rel = key.strip_prefix(&prefix).unwrap_or(key);
                let mut acc = root.trim_end_matches('/').to_string();
                let comps: Vec<&str> = rel.split('/').collect();
                for (i, c) in comps.iter().enumerate() {
                    acc = format!("{acc}/{c}");
                    if i + 1 < comps.len() {
                        if seen_dirs.insert(acc.clone()) {
                            out.push(WalkEntry {
                                path: acc.clone(),
                                kind: EntryKind::Directory,
                                size: None,
                            });
                        }
                    } else {
                        out.push(WalkEntry { path: acc.clone(), kind: EntryKind::File, size: None });
                    }
                }
            }
            Ok(out)
        }
        async fn delete(&self, path: &str) -> LhResult<()> {
            self.files.lock().unwrap().remove(&norm(path));
            Ok(())
        }
    }

    /// Test host: just wraps a [`MemFs`] and uses the default builtin dispatch.
    struct TestHost {
        fs: MemFs,
    }
    impl TestHost {
        fn new(files: &[(&str, &str)]) -> Self {
            Self { fs: MemFs::with(files) }
        }
    }
    #[async_trait::async_trait(?Send)]
    impl BashHost for TestHost {
        fn fs(&self) -> &dyn Filesystem {
            &self.fs
        }
    }

    /// Run `src` against a fresh host seeded with `files`; assert no BashError.
    async fn run_ok(files: &[(&str, &str)], src: &str) -> ScriptResult {
        let mut host = TestHost::new(files);
        run(&mut host, src).await.expect("script should run without a BashError")
    }

    // -------- lexer --------

    #[test]
    fn lex_words_pipes_and_separators() {
        use token::{Token, WordPart};
        let toks = lexer::lex("echo hi | wc -l\nls").unwrap();
        assert_eq!(toks[0], Token::Word(vec![WordPart::Lit("echo".into())]));
        assert_eq!(toks[1], Token::Word(vec![WordPart::Lit("hi".into())]));
        assert_eq!(toks[2], Token::Pipe);
        assert!(matches!(toks[5], Token::Semi)); // newline → Semi
    }

    #[test]
    fn lex_interpolation_and_quotes() {
        use token::WordPart;
        // "$x.rl" → Var(x) + Lit(.rl); single quotes are literal.
        let toks = lexer::lex(r#"echo "$x.rl" '$y'"#).unwrap();
        if let token::Token::Word(parts) = &toks[1] {
            assert_eq!(parts, &vec![WordPart::Var("x".into()), WordPart::Lit(".rl".into())]);
        } else {
            panic!("expected word");
        }
        if let token::Token::Word(parts) = &toks[2] {
            assert_eq!(parts, &vec![WordPart::Lit("$y".into())]);
        } else {
            panic!("expected literal $y");
        }
    }

    #[test]
    fn lex_command_substitution_is_balanced() {
        use token::WordPart;
        let toks = lexer::lex("x=$(echo $(ls))").unwrap();
        // The assignment word is `x=` Lit + Subst("echo $(ls)").
        if let token::Token::Word(parts) = &toks[0] {
            assert_eq!(parts[0], WordPart::Lit("x=".into()));
            assert_eq!(parts[1], WordPart::Subst("echo $(ls)".into()));
        } else {
            panic!("expected word");
        }
    }

    #[test]
    fn lex_rejects_unterminated_quote_and_subst() {
        assert_eq!(lexer::lex("echo \"oops").unwrap_err().kind, BashErrorKind::Parse);
        assert_eq!(lexer::lex("echo 'oops").unwrap_err().kind, BashErrorKind::Parse);
        assert_eq!(lexer::lex("x=$(echo").unwrap_err().kind, BashErrorKind::Parse);
    }

    #[test]
    fn lex_comment_skipped() {
        let toks = lexer::lex("echo hi # this is ignored\nls").unwrap();
        // echo hi ; ls Eof
        assert_eq!(toks.len(), 5);
    }

    // -------- parser --------

    #[test]
    fn parse_assignment_and_pipeline() {
        let body = parser::parse(&lexer::lex("n=$(ls | wc -l)").unwrap()).unwrap();
        assert!(matches!(body[0], ast::Stmt::Assign { .. }));
    }

    #[test]
    fn parse_if_for_while() {
        let prog = "if [ 1 -eq 1 ]; then echo a; elif [ 2 -eq 2 ]; then echo b; else echo c; fi";
        let body = parser::parse(&lexer::lex(prog).unwrap()).unwrap();
        match &body[0] {
            ast::Stmt::If { arms, otherwise } => {
                assert_eq!(arms.len(), 2);
                assert!(otherwise.is_some());
            }
            _ => panic!("expected if"),
        }
        assert!(matches!(
            parser::parse(&lexer::lex("for x in a b c; do echo $x; done").unwrap()).unwrap()[0],
            ast::Stmt::For { .. }
        ));
        assert!(matches!(
            parser::parse(&lexer::lex("while [ -n x ]; do echo y; done").unwrap()).unwrap()[0],
            ast::Stmt::While { .. }
        ));
    }

    #[test]
    fn parse_rejects_missing_fi() {
        // An unterminated `if` is a syntax error.
        assert_eq!(
            parser::parse(&lexer::lex("if [ 1 -eq 1 ]; then echo a").unwrap()).unwrap_err().kind,
            BashErrorKind::Parse
        );
        // `echo a echo b` is VALID — one command, three args (no run-on, since a
        // command greedily eats words until a separator/pipe/keyword).
        let body = parser::parse(&lexer::lex("echo a echo b").unwrap()).unwrap();
        match &body[0] {
            ast::Stmt::Pipeline(cmds) => assert_eq!(cmds[0].args.len(), 3),
            _ => panic!("expected pipeline"),
        }
    }

    // -------- eval: echo / vars / substitution --------

    #[tokio::test]
    async fn echo_and_variables() {
        let r = run_ok(&[], "x=world\necho hello $x").await;
        assert_eq!(r.stdout, "hello world\n");
        assert_eq!(r.exit_code, 0);
    }

    #[tokio::test]
    async fn echo_n_suppresses_newline() {
        let r = run_ok(&[], "echo -n hi").await;
        assert_eq!(r.stdout, "hi");
    }

    #[tokio::test]
    async fn command_substitution_trims_trailing_newline() {
        let r = run_ok(&[], "x=$(echo hi)\necho [$x]").await;
        assert_eq!(r.stdout, "[hi]\n");
    }

    #[tokio::test]
    async fn nested_substitution() {
        let r = run_ok(&[], "echo $(echo $(echo deep))").await;
        assert_eq!(r.stdout, "deep\n");
    }

    #[tokio::test]
    async fn exit_status_variable() {
        // `[ 1 -eq 2 ]` is false (exit 1); $? reflects it.
        let r = run_ok(&[], "[ 1 -eq 2 ]\necho $?").await;
        assert_eq!(r.stdout, "1\n");
        // A successful command resets $? to 0.
        let r = run_ok(&[], "echo hi\necho $?").await;
        assert_eq!(r.stdout, "hi\n0\n");
    }

    // -------- eval: pipes --------

    #[tokio::test]
    async fn pipe_echo_into_wc() {
        let r = run_ok(&[], "echo -n abc | wc -c").await;
        assert_eq!(r.stdout, "3\n");
    }

    #[tokio::test]
    async fn pipe_three_stages() {
        let files = &[("/a.rl", ""), ("/b.txt", ""), ("/c.rl", "")];
        // ls | grep .rl | wc -l  => 2 (.rl files)
        let r = run_ok(files, "ls | grep .rl | wc -l").await;
        assert_eq!(r.stdout, "2\n");
    }

    // -------- builtins: ls / cat / find / grep / wc / mkdir / write --------

    #[tokio::test]
    async fn ls_lists_sorted_names() {
        let r = run_ok(&[("/b", ""), ("/a", ""), ("/sub/x", "")], "ls").await;
        assert_eq!(r.stdout, "a\nb\nsub\n");
    }

    #[tokio::test]
    async fn cd_changes_resolution() {
        let files = &[("/proj/main.rl", "fn"), ("/proj/lib.rl", "fn")];
        let r = run_ok(files, "cd proj\nls | wc -l").await;
        assert_eq!(r.stdout, "2\n");
        // pwd reflects the cd.
        let r = run_ok(files, "cd proj\npwd").await;
        assert_eq!(r.stdout, "/proj\n");
    }

    #[tokio::test]
    async fn cat_concatenates() {
        let r = run_ok(&[("/x.txt", "one\n"), ("/y.txt", "two\n")], "cat x.txt y.txt").await;
        assert_eq!(r.stdout, "one\ntwo\n");
    }

    #[tokio::test]
    async fn cat_missing_file_is_nonzero_not_a_basherror() {
        let r = run_ok(&[], "cat nope.txt\necho done").await;
        // The cat fails (stderr + nonzero) but the SCRIPT continues.
        assert!(r.stderr.contains("nope.txt"));
        assert!(r.stdout.contains("done"));
    }

    #[tokio::test]
    async fn grep_filters_and_flags() {
        let r = run_ok(&[], "echo -n 'foo\nbar\nFOOBAR' | grep -i foo").await;
        assert_eq!(r.stdout, "foo\nFOOBAR\n");
        // -v inverts.
        let r = run_ok(&[], "echo -n 'a\nb\nc' | grep -v b").await;
        assert_eq!(r.stdout, "a\nc\n");
        // -c counts.
        let r = run_ok(&[], "echo -n 'x\nxy\nz' | grep -c x").await;
        assert_eq!(r.stdout, "2\n");
    }

    #[tokio::test]
    async fn find_name_glob_and_type() {
        let files = &[("/src/a.rl", ""), ("/src/b.txt", ""), ("/src/sub/c.rl", "")];
        let r = run_ok(files, "find src -name '*.rl' -type f").await;
        // sorted: src/a.rl, src/sub/c.rl
        assert_eq!(r.stdout, "src/a.rl\nsrc/sub/c.rl\n");
    }

    #[tokio::test]
    async fn wc_modes() {
        let r = run_ok(&[], "echo 'a b c' | wc -w").await;
        assert_eq!(r.stdout, "3\n");
        let r = run_ok(&[], "echo -n 'l1\nl2' | wc -l").await;
        assert_eq!(r.stdout, "2\n");
    }

    #[tokio::test]
    async fn write_create_then_read_back() {
        let mut host = TestHost::new(&[]);
        let r = run(&mut host, "write notes.txt hello there\ncat notes.txt").await.unwrap();
        assert_eq!(r.stdout, "hello there");
        // Re-creating refuses (create-only).
        let r2 = run(&mut host, "write notes.txt again").await.unwrap();
        assert!(r2.stderr.contains("already exists"));
    }

    #[tokio::test]
    async fn mkdir_makes_a_listable_dir() {
        let mut host = TestHost::new(&[]);
        let r = run(&mut host, "mkdir d\nwrite d/f.txt hi\nls d").await.unwrap();
        assert_eq!(r.stdout, ".keep\nf.txt\n");
    }

    // -------- tests `[ ... ]` --------

    #[tokio::test]
    async fn test_string_and_int_ops() {
        let r = run_ok(&[], "if [ abc = abc ]; then echo eq; fi").await;
        assert_eq!(r.stdout, "eq\n");
        let r = run_ok(&[], "if [ 3 -gt 2 ]; then echo big; fi").await;
        assert_eq!(r.stdout, "big\n");
        let r = run_ok(&[], "if [ -z '' ]; then echo empty; fi").await;
        assert_eq!(r.stdout, "empty\n");
        // A false branch falls through to else.
        let r = run_ok(&[], "if [ x = y ]; then echo a; else echo b; fi").await;
        assert_eq!(r.stdout, "b\n");
    }

    #[tokio::test]
    async fn test_missing_bracket_errors_nonzero() {
        // `[ 1 -eq 1` with no `]` → exit 2, not a panic / BashError.
        let r = run_ok(&[], "[ 1 -eq 1\necho $?").await;
        assert_eq!(r.stdout, "2\n");
    }

    // -------- control flow --------

    #[tokio::test]
    async fn for_loop_iterates_words() {
        let r = run_ok(&[], "for x in a b c; do echo $x; done").await;
        assert_eq!(r.stdout, "a\nb\nc\n");
    }

    #[tokio::test]
    async fn for_loop_over_substitution() {
        let files = &[("/one.rl", ""), ("/two.rl", "")];
        // Each filename echoed (substitution of ls gives them whitespace-joined,
        // but v1 has no field splitting — so it's ONE item line). Assert it runs
        // and contains both names.
        let r = run_ok(files, "for f in $(ls); do echo got $f; done").await;
        assert!(r.stdout.contains("one.rl"));
        assert!(r.stdout.contains("two.rl"));
    }

    #[tokio::test]
    async fn while_loop_with_counter_via_substitution() {
        // No arithmetic in v1, so count by appending to a var and measuring it.
        // Build "xxx" and stop when its char count hits 3.
        let src = "\
            s=\n\
            while [ $(echo -n $s | wc -c) -lt 3 ]; do\n\
              s=${s}x\n\
            done\n\
            echo -n $s | wc -c";
        let r = run_ok(&[], src).await;
        assert_eq!(r.stdout, "3\n");
    }

    // -------- fuel / bounds --------

    #[tokio::test]
    async fn infinite_loop_is_caught_by_fuel() {
        let mut host = TestHost::new(&[]);
        let err = run_with_fuel(&mut host, "while true; do echo x; done", 50)
            .await
            .unwrap_err();
        assert_eq!(err.kind, BashErrorKind::Fuel);
    }

    #[tokio::test]
    async fn substitution_shares_the_fuel_budget() {
        // A substitution running an unbounded loop is also fuel-bounded.
        let mut host = TestHost::new(&[]);
        let err = run_with_fuel(&mut host, "x=$(while true; do echo y; done)", 50)
            .await
            .unwrap_err();
        assert_eq!(err.kind, BashErrorKind::Fuel);
    }

    // -------- the end-to-end sample the task asks for --------

    #[tokio::test]
    async fn end_to_end_write_then_ls_grep_wc() {
        // Create a file, then ls | grep | wc it and assert stdout.
        let src = "\
            write report.rl 'fn frame() {}'\n\
            write notes.txt hi\n\
            write app.rl x\n\
            n=$(ls | grep .rl | wc -l)\n\
            echo \"$n cartridges\"";
        let r = run_ok(&[], src).await;
        assert_eq!(r.stdout, "2 cartridges\n");
        assert_eq!(r.exit_code, 0);
    }

    #[tokio::test]
    async fn unknown_command_is_127_not_a_basherror() {
        let r = run_ok(&[], "frobnicate\necho $?").await;
        assert_eq!(r.stdout, "127\n");
    }

    // -------- fractal composition: the `run` builtin --------

    #[tokio::test]
    async fn run_composes_a_subscript() {
        // A parent script runs a child .bl whose stdout it captures + combines —
        // composition, not inlining: the child's vars don't leak to the parent.
        let files = &[("/step.bl", "x=child\necho from $x")];
        let r = run_ok(files, "out=$(run step.bl)\necho [$out] x=$x").await;
        // child printed "from child"; parent's $x is unset (subshell isolation).
        assert_eq!(r.stdout, "[from child] x=\n");
    }

    #[tokio::test]
    async fn run_nests_script_within_script() {
        // a.bl runs b.bl runs (echoes) — fractal nesting, each level a real script.
        let files = &[
            ("/a.bl", "echo a-start\nrun b.bl\necho a-end"),
            ("/b.bl", "echo b-inner"),
        ];
        let r = run_ok(files, "run a.bl").await;
        assert_eq!(r.stdout, "a-start\nb-inner\na-end\n");
    }

    #[tokio::test]
    async fn run_fanout_over_discovered_scripts() {
        // The shape the vision wants: discover .bl files, run each, combine — a
        // pipeline of scripts. `find` yields paths; `run` executes each.
        let files = &[
            ("/jobs/one.bl", "echo one"),
            ("/jobs/two.bl", "echo two"),
        ];
        let r = run_ok(files, "for f in $(find jobs -name '*.bl'); do run $f; done").await;
        // find sorts; v1 has no field-splitting so $(find) is one word, but here
        // there's exactly one match per iteration path is fine — assert both ran.
        assert!(r.stdout.contains("one") && r.stdout.contains("two"), "{:?}", r.stdout);
    }

    #[tokio::test]
    async fn run_missing_file_is_nonzero_not_fatal() {
        let r = run_ok(&[], "run nope.bl\necho after=$?").await;
        assert!(r.stderr.contains("nope.bl"));
        assert!(r.stdout.contains("after=1"));
    }

    #[tokio::test]
    async fn run_broken_subscript_is_nonzero_not_fatal() {
        // A syntactically broken child must not kill the parent — exit 2, continue.
        let files = &[("/bad.bl", "if [ 1 -eq 1 ]; then echo a")]; // missing `fi`
        let r = run_ok(files, "run bad.bl\necho after=$?").await;
        assert!(r.stdout.contains("after=2"), "{:?}", r.stdout);
    }

    #[tokio::test]
    async fn run_self_recursion_is_bounded_by_fuel() {
        // self.bl runs self.bl … — fractal but FINITE: shared fuel stops it with a
        // clean Fuel error, never a hang or a stack blow-up.
        let mut host = TestHost::new(&[("/self.bl", "run self.bl")]);
        let err = run_with_fuel(&mut host, "run self.bl", 200).await.unwrap_err();
        assert_eq!(err.kind, BashErrorKind::Fuel);
    }

    // -------- host extension seam: run_builtin override --------

    /// A host that adds a custom `greet` command on top of the fs builtins — the
    /// same mechanism the CLI/browser hosts use to add `lh-*` platform commands.
    struct ExtHost {
        fs: MemFs,
    }
    #[async_trait::async_trait(?Send)]
    impl BashHost for ExtHost {
        fn fs(&self) -> &dyn Filesystem {
            &self.fs
        }
        async fn run_builtin(&mut self, cwd: &str, cmd: &str, args: &[String], stdin: &str) -> Output {
            if cmd == "greet" {
                let who = args.first().map(String::as_str).unwrap_or("world");
                return Output::ok(format!("hello {who}\n"));
            }
            crate::bashlite::builtins::dispatch_in(&self.fs, cwd, cmd, args, stdin).await
        }
    }

    #[tokio::test]
    async fn host_run_builtin_override_adds_a_command() {
        let mut host = ExtHost { fs: MemFs::with(&[("/a.txt", "x\n")]) };
        // The custom command works, pipes compose with it, and fs builtins
        // (ls/cat) still fall through to the default dispatch.
        let r = run(&mut host, "greet bashlite | wc -w\nls\ncat a.txt").await.unwrap();
        assert_eq!(r.stdout, "2\na.txt\nx\n"); // "hello bashlite" = 2 words; ls; cat
    }
}
