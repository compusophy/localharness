
pub(crate) const KEY_SUFFIX: &str = ".localharness.key";

/// Pure resolution of the config-home dir from raw env values (extracted so it's
/// unit-testable without mutating process-global env): `$LOCALHARNESS_HOME` if
/// non-empty, else `<home>/.localharness/keys` where `<home>` is `%USERPROFILE%`
/// (Windows) / `$HOME` (Unix). `None` when none are set.
pub(crate) fn key_home_dir_from(
    localharness_home: Option<&str>,
    userprofile: Option<&str>,
    home: Option<&str>,
) -> Option<std::path::PathBuf> {
    if let Some(h) = localharness_home.filter(|s| !s.is_empty()) {
        return Some(std::path::PathBuf::from(h));
    }
    let h = userprofile
        .filter(|s| !s.is_empty())
        .or_else(|| home.filter(|s| !s.is_empty()))?;
    Some(std::path::Path::new(h).join(".localharness").join("keys"))
}

/// The config home for identity keys — the SAFE location, out of any project's
/// working directory so a private key can't be accidentally `git commit`ed
/// (the test-user fleet's security persona asked for this twice). Resolution:
/// `$LOCALHARNESS_HOME` if set, else `<home>/.localharness/keys`, where `<home>`
/// is `%USERPROFILE%` on Windows / `$HOME` on Unix. No new crate dep — the home
/// dir is read from the env. Returns `None` only if neither env var is set
/// (then we fall back to the cwd, preserving the old behavior).
pub(crate) fn key_home_dir() -> Option<std::path::PathBuf> {
    let lh = std::env::var("LOCALHARNESS_HOME").ok();
    let up = std::env::var("USERPROFILE").ok();
    let home = std::env::var("HOME").ok();
    key_home_dir_from(lh.as_deref(), up.as_deref(), home.as_deref())
}

/// The config-home path for `<name>`'s key, if a home dir is resolvable.
pub(crate) fn home_key_path(name: &str) -> Option<std::path::PathBuf> {
    key_home_dir().map(|d| d.join(format!("{name}{KEY_SUFFIX}")))
}

/// The cwd path for `<name>`'s key (the legacy / back-compat location).
pub(crate) fn cwd_key_path(name: &str) -> String {
    format!("{name}{KEY_SUFFIX}")
}

/// Pure precedence rule for reading a key (extracted so it's unit-testable
/// without touching the filesystem): the cwd path wins if it exists (back-compat
/// — pre-existing local keys and the test-fleet's keep working), else the config
/// home if that exists, else `None`.
pub(crate) fn pick_key_read_path(
    cwd: String,
    cwd_exists: bool,
    home: Option<String>,
    home_exists: bool,
) -> Option<String> {
    if cwd_exists {
        return Some(cwd);
    }
    match home {
        Some(h) if home_exists => Some(h),
        _ => None,
    }
}

/// Where to READ `<name>`'s key from, honoring back-compat: prefer the cwd file
/// if it exists (so keys created before this change, and the test-fleet's, keep
/// resolving), else the config home. `None` when neither exists.
pub(crate) fn resolve_key_read_path(name: &str) -> Option<String> {
    let cwd = cwd_key_path(name);
    let cwd_exists = std::path::Path::new(&cwd).exists();
    let home = home_key_path(name);
    let home_exists = home.as_ref().map(|p| p.exists()).unwrap_or(false);
    pick_key_read_path(
        cwd,
        cwd_exists,
        home.map(|p| p.to_string_lossy().into_owned()),
        home_exists,
    )
}

/// Where to WRITE a NEW key for `<name>`: the config home (the safe default),
/// creating the directory first. Falls back to the cwd if no home dir is
/// resolvable or the directory can't be created — never blocks a `create`.
/// Existing cwd keys are left untouched (this only governs fresh writes).
pub(crate) fn key_write_path(name: &str) -> String {
    if let Some(home) = home_key_path(name) {
        if let Some(dir) = home.parent() {
            if std::fs::create_dir_all(dir).is_ok() {
                // Owner-only dir perms where the platform supports it.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
                }
                return home.to_string_lossy().into_owned();
            }
        }
    }
    cwd_key_path(name)
}

/// Sorted display paths of every identity key, scanning BOTH the working
/// directory (back-compat) and the config home, deduped by name (cwd wins so a
/// local key shadows a same-named home key). The returned strings are usable
/// paths (relative for cwd, absolute for the home dir).
pub(crate) fn identity_key_files() -> Result<Vec<String>, String> {
    use std::collections::BTreeMap;
    // stem (name) -> path. cwd inserted last so it overrides a home key.
    let mut by_name: BTreeMap<String, String> = BTreeMap::new();
    let mut scan = |dir: &std::path::Path, absolute: bool| {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                if let Ok(f) = e.file_name().into_string() {
                    if let Some(stem) = f.strip_suffix(KEY_SUFFIX) {
                        let path = if absolute {
                            dir.join(&f).to_string_lossy().into_owned()
                        } else {
                            f.clone()
                        };
                        by_name.insert(stem.to_string(), path);
                    }
                }
            }
        }
    };
    if let Some(home) = key_home_dir() {
        scan(&home, true);
    }
    // cwd last → wins on name collision (a local key keeps working).
    scan(std::path::Path::new("."), false);
    Ok(by_name.into_values().collect())
}

/// `true` if `.gitignore` content already excludes identity keys — the wildcard
/// `*.localharness.key` or the exact file, on any non-comment line.
pub(crate) fn gitignore_already_covers(existing: &str, key_file: &str) -> bool {
    existing.lines().any(|l| {
        let t = l.trim();
        t == "*.localharness.key" || t == key_file
    })
}

/// `true` if `key_file` is a bare cwd filename (no directory component) — keys
/// in the config home live outside any project repo, so they need no
/// `.gitignore` entry.
pub(crate) fn key_is_in_cwd(key_file: &str) -> bool {
    !key_file.contains('/') && !key_file.contains('\\')
}

/// Lock down a freshly-written identity key (a fix the on-chain test-user fleet
/// asked for): owner-only file perms (0600, unix) always, plus — only for a key
/// written into the working directory (back-compat fallback) — ensure
/// `.gitignore` excludes `*.localharness.key` so a raw private key can't be
/// accidentally `git commit`ed. NEW keys now default to the config home
/// (`key_write_path`), out of any repo, so this is a belt-and-suspenders for the
/// cwd fallback. Best-effort — never fails the create. Returns whether
/// `.gitignore` was created/appended.
pub(crate) fn secure_key_file(key_file: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(key_file, std::fs::Permissions::from_mode(0o600));
    }
    // A config-home key isn't inside a project — nothing to gitignore.
    if !key_is_in_cwd(key_file) {
        return false;
    }
    match std::fs::read_to_string(".gitignore") {
        Ok(existing) => {
            if gitignore_already_covers(&existing, key_file) {
                false
            } else {
                let sep = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
                std::fs::write(".gitignore", format!("{existing}{sep}*.localharness.key\n")).is_ok()
            }
        }
        Err(_) => std::fs::write(".gitignore", "*.localharness.key\n").is_ok(),
    }
}

/// The readable identity-key path to act as. With `name`, the back-compat
/// resolved path (cwd first, else the config home — `resolve_key_read_path`);
/// the path is `<name>.localharness.key` when nothing exists yet so callers can
/// surface a "run create first" error. Without a name, the sole key across both
/// locations — error (asking for `--as`) on zero or several.
pub(crate) fn resolve_caller_file(name: Option<&str>) -> Result<String, String> {
    if let Some(n) = name {
        return Ok(resolve_key_read_path(n).unwrap_or_else(|| cwd_key_path(n)));
    }
    let mut found = identity_key_files()?;
    match found.len() {
        0 => Err(
            "no identity key — run `localharness create <yourname>` first, \
             or pass --as <name>"
                .to_string(),
        ),
        1 => Ok(found.remove(0)),
        _ => Err(format!(
            "multiple identities ({}) — pick one with --as <name>",
            found.join(", ")
        )),
    }
}

/// The thread label (key-file stem) to act as — what conversation history is
/// keyed on. Does NOT read the key, so it works for `threads` / `forget`. The
/// label is the bare name (basename stem), never the directory path.
pub(crate) fn resolve_caller_label(name: Option<&str>) -> Result<String, String> {
    if let Some(n) = name {
        return Ok(n.to_string());
    }
    let file = resolve_caller_file(None)?;
    let base = std::path::Path::new(&file)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&file);
    Ok(base.strip_suffix(KEY_SUFFIX).unwrap_or(base).to_string())
}

/// Resolve which identity key signs a `call`, returning `(filename, key_hex)`.
pub(crate) fn resolve_caller_key(name: Option<&str>) -> Result<(String, String), String> {
    let file = resolve_caller_file(name)?;
    let key_hex = std::fs::read_to_string(&file)
        .map_err(|_| match name {
            Some(n) => format!("no identity key at {file} — run `localharness create {n}` first"),
            None => format!("cannot read {file}"),
        })?
        .trim()
        .to_string();
    if key_hex.is_empty() {
        return Err(format!(
            "{file} is empty — recreate it with `localharness create <name>`"
        ));
    }
    Ok((file, key_hex))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gitignore_already_covers_detects_wildcard_and_exact() {
        // wildcard covers any key file
        assert!(gitignore_already_covers("target/\n*.localharness.key\n", "alice.localharness.key"));
        // exact filename covers itself
        assert!(gitignore_already_covers("alice.localharness.key\n", "alice.localharness.key"));
        // tolerant of surrounding whitespace
        assert!(gitignore_already_covers("  *.localharness.key  \n", "x.localharness.key"));
        // not covered → needs appending
        assert!(!gitignore_already_covers("target/\nnode_modules/\n", "alice.localharness.key"));
        // a different exact key does NOT count as covering this one
        assert!(!gitignore_already_covers("bob.localharness.key\n", "alice.localharness.key"));
        // empty gitignore → not covered
        assert!(!gitignore_already_covers("", "alice.localharness.key"));
    }

    #[test]
    fn pick_key_read_path_prefers_cwd_then_home() {
        let cwd = "alice.localharness.key".to_string();
        let home = Some("/home/me/.localharness/keys/alice.localharness.key".to_string());

        // cwd exists → cwd wins (back-compat: pre-existing local keys keep working)
        assert_eq!(
            pick_key_read_path(cwd.clone(), true, home.clone(), true),
            Some(cwd.clone())
        );
        // cwd exists even when home doesn't → still cwd
        assert_eq!(
            pick_key_read_path(cwd.clone(), true, None, false),
            Some(cwd.clone())
        );
        // cwd absent, home present → home (the new safe default location)
        assert_eq!(
            pick_key_read_path(cwd.clone(), false, home.clone(), true),
            home.clone()
        );
        // neither exists → None (caller surfaces "run create first")
        assert_eq!(pick_key_read_path(cwd.clone(), false, home.clone(), false), None);
        // no home dir resolvable and no cwd key → None
        assert_eq!(pick_key_read_path(cwd, false, None, false), None);
    }

    #[test]
    fn key_is_in_cwd_distinguishes_bare_from_pathful() {
        // a bare cwd filename → in cwd (needs .gitignore protection)
        assert!(key_is_in_cwd("alice.localharness.key"));
        // a config-home (absolute) path → NOT in cwd (no project .gitignore)
        assert!(!key_is_in_cwd("/home/me/.localharness/keys/alice.localharness.key"));
        assert!(!key_is_in_cwd("C:\\Users\\me\\.localharness\\keys\\a.localharness.key"));
    }

    #[test]
    fn key_home_dir_from_honors_override_and_falls_back() {
        use std::path::PathBuf;
        // $LOCALHARNESS_HOME wins outright when set.
        assert_eq!(
            key_home_dir_from(Some("/custom/keys"), Some("/u/prof"), Some("/u/home")),
            Some(PathBuf::from("/custom/keys"))
        );
        // No override → USERPROFILE (Windows), with the .localharness/keys suffix.
        assert_eq!(
            key_home_dir_from(None, Some("/u/prof"), None),
            Some(PathBuf::from("/u/prof").join(".localharness").join("keys"))
        );
        // No override, no USERPROFILE → HOME (Unix).
        assert_eq!(
            key_home_dir_from(None, None, Some("/u/home")),
            Some(PathBuf::from("/u/home").join(".localharness").join("keys"))
        );
        // Empty strings are treated as unset.
        assert_eq!(
            key_home_dir_from(Some(""), Some(""), Some("/u/home")),
            Some(PathBuf::from("/u/home").join(".localharness").join("keys"))
        );
        // Nothing set → None (caller falls back to the cwd).
        assert_eq!(key_home_dir_from(None, None, None), None);
    }
}
