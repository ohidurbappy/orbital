//! The interactive tool picker: type to fuzzy-search, ↑/↓ to move, Enter to run.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::commands::filter::filter_commands;
use crate::commands::{Command, Ctx, COMMANDS};
use crate::components::banner::banner_lines;
use crate::core::updater::cached_update;
use crate::core::updater::refresh::run_refresh;
use crate::core::updater::state::{is_stale, now_ms, read_state};
use crate::style;
use crate::term::{self, Frame, Key, RawMode};
use crate::Res;

/// How often the input loop wakes up to notice a finished update check.
const TICK: Duration = Duration::from_millis(200);

/// Width of the command-name column, matching the TypeScript menu.
const NAME_WIDTH: usize = 12;

struct MenuState {
    query: String,
    selected: usize,
}

impl MenuState {
    /// Keep the highlighted row inside the (possibly shrunken) result list.
    fn index(&self, results: usize) -> usize {
        if results == 0 {
            0
        } else {
            self.selected.min(results - 1)
        }
    }
}

pub fn run_menu() -> Res {
    if !term::is_interactive() {
        // No TTY to prompt on: print what's available and leave, so
        // `orbital | cat` is still useful instead of hanging on a key read.
        let mut lines = banner_lines(cached_update().as_ref());
        lines.push(style::bold_cyan("Available tools"));
        for command in COMMANDS {
            lines.push(format!(
                "  {}{}",
                style::cyan(&style::pad(command.name, NAME_WIDTH)),
                style::dim(command.description)
            ));
        }
        term::emit(&lines);
        return Ok(());
    }

    let _raw = RawMode::enable()?;
    let mut frame = Frame::new();
    let mut state = MenuState {
        query: String::new(),
        selected: 0,
    };
    // Guards against stacking up refresh threads while one is in flight.
    let refreshing = Arc::new(AtomicBool::new(false));

    loop {
        maybe_refresh(&refreshing);
        let results = filter_commands(COMMANDS, &state.query);
        let index = state.index(results.len());
        frame.draw(&render(&state, &results, index))?;

        let key = match term::read_key_timeout(TICK)? {
            Some(key) => key,
            // Timed out: loop round to repaint, which is how a background
            // update check makes its banner appear without a keypress.
            None => continue,
        };

        match key {
            Key::Interrupt => break,
            Key::Escape => {
                if state.query.is_empty() {
                    break;
                }
                state.query.clear();
                state.selected = 0;
            }
            Key::Enter => {
                if let Some(command) = results.get(index).copied() {
                    frame.clear()?;
                    open(command)?;
                    // The command left its own output on screen; start a fresh
                    // frame below it rather than painting over it.
                    frame = Frame::new();
                }
            }
            Key::Up => {
                state.selected = state
                    .selected
                    .min(results.len().saturating_sub(1))
                    .saturating_sub(1)
            }
            Key::Down => state.selected = (state.selected + 1).min(results.len().saturating_sub(1)),
            Key::Backspace => {
                state.query.pop();
                state.selected = 0;
            }
            Key::Char(c) => {
                state.query.push(c);
                state.selected = 0;
            }
            Key::Other => {}
        }
    }

    frame.clear()?;
    Ok(())
}

/// Run a command picked from the menu, then wait for Esc to come back.
fn open(command: &Command) -> Res {
    let mut header = Frame::new();
    header.draw(&[
        format!(
            "{}{}",
            style::bold_green(command.name),
            style::dim(" — press Esc to go back")
        ),
        String::new(),
    ])?;
    header.keep();

    (command.view)(&Ctx::menu())?;

    let mut hint = Frame::new();
    hint.draw(&[String::new(), style::dim("Press Esc to go back.")])?;
    loop {
        match term::read_key()? {
            Key::Escape | Key::Interrupt | Key::Enter => break,
            _ => {}
        }
    }
    hint.clear()?;
    Ok(())
}

/// Kick off a cache refresh in the background when the cached check has aged
/// out. Mirrors the 10-minute poll the TypeScript menu ran while open.
fn maybe_refresh(refreshing: &Arc<AtomicBool>) {
    if refreshing.load(Ordering::Relaxed) || !is_stale(read_state().as_ref(), now_ms()) {
        return;
    }
    refreshing.store(true, Ordering::Relaxed);
    let flag = Arc::clone(refreshing);
    let _ = std::thread::Builder::new().spawn(move || {
        run_refresh();
        flag.store(false, Ordering::Relaxed);
    });
}

fn render(state: &MenuState, results: &[&Command], index: usize) -> Vec<String> {
    let mut lines = banner_lines(cached_update().as_ref());

    let prompt = format!("{}{}", style::bold_cyan("❯ "), state.query);
    lines.push(if state.query.is_empty() {
        format!(
            "{prompt}{}",
            style::dim("Type to search… (↑/↓, Enter; Esc to quit)")
        )
    } else {
        prompt
    });
    lines.push(String::new());

    if results.is_empty() {
        lines.push(style::dim("No matching tools."));
        return lines;
    }

    for (i, command) in results.iter().enumerate() {
        let selected = i == index;
        let marker = if selected {
            style::green("❯ ")
        } else {
            "  ".to_string()
        };
        let name = style::pad(command.name, NAME_WIDTH);
        let name = if selected {
            style::bold_green(&name)
        } else {
            style::cyan(&name)
        };
        let description = if selected {
            command.description.to_string()
        } else {
            style::dim(command.description)
        };
        lines.push(format!("{marker}{name}{description}"));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(query: &str, selected: usize) -> MenuState {
        MenuState {
            query: query.to_string(),
            selected,
        }
    }

    #[test]
    fn clamps_the_selection_to_the_result_count() {
        assert_eq!(state("", 9).index(3), 2);
        assert_eq!(state("", 1).index(3), 1);
    }

    #[test]
    fn an_empty_result_list_selects_nothing() {
        assert_eq!(state("zzz", 4).index(0), 0);
    }

    #[test]
    fn renders_every_command_with_no_query() {
        let results = filter_commands(COMMANDS, "");
        let lines = render(&state("", 0), &results, 0);
        for command in COMMANDS {
            assert!(
                lines.iter().any(|l| l.contains(command.name)),
                "{} missing from the menu",
                command.name
            );
        }
    }

    #[test]
    fn shows_the_hint_only_while_the_query_is_empty() {
        let results = filter_commands(COMMANDS, "");
        let empty = render(&state("", 0), &results, 0);
        assert!(empty.iter().any(|l| l.contains("Type to search")));

        let typed = render(&state("ip", 0), &filter_commands(COMMANDS, "ip"), 0);
        assert!(!typed.iter().any(|l| l.contains("Type to search")));
    }

    #[test]
    fn says_so_when_nothing_matches() {
        let results = filter_commands(COMMANDS, "zzzzz");
        let lines = render(&state("zzzzz", 0), &results, 0);
        assert!(lines.iter().any(|l| l.contains("No matching tools.")));
    }

    #[test]
    fn marks_only_the_selected_row() {
        let results = filter_commands(COMMANDS, "");
        let lines = render(&state("", 1), &results, 1);
        let row = |name: &str| {
            lines
                .iter()
                .find(|l| l.contains(name) && !l.contains("Type to search"))
                .unwrap_or_else(|| panic!("no row for {name}"))
        };
        assert!(row(results[1].name).starts_with("❯ "));
        assert!(row(results[0].name).starts_with("  "));
    }
}
