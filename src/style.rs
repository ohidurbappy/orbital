//! Minimal ANSI styling.
//!
//! Colour is resolved once per process: disabled when `NO_COLOR` is set, when
//! `TERM=dumb`, or when stdout is not a terminal — so piped output stays clean.

use std::io::IsTerminal;
use std::sync::OnceLock;

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        if std::env::var("TERM").is_ok_and(|t| t == "dumb") {
            return false;
        }
        #[cfg(windows)]
        {
            // Turns on virtual-terminal processing on older consoles; without
            // it the escapes below would be printed literally.
            let _ = crossterm::ansi_support::supports_ansi();
        }
        std::io::stdout().is_terminal()
    })
}

fn paint(text: &str, codes: &str) -> String {
    if enabled() {
        format!("\x1b[{codes}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

pub fn bold(text: &str) -> String {
    paint(text, "1")
}

pub fn dim(text: &str) -> String {
    paint(text, "2")
}

pub fn green(text: &str) -> String {
    paint(text, "32")
}

pub fn yellow(text: &str) -> String {
    paint(text, "33")
}

pub fn cyan(text: &str) -> String {
    paint(text, "36")
}

pub fn red(text: &str) -> String {
    paint(text, "31")
}

pub fn bold_green(text: &str) -> String {
    paint(text, "1;32")
}

pub fn bold_cyan(text: &str) -> String {
    paint(text, "1;36")
}

/// Colour by name, for the few places that pick a colour at runtime.
pub fn colored(text: &str, color: &str) -> String {
    match color {
        "green" => green(text),
        "yellow" => yellow(text),
        "cyan" => cyan(text),
        "red" => red(text),
        _ => text.to_string(),
    }
}

/// Pad `text` with spaces to `width` display cells, the equivalent of Ink's
/// fixed-width `<Box width={n}>` columns. Text longer than the column is left
/// intact rather than truncated.
pub fn pad(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(width - len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_to_the_column_width() {
        assert_eq!(pad("ip", 5), "ip   ");
    }

    #[test]
    fn leaves_over_long_text_alone() {
        assert_eq!(pad("sysinfo", 4), "sysinfo");
    }

    #[test]
    fn styling_is_inert_when_disabled() {
        // Tests capture stdout, so colour is off and helpers are pass-through.
        assert_eq!(bold("x"), "x");
        assert_eq!(colored("x", "green"), "x");
    }
}
