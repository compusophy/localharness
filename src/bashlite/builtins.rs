//! The v1 builtin commands — fs-only, over [`crate::filesystem::Filesystem`].
//!
//! Read/search/create only: `echo`, `ls`, `cat`, `wc`, `grep`, `find`, `mkdir`,
//! and `write`/`create` (create-only — they refuse to overwrite). NO
//! value-moving / `lh-*` platform commands (v2). `cd` is NOT here — it mutates
//! the evaluator's current directory, so the evaluator handles it directly.
//!
//! Every builtin is TOTAL: a bad path / missing file / regex error becomes a
//! nonzero exit + stderr text, never a panic. Paths are resolved against `cwd`
//! ([`resolve`]); the sandbox root is `/` and there is no escaping it
//! (`..` that would climb above root clamps to root).

use crate::filesystem::{EntryKind, Filesystem};

use super::host::Output;

/// Dispatch a builtin by name. Unknown commands return a nonzero exit (so a
/// script can branch on it) rather than erroring the whole run — matching a
/// shell's `command not found` (exit 127).
pub async fn dispatch(fs: &dyn Filesystem, cmd: &str, args: &[String], stdin: &str) -> Output {
    dispatch_in(fs, "/", cmd, args, stdin).await
}

/// Dispatch with an explicit current directory (the evaluator's `cwd`).
pub async fn dispatch_in(
    fs: &dyn Filesystem,
    cwd: &str,
    cmd: &str,
    args: &[String],
    stdin: &str,
) -> Output {
    match cmd {
        "echo" => echo(args),
        "true" => Output::ok(""),
        "false" => Output { stdout: String::new(), stderr: String::new(), code: 1 },
        "[" | "test" => test_cmd(cmd, args),
        "ls" => ls(fs, cwd, args).await,
        "cat" => cat(fs, cwd, args).await,
        "wc" => Output::ok(wc(args, stdin)),
        "grep" => grep(args, stdin),
        "find" => find(fs, cwd, args).await,
        "mkdir" => mkdir(fs, cwd, args).await,
        "write" | "create" => write_create(fs, cwd, args).await,
        other => Output::err(format!("{other}: command not found"), 127),
    }
}

/// `echo [-n] ARGS...` — print args space-joined + newline (`-n` omits it).
fn echo(args: &[String]) -> Output {
    let (no_newline, rest) = match args.first() {
        Some(f) if f == "-n" => (true, &args[1..]),
        _ => (false, args),
    };
    let mut s = rest.join(" ");
    if !no_newline {
        s.push('\n');
    }
    Output::ok(s)
}

/// `ls [path...]` — list directory contents (names only, one per line, sorted).
/// With no args, lists `cwd`. A file path lists just that name.
async fn ls(fs: &dyn Filesystem, cwd: &str, args: &[String]) -> Output {
    let targets: Vec<String> =
        if args.is_empty() { vec![cwd.to_string()] } else { args.iter().map(|a| resolve(cwd, a)).collect() };
    let multi = targets.len() > 1;
    let mut out = String::new();
    let mut code = 0;
    for (i, t) in targets.iter().enumerate() {
        match fs.metadata(t).await {
            Ok(Some(m)) if m.kind == EntryKind::Directory => {
                match fs.read_dir(t).await {
                    Ok(mut entries) => {
                        entries.sort_by(|a, b| a.name.cmp(&b.name));
                        if multi {
                            out.push_str(&format!("{}:\n", display_path(t)));
                        }
                        for e in entries {
                            out.push_str(&e.name);
                            out.push('\n');
                        }
                        if multi && i + 1 < targets.len() {
                            out.push('\n');
                        }
                    }
                    Err(e) => {
                        return Output::err(format!("ls: {}: {e}", display_path(t)), 1);
                    }
                }
            }
            Ok(Some(_)) => {
                // A plain file — echo the path (like `ls file`).
                out.push_str(&display_path(t));
                out.push('\n');
            }
            Ok(None) => {
                return Output::err(format!("ls: {}: no such file or directory", display_path(t)), 1);
            }
            Err(e) => {
                code = 1;
                out.push_str(&format!("ls: {}: {e}\n", display_path(t)));
            }
        }
    }
    Output { stdout: out, stderr: String::new(), code }
}

/// `cat PATH...` — concatenate file contents to stdout.
async fn cat(fs: &dyn Filesystem, cwd: &str, args: &[String]) -> Output {
    if args.is_empty() {
        return Output::err("cat: no file given", 1);
    }
    let mut out = String::new();
    for a in args {
        let p = resolve(cwd, a);
        match fs.read(&p).await {
            Ok(bytes) => out.push_str(&String::from_utf8_lossy(&bytes)),
            Err(e) => return Output::err(format!("cat: {a}: {e}"), 1),
        }
    }
    Output::ok(out)
}

/// `wc [-l|-w|-c]` — count lines/words/bytes of stdin (default: `lines words
/// bytes`). Operates on piped stdin only in v1 (no file args).
fn wc(args: &[String], stdin: &str) -> String {
    let lines = stdin.lines().count();
    let words = stdin.split_whitespace().count();
    let bytes = stdin.len();
    match args.first().map(String::as_str) {
        Some("-l") => format!("{lines}\n"),
        Some("-w") => format!("{words}\n"),
        Some("-c") => format!("{bytes}\n"),
        _ => format!("{lines} {words} {bytes}\n"),
    }
}

/// `grep PATTERN` — filter stdin to lines containing PATTERN (literal substring;
/// `-i` case-insensitive, `-v` invert, `-c` count). v1 is a literal matcher (no
/// regex) — deterministic + total, and enough for the common pipe filter. (v2:
/// real regex via the existing search_directory engine.)
fn grep(args: &[String], stdin: &str) -> Output {
    let mut invert = false;
    let mut ignore_case = false;
    let mut count = false;
    let mut pattern: Option<&str> = None;
    for a in args {
        match a.as_str() {
            "-v" => invert = true,
            "-i" => ignore_case = true,
            "-c" => count = true,
            // Combined short flags like `-iv`.
            s if s.starts_with('-') && s.len() > 1 && s[1..].chars().all(|c| "vic".contains(c)) => {
                for c in s[1..].chars() {
                    match c {
                        'v' => invert = true,
                        'i' => ignore_case = true,
                        'c' => count = true,
                        _ => {}
                    }
                }
            }
            _ => {
                if pattern.is_none() {
                    pattern = Some(a);
                }
            }
        }
    }
    let Some(pat) = pattern else {
        return Output::err("grep: no pattern given", 2);
    };
    let needle = if ignore_case { pat.to_lowercase() } else { pat.to_string() };
    let mut matched = Vec::new();
    for line in stdin.lines() {
        let hay = if ignore_case { line.to_lowercase() } else { line.to_string() };
        let hit = hay.contains(&needle);
        if hit != invert {
            matched.push(line);
        }
    }
    if count {
        return Output::ok(format!("{}\n", matched.len()));
    }
    // grep's exit is 1 when nothing matched (lets scripts branch on it).
    let code = if matched.is_empty() { 1 } else { 0 };
    let mut out = matched.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    Output { stdout: out, stderr: String::new(), code }
}

/// `find [path] [-name GLOB] [-type f|d]` — recursively list paths under `path`
/// (default `cwd`), optionally filtered by a name glob and/or entry type.
async fn find(fs: &dyn Filesystem, cwd: &str, args: &[String]) -> Output {
    let mut root: Option<String> = None;
    let mut name_glob: Option<String> = None;
    let mut type_filter: Option<EntryKind> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-name" => {
                i += 1;
                match args.get(i) {
                    Some(g) => name_glob = Some(g.clone()),
                    None => return Output::err("find: -name needs an argument", 1),
                }
            }
            "-type" => {
                i += 1;
                match args.get(i).map(String::as_str) {
                    Some("f") => type_filter = Some(EntryKind::File),
                    Some("d") => type_filter = Some(EntryKind::Directory),
                    _ => return Output::err("find: -type expects f or d", 1),
                }
            }
            other if root.is_none() && !other.starts_with('-') => root = Some(resolve(cwd, other)),
            other => return Output::err(format!("find: unknown argument: {other}"), 1),
        }
        i += 1;
    }
    let root = root.unwrap_or_else(|| cwd.to_string());
    match fs.walk(&root, None).await {
        Ok(mut entries) => {
            entries.sort_by(|a, b| a.path.cmp(&b.path));
            let mut out = String::new();
            for e in entries {
                if let Some(t) = type_filter {
                    if e.kind != t {
                        continue;
                    }
                }
                if let Some(g) = &name_glob {
                    let name = crate::filesystem::file_name(&e.path);
                    if !glob_match(g, name) {
                        continue;
                    }
                }
                out.push_str(&display_path(&e.path));
                out.push('\n');
            }
            Output::ok(out)
        }
        Err(e) => Output::err(format!("find: {}: {e}", display_path(&root)), 1),
    }
}

/// `mkdir PATH...` — create a directory (and parents). Implemented by writing a
/// `.keep` marker under it (OPFS/Native both auto-create parent dirs on a write,
/// and OPFS has no empty-dir concept). Idempotent.
async fn mkdir(fs: &dyn Filesystem, cwd: &str, args: &[String]) -> Output {
    if args.is_empty() {
        return Output::err("mkdir: no directory given", 1);
    }
    for a in args {
        // `-p` is the default behaviour (parents always created); accept + skip.
        if a == "-p" {
            continue;
        }
        let dir = resolve(cwd, a);
        let marker = format!("{}/.keep", dir.trim_end_matches('/'));
        if let Err(e) = fs.write_atomic(&marker, b"").await {
            return Output::err(format!("mkdir: {a}: {e}"), 1);
        }
    }
    Output::ok("")
}

/// `write PATH CONTENT...` / `create PATH CONTENT...` — create a NEW file with
/// the space-joined content (CREATE ONLY: refuses to overwrite an existing
/// file, so a script can't clobber data in v1). With no content, writes piped
/// stdin instead.
async fn write_create(fs: &dyn Filesystem, cwd: &str, args: &[String]) -> Output {
    let Some(path) = args.first() else {
        return Output::err("write: usage: write PATH [CONTENT...]", 1);
    };
    let p = resolve(cwd, path);
    match fs.metadata(&p).await {
        Ok(Some(_)) => {
            return Output::err(format!("write: {path}: already exists (create-only in v1)"), 1)
        }
        Ok(None) => {}
        Err(e) => return Output::err(format!("write: {path}: {e}"), 1),
    }
    let content = args[1..].join(" ");
    match fs.write_atomic(&p, content.as_bytes()).await {
        Ok(()) => Output::ok(""),
        Err(e) => Output::err(format!("write: {path}: {e}"), 1),
    }
}

/// `[ EXPR ]` / `test EXPR` — the POSIX test builtin (subset). Supported:
/// string `=`/`!=`, `-z`/`-n`, integer `-eq -ne -lt -le -gt -ge`, and a single
/// non-empty operand (true iff non-empty). Exit 0 = true, 1 = false, 2 = error.
fn test_cmd(cmd: &str, args: &[String]) -> Output {
    // `[` requires a trailing `]`; strip it. `test` must not have one.
    let operands: Vec<&str> = if cmd == "[" {
        match args.last() {
            Some(last) if last == "]" => args[..args.len() - 1].iter().map(String::as_str).collect(),
            _ => return Output::err("[: missing closing `]`", 2),
        }
    } else {
        args.iter().map(String::as_str).collect()
    };
    let result = eval_test(&operands);
    match result {
        Ok(true) => Output { stdout: String::new(), stderr: String::new(), code: 0 },
        Ok(false) => Output { stdout: String::new(), stderr: String::new(), code: 1 },
        Err(msg) => Output::err(format!("{cmd}: {msg}"), 2),
    }
}

fn eval_test(ops: &[&str]) -> Result<bool, String> {
    match ops {
        // Empty test is false.
        [] => Ok(false),
        // A single operand: true iff non-empty.
        [a] => Ok(!a.is_empty()),
        // Unary string ops.
        ["-z", a] => Ok(a.is_empty()),
        ["-n", a] => Ok(!a.is_empty()),
        // Binary ops.
        [a, op, b] => match *op {
            "=" | "==" => Ok(a == b),
            "!=" => Ok(a != b),
            "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge" => {
                let x: i64 = a.parse().map_err(|_| format!("integer expected: {a}"))?;
                let y: i64 = b.parse().map_err(|_| format!("integer expected: {b}"))?;
                Ok(match *op {
                    "-eq" => x == y,
                    "-ne" => x != y,
                    "-lt" => x < y,
                    "-le" => x <= y,
                    "-gt" => x > y,
                    "-ge" => x >= y,
                    _ => unreachable!(),
                })
            }
            other => Err(format!("unknown operator: {other}")),
        },
        _ => Err("too many arguments".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Path + glob helpers
// ---------------------------------------------------------------------------

/// Resolve `path` against `cwd` into a normalized absolute sandbox path. An
/// absolute `path` (leading `/`) ignores `cwd`. `.`/`..`/empty components are
/// collapsed; a `..` at root clamps to root (no sandbox escape). Always returns
/// a leading-`/` path with no trailing slash (except the root `/`).
pub(crate) fn resolve(cwd: &str, path: &str) -> String {
    let base = if path.starts_with('/') { String::new() } else { cwd.to_string() };
    let combined = format!("{base}/{path}");
    let mut stack: Vec<&str> = Vec::new();
    for comp in combined.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            c => stack.push(c),
        }
    }
    if stack.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", stack.join("/"))
    }
}

/// Render a resolved path for display: strip the leading `/` so output reads
/// like a relative listing (`a/b.rl` not `/a/b.rl`), keeping `/` for root.
fn display_path(p: &str) -> String {
    match p.strip_prefix('/') {
        Some("") | None => p.to_string(),
        Some(rest) => rest.to_string(),
    }
}

/// Minimal glob match: `*` (any run), `?` (one char), literals otherwise.
/// Anchored at both ends (whole-name match), like `find -name`.
pub(crate) fn glob_match(pat: &str, name: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let n: Vec<char> = name.chars().collect();
    glob_rec(&p, &n)
}

fn glob_rec(p: &[char], n: &[char]) -> bool {
    match p.first() {
        None => n.is_empty(),
        Some('*') => {
            // `*` matches zero+ chars: try consuming none, else one and recurse.
            glob_rec(&p[1..], n) || (!n.is_empty() && glob_rec(p, &n[1..]))
        }
        Some('?') => !n.is_empty() && glob_rec(&p[1..], &n[1..]),
        Some(c) => n.first() == Some(c) && glob_rec(&p[1..], &n[1..]),
    }
}
