//! `orbital install` — put this binary somewhere the shell will find it.
//!
//! Deliberately per-user and elevation-free: it copies the running executable
//! into a directory the user owns and makes sure that directory is on PATH.
//! Installing for *all* users would need administrator/root rights and a
//! password prompt, which a command that may be launched from the interactive
//! menu must never do.

pub mod plan;
#[cfg(windows)]
pub mod winenv;

use std::fs;
use std::path::{Path, PathBuf};

use plan::{
    parse_install_flags, plan_install, resolve_on_path, strip_verbatim, unix_candidates, Env, Os,
    PathAction, Plan, Probes,
};

use crate::commands::{Command, Ctx, Stdin};
use crate::components::key_value::key_value;
use crate::core::binfile::place_executable;
use crate::core::version::{BIN, VERSION};
use crate::style;
use crate::term;
use crate::Res;

pub const COMMAND: Command = Command {
    name: "install",
    description: "Install this binary so `orbital` runs from any shell",
    aliases: &["setup", "self-install"],
    run: None,
    view,
    stdin: Stdin::Never,
};

/// What to print, and whether the run should still be considered a failure.
struct Rendered {
    lines: Vec<String>,
    /// Set when something went wrong that the report describes. The lines are
    /// printed first, then this becomes the process's error.
    failure: Option<String>,
}

fn view(ctx: &Ctx) -> Res {
    match execute(ctx) {
        Ok(rendered) => {
            term::emit(&rendered.lines);
            match rendered.failure {
                // Reported *and* non-zero: the copy may have worked while the
                // PATH update did not, and the user needs both facts.
                Some(message) => Err(message.into()),
                None => Ok(()),
            }
        }
        // Nothing was done, so there is no report — just the error, on stderr.
        Err(message) => Err(message.into()),
    }
}

fn execute(ctx: &Ctx) -> Result<Rendered, String> {
    let flags = parse_install_flags(ctx.args);
    let env = real_env()?;
    let probes = probe_candidates(&env, &flags);
    let plan = plan_install(&env, &probes, &flags)?;

    if plan.dry_run {
        return Ok(Rendered {
            lines: render_plan(&plan, &env),
            failure: None,
        });
    }

    let outcome = perform(&plan, &env)?;
    Ok(Rendered {
        lines: render_report(&plan, &env, &outcome),
        failure: outcome.path_problem.clone(),
    })
}

/// Read the world once, so planning can stay pure.
fn real_env() -> Result<Env, String> {
    let exe = std::env::current_exe()
        .map_err(|err| format!("Could not find this binary's own path: {err}"))?;
    // Canonicalize so "am I already installed here?" survives symlinks and
    // relative invocations. If it fails, the raw path is still better than
    // nothing.
    let exe = exe.canonicalize().unwrap_or(exe);

    let os = Os::current();
    Ok(Env {
        os,
        exe,
        install_dir_var: std::env::var("ORBITAL_INSTALL_DIR")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        // The whole environment: a Windows PATH entry can reference any
        // variable, and resolving those is what stops a duplicate being
        // appended on every install. `vars_os` because `vars` panics on a
        // variable that is not valid Unicode, and one that isn't cannot match a
        // `%VAR%` reference anyway.
        vars: std::env::vars_os()
            .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
            .collect(),
        // A relative --dir is resolved against this rather than reaching PATH.
        cwd: std::env::current_dir().unwrap_or_default(),
        // Either variable means `sudo`; neither is set by a plain root login.
        sudo: std::env::var_os("SUDO_USER").is_some() || std::env::var_os("SUDO_UID").is_some(),
        search_path: std::env::var("PATH").unwrap_or_default(),
        user_path: read_user_path_text(),
    })
}

/// Windows: the user PATH from the registry, which is the only value the
/// install ever writes back. `None` everywhere else.
fn read_user_path_text() -> Option<String> {
    #[cfg(windows)]
    {
        winenv::read_user_path().ok().map(|value| value.to_text())
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Find out which candidate directories can actually be written to.
///
/// A capability test, not a permission calculation: ACLs, read-only mounts and
/// immutable flags all make `access()`-style checks disagree with the write
/// that follows.
fn probe_candidates(env: &Env, flags: &plan::InstallFlags) -> Probes {
    let mut probes = Probes::new();
    // An explicit directory is checked when it is used, with a clear error, so
    // there is nothing to probe here.
    if flags.dir.is_some() || env.install_dir_var.is_some() || env.os == Os::Windows {
        return probes;
    }
    for dir in unix_candidates(env) {
        let writable = can_write_to(&dir);
        probes.set(dir, writable);
    }
    probes
}

/// Could we put a binary in `dir`, creating it if necessary?
///
/// Walks up to the nearest existing ancestor, so a candidate that does not exist
/// yet — `~/.local/bin` on a fresh account, or `/usr/local/bin` under `sudo` on a
/// machine that lacks it — is judged by whether we could create it.
fn can_write_to(dir: &Path) -> bool {
    let mut current = dir;
    loop {
        if current.is_dir() {
            return probe_write(current);
        }
        // An existing non-directory can never hold the binary.
        if current.exists() {
            return false;
        }
        match current.parent() {
            Some(parent) if parent != current && !parent.as_os_str().is_empty() => current = parent,
            _ => return false,
        }
    }
}

/// Can we actually create a file in this existing directory? A capability test,
/// not a permission calculation: ACLs, read-only mounts and immutable flags all
/// make an `access()`-style check disagree with the write that follows.
fn probe_write(dir: &Path) -> bool {
    let probe = dir.join(format!(".{BIN}.{}.probe", std::process::id()));
    match fs::File::options()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// What actually happened.
#[derive(Debug, Default)]
struct Outcome {
    copied: bool,
    created_dir: bool,
    path_updated: bool,
    /// The version the installed binary reports, proving it can execute.
    verified: Option<String>,
    /// Why the post-install check failed, when it did.
    problem: Option<String>,
    /// A different `orbital` that would still win on PATH.
    shadowed_by: Option<PathBuf>,
    /// Why the PATH update failed, when it did. Reported rather than returned,
    /// so a successful copy is still told to the user.
    path_problem: Option<String>,
}

/// The only function here that changes anything.
fn perform(plan: &Plan, env: &Env) -> Result<Outcome, String> {
    let mut outcome = Outcome::default();

    if !plan.already_installed {
        if !plan.dir.is_dir() {
            fs::create_dir_all(&plan.dir)
                .map_err(|err| format!("Could not create {}: {err}", display(&plan.dir)))?;
            outcome.created_dir = true;
        }
        // Fail before the PATH is touched, so a refused copy can never leave a
        // PATH entry pointing at nothing.
        if !probe_write(&plan.dir) {
            return Err(format!(
                "Cannot write to {} — pass --dir <path> or set ORBITAL_INSTALL_DIR to a directory you own.",
                display(&plan.dir)
            ));
        }

        let bytes = fs::read(&plan.source)
            .map_err(|err| format!("Could not read {}: {err}", display(&plan.source)))?;
        place_executable(&plan.target, &bytes)
            .map_err(|err| format!("Could not install to {}: {err}", display(&plan.target)))?;
        outcome.copied = true;
    }

    if plan.path_action == PathAction::UpdateUserPath {
        // The binary is already in place, so a PATH failure is a partial
        // success: report it, and let the caller exit non-zero.
        match update_user_path(&plan.dir) {
            Ok(updated) => outcome.path_updated = updated,
            Err(message) => outcome.path_problem = Some(message),
        }
    }

    // Prove it runs, rather than assuming: a noexec mount, a filesystem that
    // drops the execute bit, macOS quarantine, or an AppLocker policy that
    // forbids executables in user directories would all leave a correct-looking
    // install that cannot start.
    match std::process::Command::new(&plan.target)
        .arg("--version")
        .output()
    {
        Ok(output) if output.status.success() => {
            outcome.verified = Some(String::from_utf8_lossy(&output.stdout).trim().to_string());
        }
        Ok(output) => {
            outcome.problem = Some(format!(
                "{} ran but exited with {}",
                display(&plan.target),
                output.status
            ));
        }
        Err(err) => {
            outcome.problem = Some(format!("{} could not be run: {err}", display(&plan.target)));
        }
    }

    // Would a different copy still win?
    let effective = effective_path(plan, env);
    if let Some(found) = resolve_on_path(&effective, env.os, &|p| p.exists(), &|name| {
        env.expand_var(name)
    }) {
        if !plan::same_path(&found, &plan.target, env.os) {
            outcome.shadowed_by = Some(found);
        }
    }

    Ok(outcome)
}

/// The PATH a new shell will see, for the shadowing check.
fn effective_path(plan: &Plan, env: &Env) -> String {
    match plan.path_action {
        // The directory is about to be on PATH, at the end.
        PathAction::UpdateUserPath => {
            plan::append_entry(&env.search_path, &strip_verbatim(&plan.dir), env.os)
        }
        _ => env.search_path.clone(),
    }
}

/// Append the install directory to the user PATH, returning whether anything
/// was written.
#[cfg(windows)]
fn update_user_path(dir: &Path) -> Result<bool, String> {
    let current =
        winenv::read_user_path().map_err(|err| format!("Could not read your user PATH: {err}"))?;

    // Appended as raw UTF-16, with the value type carried over unchanged — see
    // the module docs in winenv.rs for what the obvious approaches destroy.
    let raw = plan::append_entry_utf16(&current.raw, &strip_verbatim(dir));

    winenv::write_user_path(&raw, current.kind)
        .map_err(|err| format!("Could not update your user PATH: {err}"))?;
    // So freshly launched programs see it without a sign-out.
    winenv::broadcast_change();
    Ok(true)
}

#[cfg(not(windows))]
fn update_user_path(_dir: &Path) -> Result<bool, String> {
    // Unix never edits the environment behind the user's back; the hint in
    // `render_report` is the whole story.
    Ok(false)
}

fn display(path: &Path) -> String {
    strip_verbatim(path)
}

/// `--dry-run`: say what would happen, change nothing.
fn render_plan(plan: &Plan, env: &Env) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{}{}",
            style::bold_cyan("Would install "),
            display(&plan.target)
        ),
        String::new(),
        key_value("From", &display(&plan.source)),
    ];
    if plan.already_installed {
        lines.push(key_value("Copy", "not needed — already at that path"));
    }
    lines.push(key_value("PATH", &path_summary(plan, env, false)));
    lines.push(String::new());
    lines.push(style::dim("Nothing was changed (--dry-run)."));
    lines
}

fn render_report(plan: &Plan, env: &Env, outcome: &Outcome) -> Vec<String> {
    let mut lines = Vec::new();

    if outcome.copied {
        lines.push(format!(
            "{}{}",
            style::bold_green("Installed "),
            display(&plan.target)
        ));
    } else {
        lines.push(format!(
            "{}{}",
            style::cyan("Already installed at "),
            display(&plan.target)
        ));
    }
    lines.push(String::new());

    lines.push(key_value(
        "Version",
        outcome.verified.as_deref().unwrap_or(VERSION),
    ));
    if outcome.copied {
        lines.push(key_value("From", &display(&plan.source)));
    }
    lines.push(key_value(
        "PATH",
        &path_summary(plan, env, outcome.path_updated),
    ));

    // Anything the user has to act on comes last, where it is read.
    if let Some(problem) = &outcome.problem {
        lines.push(String::new());
        lines.push(style::red(&format!("Installed, but {problem}")));
        if env.os == Os::Macos {
            lines.push(style::dim(&format!(
                "If macOS quarantined it: xattr -d com.apple.quarantine {}",
                display(&plan.target)
            )));
        } else {
            lines.push(style::dim(
                "Check that the directory is not mounted noexec and that policy allows it.",
            ));
        }
    }

    if let Some(problem) = &outcome.path_problem {
        lines.push(String::new());
        lines.push(style::red(problem));
        lines.push(style::dim(&format!(
            "The binary is in place; add {} to your PATH by hand.",
            display(&plan.dir)
        )));
    }

    if let Some(shadow) = &outcome.shadowed_by {
        lines.push(String::new());
        lines.push(style::yellow(&format!(
            "Another {BIN} comes first on PATH: {}",
            display(shadow)
        )));
        lines.push(style::dim(&format!(
            "Remove it, or put {} earlier, or that one keeps running.",
            display(&plan.dir)
        )));
    }

    match plan.path_action {
        PathAction::UpdateUserPath if outcome.path_updated => {
            lines.push(String::new());
            // "Restart your shell" is wrong on Windows: a new tab inherits the
            // terminal host's stale environment block.
            lines.push(style::dim("Open a new terminal to pick up the new PATH."));
            lines.push(style::dim(&format!(
                "For this one:  $env:Path += ';{}'",
                display(&plan.dir)
            )));
        }
        PathAction::ShowHint => {
            lines.push(String::new());
            lines.extend(unix_path_hint(plan, env, outcome.created_dir));
        }
        PathAction::Skipped => {
            lines.push(String::new());
            lines.push(style::dim("PATH was left alone (--no-path)."));
        }
        _ => {}
    }

    lines.push(String::new());
    lines.push(style::dim(&format!("Run: {BIN} --help")));
    lines
}

/// Wrap in single quotes, which are literal in bash, zsh and fish alike.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn path_summary(plan: &Plan, env: &Env, updated: bool) -> String {
    match plan.path_action {
        PathAction::AlreadyOnPath => format!("{} is already on PATH", display(&plan.dir)),
        PathAction::UpdateUserPath if updated => {
            format!("added {} to your user PATH", display(&plan.dir))
        }
        PathAction::UpdateUserPath => format!("will add {} to your user PATH", display(&plan.dir)),
        PathAction::ShowHint => format!("{} is not on PATH yet", display(&plan.dir)),
        PathAction::Skipped => {
            let _ = env;
            format!("not changed — {} may not be on PATH", display(&plan.dir))
        }
    }
}

/// Tell the user how to put the directory on PATH. Never edit a shell profile
/// on their behalf: the right file is unknowable from in here, and a bad guess
/// silently breaks logins.
fn unix_path_hint(plan: &Plan, env: &Env, created_dir: bool) -> Vec<String> {
    let dir = display(&plan.dir);
    let shell = env
        .var_opt("SHELL")
        .and_then(|s| s.rsplit('/').next().map(str::to_string))
        .unwrap_or_default();

    // Single-quoted so a directory containing a space, `$` or `~` is one
    // literal argument. Unquoted, `fish_add_path /my dir` silently adds two.
    let quoted = shell_quote(&dir);
    let export = format!("export PATH={quoted}:$PATH");
    let (rc, line) = match shell.as_str() {
        "fish" => (
            "~/.config/fish/config.fish".to_string(),
            format!("fish_add_path {quoted}"),
        ),
        "zsh" => ("~/.zshrc".to_string(), export),
        "bash" => (
            if env.os == Os::Macos {
                "~/.bash_profile"
            } else {
                "~/.bashrc"
            }
            .to_string(),
            export,
        ),
        _ => ("your shell profile".to_string(), export),
    };

    let mut lines = vec![
        style::yellow(&format!("{dir} is not on your PATH.")),
        style::dim(&format!("Add this to {rc}:")),
        format!("  {line}"),
    ];
    if created_dir {
        lines.push(style::dim(
            "(the directory was just created — some shells only add it at login)",
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use plan::InstallFlags;
    use std::collections::BTreeMap;

    fn env(os: Os) -> Env {
        let mut vars = BTreeMap::new();
        match os {
            Os::Windows => {
                vars.insert("LOCALAPPDATA".into(), r"C:\Users\dev\AppData\Local".into());
            }
            _ => {
                vars.insert("HOME".into(), "/home/dev".into());
                vars.insert("SHELL".into(), "/bin/zsh".into());
            }
        }
        Env {
            os,
            exe: PathBuf::from(if os == Os::Windows {
                r"D:\src\target\release\orbital.exe"
            } else {
                "/src/target/release/orbital"
            }),
            install_dir_var: None,
            vars,
            cwd: PathBuf::from("/work"),
            sudo: false,
            search_path: String::new(),
            user_path: None,
        }
    }

    fn plan_for(os: Os) -> Plan {
        plan_install(&env(os), &Probes::new(), &InstallFlags::default()).unwrap()
    }

    fn installed() -> Outcome {
        Outcome {
            copied: true,
            verified: Some("0.1.0".into()),
            ..Outcome::default()
        }
    }

    #[test]
    fn reports_where_it_installed_and_from_where() {
        let plan = plan_for(Os::Unix);
        let out = render_report(&plan, &env(Os::Unix), &installed()).join("\n");
        assert!(out.contains("Installed /home/dev/.local/bin/orbital"));
        assert!(out.contains("/src/target/release/orbital"));
        assert!(out.contains("0.1.0"));
        assert!(out.contains("Run: orbital --help"));
    }

    #[test]
    fn says_already_installed_instead_of_claiming_a_copy() {
        let mut env = env(Os::Unix);
        env.exe = PathBuf::from("/home/dev/.local/bin/orbital");
        let plan = plan_install(&env, &Probes::new(), &InstallFlags::default()).unwrap();
        let out = render_report(&plan, &env, &Outcome::default()).join("\n");
        assert!(out.contains("Already installed at /home/dev/.local/bin/orbital"));
        assert!(!out.contains("Installed /home"));
    }

    #[test]
    fn windows_reports_the_path_write_and_how_to_use_it_now() {
        let plan = plan_for(Os::Windows);
        let outcome = Outcome {
            path_updated: true,
            ..installed()
        };
        let out = render_report(&plan, &env(Os::Windows), &outcome).join("\n");
        assert!(out.contains(r"added C:\Users\dev\AppData\Local\orbital\bin to your user PATH"));
        assert!(out.contains("Open a new terminal"));
        assert!(out.contains("$env:Path +="));
    }

    #[test]
    fn unix_prints_a_shell_specific_hint_and_never_edits_a_profile() {
        let out = render_report(&plan_for(Os::Unix), &env(Os::Unix), &installed()).join("\n");
        assert!(out.contains("is not on your PATH"));
        assert!(out.contains("~/.zshrc"));
        assert!(
            out.contains("export PATH='/home/dev/.local/bin':$PATH"),
            "got: {out}"
        );
    }

    #[test]
    fn the_hint_follows_the_users_shell() {
        let mut fish = env(Os::Unix);
        fish.vars.insert("SHELL".into(), "/usr/bin/fish".into());
        let out = render_report(&plan_for(Os::Unix), &fish, &installed()).join("\n");
        assert!(
            out.contains("fish_add_path '/home/dev/.local/bin'"),
            "got: {out}"
        );
        assert!(out.contains("config.fish"));

        let mut bash = env(Os::Macos);
        bash.vars.insert("HOME".into(), "/Users/dev".into());
        bash.vars.insert("SHELL".into(), "/bin/bash".into());
        let plan = plan_install(&bash, &Probes::new(), &InstallFlags::default()).unwrap();
        let out = render_report(&plan, &bash, &installed()).join("\n");
        assert!(
            out.contains("~/.bash_profile"),
            "macOS bash uses bash_profile"
        );
    }

    #[test]
    fn an_unknown_shell_still_gets_a_usable_line() {
        let mut env = env(Os::Unix);
        env.vars.remove("SHELL");
        let out = render_report(&plan_for(Os::Unix), &env, &installed()).join("\n");
        assert!(out.contains("your shell profile"));
        assert!(out.contains("export PATH="));
    }

    #[test]
    fn says_nothing_about_path_when_the_directory_is_already_on_it() {
        let mut env = env(Os::Unix);
        env.search_path = "/home/dev/.local/bin".into();
        let plan = plan_install(&env, &Probes::new(), &InstallFlags::default()).unwrap();
        let out = render_report(&plan, &env, &installed()).join("\n");
        assert!(out.contains("already on PATH"));
        assert!(!out.contains("not on your PATH"));
    }

    #[test]
    fn reports_that_path_was_deliberately_left_alone() {
        let flags = InstallFlags {
            no_path: true,
            ..InstallFlags::default()
        };
        let env = env(Os::Windows);
        let plan = plan_install(&env, &Probes::new(), &flags).unwrap();
        let out = render_report(&plan, &env, &installed()).join("\n");
        assert!(out.contains("--no-path"));
        assert!(!out.contains("user PATH"));
    }

    #[test]
    fn a_binary_that_cannot_run_is_not_reported_as_a_success() {
        let outcome = Outcome {
            copied: true,
            problem: Some("it could not be run: Exec format error".into()),
            ..Outcome::default()
        };
        let out = render_report(&plan_for(Os::Unix), &env(Os::Unix), &outcome).join("\n");
        assert!(out.contains("Installed, but"));
    }

    #[test]
    fn macos_suggests_clearing_quarantine_when_the_check_fails() {
        let mut env = env(Os::Macos);
        env.vars.insert("HOME".into(), "/Users/dev".into());
        let plan = plan_install(&env, &Probes::new(), &InstallFlags::default()).unwrap();
        let outcome = Outcome {
            copied: true,
            problem: Some("it could not be run".into()),
            ..Outcome::default()
        };
        let out = render_report(&plan, &env, &outcome).join("\n");
        assert!(out.contains("com.apple.quarantine"));
    }

    #[test]
    fn warns_when_another_copy_would_still_win() {
        let outcome = Outcome {
            shadowed_by: Some(PathBuf::from("/usr/local/bin/orbital")),
            ..installed()
        };
        let out = render_report(&plan_for(Os::Unix), &env(Os::Unix), &outcome).join("\n");
        assert!(out.contains("comes first on PATH: /usr/local/bin/orbital"));
    }

    #[test]
    fn a_dry_run_says_what_would_happen_and_that_nothing_changed() {
        let flags = InstallFlags {
            dry_run: true,
            ..InstallFlags::default()
        };
        let env = env(Os::Unix);
        let plan = plan_install(&env, &Probes::new(), &flags).unwrap();
        let out = render_plan(&plan, &env).join("\n");
        assert!(out.contains("Would install /home/dev/.local/bin/orbital"));
        assert!(out.contains("--dry-run"));
        assert!(!out.contains("Installed "));
    }

    #[test]
    fn dry_run_changes_nothing_on_disk() {
        // The one execute()-level test, and it is only safe because --dry-run
        // returns before `perform`. No test may call `perform`: it would really
        // install the test binary and really edit PATH.
        let args = vec!["--dry-run".to_string(), "--dir".to_string(), dry_dir()];
        let ctx = Ctx {
            args: &args,
            input: None,
            interactive: false,
        };
        let rendered = execute(&ctx).unwrap();
        assert!(rendered.lines.iter().any(|l| l.contains("Would install")));
        assert!(rendered.failure.is_none());
        assert!(
            !PathBuf::from(dry_dir()).exists(),
            "a dry run created a directory"
        );
    }

    fn dry_dir() -> String {
        std::env::temp_dir()
            .join(format!("orbital-install-dry-{}", std::process::id()))
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn a_failed_path_write_still_reports_the_binary_that_was_placed() {
        // The copy succeeded; discarding the report would leave the user unable
        // to find the binary that is now on disk.
        let plan = plan_for(Os::Windows);
        let outcome = Outcome {
            path_problem: Some("Could not update your user PATH: access denied".into()),
            ..installed()
        };
        let out = render_report(&plan, &env(Os::Windows), &outcome).join("\n");
        assert!(
            out.contains("Installed "),
            "the copy must still be reported"
        );
        assert!(out.contains("Could not update your user PATH"));
        assert!(out.contains("add"), "and how to finish the job by hand");
    }

    #[test]
    fn the_path_hint_quotes_a_directory_containing_a_space() {
        let mut env = env(Os::Unix);
        env.vars.insert("SHELL".into(), "/usr/bin/fish".into());
        let flags = plan::InstallFlags {
            dir: Some("/home/dev/my tools".into()),
            ..plan::InstallFlags::default()
        };
        let plan = plan_install(&env, &Probes::new(), &flags).unwrap();
        let out = render_report(&plan, &env, &installed()).join("\n");
        // Unquoted, `fish_add_path /home/dev/my tools` adds two wrong entries.
        assert!(
            out.contains("fish_add_path '/home/dev/my tools'"),
            "got: {out}"
        );
    }

    #[test]
    fn quoting_survives_an_apostrophe_in_the_path() {
        // The shell idiom for a literal apostrophe inside single quotes:
        // close the quote, escape one, reopen.
        assert_eq!(shell_quote("/home/o'brien/bin"), r"'/home/o'\''brien/bin'");
    }

    #[test]
    fn every_row_is_its_own_line_so_raw_mode_cannot_staircase_it() {
        // term::emit picks \r\n per element; an embedded \n inside one element
        // would staircase when the menu holds raw mode.
        for line in render_report(&plan_for(Os::Unix), &env(Os::Unix), &installed()) {
            assert!(!line.contains('\n'), "embedded newline in {line:?}");
        }
    }
}
