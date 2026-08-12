//! Deciding what `orbital install` will do, as pure functions over plain data.
//!
//! Every reading of the world — the current executable, environment variables,
//! whether a directory is writable — is collected once into [`Env`] and
//! [`Probes`] at the edge. [`plan_install`] then does nothing but path and
//! string arithmetic, so the tests below cover Windows behaviour while running
//! on Linux, and no test can install anything or touch a real PATH.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::core::version::BIN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Macos,
    Unix,
}

impl Os {
    pub fn current() -> Self {
        match std::env::consts::OS {
            "windows" => Os::Windows,
            "macos" => Os::Macos,
            _ => Os::Unix,
        }
    }

    /// The character PATH entries are separated by.
    pub fn path_separator(self) -> char {
        if self == Os::Windows {
            ';'
        } else {
            ':'
        }
    }

    /// What the installed file is called.
    pub fn binary_name(self) -> String {
        if self == Os::Windows {
            format!("{BIN}.exe")
        } else {
            BIN.to_string()
        }
    }

    /// The separator between path components.
    pub fn dir_separator(self) -> char {
        if self == Os::Windows {
            '\\'
        } else {
            '/'
        }
    }
}

/// Join path components using the separator of the OS being planned for.
///
/// `Path::join` uses the *host* separator, which would make planning a Windows
/// install on a Linux CI runner (or the reverse) produce mixed separators. Every
/// path this module builds goes through here so the plan is the same on every
/// host.
fn join(os: Os, base: &str, parts: &[&str]) -> PathBuf {
    let mut text = base.trim_end_matches(['/', '\\']).to_string();
    for part in parts {
        text.push(os.dir_separator());
        text.push_str(part);
    }
    PathBuf::from(text)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallFlags {
    /// `--dir <path>`: install here instead of the platform default.
    pub dir: Option<String>,
    /// `--no-path`: place the binary but leave PATH alone.
    pub no_path: bool,
    /// `--dry-run`: report the plan and change nothing.
    pub dry_run: bool,
}

/// Parse the flags `orbital install` accepts. Unrecognised tokens are ignored,
/// matching the other commands' lenient style.
pub fn parse_install_flags(args: &[String]) -> InstallFlags {
    let mut flags = InstallFlags::default();
    let mut expecting_dir = false;

    for arg in args {
        if expecting_dir {
            expecting_dir = false;
            // A following flag means the value is missing, not that the flag is
            // the directory. Swallowing it would turn `--dir --dry-run` into a
            // real install.
            if !arg.trim().is_empty() && !arg.starts_with('-') {
                flags.dir = Some(arg.clone());
                continue;
            }
        }
        match arg.as_str() {
            "--no-path" | "--skip-path" => flags.no_path = true,
            "--dry-run" | "-n" => flags.dry_run = true,
            "--dir" | "-d" => expecting_dir = true,
            other => {
                if let Some(value) = other
                    .strip_prefix("--dir=")
                    .or_else(|| other.strip_prefix("-d="))
                {
                    if !value.trim().is_empty() {
                        flags.dir = Some(value.to_string());
                    }
                }
            }
        }
    }

    flags
}

/// Everything read from the world, gathered once so planning stays pure.
#[derive(Debug, Clone)]
pub struct Env {
    pub os: Os,
    /// The running binary, canonicalized.
    pub exe: PathBuf,
    /// `ORBITAL_INSTALL_DIR`, honoured by `install.sh`/`install.ps1` too.
    pub install_dir_var: Option<String>,
    /// Environment variables the candidate directories are built from.
    pub vars: BTreeMap<String, String>,
    /// The working directory, used to absolutize a relative `--dir`.
    pub cwd: PathBuf,
    /// True when running under `sudo`. `HOME` is then root's on Linux but the
    /// invoking user's on macOS, and nothing in-process can tell which.
    pub sudo: bool,
    /// The process `PATH`, used only to answer "is this directory on PATH?".
    pub search_path: String,
    /// Windows only: the *user* PATH from the registry, which is the only value
    /// [`crate::commands::install`] ever writes back. Never the process PATH,
    /// which is machine and user merged.
    pub user_path: Option<String>,
}

impl Env {
    /// Windows environment variable names are case-insensitive, and PATH
    /// entries in the wild are written `%LocalAppData%` as often as
    /// `%LOCALAPPDATA%`.
    fn lookup(&self, name: &str) -> Option<&str> {
        if self.os == Os::Windows {
            self.vars
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        } else {
            self.vars.get(name).map(String::as_str)
        }
    }

    fn var(&self, name: &str) -> Option<&str> {
        self.lookup(name).filter(|v| !v.trim().is_empty())
    }

    /// A variable's value, for callers outside this module.
    pub fn var_opt(&self, name: &str) -> Option<&str> {
        self.var(name)
    }

    /// The expander used when normalising Windows PATH entries.
    pub fn expand_var(&self, name: &str) -> Option<String> {
        self.lookup(name).map(str::to_string)
    }
}

/// Which candidate directories can actually be written to.
#[derive(Debug, Clone, Default)]
pub struct Probes(BTreeMap<PathBuf, bool>);

impl Probes {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn set(&mut self, dir: impl Into<PathBuf>, writable: bool) {
        self.0.insert(dir.into(), writable);
    }

    fn writable(&self, dir: &Path) -> bool {
        self.0.get(dir).copied().unwrap_or(false)
    }
}

/// What the install will do about PATH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathAction {
    /// The directory is already on PATH; nothing to do.
    AlreadyOnPath,
    /// Windows: append the directory to the user PATH in the registry.
    UpdateUserPath,
    /// Unix: tell the user how to add it, but never edit their shell profile.
    ShowHint,
    /// `--no-path` was given.
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub source: PathBuf,
    pub dir: PathBuf,
    pub target: PathBuf,
    /// The target is the very file we are executing — there is nothing to copy.
    pub already_installed: bool,
    pub path_action: PathAction,
    pub dry_run: bool,
}

/// Work out where the binary should go and what PATH needs.
pub fn plan_install(env: &Env, probes: &Probes, flags: &InstallFlags) -> Result<Plan, String> {
    let dir = resolve_dir(env, probes, flags)?;
    let target = join(env.os, &strip_verbatim(&dir), &[&env.os.binary_name()]);

    // Windows can't rename a running .exe aside onto itself, and on Unix this
    // would copy a file over itself; either way there is nothing to do.
    let already_installed = same_path(&target, &env.exe, env.os);

    let on_path = path_contains(&env.search_path, &dir, env.os, &|name| env.expand_var(name))
        || env
            .user_path
            .as_deref()
            .is_some_and(|user| path_contains(user, &dir, env.os, &|name| env.expand_var(name)));

    let path_action = if flags.no_path {
        PathAction::Skipped
    } else if on_path {
        PathAction::AlreadyOnPath
    } else if env.os == Os::Windows {
        PathAction::UpdateUserPath
    } else {
        PathAction::ShowHint
    };

    // A PATH entry has no escaping form that every consumer honours, so a
    // directory containing the separator can only be refused: appending it would
    // corrupt the value into two entries that both point nowhere.
    if path_action != PathAction::Skipped {
        let entry = strip_verbatim(&dir);
        let separator = env.os.path_separator();
        if entry.contains(separator) || entry.contains('\"') {
            return Err(format!(
                "{entry} cannot go on PATH: a `{separator}` or `\"` in the path would corrupt it. Install somewhere else, or pass --no-path."
            ));
        }
    }

    Ok(Plan {
        source: env.exe.clone(),
        dir,
        target,
        already_installed,
        path_action,
        dry_run: flags.dry_run,
    })
}

/// `--dir` wins, then `ORBITAL_INSTALL_DIR`, then the platform default.
fn resolve_dir(env: &Env, probes: &Probes, flags: &InstallFlags) -> Result<PathBuf, String> {
    if let Some(dir) = flags.dir.as_deref().filter(|d| !d.trim().is_empty()) {
        return absolutize(env, dir);
    }
    // Matches the `ORBITAL_INSTALL_DIR` knob install.sh and install.ps1 honour.
    if let Some(dir) = env
        .install_dir_var
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        return absolutize(env, dir);
    }
    default_dir(env, probes)
}

/// Turn a user-supplied directory into an absolute path.
///
/// A relative directory must never reach PATH: `orbital install --dir .` would
/// otherwise append `.` to the user's PATH permanently, making every command
/// lookup in every program they run search the working directory first — the
/// classic dot-on-PATH execution hazard.
fn absolutize(env: &Env, raw: &str) -> Result<PathBuf, String> {
    let text = raw.trim();

    // A shell expands `~` before the program sees it; arriving literally, it has
    // to be expanded here or we would create a directory actually named `~`.
    let tilde = text == "~" || text.starts_with("~/") || text.starts_with(r"~\");
    let expanded = if tilde {
        let home = env
            .var("HOME")
            .or_else(|| env.var("USERPROFILE"))
            .ok_or_else(|| format!("Cannot expand `~` in {text} — pass an absolute path."))?;
        let rest = text[1..].trim_start_matches(['/', '\\']);
        if rest.is_empty() {
            home.to_string()
        } else {
            strip_verbatim(&join(env.os, home, &[rest]))
        }
    } else {
        text.to_string()
    };

    if is_absolute_for(env.os, &expanded) {
        return Ok(PathBuf::from(expanded));
    }

    let cwd = strip_verbatim(&env.cwd);
    if cwd.is_empty() {
        return Err(format!(
            "{text} is relative and the working directory is unknown — pass an absolute path."
        ));
    }
    Ok(join(env.os, &cwd, &[&expanded]))
}

/// Is this path absolute *for the OS being planned for*? `Path::is_absolute`
/// answers for the host, which is the wrong question here.
pub fn is_absolute_for(os: Os, text: &str) -> bool {
    if os == Os::Windows {
        if text.starts_with(r"\\") || text.starts_with("//") {
            return true; // UNC, or a verbatim prefix
        }
        let bytes = text.as_bytes();
        // `C:\x` is absolute; `C:x` is relative to that drive's own directory.
        bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/')
    } else {
        text.starts_with('/')
    }
}

/// The default install directory.
///
/// Deliberately agrees with the standalone installers: `install.ps1` uses
/// `%LOCALAPPDATA%\orbital\bin` and `install.sh` prefers `/usr/local/bin` when
/// it is writable, falling back to `~/.local/bin`. Drifting from them would put
/// two copies of `orbital` on PATH.
pub fn default_dir(env: &Env, probes: &Probes) -> Result<PathBuf, String> {
    if env.os == Os::Windows {
        // Local rather than Roaming: a native binary must not sync to another
        // machine or architecture.
        if let Some(local) = env.var("LOCALAPPDATA") {
            return Ok(join(env.os, local, &[BIN, "bin"]));
        }
        if let Some(profile) = env.var("USERPROFILE") {
            return Ok(join(env.os, profile, &["AppData", "Local", BIN, "bin"]));
        }
        return Err("Could not determine %LOCALAPPDATA% — pass --dir <path>.".to_string());
    }

    // Prefer a directory that is both writable and already on PATH, so the
    // install needs no follow-up from the user. Under `sudo` /usr/local/bin is
    // writable and wins, which is also how the home-directory trap is avoided:
    // HOME under sudo is root's on Linux but the user's on macOS, and nothing
    // inside the process can tell which.
    let candidates = unix_candidates(env);
    for dir in &candidates {
        if probes.writable(dir)
            && path_contains(&env.search_path, dir, env.os, &|name| {
                env.vars.get(name).cloned()
            })
        {
            return Ok(dir.clone());
        }
    }
    // Otherwise the first writable one, which keeps us out of system
    // directories that would need a password we must never prompt for.
    for dir in &candidates {
        if probes.writable(dir) {
            return Ok(dir.clone());
        }
    }
    if env.sudo {
        return Err("Running under sudo, but /usr/local/bin is not writable — pass --dir <path>, or run without sudo.".to_string());
    }
    // Nothing was writable yet — fall back to the user-owned directory the
    // install can create itself.
    home_bin(env).ok_or_else(|| {
        "Could not determine a writable install directory — pass --dir <path>.".to_string()
    })
}

/// Unix candidates in preference order.
pub fn unix_candidates(env: &Env) -> Vec<PathBuf> {
    // Writable /usr/local/bin means either a Homebrew-style prefix or root, and
    // it is on PATH essentially everywhere.
    let mut candidates = vec![PathBuf::from("/usr/local/bin")];
    // Under `sudo` every home-directory candidate is wrong: on Linux HOME is
    // root's, so the install lands in /root where the user never sees it; on
    // macOS HOME is still theirs, so root-owned files land in their home and
    // break every later non-root run.
    if !env.sudo {
        if let Some(xdg) = env.var("XDG_BIN_HOME") {
            candidates.push(PathBuf::from(xdg));
        }
        if let Some(home) = home_bin(env) {
            candidates.push(home);
        }
        if let Some(home) = env.var("HOME") {
            candidates.push(join(env.os, home, &["bin"]));
        }
    }
    candidates.dedup();
    candidates
}

fn home_bin(env: &Env) -> Option<PathBuf> {
    env.var("HOME")
        .map(|home| join(env.os, home, &[".local", "bin"]))
}

/// Compare two paths for "the same file", tolerating trailing separators and,
/// on Windows, case differences.
pub fn same_path(a: &Path, b: &Path, os: Os) -> bool {
    let clean = |p: &Path| {
        let text = strip_verbatim(p);
        let trimmed = text.trim_end_matches(['/', '\\']);
        // Keep a bare root: `C:\` must not collapse to `C:`.
        let kept = if trimmed.is_empty() {
            text.as_str()
        } else {
            trimmed
        };
        if os == Os::Windows {
            kept.replace('/', "\\").to_lowercase()
        } else {
            kept.to_string()
        }
    };
    clean(a) == clean(b)
}

/// Windows verbatim paths (`\\?\C:\…`) come back from `canonicalize`. They are
/// unusable in PATH and ugly in output, so they are stripped for both.
pub fn strip_verbatim(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
}

/// Normalise one PATH entry for comparison, resolving `%VAR%` on Windows.
///
/// Entries in the wild are quoted, have trailing separators, use the wrong slash,
/// differ in case, and — on Windows — are frequently stored unexpanded.
fn normalize_entry(entry: &str, os: Os, expand: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let mut text = entry.trim().trim_matches('"').trim().to_string();
    if text.is_empty() {
        return None;
    }
    if os == Os::Windows {
        text = expand_vars(&text, expand);
        text = text.replace('/', "\\");
    }
    let trimmed = text.trim_end_matches(if os == Os::Windows { '\\' } else { '/' });
    let kept = if trimmed.is_empty() { &text } else { trimmed };
    Some(if os == Os::Windows {
        // `to_lowercase`, not `eq_ignore_ascii_case`: profile names are not ASCII.
        kept.to_lowercase()
    } else {
        kept.to_string()
    })
}

/// Expand `%VAR%` references. Unknown variables are left as written.
pub fn expand_vars(text: &str, expand: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('%') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('%') {
            Some(close) => {
                let name = &after[..close];
                match expand(name) {
                    Some(value) => out.push_str(&value),
                    // `%%` or an unknown name: keep it verbatim.
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push('%');
                out.push_str(after);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Is `dir` already one of the entries in `path_var`?
pub fn path_contains(
    path_var: &str,
    dir: &Path,
    os: Os,
    expand: &dyn Fn(&str) -> Option<String>,
) -> bool {
    let wanted = match normalize_entry(&strip_verbatim(dir), os, expand) {
        Some(wanted) => wanted,
        None => return false,
    };
    path_var
        .split(os.path_separator())
        .filter_map(|entry| normalize_entry(entry, os, expand))
        .any(|entry| entry == wanted)
}

/// Append `dir` to an existing PATH value.
///
/// The existing value is treated as opaque: it is never expanded, re-quoted or
/// reordered, because on Windows it routinely contains `%VAR%` references that
/// must survive verbatim.
pub fn append_entry(existing: &str, dir: &str, os: Os) -> String {
    let separator = os.path_separator();
    let trimmed = existing.trim_end_matches(separator);
    if trimmed.is_empty() {
        dir.to_string()
    } else {
        format!("{trimmed}{separator}{dir}")
    }
}

/// Append an entry to a raw UTF-16 PATH value.
///
/// The Windows user PATH is appended to as UTF-16 rather than as a `String`
/// because the stored value is written back verbatim: decoding and re-encoding
/// it could mangle an unpaired surrogate, and the `%VAR%` references it usually
/// contains must survive untouched.
///
/// Only *called* on Windows, but deliberately not `#[cfg(windows)]`: keeping it
/// compiled everywhere is what lets the tests below cover it on every CI
/// runner.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn append_entry_utf16(existing: &[u16], entry: &str) -> Vec<u16> {
    const SEPARATOR: u16 = b';' as u16;
    let mut out = existing.to_vec();
    // Drop trailing separators so no empty segment is created.
    while out.last() == Some(&SEPARATOR) {
        out.pop();
    }
    if !out.is_empty() {
        out.push(SEPARATOR);
    }
    out.extend(entry.encode_utf16());
    out
}

/// Resolve `orbital` the way a shell would, so the install can warn when an
/// older copy earlier on PATH would keep winning.
pub fn resolve_on_path(
    path_var: &str,
    os: Os,
    exists: &dyn Fn(&Path) -> bool,
    expand: &dyn Fn(&str) -> Option<String>,
) -> Option<PathBuf> {
    // Directories are the outer loop and extensions the inner one — that is the
    // order cmd.exe uses, and it means directory order beats PATHEXT order.
    let names: Vec<String> = if os == Os::Windows {
        vec![
            format!("{BIN}.exe"),
            format!("{BIN}.cmd"),
            format!("{BIN}.bat"),
        ]
    } else {
        vec![BIN.to_string()]
    };

    for entry in path_var.split(os.path_separator()) {
        let entry = entry.trim().trim_matches('"').trim();
        if entry.is_empty() {
            continue;
        }
        let dir = if os == Os::Windows {
            expand_vars(entry, expand)
        } else {
            entry.to_string()
        };
        for name in &names {
            let candidate = join(os, &dir, &[name]);
            if exists(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn no_vars(_: &str) -> Option<String> {
        None
    }

    fn env(os: Os) -> Env {
        let mut vars = BTreeMap::new();
        match os {
            Os::Windows => {
                vars.insert("LOCALAPPDATA".into(), r"C:\Users\dev\AppData\Local".into());
                vars.insert("USERPROFILE".into(), r"C:\Users\dev".into());
            }
            _ => {
                vars.insert("HOME".into(), "/home/dev".into());
            }
        }
        Env {
            os,
            exe: PathBuf::from(if os == Os::Windows {
                r"D:\src\orbital\target\release\orbital.exe"
            } else {
                "/src/orbital/target/release/orbital"
            }),
            install_dir_var: None,
            vars,
            cwd: PathBuf::from(if os == Os::Windows {
                r"D:\work"
            } else {
                "/work"
            }),
            sudo: false,
            search_path: String::new(),
            user_path: None,
        }
    }

    fn probes(dirs: &[(&str, bool)]) -> Probes {
        let mut probes = Probes::new();
        for (dir, writable) in dirs {
            probes.set(PathBuf::from(dir), *writable);
        }
        probes
    }

    #[test]
    fn flags_default_to_a_plain_install() {
        assert_eq!(parse_install_flags(&[]), InstallFlags::default());
    }

    #[test]
    fn parses_every_flag_form() {
        let flags = parse_install_flags(&args(&["--dir", "/opt/bin", "--no-path", "--dry-run"]));
        assert_eq!(flags.dir.as_deref(), Some("/opt/bin"));
        assert!(flags.no_path);
        assert!(flags.dry_run);

        assert_eq!(
            parse_install_flags(&args(&["--dir=/opt/bin"]))
                .dir
                .as_deref(),
            Some("/opt/bin")
        );
        assert_eq!(
            parse_install_flags(&args(&["-d", "/opt/bin"]))
                .dir
                .as_deref(),
            Some("/opt/bin")
        );
        assert!(parse_install_flags(&args(&["-n"])).dry_run);
        assert!(parse_install_flags(&args(&["--skip-path"])).no_path);
    }

    #[test]
    fn a_dangling_dir_flag_is_ignored() {
        assert!(parse_install_flags(&args(&["--dir"])).dir.is_none());
        assert!(parse_install_flags(&args(&["--dir", ""])).dir.is_none());
    }

    #[test]
    fn windows_defaults_to_local_appdata() {
        let plan =
            plan_install(&env(Os::Windows), &Probes::new(), &InstallFlags::default()).unwrap();
        assert_eq!(
            plan.dir,
            PathBuf::from(r"C:\Users\dev\AppData\Local\orbital\bin")
        );
        // Compared as a string: `Path::file_name` parses with *host* rules, so
        // on Linux this whole backslash path is one component.
        assert_eq!(
            strip_verbatim(&plan.target),
            r"C:\Users\dev\AppData\Local\orbital\bin\orbital.exe"
        );
    }

    #[test]
    fn windows_falls_back_to_the_user_profile() {
        let mut env = env(Os::Windows);
        env.vars.remove("LOCALAPPDATA");
        let plan = plan_install(&env, &Probes::new(), &InstallFlags::default()).unwrap();
        assert_eq!(
            plan.dir,
            PathBuf::from(r"C:\Users\dev\AppData\Local\orbital\bin")
        );
    }

    #[test]
    fn windows_without_any_profile_variable_asks_for_a_directory() {
        let mut env = env(Os::Windows);
        env.vars.clear();
        let error = plan_install(&env, &Probes::new(), &InstallFlags::default()).unwrap_err();
        assert!(error.contains("--dir"), "unhelpful error: {error}");
    }

    #[test]
    fn unix_prefers_a_writable_directory_already_on_path() {
        let mut env = env(Os::Unix);
        env.search_path = "/usr/local/bin:/usr/bin".into();
        let plan = plan_install(
            &env,
            &probes(&[("/usr/local/bin", true), ("/home/dev/.local/bin", true)]),
            &InstallFlags::default(),
        )
        .unwrap();
        assert_eq!(plan.dir, PathBuf::from("/usr/local/bin"));
        assert_eq!(plan.path_action, PathAction::AlreadyOnPath);
    }

    #[test]
    fn unix_skips_a_system_directory_it_cannot_write() {
        let mut env = env(Os::Unix);
        env.search_path = "/usr/local/bin:/usr/bin".into();
        let plan = plan_install(
            &env,
            &probes(&[("/usr/local/bin", false), ("/home/dev/.local/bin", true)]),
            &InstallFlags::default(),
        )
        .unwrap();
        assert_eq!(plan.dir, PathBuf::from("/home/dev/.local/bin"));
        // Not on PATH, so the user gets a hint — never an edited shell profile.
        assert_eq!(plan.path_action, PathAction::ShowHint);
    }

    #[test]
    fn unix_falls_back_to_home_local_bin_when_nothing_is_writable_yet() {
        let plan = plan_install(&env(Os::Unix), &Probes::new(), &InstallFlags::default()).unwrap();
        assert_eq!(plan.dir, PathBuf::from("/home/dev/.local/bin"));
    }

    #[test]
    fn unix_honours_xdg_bin_home_over_local_bin() {
        let mut env = env(Os::Unix);
        env.vars
            .insert("XDG_BIN_HOME".into(), "/home/dev/.bin".into());
        let plan = plan_install(
            &env,
            &probes(&[("/home/dev/.bin", true), ("/home/dev/.local/bin", true)]),
            &InstallFlags::default(),
        )
        .unwrap();
        assert_eq!(plan.dir, PathBuf::from("/home/dev/.bin"));
    }

    #[test]
    fn the_dir_flag_beats_the_environment_variable_and_the_default() {
        let mut env = env(Os::Unix);
        env.install_dir_var = Some("/from/env".into());
        let flags = InstallFlags {
            dir: Some("/from/flag".into()),
            ..InstallFlags::default()
        };
        let plan = plan_install(&env, &Probes::new(), &flags).unwrap();
        assert_eq!(plan.dir, PathBuf::from("/from/flag"));
    }

    #[test]
    fn the_environment_variable_beats_the_default() {
        let mut env = env(Os::Unix);
        env.install_dir_var = Some("/from/env".into());
        let plan = plan_install(&env, &Probes::new(), &InstallFlags::default()).unwrap();
        assert_eq!(plan.dir, PathBuf::from("/from/env"));
    }

    #[test]
    fn an_empty_environment_variable_falls_through_to_the_default() {
        let mut env = env(Os::Unix);
        env.install_dir_var = Some("   ".into());
        let plan = plan_install(&env, &Probes::new(), &InstallFlags::default()).unwrap();
        assert_eq!(plan.dir, PathBuf::from("/home/dev/.local/bin"));
    }

    #[test]
    fn recognizes_that_it_is_already_installed() {
        let mut env = env(Os::Unix);
        env.exe = PathBuf::from("/home/dev/.local/bin/orbital");
        let plan = plan_install(&env, &Probes::new(), &InstallFlags::default()).unwrap();
        assert!(plan.already_installed);
    }

    #[test]
    fn a_fresh_target_is_not_already_installed() {
        let plan = plan_install(&env(Os::Unix), &Probes::new(), &InstallFlags::default()).unwrap();
        assert!(!plan.already_installed);
    }

    #[test]
    fn windows_updates_the_user_path_when_the_directory_is_missing_from_it() {
        let plan =
            plan_install(&env(Os::Windows), &Probes::new(), &InstallFlags::default()).unwrap();
        assert_eq!(plan.path_action, PathAction::UpdateUserPath);
    }

    #[test]
    fn an_unexpanded_user_path_entry_still_counts_as_on_path() {
        // The real trap: the registry stores `%LOCALAPPDATA%\orbital\bin`, so a
        // literal comparison against the absolute directory would miss it and
        // append a duplicate on every install.
        let mut env = env(Os::Windows);
        env.user_path = Some(r"%LOCALAPPDATA%\orbital\bin".into());
        let plan = plan_install(&env, &Probes::new(), &InstallFlags::default()).unwrap();
        assert_eq!(plan.path_action, PathAction::AlreadyOnPath);
    }

    #[test]
    fn no_path_skips_all_path_work() {
        let flags = InstallFlags {
            no_path: true,
            ..InstallFlags::default()
        };
        let plan = plan_install(&env(Os::Windows), &Probes::new(), &flags).unwrap();
        assert_eq!(plan.path_action, PathAction::Skipped);
    }

    #[test]
    fn same_path_tolerates_separators_and_windows_case() {
        assert!(same_path(
            Path::new(r"C:\Bin\orbital.exe"),
            Path::new(r"c:\bin\orbital.exe"),
            Os::Windows
        ));
        assert!(same_path(
            Path::new(r"C:\bin\orbital.exe"),
            Path::new("C:/bin/orbital.exe"),
            Os::Windows
        ));
        assert!(!same_path(
            Path::new("/bin/orbital"),
            Path::new("/Bin/orbital"),
            Os::Unix
        ));
        assert!(same_path(
            Path::new("/bin/orbital"),
            Path::new("/bin/orbital/"),
            Os::Unix
        ));
    }

    #[test]
    fn strips_the_windows_verbatim_prefix() {
        assert_eq!(strip_verbatim(Path::new(r"\\?\C:\bin")), r"C:\bin");
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\srv\share")),
            r"\\srv\share"
        );
        assert_eq!(
            strip_verbatim(Path::new("/usr/local/bin")),
            "/usr/local/bin"
        );
    }

    #[test]
    fn path_membership_ignores_quotes_case_and_trailing_separators() {
        let path = r#""C:\Bin\";C:\Windows"#;
        assert!(path_contains(
            path,
            Path::new(r"c:\bin"),
            Os::Windows,
            &no_vars
        ));
        assert!(path_contains(
            path,
            Path::new("C:/Bin"),
            Os::Windows,
            &no_vars
        ));
        assert!(!path_contains(
            path,
            Path::new(r"C:\Other"),
            Os::Windows,
            &no_vars
        ));
    }

    #[test]
    fn path_membership_expands_variables_on_windows() {
        let expand = |name: &str| match name {
            "LOCALAPPDATA" => Some(r"C:\Users\dev\AppData\Local".to_string()),
            _ => None,
        };
        assert!(path_contains(
            r"%LOCALAPPDATA%\orbital\bin",
            Path::new(r"C:\Users\dev\AppData\Local\orbital\bin"),
            Os::Windows,
            &expand
        ));
    }

    #[test]
    fn an_empty_path_segment_matches_nothing() {
        assert!(!path_contains(
            ";;",
            Path::new("C:\\bin"),
            Os::Windows,
            &no_vars
        ));
        assert!(!path_contains("::", Path::new(""), Os::Unix, &no_vars));
    }

    #[test]
    fn unix_path_membership_is_case_sensitive() {
        assert!(path_contains(
            "/usr/local/bin",
            Path::new("/usr/local/bin/"),
            Os::Unix,
            &no_vars
        ));
        assert!(!path_contains(
            "/usr/local/bin",
            Path::new("/usr/local/BIN"),
            Os::Unix,
            &no_vars
        ));
    }

    #[test]
    fn expands_only_known_variables() {
        let expand = |name: &str| (name == "HOME").then(|| "/home/dev".to_string());
        assert_eq!(expand_vars("%HOME%/bin", &expand), "/home/dev/bin");
        assert_eq!(expand_vars("%NOPE%/bin", &expand), "%NOPE%/bin");
        assert_eq!(expand_vars("100%", &expand), "100%");
        assert_eq!(expand_vars("plain", &expand), "plain");
    }

    #[test]
    fn appends_without_disturbing_the_existing_value() {
        // The existing value must survive verbatim, `%VAR%` references included.
        assert_eq!(
            append_entry(r"%USERPROFILE%\.cargo\bin", r"C:\new", Os::Windows),
            r"%USERPROFILE%\.cargo\bin;C:\new"
        );
        assert_eq!(append_entry("/a:/b", "/c", Os::Unix), "/a:/b:/c");
    }

    #[test]
    fn appending_does_not_create_an_empty_segment() {
        assert_eq!(append_entry(r"C:\a;", r"C:\b", Os::Windows), r"C:\a;C:\b");
        assert_eq!(append_entry("", "/b", Os::Unix), "/b");
        assert_eq!(append_entry(":", "/b", Os::Unix), "/b");
    }

    #[test]
    fn appends_to_a_raw_utf16_value_the_same_way_as_to_a_string() {
        let cases = [
            (r"%USERPROFILE%\.cargo\bin", r"C:\new"),
            (r"C:\a;", r"C:\b"),
            ("", r"C:\b"),
            (";;", r"C:\b"),
        ];
        for (existing, entry) in cases {
            let raw: Vec<u16> = existing.encode_utf16().collect();
            let appended = String::from_utf16(&append_entry_utf16(&raw, entry)).unwrap();
            assert_eq!(
                appended,
                append_entry(existing, entry, Os::Windows),
                "diverged for {existing:?}"
            );
        }
    }

    #[test]
    fn appending_utf16_preserves_the_existing_units_exactly() {
        // A lone surrogate would be destroyed by a String round trip.
        let existing = vec![0xD800u16, ';' as u16];
        let appended = append_entry_utf16(&existing, "x");
        assert_eq!(appended, vec![0xD800, ';' as u16, 'x' as u16]);
    }

    #[test]
    fn a_flag_is_never_taken_as_the_directory_value() {
        // `--dir --dry-run` used to swallow the flag, turning a rehearsal into a
        // real, persistent install.
        let flags = parse_install_flags(&args(&["--dir", "--dry-run"]));
        assert!(flags.dir.is_none());
        assert!(flags.dry_run, "the flag must still be seen as a flag");
    }

    #[test]
    fn a_relative_directory_is_resolved_against_the_working_directory() {
        // Never relative: `.` on PATH means "search the working directory", so a
        // relative entry in a persistent PATH is a code-execution hazard.
        let flags = InstallFlags {
            dir: Some("tools".into()),
            ..InstallFlags::default()
        };
        let plan = plan_install(&env(Os::Unix), &Probes::new(), &flags).unwrap();
        assert_eq!(plan.dir, PathBuf::from("/work/tools"));

        let flags = InstallFlags {
            dir: Some(".".into()),
            ..InstallFlags::default()
        };
        let plan = plan_install(&env(Os::Windows), &Probes::new(), &flags).unwrap();
        assert_eq!(strip_verbatim(&plan.dir), r"D:\work\.");
    }

    #[test]
    fn a_relative_environment_variable_is_absolutized_too() {
        let mut env = env(Os::Unix);
        env.install_dir_var = Some("rel/bin".into());
        let plan = plan_install(&env, &Probes::new(), &InstallFlags::default()).unwrap();
        assert_eq!(plan.dir, PathBuf::from("/work/rel/bin"));
    }

    #[test]
    fn a_literal_tilde_is_expanded_rather_than_taken_as_a_directory_name() {
        let flags = InstallFlags {
            dir: Some("~/bin".into()),
            ..InstallFlags::default()
        };
        let plan = plan_install(&env(Os::Unix), &Probes::new(), &flags).unwrap();
        assert_eq!(plan.dir, PathBuf::from("/home/dev/bin"));

        let flags = InstallFlags {
            dir: Some("~".into()),
            ..InstallFlags::default()
        };
        let plan = plan_install(&env(Os::Unix), &Probes::new(), &flags).unwrap();
        assert_eq!(plan.dir, PathBuf::from("/home/dev"));
    }

    #[test]
    fn an_absolute_directory_is_left_alone() {
        for (os, dir) in [
            (Os::Unix, "/opt/bin"),
            (Os::Windows, r"C:\tools"),
            (Os::Windows, r"\\server\share"),
        ] {
            let flags = InstallFlags {
                dir: Some(dir.to_string()),
                ..InstallFlags::default()
            };
            let plan = plan_install(&env(os), &Probes::new(), &flags).unwrap();
            assert_eq!(strip_verbatim(&plan.dir), dir);
        }
    }

    #[test]
    fn recognizes_absolute_paths_for_the_planned_os() {
        assert!(is_absolute_for(Os::Windows, r"C:\x"));
        assert!(is_absolute_for(Os::Windows, "C:/x"));
        assert!(is_absolute_for(Os::Windows, r"\\server\share"));
        // Drive-relative: `C:x` means "x, in C:'s own current directory".
        assert!(!is_absolute_for(Os::Windows, "C:x"));
        assert!(!is_absolute_for(Os::Windows, r"\x"));
        assert!(!is_absolute_for(Os::Windows, "x"));
        assert!(is_absolute_for(Os::Unix, "/x"));
        assert!(!is_absolute_for(Os::Unix, "x"));
        assert!(!is_absolute_for(Os::Unix, r"C:\x"));
    }

    #[test]
    fn sudo_never_installs_into_a_home_directory() {
        // HOME under sudo is root's on Linux but the user's on macOS, and the
        // process cannot tell which; either choice is wrong.
        let mut env = env(Os::Unix);
        env.sudo = true;
        let candidates = unix_candidates(&env);
        assert_eq!(candidates, vec![PathBuf::from("/usr/local/bin")]);

        let plan = plan_install(
            &env,
            &probes(&[("/usr/local/bin", true)]),
            &InstallFlags::default(),
        )
        .unwrap();
        assert_eq!(plan.dir, PathBuf::from("/usr/local/bin"));
    }

    #[test]
    fn sudo_without_a_writable_system_directory_asks_rather_than_guessing() {
        let mut env = env(Os::Unix);
        env.sudo = true;
        let error = plan_install(&env, &Probes::new(), &InstallFlags::default()).unwrap_err();
        assert!(error.contains("sudo"), "unhelpful error: {error}");
        assert!(error.contains("--dir"));
    }

    #[test]
    fn a_directory_containing_the_path_separator_is_refused() {
        // There is no escaping form for a PATH entry that all consumers honour,
        // so appending this would split into two entries pointing nowhere.
        let flags = InstallFlags {
            dir: Some("/opt/a:b".into()),
            ..InstallFlags::default()
        };
        let error = plan_install(&env(Os::Unix), &Probes::new(), &flags).unwrap_err();
        assert!(error.contains("cannot go on PATH"), "got: {error}");

        // …but it is allowed when the user takes PATH into their own hands.
        let flags = InstallFlags {
            dir: Some("/opt/a:b".into()),
            no_path: true,
            ..InstallFlags::default()
        };
        assert!(plan_install(&env(Os::Unix), &Probes::new(), &flags).is_ok());
    }

    #[test]
    fn resolves_the_binary_the_way_a_shell_would() {
        let exists = |p: &Path| p == Path::new("/usr/local/bin/orbital");
        assert_eq!(
            resolve_on_path(
                "/home/dev/.local/bin:/usr/local/bin",
                Os::Unix,
                &exists,
                &no_vars
            ),
            Some(PathBuf::from("/usr/local/bin/orbital"))
        );
    }

    #[test]
    fn directory_order_beats_extension_order_on_windows() {
        // A leftover orbital.cmd in an earlier directory shadows our .exe, which
        // is exactly the "install did nothing" report.
        let exists = |p: &Path| {
            p == Path::new(r"C:\first\orbital.cmd") || p == Path::new(r"C:\second\orbital.exe")
        };
        assert_eq!(
            resolve_on_path(r"C:\first;C:\second", Os::Windows, &exists, &no_vars),
            Some(PathBuf::from(r"C:\first\orbital.cmd"))
        );
    }

    #[test]
    fn resolving_finds_nothing_on_an_empty_path() {
        assert_eq!(resolve_on_path("", Os::Unix, &|_| true, &no_vars), None);
    }

    #[test]
    fn the_binary_name_carries_the_platform_suffix() {
        assert_eq!(Os::Windows.binary_name(), "orbital.exe");
        assert_eq!(Os::Unix.binary_name(), "orbital");
        assert_eq!(Os::Macos.binary_name(), "orbital");
    }
}
