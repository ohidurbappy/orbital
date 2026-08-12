//! The command registry — the single source of truth for available tools.
//!
//! Register a new command here and it automatically appears in `--help`, the
//! interactive menu, and CLI dispatch.

pub mod filter;
pub mod ip;
pub mod qr;
pub mod serve;
pub mod sysinfo;
pub mod table;
pub mod update;

use crate::Res;

/// Inputs the one-shot runner resolves from argv/stdin and hands to a command.
pub struct Ctx<'a> {
    /// Positional CLI tokens after the command name, e.g. `orbital qr hi there`
    /// → `["hi", "there"]`.
    pub args: &'a [String],
    /// Text piped via stdin, populated only when the command sets `reads_stdin`.
    pub input: Option<&'a str>,
    /// True when stdin/stdout are a terminal, so the command may take over the
    /// screen and read keystrokes. False under a pipe, where commands fall back
    /// to printing a single frame.
    pub interactive: bool,
}

impl Ctx<'_> {
    /// A `Ctx` with no args or piped input, used when a command is launched
    /// from the interactive menu.
    pub fn menu() -> Ctx<'static> {
        Ctx {
            args: &[],
            input: None,
            interactive: true,
        }
    }
}

/// Plain-text handler for one-shot, script-friendly invocations (e.g.
/// `orbital ip --local`). Returning `Ok(Some(text))` prints it to stdout and
/// skips the UI entirely — no menu chrome, no update banner — so the output is
/// pipeable. `Ok(None)` falls through to `view`. `Err` is printed to stderr and
/// exits non-zero. Only consulted for one-shot CLI runs, never from the menu.
pub type RunFn = fn(&Ctx) -> Result<Option<String>, String>;

/// Renders the command. Long-lived and interactive commands drive their own
/// input loop here and return when the user is done.
pub type ViewFn = fn(&Ctx) -> Res;

/// When a command wants the contents of a pipe. Piped stdin is only ever read
/// when stdin is not a TTY, so an interactive run never blocks waiting on EOF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stdin {
    /// This command takes no piped input.
    Never,
    /// Read the pipe only when no arguments were given, because the arguments
    /// *are* the payload — `orbital qr hello` must not wait on stdin.
    WhenNoArgs,
    /// Always read the pipe: the arguments are options, not data, so
    /// `orbital table --csv` still gets its input.
    Always,
}

/// A single tool in the orbital toolbox.
///
/// To add a new command: create a module under `src/commands/<name>/`, expose a
/// `COMMAND` descriptor, and register it in [`COMMANDS`].
pub struct Command {
    /// Canonical name used on the CLI, e.g. `orbital ip`.
    pub name: &'static str,
    /// One-line description shown in `--help` and the interactive menu.
    pub description: &'static str,
    /// Alternate names that also resolve to this command.
    pub aliases: &'static [&'static str],
    /// Optional plain-output fast path; see [`RunFn`].
    pub run: Option<RunFn>,
    /// The command's own rendering / interaction loop.
    pub view: ViewFn,
    /// Whether the one-shot runner should read piped stdin and pass it as
    /// `Ctx::input`.
    pub stdin: Stdin,
}

/// Every tool, in the order they appear in `--help` and the menu.
pub static COMMANDS: &[Command] = &[
    ip::COMMAND,
    qr::COMMAND,
    serve::COMMAND,
    sysinfo::COMMAND,
    table::COMMAND,
    update::COMMAND,
];

/// Resolve a command by its name or one of its aliases.
pub fn find_command(name: &str) -> Option<&'static Command> {
    COMMANDS
        .iter()
        .find(|c| c.name == name || c.aliases.contains(&name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn resolves_by_canonical_name() {
        assert_eq!(find_command("ip").unwrap().name, "ip");
    }

    #[test]
    fn resolves_by_alias() {
        assert_eq!(find_command("neofetch").unwrap().name, "sysinfo");
        assert_eq!(find_command("upgrade").unwrap().name, "update");
    }

    #[test]
    fn unknown_commands_resolve_to_nothing() {
        assert!(find_command("nope").is_none());
    }

    #[test]
    fn every_command_has_a_unique_name() {
        let names: HashSet<&str> = COMMANDS.iter().map(|c| c.name).collect();
        assert_eq!(names.len(), COMMANDS.len());
    }

    #[test]
    fn no_alias_collides_with_another_command() {
        let mut seen: HashSet<&str> = HashSet::new();
        for command in COMMANDS {
            assert!(
                seen.insert(command.name),
                "duplicate token: {}",
                command.name
            );
            for alias in command.aliases {
                assert!(seen.insert(alias), "duplicate token: {alias}");
            }
        }
    }
}
