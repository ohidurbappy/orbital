//! `orbital` — a growable cross-platform CLI toolbox.
//!
//! Argument parsing and dispatch live here; every tool is registered in
//! [`commands::COMMANDS`], which drives `--help`, the interactive menu, and CLI
//! dispatch alike.

mod commands;
mod components;
mod core;
mod style;
mod term;

use std::io::{IsTerminal, Read};

use commands::{find_command, Ctx, COMMANDS};
use components::banner::banner_lines;
use components::menu::run_menu;
use core::updater::cached_update;
use core::updater::refresh::{run_refresh, spawn_background_refresh, REFRESH_ARG};
use core::version::{BIN, VERSION};

/// The error type every fallible path funnels into.
pub type Res<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Default, PartialEq, Eq)]
struct ParsedArgs {
    command_name: Option<String>,
    /// Positional tokens after the command name, forwarded to the command.
    command_args: Vec<String>,
    help: bool,
    version: bool,
}

fn parse_args(argv: &[String]) -> ParsedArgs {
    let mut parsed = ParsedArgs::default();

    for arg in argv {
        if arg == "--help" || arg == "-h" {
            parsed.help = true;
        } else if arg == "--version" || arg == "-v" {
            parsed.version = true;
        } else if parsed.command_name.is_none() && !arg.starts_with('-') {
            parsed.command_name = Some(arg.clone());
        } else {
            // Everything after the command name — positionals AND flags like
            // `--public` — is forwarded so the command parses its own options.
            parsed.command_args.push(arg.clone());
        }
    }

    parsed
}

/// Drop a leading byte-order mark. PowerShell prefixes one when it pipes text,
/// and it would otherwise be encoded as part of the payload.
fn strip_bom(text: String) -> String {
    match text.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_string(),
        None => text,
    }
}

fn help_text() -> String {
    let mut lines = vec![
        format!("{BIN} — a growable cross-platform CLI toolbox"),
        String::new(),
        "Usage:".to_string(),
        format!("  {BIN}              Open the interactive menu"),
        format!("  {BIN} <command>    Run a command directly"),
        String::new(),
        "Commands:".to_string(),
    ];
    lines.extend(
        COMMANDS
            .iter()
            .map(|c| format!("  {}{}", style::pad(c.name, 12), c.description)),
    );
    lines.extend([
        String::new(),
        "Flags:".to_string(),
        "  -h, --help       Show this help".to_string(),
        "  -v, --version    Show version".to_string(),
    ]);
    lines.join("\n")
}

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    };
    std::process::exit(code);
}

fn run() -> Res<i32> {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    // Hidden command used by the detached background update check.
    if argv.first().map(String::as_str) == Some(REFRESH_ARG) {
        run_refresh();
        return Ok(0);
    }

    let parsed = parse_args(&argv);

    if parsed.version {
        println!("{VERSION}");
        return Ok(0);
    }
    if parsed.help {
        println!("{}", help_text());
        return Ok(0);
    }

    let name = match &parsed.command_name {
        Some(name) => name,
        // No command → the interactive menu, which runs its own update poll.
        None => {
            run_menu()?;
            return Ok(0);
        }
    };

    let command = match find_command(name) {
        Some(command) => command,
        None => {
            eprintln!("Unknown command: {name}\n");
            println!("{}", help_text());
            return Ok(1);
        }
    };

    // Pull piped input for commands that want it. Only when no positional args
    // were given (they take precedence, so there's nothing to wait for) and
    // stdin isn't a TTY (an interactive `orbital qr` must not block on EOF).
    let input =
        if command.reads_stdin && parsed.command_args.is_empty() && !std::io::stdin().is_terminal()
        {
            let mut buffer = String::new();
            std::io::stdin().read_to_string(&mut buffer)?;
            Some(strip_bom(buffer))
        } else {
            None
        };

    let ctx = Ctx {
        args: &parsed.command_args,
        input: input.as_deref(),
        interactive: term::is_interactive(),
    };

    // Plain-output handler (e.g. `orbital ip --local`): print to stdout and skip
    // the UI entirely — no menu chrome, no update banner — so it stays pipeable.
    if let Some(plain) = command.run {
        match plain(&ctx) {
            Ok(Some(out)) => {
                println!("{out}");
                spawn_background_refresh();
                return Ok(0);
            }
            Ok(None) => {}
            Err(message) => {
                eprintln!("{message}");
                return Ok(1);
            }
        }
    }

    let banner = banner_lines(cached_update().as_ref());
    if !banner.is_empty() {
        term::emit(&banner);
    }
    (command.view)(&ctx)?;

    // Refresh the update cache for next time without blocking this run.
    spawn_background_refresh();
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_a_command_name() {
        assert_eq!(
            parse_args(&argv(&["ip"])).command_name.as_deref(),
            Some("ip")
        );
    }

    #[test]
    fn parses_flags() {
        assert!(parse_args(&argv(&["--version"])).version);
        assert!(parse_args(&argv(&["-h"])).help);
    }

    #[test]
    fn takes_the_first_non_flag_token_as_the_command() {
        assert_eq!(
            parse_args(&argv(&["--verbose", "sysinfo"]))
                .command_name
                .as_deref(),
            Some("sysinfo")
        );
    }

    #[test]
    fn returns_no_command_when_only_flags_are_given() {
        assert!(parse_args(&[]).command_name.is_none());
    }

    #[test]
    fn collects_positional_tokens_after_the_command() {
        let parsed = parse_args(&argv(&["qr", "hello", "world"]));
        assert_eq!(parsed.command_name.as_deref(), Some("qr"));
        assert_eq!(parsed.command_args, argv(&["hello", "world"]));
    }

    #[test]
    fn keeps_command_args_empty_when_only_the_command_is_given() {
        assert!(parse_args(&argv(&["ip"])).command_args.is_empty());
    }

    #[test]
    fn forwards_flags_after_the_command_to_the_command() {
        let parsed = parse_args(&argv(&["ip", "--public"]));
        assert_eq!(parsed.command_name.as_deref(), Some("ip"));
        assert_eq!(parsed.command_args, argv(&["--public"]));
    }

    #[test]
    fn still_treats_help_and_version_as_global_after_a_command() {
        assert!(parse_args(&argv(&["ip", "--help"])).help);
        assert!(parse_args(&argv(&["ip", "-v"])).version);
    }

    #[test]
    fn help_lists_every_registered_command() {
        let help = help_text();
        for command in COMMANDS {
            assert!(
                help.contains(command.name),
                "{} missing from help",
                command.name
            );
            assert!(help.contains(command.description));
        }
    }

    #[test]
    fn strips_a_leading_byte_order_mark_from_piped_input() {
        assert_eq!(strip_bom("\u{feff}hello".to_string()), "hello");
        assert_eq!(strip_bom("hello".to_string()), "hello");
        // Only the leading one — a BOM mid-text is real content.
        assert_eq!(strip_bom("a\u{feff}b".to_string()), "a\u{feff}b");
    }

    #[test]
    fn help_documents_the_global_flags() {
        let help = help_text();
        assert!(help.contains("-h, --help"));
        assert!(help.contains("-v, --version"));
        assert!(help.contains("orbital <command>"));
    }
}
